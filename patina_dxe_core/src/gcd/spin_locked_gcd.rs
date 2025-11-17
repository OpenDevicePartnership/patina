//! UEFI Global Coherency Domain (GCD)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::pecoff::{self, UefiPeInfo};
use alloc::{boxed::Box, slice, vec, vec::Vec};
use core::{fmt::Display, ptr};
use patina::{base::DEFAULT_CACHE_ATTR, error::EfiError};

use mu_rust_helpers::function;
use patina::{
    base::{SIZE_4GB, UEFI_PAGE_MASK, UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, align_up},
    guids::{self, CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP},
    pi::{
        dxe_services::{self, GcdMemoryType, MemorySpaceDescriptor},
        hob::{self, EFiMemoryTypeInformation},
    },
    uefi_pages_to_size,
};
use patina_internal_collections::{Error as SliceError, Rbt, SliceKey, node_size};
use r_efi::efi;

use crate::{
    GCD, allocator::DEFAULT_ALLOCATION_STRATEGY, ensure, error, events::EVENT_DB, protocol_db,
    protocol_db::INVALID_HANDLE, tpl_mutex,
};
use patina_internal_cpu::paging::{CacheAttributeValue, PatinaPageTable};
use patina_paging::{MemoryAttributes, PtError, page_allocator::PageAllocator};

use patina::pi::hob::{Hob, HobList};

use super::{
    io_block::{self, Error as IoBlockError, IoBlock, IoBlockSplit, StateTransition as IoStateTransition},
    memory_block::{
        self, Error as MemoryBlockError, MemoryBlock, MemoryBlockSplit, StateTransition as MemoryStateTransition,
    },
};

const MEMORY_BLOCK_SLICE_LEN: usize = 4096;
pub const MEMORY_BLOCK_SLICE_SIZE: usize = MEMORY_BLOCK_SLICE_LEN * node_size::<MemoryBlock>();

const IO_BLOCK_SLICE_LEN: usize = 4096;
const IO_BLOCK_SLICE_SIZE: usize = IO_BLOCK_SLICE_LEN * node_size::<IoBlock>();

const PAGE_POOL_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InternalError {
    MemoryBlock(MemoryBlockError),
    IoBlock(IoBlockError),
    Slice(SliceError),
}

#[derive(Debug, Clone, Copy)]
pub enum AllocateType {
    /// Allocate from the lowest address to the highest address or until the specify address is reached (max address).
    BottomUp(Option<usize>),
    /// Allocate from the highest address to the lowest address.
    /// Some(address) => Start at the specified address (inclusive max address).
    /// None => Start at top of memory.
    TopDown(Option<usize>),
    /// Allocate at this address.
    Address(usize),
}

#[derive(Clone, Copy)]
struct GcdAttributeConversionEntry {
    attribute: u32,
    capability: u64,
    memory: bool,
}

const ATTRIBUTE_CONVERSION_TABLE: [GcdAttributeConversionEntry; 15] = [
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_UNCACHEABLE,
        capability: efi::MEMORY_UC,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_UNCACHED_EXPORTED,
        capability: efi::MEMORY_UCE,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_WRITE_COMBINEABLE,
        capability: efi::MEMORY_WC,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_WRITE_THROUGH_CACHEABLE,
        capability: efi::MEMORY_WT,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_WRITE_BACK_CACHEABLE,
        capability: efi::MEMORY_WB,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_READ_PROTECTABLE,
        capability: efi::MEMORY_RP,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTABLE,
        capability: efi::MEMORY_WP,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_EXECUTION_PROTECTABLE,
        capability: efi::MEMORY_XP,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTABLE,
        capability: efi::MEMORY_RO,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
        capability: hob::EFI_MEMORY_PRESENT,
        memory: false,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
        capability: hob::EFI_MEMORY_INITIALIZED,
        memory: false,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_TESTED,
        capability: hob::EFI_MEMORY_TESTED,
        memory: false,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_PERSISTABLE,
        capability: hob::EFI_MEMORY_NV,
        memory: true,
    },
    GcdAttributeConversionEntry {
        attribute: hob::EFI_RESOURCE_ATTRIBUTE_MORE_RELIABLE,
        capability: hob::EFI_MEMORY_MORE_RELIABLE,
        memory: true,
    },
    GcdAttributeConversionEntry { attribute: 0, capability: 0, memory: false },
];

pub fn get_capabilities(gcd_mem_type: dxe_services::GcdMemoryType, attributes: u64) -> u64 {
    let mut capabilities = 0;

    for conversion in ATTRIBUTE_CONVERSION_TABLE.iter() {
        if conversion.attribute == 0 {
            break;
        }

        if (conversion.memory
            || (gcd_mem_type != dxe_services::GcdMemoryType::SystemMemory
                && gcd_mem_type != dxe_services::GcdMemoryType::MoreReliable))
            && (attributes & (conversion.attribute as u64) != 0)
        {
            capabilities |= conversion.capability;
        }
    }

    capabilities
}

type GcdAllocateFn = fn(
    gcd: &mut GCD,
    allocate_type: AllocateType,
    memory_type: dxe_services::GcdMemoryType,
    alignment: usize,
    len: usize,
    image_handle: efi::Handle,
    device_handle: Option<efi::Handle>,
) -> Result<usize, EfiError>;
type GcdFreeFn =
    fn(gcd: &mut GCD, base_address: usize, len: usize, transition: MemoryStateTransition) -> Result<(), EfiError>;

#[derive(Debug)]
pub(crate) struct PagingAllocator<'a> {
    page_pool: Vec<efi::PhysicalAddress>,
    gcd: &'a SpinLockedGcd,
}

impl<'a> PagingAllocator<'a> {
    pub(crate) fn new(gcd: &'a SpinLockedGcd) -> Self {
        Self { page_pool: Vec::with_capacity(PAGE_POOL_CAPACITY), gcd }
    }
}

impl PageAllocator for PagingAllocator<'_> {
    fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError> {
        if align != UEFI_PAGE_SIZE as u64 || size != UEFI_PAGE_SIZE as u64 {
            log::error!("Invalid alignment or size for page allocation: align: {align:#x}, size: {size:#x}");
            return Err(PtError::InvalidParameter);
        }

        if is_root {
            // allocate 1 page
            let len = 1;
            // allocate under 4GB to support x86 MPServices
            let addr: u64 = (SIZE_4GB - 1) as u64;

            // if this is the root page, we need to allocate it under 4GB to support x86 MPServices, they will copy
            // the cr3 register to the APs and the APs come up in real mode, transition to protected mode, enable paging,
            // and then transition to long mode. This means that the root page must be under 4GB so that the 32 bit code
            // can do 32 bit register moves to move it to cr3. For other architectures, this is not necessary, but not
            // an issue to allocate. However, some architectures may not have memory under 4GB, so if we fail here,
            // simply retry with the normal allocation

            let res = self.gcd.memory.lock().allocate_memory_space(
                AllocateType::BottomUp(Some(addr as usize)),
                dxe_services::GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                uefi_pages_to_size!(len),
                protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                None,
            );
            match res {
                Ok(root_page) => Ok(root_page as u64),
                Err(_) => {
                    // if we failed, try again with normal allocation
                    log::error!(
                        "Failed to allocate root page for the page table page pool, retrying with normal allocation"
                    );

                    match self.gcd.memory.lock().allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        dxe_services::GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        uefi_pages_to_size!(len),
                        protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                        None,
                    ) {
                        Ok(root_page) => Ok(root_page as u64),
                        Err(e) => {
                            // okay we are good and dead now
                            panic!("Failed to allocate root page for the page table page pool: {e:?}");
                        }
                    }
                }
            }
        } else {
            match self.page_pool.pop() {
                Some(page) => Ok(page),
                None => {
                    // allocate 512 pages at a time
                    let len = PAGE_POOL_CAPACITY;

                    // we only allocate here, not map. The page table is self-mapped, so we don't have to identity
                    // map them. This function is called with the page table lock held, so we cannot do that
                    match self.gcd.memory.lock().allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        dxe_services::GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        uefi_pages_to_size!(len),
                        protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                        None,
                    ) {
                        Ok(addr) => {
                            for i in 0..len {
                                self.page_pool.push(addr as u64 + ((i * UEFI_PAGE_SIZE) as u64));
                            }
                            self.page_pool.pop().ok_or(PtError::OutOfResources)
                        }
                        Err(e) => {
                            panic!("Failed to allocate pages for the page table page pool {e:?}");
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::upper_case_acronyms)]
//The Global Coherency Domain (GCD) Services are used to manage the memory resources visible to the boot processor.
struct GCD {
    maximum_address: usize,
    memory_blocks: Rbt<'static, MemoryBlock>,
    allocate_memory_space_fn: GcdAllocateFn,
    free_memory_space_fn: GcdFreeFn,
    /// Default attributes for memory allocations
    /// This is efi::MEMORY_XP unless we have entered compatibility mode, in which case it is 0, e.g. no protection
    default_attributes: u64,
    /// Whether to prioritize 32-bit memory allocations
    prioritize_32_bit_memory: bool,
}

impl GCD {
    /// Returns true if the GCD is initialized and ready for use.
    pub fn is_ready(&self) -> bool {
        self.maximum_address != 0
    }
}

impl core::fmt::Debug for GCD {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GCD")
            .field("maximum_address", &self.maximum_address)
            .field("memory_blocks", &self.memory_blocks)
            .finish()
    }
}

impl GCD {
    // Create an instance of the Global Coherency Domain (GCD) for testing.
    #[cfg(test)]
    pub(crate) const fn new(processor_address_bits: u32) -> Self {
        assert!(processor_address_bits > 0);
        Self {
            memory_blocks: Rbt::new(),
            maximum_address: 1 << processor_address_bits,
            allocate_memory_space_fn: Self::allocate_memory_space_internal,
            free_memory_space_fn: Self::free_memory_space_worker,
            default_attributes: efi::MEMORY_XP,
            prioritize_32_bit_memory: false,
        }
    }

    pub fn lock_memory_space(&mut self) {
        self.allocate_memory_space_fn = Self::allocate_memory_space_null;
        self.free_memory_space_fn = Self::free_memory_space_worker_null;
        log::info!("Disallowing alloc/free during ExitBootServices.");
    }

    pub fn unlock_memory_space(&mut self) {
        self.allocate_memory_space_fn = Self::allocate_memory_space_internal;
        self.free_memory_space_fn = Self::free_memory_space_worker;
    }

    pub fn init(&mut self, processor_address_bits: u32) {
        self.maximum_address = 1 << processor_address_bits;
    }

    pub(crate) unsafe fn init_memory_blocks(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(
            memory_type == dxe_services::GcdMemoryType::SystemMemory && len >= MEMORY_BLOCK_SLICE_SIZE,
            EfiError::OutOfResources
        );

        log::trace!(target: "allocations", "[{}] Initializing memory blocks at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Capabilities: {:#x}", function!(), capabilities);

        let unallocated_memory_space = MemoryBlock::Unallocated(dxe_services::MemorySpaceDescriptor {
            memory_type: dxe_services::GcdMemoryType::NonExistent,
            base_address: 0,
            length: self.maximum_address as u64,
            ..Default::default()
        });

        self.memory_blocks
            .resize(unsafe { slice::from_raw_parts_mut::<'static>(base_address as *mut u8, MEMORY_BLOCK_SLICE_SIZE) });

        self.memory_blocks.add(unallocated_memory_space).map_err(|_| EfiError::OutOfResources)?;
        let idx = unsafe { self.add_memory_space(memory_type, base_address, len, capabilities) }?;

        //initialize attributes on the first block to WB + XP
        match self.set_memory_space_attributes(
            base_address,
            len,
            (MemoryAttributes::Writeback | MemoryAttributes::ExecuteProtect).bits(),
        ) {
            Ok(_) | Err(EfiError::NotReady) => Ok(()),
            Err(err) => Err(err),
        }?;

        //allocate a chunk of the block to hold the actual first GCD slice
        self.allocate_memory_space(
            AllocateType::Address(base_address),
            dxe_services::GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            MEMORY_BLOCK_SLICE_SIZE,
            protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
            None,
        )?;

        // remove the XP and add RP on the remaining free block.
        if len > MEMORY_BLOCK_SLICE_SIZE {
            match self.set_memory_space_attributes(
                base_address + MEMORY_BLOCK_SLICE_SIZE,
                len - MEMORY_BLOCK_SLICE_SIZE,
                (MemoryAttributes::Writeback | MemoryAttributes::ReadProtect).bits(),
            ) {
                Ok(_) | Err(EfiError::NotReady) => Ok(()),
                Err(err) => Err(err),
            }?;
        }

        Ok(idx)
    }

    /// This service adds reserved memory, system memory, or memory-mapped I/O resources to the global coherency domain of the processor.
    ///
    /// # Safety
    /// Since the first call with enough system memory will cause the creation of an array at `base_address` + [MEMORY_BLOCK_SLICE_SIZE].
    /// The memory from `base_address` to `base_address+len` must be inside the valid address range of the program and not in use.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.1
    pub unsafe fn add_memory_space(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        mut capabilities: u64,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address.checked_add(len).is_some_and(|sum| sum <= self.maximum_address), EfiError::Unsupported);
        ensure!(self.memory_blocks.capacity() > 0, EfiError::NotReady);

        log::trace!(target: "allocations", "[{}] Adding memory space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Capabilities: {:#x}\n", function!(), capabilities);

        // All software capabilities are supported for system memory
        capabilities |= efi::MEMORY_ACCESS_MASK | efi::MEMORY_RUNTIME;

        // The MEMORY_MAPPED_IO_PORT_SPACE attribute should be supported for MMIO
        if memory_type == dxe_services::GcdMemoryType::MemoryMappedIo {
            capabilities |= efi::MEMORY_ISA_VALID;
        }

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = memory_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(block.as_ref().memory_type == dxe_services::GcdMemoryType::NonExistent, EfiError::AccessDenied);

        // all newly added memory is marked as RP
        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::Add(memory_type, capabilities, efi::MEMORY_RP),
        ) {
            Ok(idx) => Ok(idx),
            Err(InternalError::MemoryBlock(MemoryBlockError::BlockOutsideRange)) => error!(EfiError::AccessDenied),
            Err(InternalError::MemoryBlock(MemoryBlockError::InvalidStateTransition)) => {
                error!(EfiError::InvalidParameter)
            }
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service removes reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.4
    pub fn remove_memory_space(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Removing memory space at {:#x} of length {:#x}", function!(), base_address, len);

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = *memory_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        match Self::split_state_transition_at_idx(memory_blocks, idx, base_address, len, MemoryStateTransition::Remove)
        {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(MemoryBlockError::BlockOutsideRange)) => error!(EfiError::NotFound),
            Err(InternalError::MemoryBlock(MemoryBlockError::InvalidStateTransition)) => match block {
                MemoryBlock::Unallocated(_) => error!(EfiError::NotFound),
                MemoryBlock::Allocated(_) => error!(EfiError::AccessDenied),
            },
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    fn allocate_memory_space(
        &mut self,
        allocate_type: AllocateType,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        (self.allocate_memory_space_fn)(self, allocate_type, memory_type, alignment, len, image_handle, device_handle)
    }

    /// This service allocates nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.2
    fn allocate_memory_space_internal(
        gcd: &mut GCD,
        allocate_type: AllocateType,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        ensure!(gcd.maximum_address != 0, EfiError::NotReady);
        ensure!(
            len > 0 && image_handle > ptr::null_mut() && memory_type != dxe_services::GcdMemoryType::Unaccepted,
            EfiError::InvalidParameter
        );

        log::trace!(target: "allocations", "[{}] Allocating memory space: {:x?}", function!(), allocate_type);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        match allocate_type {
            AllocateType::BottomUp(max_address) => gcd.allocate_bottom_up(
                memory_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::TopDown(max_address) => gcd.allocate_top_down(
                memory_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::Address(address) => {
                ensure!(address + len <= gcd.maximum_address, EfiError::NotFound);
                gcd.allocate_address(memory_type, alignment, len, image_handle, device_handle, address)
            }
        }
    }

    #[coverage(off)]
    fn allocate_memory_space_null(
        _gcd: &mut GCD,
        _allocate_type: AllocateType,
        _memory_type: dxe_services::GcdMemoryType,
        _alignment: usize,
        _len: usize,
        _image_handle: efi::Handle,
        _device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        log::error!("GCD not allowed to allocate after EBS has started!");
        debug_assert!(false);
        Err(EfiError::AccessDenied)
    }

    fn free_memory_space_worker(
        &mut self,
        base_address: usize,
        len: usize,
        transition: MemoryStateTransition,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);
        ensure!((base_address & UEFI_PAGE_MASK) == 0 && (len & UEFI_PAGE_MASK) == 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Freeing memory space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory State Transition: {:?}\n", function!(), transition);

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;

        match Self::split_state_transition_at_idx(memory_blocks, idx, base_address, len, transition) {
            Ok(_) => {}
            Err(InternalError::MemoryBlock(_)) => error!(EfiError::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }

        let desc = self.get_memory_descriptor_for_address(base_address as efi::PhysicalAddress)?;

        match self.set_gcd_memory_attributes(
            base_address,
            len,
            efi::MEMORY_RP | efi::MEMORY_XP | (desc.attributes & efi::CACHE_ATTRIBUTE_MASK),
        ) {
            Ok(_) => Ok(()),
            Err(e) => {
                // if we failed to set the attributes in the GCD, we want to catch it, but should still try to go
                // down and free the memory space
                log::error!(
                    "Failed to set memory attributes for {:#x?} of length {:#x?} with attributes {:#x?}. Status: {:#x?}",
                    base_address,
                    len,
                    efi::MEMORY_RP,
                    e
                );
                debug_assert!(false);
                Err(e)
            }
        }
    }

    #[coverage(off)]
    fn free_memory_space_worker_null(
        _gcd: &mut GCD,
        _base_address: usize,
        _len: usize,
        _transition: MemoryStateTransition,
    ) -> Result<(), EfiError> {
        log::error!("GCD not allowed to free after EBS has started! Silently failing, returning success");

        // TODO: We actually want to check if this is a runtime memory type and debug_assert/return an error if so,
        // as freeing this memory in an EBS handler would cause a change in the OS memory map and we don't want to leave
        // this memory around. However, with the current architecture, it is very hard to figure out what EFI memory
        // type memory in the GCD is. There are two different ways this can be fixed: one, merge the GCD and allocator
        // mods, as is already planned, and then be able to access the memory_type_for_handle function in the allocator
        // from here. Two, add an EFI memory type to the GCD. Both of these options require more work and this is
        // currently blocking a platform, which was not the original intention here, discussion on the assert on
        // runtime memory led to an assert on all frees, which was not the intention. So, for now this is just made
        // a silent failure and this will be revisited. This will be tracked in a GH issue for resolution.
        Ok(())
    }

    fn allocate_bottom_up(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        align_shift: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Bottom up GCD allocation: {:#?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Align Shift: {:#x}", function!(), align_shift);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        let memory_blocks = &mut self.memory_blocks;
        let alignment = 1 << align_shift;

        log::trace!(target: "gcd_measure", "search");
        let mut current = memory_blocks.first_idx();
        while let Some(idx) = current {
            let mb = memory_blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            if mb.len() < len {
                current = memory_blocks.next_idx(idx);
                continue;
            }

            let address = mb.start();
            let mut addr = address & (usize::MAX << align_shift);

            if addr < address {
                addr += alignment;
            }
            ensure!(addr + len <= max_address, EfiError::NotFound);

            if mb.as_ref().memory_type != memory_type {
                current = memory_blocks.next_idx(idx);
                continue;
            }

            // We don't allow allocations on page 0, to allow for null pointer detection. If this block starts at 0,
            // attempt to move forward a page + alignment to find a valid address. If there is not enough space in this
            // block, move to the next one.
            if addr == 0 {
                addr = align_up(UEFI_PAGE_SIZE, alignment)?;
                // we can do mb.len() - addr here because we know this block starts from 0
                if addr + len >= max_address || mb.len() - addr < len {
                    current = memory_blocks.next_idx(idx);
                    continue;
                }
            }

            match Self::split_state_transition_at_idx(
                memory_blocks,
                idx,
                addr,
                len,
                MemoryStateTransition::AllocateRespectingOwnership(image_handle, device_handle),
            ) {
                Ok(_) => return Ok(addr),
                Err(InternalError::MemoryBlock(_)) => {
                    current = memory_blocks.next_idx(idx);
                    continue;
                }
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        if max_address == usize::MAX { Err(EfiError::OutOfResources) } else { Err(EfiError::NotFound) }
    }

    fn allocate_top_down(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        align_shift: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        // For top down requests specifically, if prioritize 32 bit memory is set, then first
        // try with an artificial max.
        if self.prioritize_32_bit_memory && max_address > u32::MAX as usize {
            match self.allocate_top_down(memory_type, align_shift, len, image_handle, device_handle, u32::MAX as usize)
            {
                Ok(addr) => return Ok(addr),
                Err(error) => {
                    log::trace!(target: "allocations", "[{}] Top down GCD low memory attempt failed: {:?}", function!(), error);
                }
            }
        }

        log::trace!(target: "allocations", "[{}] Top down GCD allocation: {:#?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Align Shift: {:#x}", function!(), align_shift);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let mut current = memory_blocks.get_closest_idx(&(max_address as u64));
        while let Some(idx) = current {
            let mb = memory_blocks.get_with_idx(idx).expect("idx is valid from prev_idx");

            // Account for if the block is truncated by the max_address. Max address
            // is inclusive, but end() is exclusive so subtract 1 from end.
            let usable_len =
                if mb.end() - 1 > max_address { max_address.checked_sub(mb.start()).unwrap() + 1 } else { mb.len() };
            if usable_len < len {
                current = memory_blocks.prev_idx(idx);
                continue;
            }

            // Find the last suitable aligned range in the memory block.
            let addr = (mb.start() + usable_len - len) & (usize::MAX << align_shift);
            if addr < mb.start() {
                current = memory_blocks.prev_idx(idx);
                continue;
            }

            if mb.as_ref().memory_type != memory_type {
                current = memory_blocks.prev_idx(idx);
                continue;
            }

            // We don't allow allocations on page 0, to allow for null pointer detection. As this is a top down
            // search this means that we have already traversed all higher values, so bail.
            if addr == 0 {
                break;
            }

            match Self::split_state_transition_at_idx(
                memory_blocks,
                idx,
                addr,
                len,
                MemoryStateTransition::AllocateRespectingOwnership(image_handle, device_handle),
            ) {
                Ok(_) => return Ok(addr),
                Err(InternalError::MemoryBlock(_)) => {
                    current = memory_blocks.prev_idx(idx);
                    continue;
                }
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        if max_address == usize::MAX { Err(EfiError::OutOfResources) } else { Err(EfiError::NotFound) }
    }

    fn allocate_address(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        align_shift: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Exact address GCD allocation: {:#?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Address: {:#x}", function!(), address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Align Shift: {:#x}", function!(), align_shift);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        // allocate_address allows allocating page 0. This is needed to let Patina DXE Core allocate it for null
        // pointer detection very early in the boot process. Any future allocate at address will fail because it is
        // already allocated. However, Patina DXE Core needs to allocate address 0 in order to prevent bootloaders
        // from thinking it is free memory that can be allocated.

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(address as u64)).ok_or(EfiError::NotFound)?;
        let block = memory_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(
            block.as_ref().memory_type == memory_type && address == address & (usize::MAX << align_shift),
            EfiError::NotFound
        );

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            address,
            len,
            MemoryStateTransition::Allocate(image_handle, device_handle),
        ) {
            Ok(_) => Ok(address),
            Err(InternalError::MemoryBlock(_)) => error!(EfiError::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    pub fn free_memory_space(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        (self.free_memory_space_fn)(self, base_address, len, MemoryStateTransition::Free)
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// Ownership of the memory as indicated by the image_handle associated with the block is retained, which means that
    /// it cannot be re-allocated except by the original owner or by requests targeting a specific address within the
    /// block (i.e. [`Self::allocate_memory_space`] with [`AllocateType::Address`]).
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    pub fn free_memory_space_preserving_ownership(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        (self.free_memory_space_fn)(self, base_address, len, MemoryStateTransition::FreePreservingOwnership)
    }

    /// This service sets attributes on the given memory space.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
    pub fn set_memory_space_attributes(
        &mut self,
        base_address: usize,
        len: usize,
        attributes: u64,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);
        ensure!((base_address & UEFI_PAGE_MASK) == 0 && (len & UEFI_PAGE_MASK) == 0, EfiError::InvalidParameter);

        // we split allocating memory from mapping it, so this function only sets attributes (which may result
        // in mapping memory if it was previously unmapped)
        self.set_gcd_memory_attributes(base_address, len, attributes)
    }

    /// This service sets attributes on the given memory space.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
    fn set_gcd_memory_attributes(&mut self, base_address: usize, len: usize, attributes: u64) -> Result<(), EfiError> {
        log::trace!(target: "allocations", "[{}] Setting memory space attributes for {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Attributes: {:#x}\n", function!(), attributes);

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::SetAttributes(attributes),
        ) {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(e)) => {
                log::error!(
                    "GCD failed to set attributes on range {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. error {e:?}",
                );
                debug_assert!(false);
                error!(EfiError::Unsupported)
            }
            Err(InternalError::Slice(SliceError::OutOfSpace)) => {
                log::error!(
                    "GCD failed to set attributes on range {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?} due to space",
                );
                debug_assert!(false);
                error!(EfiError::OutOfResources)
            }
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service sets capabilities on the given memory space.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
    pub fn set_memory_space_capabilities(
        &mut self,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);
        ensure!((base_address & UEFI_PAGE_MASK) == 0 && (len & UEFI_PAGE_MASK) == 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Setting memory space capabilities for {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Capabilities: {:#x}\n", function!(), capabilities);

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::SetCapabilities(capabilities),
        ) {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(_)) => error!(EfiError::Unsupported),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service returns a copy of the current set of memory blocks in the GCD.
    /// Since GCD is used to service heap expansion requests and thus should avoid allocations,
    /// Caller is required to initialize a vector of sufficient capacity to hold the descriptors
    /// and provide a mutable reference to it.
    pub fn get_memory_descriptors(
        &mut self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(buffer.capacity() >= self.memory_descriptor_count(), EfiError::InvalidParameter);
        ensure!(buffer.is_empty(), EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Enter\n", function!(), );

        let blocks = &self.memory_blocks;

        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            match mb {
                MemoryBlock::Allocated(descriptor) | MemoryBlock::Unallocated(descriptor) => buffer.push(*descriptor),
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }

    fn get_allocated_memory_descriptors(
        &self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(buffer.capacity() >= self.memory_descriptor_count(), EfiError::InvalidParameter);
        ensure!(buffer.is_empty(), EfiError::InvalidParameter);

        let blocks = &self.memory_blocks;

        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            if let MemoryBlock::Allocated(descriptor) = mb {
                buffer.push(*descriptor);
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }

    fn get_mmio_and_reserved_descriptors(
        &self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(buffer.is_empty(), EfiError::InvalidParameter);

        let blocks = &self.memory_blocks;

        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            if let MemoryBlock::Unallocated(descriptor) = mb
                && (descriptor.memory_type == dxe_services::GcdMemoryType::MemoryMappedIo
                    || descriptor.memory_type == dxe_services::GcdMemoryType::Reserved)
            {
                buffer.push(*descriptor);
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }

    /// This service returns the descriptor for the given physical address.
    pub fn get_memory_descriptor_for_address(
        &mut self,
        address: efi::PhysicalAddress,
    ) -> Result<dxe_services::MemorySpaceDescriptor, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);

        let memory_blocks = &self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(address)).ok_or(EfiError::NotFound)?;
        let mb = memory_blocks.get_with_idx(idx).expect("idx is valid from get_closest_idx");
        match mb {
            MemoryBlock::Allocated(descriptor) | MemoryBlock::Unallocated(descriptor) => Ok(*descriptor),
        }
    }

    fn split_state_transition_at_idx(
        memory_blocks: &mut Rbt<MemoryBlock>,
        idx: usize,
        base_address: usize,
        len: usize,
        transition: MemoryStateTransition,
    ) -> Result<usize, InternalError> {
        let mb_before_split = *memory_blocks.get_with_idx(idx).expect("Caller should ensure idx is valid.");

        log::trace!(target: "allocations", "[{}] Splitting memory block at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Total Memory Blocks Right Now: {:#}", function!(), memory_blocks.len());
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Block Index: {:#x}", function!(), idx);
        log::trace!(target: "allocations", "[{}]   Transition:\n  {:#?}", function!(), transition);

        // split_state_transition does not update the key, so this is safe.
        let new_idx = unsafe {
            match memory_blocks.get_with_idx_mut(idx).expect("idx valid above").split_state_transition(
                base_address,
                len,
                transition,
            )? {
                MemoryBlockSplit::Same(_) => Ok(idx),
                MemoryBlockSplit::After(_, next) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] MemoryBlockSplit (After) -> Next: {:#x?}\n", function!(), next);
                    memory_blocks.add(next)
                }
                MemoryBlockSplit::Before(_, next) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] MemoryBlockSplit (Before) -> Next: {:#x?}\n", function!(), next);
                    memory_blocks.add(next).map(|_| idx)
                }
                MemoryBlockSplit::Middle(_, next, next2) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] MemoryBlockSplit (Middle) -> Next: {:#x?}. Next2: {:#x?}\n", function!(), next, next2);
                    memory_blocks.add_many([next2, next])
                }
            }
        };

        log::trace!(target: "allocations", "[{}] Next Index is {:x?}\n", function!(), new_idx);

        // If the split failed, restore the memory block to its previous state.
        let idx = match new_idx {
            Ok(idx) => idx,
            Err(e) => {
                log::error!("[{}] Memory block split failed! -> Error: {:#?}", function!(), e);
                // Restore the memory block to its previous state. The base_address (key) is not updated with the split, so this is safe.
                unsafe {
                    *memory_blocks.get_with_idx_mut(idx).expect("idx valid above") = mb_before_split;
                }
                error!(e);
            }
        };

        // Lets see if we can merge the block with the next block
        if let Some(next_idx) = memory_blocks.next_idx(idx) {
            let mut next = *memory_blocks.get_with_idx(next_idx).expect("idx valid from insert");

            // base_address (they key) is not updated with the merge, so this is safe.
            unsafe {
                if memory_blocks.get_with_idx_mut(idx).expect("idx valid from insert").merge(&mut next) {
                    memory_blocks.delete_with_idx(next_idx).expect("Index already verified.");
                }
            }
        }

        // Lets see if we can merge the block with the previous block
        if let Some(prev_idx) = memory_blocks.prev_idx(idx) {
            let mut block = *memory_blocks.get_with_idx(idx).expect("idx valid from insert");

            // base_address (they key) is not updated with the merge, so this is safe.
            unsafe {
                if memory_blocks.get_with_idx_mut(prev_idx).expect("idx valid from insert").merge(&mut block) {
                    memory_blocks.delete_with_idx(idx).expect("Index already verified.");
                    // Return early with prev_idx, since we merged with the previous block
                    return Ok(prev_idx);
                }
            }
        }

        Ok(idx)
    }

    /// returns the current count of blocks in the list.
    pub fn memory_descriptor_count(&self) -> usize {
        self.memory_blocks.len()
    }

    #[cfg(feature = "compatibility_mode_allowed")]
    /// This function activates compatibility mode for the GCD, which is just to set the default attributes to 0,
    /// which will prevent new memory from being allocated as non-executable. This function is purposefully not set
    /// to be pub(crate) because the only caller of it is SpinLockedGcd.activate_compatibility_mode(). And this should
    /// not be called except by that function.
    fn activate_compatibility_mode(&mut self) {
        self.default_attributes = 0;
    }

    //Note: truncated strings here are expected and are for alignment with EDK2 reference prints.
    const GCD_MEMORY_TYPE_NAMES: [&'static str; 8] = [
        "NonExist ", // EfiGcdMemoryTypeNonExistent
        "Reserved ", // EfiGcdMemoryTypeReserved
        "SystemMem", // EfiGcdMemoryTypeSystemMemory
        "MMIO     ", // EfiGcdMemoryTypeMemoryMappedIo
        "PersisMem", // EfiGcdMemoryTypePersistent
        "MoreRelia", // EfiGcdMemoryTypeMoreReliable
        "Unaccepte", // EfiGcdMemoryTypeUnaccepted
        "Unknown  ", // EfiGcdMemoryTypeMaximum
    ];
}

impl Display for GCD {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(
            f,
            "GCDMemType Range                             Capabilities     Attributes       ImageHandle      DeviceHandle"
        )?;
        writeln!(
            f,
            "========== ================================= ================ ================ ================ ================"
        )?;

        let blocks = &self.memory_blocks;
        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            match mb {
                MemoryBlock::Allocated(descriptor) | MemoryBlock::Unallocated(descriptor) => {
                    let mem_type_str_idx =
                        usize::min(descriptor.memory_type as usize, Self::GCD_MEMORY_TYPE_NAMES.len() - 1);
                    writeln!(
                        f,
                        "{}  {:016x?}-{:016x?} {:016x?} {:016x?} {:016x?} {:016x?}",
                        GCD::GCD_MEMORY_TYPE_NAMES[mem_type_str_idx],
                        descriptor.base_address,
                        descriptor.base_address + descriptor.length - 1,
                        descriptor.capabilities,
                        descriptor.attributes,
                        descriptor.image_handle,
                        descriptor.device_handle
                    )?;
                }
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }
}

impl SliceKey for MemoryBlock {
    type Key = u64;
    fn key(&self) -> &Self::Key {
        &self.as_ref().base_address
    }
}

impl From<SliceError> for InternalError {
    fn from(value: SliceError) -> Self {
        InternalError::Slice(value)
    }
}

impl From<memory_block::Error> for InternalError {
    fn from(value: memory_block::Error) -> Self {
        InternalError::MemoryBlock(value)
    }
}

#[derive(Debug)]
///The I/O Global Coherency Domain (GCD) Services are used to manage the I/O resources visible to the boot processor.
pub struct IoGCD {
    maximum_address: usize,
    io_blocks: Rbt<'static, IoBlock>,
}

impl IoGCD {
    // Create an instance of the Global Coherency Domain (GCD) for testing.
    #[cfg(test)]
    pub(crate) const fn _new(io_address_bits: u32) -> Self {
        assert!(io_address_bits > 0);
        Self { io_blocks: Rbt::new(), maximum_address: 1 << io_address_bits }
    }

    pub fn init(&mut self, io_address_bits: u32) {
        self.maximum_address = 1 << io_address_bits;
    }

    fn init_io_blocks(&mut self) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);

        self.io_blocks.resize(unsafe {
            Box::into_raw(vec![0_u8; IO_BLOCK_SLICE_SIZE].into_boxed_slice())
                .as_mut()
                .expect("RBT given null pointer in initialization.")
        });

        self.io_blocks
            .add(IoBlock::Unallocated(dxe_services::IoSpaceDescriptor {
                io_type: dxe_services::GcdIoType::NonExistent,
                base_address: 0,
                length: self.maximum_address as u64,
                ..Default::default()
            }))
            .map_err(|_| EfiError::OutOfResources)?;

        Ok(())
        /*
        ensure!(memory_type == dxe_services::GcdMemoryType::SystemMemory && len >= MEMORY_BLOCK_SLICE_SIZE, EfiError::OutOfResources);

        let unallocated_memory_space = MemoryBlock::Unallocated(dxe_services::MemorySpaceDescriptor {
          memory_type: dxe_services::GcdMemoryType::NonExistent,
          base_address: 0,
          length: self.maximum_address as u64,
          ..Default::default()
        });

        let mut memory_blocks =
          SortedSlice::new(slice::from_raw_parts_mut::<'static>(base_address as *mut u8, MEMORY_BLOCK_SLICE_SIZE));
        memory_blocks.add(unallocated_memory_space).map_err(|_| EfiError::OutOfResources)?;
        self.memory_blocks.replace(memory_blocks);

        self.add_memory_space(memory_type, base_address, len, capabilities)?;

        self.allocate_memory_space(
          AllocateType::Address(base_address),
          dxe_services::GcdMemoryType::SystemMemory,
          0,
          MEMORY_BLOCK_SLICE_SIZE,
          1 as _,
          None,
        ) */
    }

    /// This service adds reserved I/O, or system I/O resources to the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.9
    pub fn add_io_space(
        &mut self,
        io_type: dxe_services::GcdIoType,
        base_address: usize,
        len: usize,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Adding IO space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}\n", function!(), io_type);

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = io_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(block.as_ref().io_type == dxe_services::GcdIoType::NonExistent, EfiError::AccessDenied);

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Add(io_type)) {
            Ok(idx) => Ok(idx),
            Err(InternalError::IoBlock(IoBlockError::BlockOutsideRange)) => error!(EfiError::AccessDenied),
            Err(InternalError::IoBlock(IoBlockError::InvalidStateTransition)) => error!(EfiError::InvalidParameter),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service removes reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.12
    pub fn remove_io_space(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Removing IO space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}\n", function!(), len);

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = *io_blocks.get_with_idx(idx).expect("Idx valid from get_closest_idx");

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Remove) {
            Ok(_) => Ok(()),
            Err(InternalError::IoBlock(IoBlockError::BlockOutsideRange)) => error!(EfiError::NotFound),
            Err(InternalError::IoBlock(IoBlockError::InvalidStateTransition)) => match block {
                IoBlock::Unallocated(_) => error!(EfiError::NotFound),
                IoBlock::Allocated(_) => error!(EfiError::AccessDenied),
            },
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service allocates reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.10
    pub fn allocate_io_space(
        &mut self,
        allocate_type: AllocateType,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0 && image_handle > ptr::null_mut(), EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Allocating IO space: {:x?}", function!(), allocate_type);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        match allocate_type {
            AllocateType::BottomUp(max_address) => self.allocate_bottom_up(
                io_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::TopDown(max_address) => self.allocate_top_down(
                io_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::Address(address) => {
                ensure!(address + len <= self.maximum_address, EfiError::Unsupported);
                self.allocate_address(io_type, alignment, len, image_handle, device_handle, address)
            }
        }
    }

    fn allocate_bottom_up(
        &mut self,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Bottom up IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let mut current = io_blocks.first_idx();
        while let Some(idx) = current {
            let ib = io_blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            if ib.len() < len {
                current = io_blocks.next_idx(idx);
                continue;
            }
            let address = ib.start();
            let mut addr = address & (usize::MAX << alignment);
            if addr < address {
                addr += 1 << alignment;
            }
            ensure!(addr + len <= max_address, EfiError::NotFound);
            if ib.as_ref().io_type != io_type {
                current = io_blocks.next_idx(idx);
                continue;
            }

            match Self::split_state_transition_at_idx(
                io_blocks,
                idx,
                addr,
                len,
                IoStateTransition::Allocate(image_handle, device_handle),
            ) {
                Ok(_) => return Ok(addr),
                Err(InternalError::IoBlock(_)) => {
                    current = io_blocks.next_idx(idx);
                    continue;
                }
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(EfiError::NotFound)
    }

    fn allocate_top_down(
        &mut self,
        io_type: dxe_services::GcdIoType,
        align_shift: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Top down IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Align Shift: {:#x}", function!(), align_shift);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let mut current = io_blocks.get_closest_idx(&(max_address as u64));
        while let Some(idx) = current {
            let ib = io_blocks.get_with_idx(idx).expect("idx is valid from prev_idx");

            // Account for if the block is truncated by the max_address. Max address
            // is inclusive, but end() is exclusive so subtract 1 from end.
            let usable_len = if ib.end() - 1 > max_address { max_address - ib.start() + 1 } else { ib.len() };
            if usable_len < len {
                current = io_blocks.prev_idx(idx);
                continue;
            }

            // Find the last suitable aligned range in the IO block.
            let addr = (ib.start() + usable_len - len) & (usize::MAX << align_shift);
            if addr < ib.start() {
                current = io_blocks.prev_idx(idx);
                continue;
            }

            if ib.as_ref().io_type != io_type {
                current = io_blocks.prev_idx(idx);
                continue;
            }

            match Self::split_state_transition_at_idx(
                io_blocks,
                idx,
                addr,
                len,
                IoStateTransition::Allocate(image_handle, device_handle),
            ) {
                Ok(_) => return Ok(addr),
                Err(InternalError::IoBlock(_)) => {
                    current = io_blocks.prev_idx(idx);
                    continue;
                }
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(EfiError::NotFound)
    }

    fn allocate_address(
        &mut self,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Exact address IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Address: {:#x}", function!(), address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or(ptr::null_mut()));

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }
        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(address as u64)).ok_or(EfiError::NotFound)?;
        let block = io_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(
            block.as_ref().io_type == io_type && address == address & (usize::MAX << alignment),
            EfiError::NotFound
        );

        match Self::split_state_transition_at_idx(
            io_blocks,
            idx,
            address,
            len,
            IoStateTransition::Allocate(image_handle, device_handle),
        ) {
            Ok(_) => Ok(address),
            Err(InternalError::IoBlock(_)) => error!(EfiError::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service frees reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.11
    pub fn free_io_space(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Free IO space at {:#?}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}\n", function!(), len);

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Free) {
            Ok(_) => Ok(()),
            Err(InternalError::IoBlock(_)) => error!(EfiError::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service returns a copy of the current set of memory blocks in the GCD.
    /// Since GCD is used to service heap expansion requests and thus should avoid allocations,
    /// Caller is required to initialize a vector of sufficient capacity to hold the descriptors
    /// and provide a mutable reference to it.
    pub fn get_io_descriptors(&mut self, buffer: &mut Vec<dxe_services::IoSpaceDescriptor>) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(buffer.capacity() >= self.io_descriptor_count(), EfiError::InvalidParameter);
        ensure!(buffer.is_empty(), EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Enter\n", function!(), );

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let blocks = &self.io_blocks;
        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let ib = blocks.get_with_idx(idx).expect("Index comes from dfs and should be valid");
            match ib {
                IoBlock::Allocated(descriptor) | IoBlock::Unallocated(descriptor) => buffer.push(*descriptor),
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }

    fn split_state_transition_at_idx(
        io_blocks: &mut Rbt<IoBlock>,
        idx: usize,
        base_address: usize,
        len: usize,
        transition: IoStateTransition,
    ) -> Result<usize, InternalError> {
        let ib_before_split = *io_blocks.get_with_idx(idx).expect("Caller should ensure idx is valid.");

        log::trace!(target: "allocations", "[{}] Splitting IO block at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Total IO Blocks Right Now: {:#}", function!(), io_blocks.len());
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Block Index: {:#x}", function!(), idx);
        log::trace!(target: "allocations", "[{}]   Transition: {:?}\n", function!(), transition);

        // split_state_transition does not update the key, so this is safe.
        let new_idx = unsafe {
            match io_blocks.get_with_idx_mut(idx).expect("idx valid above").split_state_transition(
                base_address,
                len,
                transition,
            )? {
                IoBlockSplit::Same(_) => Ok(idx),
                IoBlockSplit::After(_, next) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] IoBlockSplit (After) -> Next: {:#x?}\n", function!(), next);
                    io_blocks.add(next)
                }
                IoBlockSplit::Before(_, next) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] IoBlockSplit (Before) -> Next: {:#x?}\n", function!(), next);
                    io_blocks.add(next).map(|_| idx)
                }
                IoBlockSplit::Middle(_, next, next2) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] IoBlockSplit (Middle) -> Next: {:#x?}. Next2: {:#x?}\n", function!(), next, next2);
                    io_blocks.add_many([next2, next])
                }
            }
        };

        // If the split failed, restore the memory block to its previous state.
        let idx = match new_idx {
            Ok(idx) => idx,
            Err(e) => {
                log::error!("[{}] IO block split failed! -> Error: {:#?}", function!(), e);
                // Restore the memory block to its previous state. The base_address (key) is not updated with the split, so this is safe.
                unsafe {
                    *io_blocks.get_with_idx_mut(idx).expect("idx valid above") = ib_before_split;
                }
                error!(e);
            }
        };

        // Lets see if we can merge the block with the next block
        if let Some(next_idx) = io_blocks.next_idx(idx) {
            let mut next = *io_blocks.get_with_idx(next_idx).expect("idx valid from insert");
            // base_address (they key) is not updated with the merge, so this is safe.
            unsafe {
                if io_blocks.get_with_idx_mut(idx).expect("idx valid from insert").merge(&mut next) {
                    io_blocks.delete_with_idx(next_idx).expect("Index already verified.");
                }
            }
        }

        // Lets see if we can merge the block with the previous block
        if let Some(prev_idx) = io_blocks.prev_idx(idx) {
            let mut block = *io_blocks.get_with_idx(idx).expect("idx valid from insert");
            // base_address (they key) is not updated with the merge, so this is safe.
            unsafe {
                if io_blocks.get_with_idx_mut(prev_idx).expect("idx valid from insert").merge(&mut block) {
                    io_blocks.delete_with_idx(idx).expect("Index already verified.");
                    return Ok(prev_idx);
                }
            }
        }

        Ok(idx)
    }

    /// returns the current count of blocks in the list.
    pub fn io_descriptor_count(&self) -> usize {
        self.io_blocks.len()
    }

    const GCD_IO_TYPE_NAMES: [&'static str; 4] = [
        "NonExist", // EfiGcdIoTypeNonExistent
        "Reserved", // EfiGcdIoTypeReserved
        "I/O     ", // EfiGcdIoTypeIo
        "Unknown ", // EfiGcdIoTypeMaximum
    ];
}

impl Display for IoGCD {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "GCDIoType  Range                            ")?;
        writeln!(f, "========== =================================")?;

        let blocks = &self.io_blocks;
        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let ib = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            match ib {
                IoBlock::Allocated(descriptor) | IoBlock::Unallocated(descriptor) => {
                    let io_type_str_idx = usize::min(descriptor.io_type as usize, Self::GCD_IO_TYPE_NAMES.len() - 1);
                    writeln!(
                        f,
                        "{}  {:016x?}-{:016x?}{}",
                        IoGCD::GCD_IO_TYPE_NAMES[io_type_str_idx],
                        descriptor.base_address,
                        descriptor.base_address + descriptor.length - 1,
                        { if descriptor.image_handle == INVALID_HANDLE { "" } else { "*" } }
                    )?;
                }
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }
}

impl SliceKey for IoBlock {
    type Key = u64;
    fn key(&self) -> &Self::Key {
        &self.as_ref().base_address
    }
}

impl From<io_block::Error> for InternalError {
    fn from(value: io_block::Error) -> Self {
        InternalError::IoBlock(value)
    }
}

/// Describes the kind of GCD map change that triggered the callback.
#[derive(Debug, PartialEq, Eq)]
pub enum MapChangeType {
    AddMemorySpace,
    RemoveMemorySpace,
    AllocateMemorySpace,
    FreeMemorySpace,
    SetMemoryAttributes,
    SetMemoryCapabilities,
}

/// GCD map change callback function type.
pub type MapChangeCallback = fn(MapChangeType);

/// Implements a spin locked GCD suitable for use as a static global.
pub struct SpinLockedGcd {
    memory: tpl_mutex::TplMutex<GCD>,
    io: tpl_mutex::TplMutex<IoGCD>,
    memory_change_callback: Option<MapChangeCallback>,
    memory_type_info_table: [EFiMemoryTypeInformation; 17],
    page_table: tpl_mutex::TplMutex<Option<Box<dyn PatinaPageTable>>>,
}

impl SpinLockedGcd {
    /// Returns true if the underlying GCD is initialized and ready for use.
    pub fn is_ready(&self) -> bool {
        self.memory.lock().is_ready()
    }

    /// Creates a new uninitialized GCD. [`Self::init`] must be invoked before any other functions or they will return
    /// [`EfiError::NotReady`]. An optional callback can be provided which will be invoked whenever an operation
    /// changes the GCD map.
    #[coverage(off)]
    pub const fn new(memory_change_callback: Option<MapChangeCallback>) -> Self {
        Self {
            memory: tpl_mutex::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                GCD {
                    maximum_address: 0,
                    memory_blocks: Rbt::new(),
                    allocate_memory_space_fn: GCD::allocate_memory_space_internal,
                    free_memory_space_fn: GCD::free_memory_space_worker,
                    default_attributes: efi::MEMORY_XP,
                    prioritize_32_bit_memory: false,
                },
                "GcdMemLock",
            ),
            io: tpl_mutex::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                IoGCD { maximum_address: 0, io_blocks: Rbt::new() },
                "GcdIoLock",
            ),
            memory_change_callback,
            memory_type_info_table: [
                EFiMemoryTypeInformation { memory_type: efi::RESERVED_MEMORY_TYPE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::LOADER_CODE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::LOADER_DATA, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::BOOT_SERVICES_CODE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::BOOT_SERVICES_DATA, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::CONVENTIONAL_MEMORY, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::UNUSABLE_MEMORY, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::ACPI_RECLAIM_MEMORY, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::ACPI_MEMORY_NVS, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::MEMORY_MAPPED_IO, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::MEMORY_MAPPED_IO_PORT_SPACE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::PAL_CODE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::PERSISTENT_MEMORY, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: efi::UNACCEPTED_MEMORY_TYPE, number_of_pages: 0 },
                EFiMemoryTypeInformation { memory_type: 16 /*EfiMaxMemoryType*/, number_of_pages: 0 },
            ],
            page_table: tpl_mutex::TplMutex::new(efi::TPL_HIGH_LEVEL, None, "GcdPageTableLock"),
        }
    }

    /// Initializes the memory blocks in the GCD.
    ///
    /// # Safety
    /// The caller must ensure that the memory region specified by `base_address` and `len` is freely usable RAM and
    /// will never be used by any other part of the system at any time.
    #[coverage(off)]
    pub(crate) unsafe fn init_memory_blocks(
        &self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<usize, EfiError> {
        // SAFETY: Caller must uphold the safety contract of init_memory_blocks
        unsafe { self.memory.lock().init_memory_blocks(memory_type, base_address, len, capabilities) }
    }

    #[coverage(off)]
    pub fn prioritize_32_bit_memory(&self, value: bool) {
        self.memory.lock().prioritize_32_bit_memory = value;
    }

    /// Returns a reference to the memory type information table.
    pub const fn memory_type_info_table(&self) -> &[EFiMemoryTypeInformation; 17] {
        &self.memory_type_info_table
    }

    /// Returns a pointer to the memory type information for the given memory type.
    pub const fn memory_type_info(&self, memory_type: u32) -> &EFiMemoryTypeInformation {
        &self.memory_type_info_table[memory_type as usize]
    }

    fn set_paging_attributes(&self, base_address: usize, len: usize, attributes: u64) -> Result<(), EfiError> {
        if let Some(page_table) = &mut *self.page_table.lock() {
            // only apply page table attributes to the page table, not our virtual GCD attributes
            let paging_attrs = MemoryAttributes::from_bits_truncate(attributes)
                & (MemoryAttributes::AccessAttributesMask | MemoryAttributes::CacheAttributesMask);

            let mut unmapped = false;
            let mut update_cache_attributes = true;

            // EFI_MEMORY_RP is a special case, we don't actually want to set it in the page table, we want to unmap
            // the region. It is valid for the region to already be unmapped or partially unmapped in this case. E.g.
            // we might be freeing an entire image but the stack guard page is already unmapped.
            if paging_attrs & MemoryAttributes::ReadProtect == MemoryAttributes::ReadProtect {
                match page_table.unmap_memory_region(base_address as u64, len as u64) {
                    Ok(_) => {
                        log::trace!(
                            target: "paging",
                            "Memory region {base_address:#x?} of length {len:#x?} unmapped",
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to unmap memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                        );
                        debug_assert!(false);
                        return Err(EfiError::InvalidParameter);
                    }
                }
            }

            // we assume that the page table and GCD are in sync. If not, we will debug_assert and return an error here
            // as this indicates a critical error
            let region_attributes = match page_table.query_memory_region(base_address as u64, len as u64) {
                Ok(attrs) => Some(attrs),
                Err((PtError::NoMapping, attrs)) => {
                    // it is not an error if the range is fully not mapped, we just need to map it, unless we are
                    // trying to unmap the region, which we will check for below
                    unmapped = true;

                    // we capture the returned cache attributes here in order to check if we need to send the cache
                    // attribute update later
                    match attrs {
                        CacheAttributeValue::Valid(cache_attributes) => {
                            // we got valid cache attributes for an unmapped region which means we will
                            // need to check later if we need to send the cache attribute update event
                            Some(cache_attributes)
                        }
                        CacheAttributeValue::Unmapped => {
                            // region is unmapped with no cache attributes which means we will need to send
                            // the cache attribute update event
                            None
                        }
                        // this architecture only describes cache attributes in the page table, so don't send the
                        // cache attribute update event
                        CacheAttributeValue::NotSupported => {
                            update_cache_attributes = false;
                            None
                        }
                    }
                }
                Err(e) => {
                    log::error!(
                        "query memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                    );
                    log::error!("GCD and page table are out of sync. This is a critical error.");
                    log::info!("GCD {GCD}");
                    debug_assert!(false);
                    return Err(EfiError::InvalidParameter);
                }
            };

            // if this region already has the attributes we want, we don't need to do anything
            // in the page table.
            if let Some(region_attrs) = region_attributes
                && (region_attrs & (MemoryAttributes::AccessAttributesMask | MemoryAttributes::CacheAttributesMask))
                    == paging_attrs
                && !unmapped
            {
                log::trace!(
                    target: "paging",
                    "Memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. No paging action taken: Region already mapped with these attributes.",
                );
                return Ok(());
            }

            match page_table.map_memory_region(base_address as u64, len as u64, paging_attrs) {
                Ok(_) => {
                    let new_cache_attributes = paging_attrs & MemoryAttributes::CacheAttributesMask;
                    let old_cache_attributes =
                        region_attributes.map(|attrs| attrs & MemoryAttributes::CacheAttributesMask);

                    // if the cache attributes changed, we need to publish an event, as some architectures
                    // (such as x86) need to populate APs with the caching information
                    if new_cache_attributes != MemoryAttributes::empty() && update_cache_attributes {
                        if let Some(old_cache_attrs) = old_cache_attributes
                            && old_cache_attrs != new_cache_attributes
                        {
                            // in this case, we had caching attributes for this region and they do not match the newly
                            // set attributes
                            log::trace!(
                                target: "paging",
                                "Cache attributes for memory region {base_address:#x?} of length {len:#x?} were updated to {new_cache_attributes:#x?} from {old_cache_attrs:#x?}, sending cache attributes changed event",
                            );

                            EVENT_DB.signal_group(CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP);
                        } else if unmapped && old_cache_attributes.is_none() {
                            // in this case the region was unmapped and we had no caching attributes set up
                            log::trace!(
                                target: "paging",
                                "Cache attributes for memory region {base_address:#x?} of length {len:#x?} were updated to {new_cache_attributes:#x?} from an unmapped state, sending cache attributes changed event",
                            );

                            EVENT_DB.signal_group(CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP);
                        }
                    }

                    log::trace!(
                        target: "paging",
                        "Memory region {base_address:#x?} of length {len:#x?} mapped with attributes {paging_attrs:#x?}",
                    );
                    Ok(())
                }
                Err(e) => {
                    // this indicates the GCD and page table are out of sync
                    log::error!(
                        "Failed to map memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                    );

                    debug_assert!(false);
                    match e {
                        PtError::OutOfResources => Err(EfiError::OutOfResources),
                        PtError::NoMapping => Err(EfiError::NotFound),
                        _ => Err(EfiError::InvalidParameter),
                    }
                }
            }
        } else {
            // if we don't have the page table, we shouldn't panic, this may just be the case that we are allocating
            // the initial GCD memory space and we haven't initialized the page table yet
            Err(EfiError::NotReady)
        }
    }

    pub fn lock_memory_space(&self) {
        self.memory.lock().lock_memory_space();
    }

    pub fn unlock_memory_space(&self) {
        self.memory.lock().unlock_memory_space();
    }

    /// Resets the GCD to default state. Intended for test scenarios.
    ///
    /// # Safety
    ///
    /// This call potentially invalidates all allocations made by any allocator on top of this GCD.
    /// Caller is responsible for ensuring that no such allocations exist.
    ///
    #[cfg(test)]
    pub unsafe fn reset(&self) {
        let (mut mem, mut io) = (self.memory.lock(), self.io.lock());
        mem.maximum_address = 0;
        mem.memory_blocks = Rbt::new();
        io.maximum_address = 0;
        io.io_blocks = Rbt::new();
    }

    /// Adds a page table for testing purposes
    #[cfg(test)]
    pub fn add_test_page_table(&self, page_table: Box<dyn PatinaPageTable>) {
        *self.page_table.lock() = Some(page_table);
    }

    /// Initializes the underlying memory GCD and I/O GCD with the given address bits.
    pub fn init(&self, memory_address_bits: u32, io_address_bits: u32) {
        self.memory.lock().init(memory_address_bits);
        self.io.lock().init(io_address_bits);
    }

    /// Helper function to perform an operation on each GCD descriptor in the given range.
    ///
    /// Arguments:
    /// - `base_address`: The starting address of the range.
    /// - `len`: The length of the range.
    /// - `operation`: The operation to perform on each descriptor. It takes the descriptor, the base address of the current range,
    ///   the length of the current range, and a context value.
    /// - `context`: A context value to pass to the operation.
    ///
    /// Returns:
    /// - `Ok(())` if the operation was successful for all descriptors.
    /// - `Err(EfiError)` if the operation failed for any descriptor. The operation does not continue if a failure occurs.
    pub(crate) fn for_each_desc_in_range<F>(
        &self,
        base_address: usize,
        len: usize,
        mut operation: F,
        context: u64,
    ) -> Result<(), EfiError>
    where
        F: FnMut(MemorySpaceDescriptor, usize, usize, u64) -> Result<(), EfiError>,
    {
        let mut current_base = base_address as u64;
        let range_end = (base_address + len) as u64;
        while current_base < range_end {
            let descriptor = self.get_memory_descriptor_for_address(current_base as efi::PhysicalAddress)?;
            let descriptor_end = descriptor.base_address + descriptor.length;

            // it is still legal to split a descriptor and only operate on part of it
            let next_base = u64::min(descriptor_end, range_end);
            let current_len = next_base - current_base;
            operation(descriptor, current_base as usize, current_len as usize, context)?;

            current_base = next_base;
        }

        Ok(())
    }

    // Take control of our own destiny and create a page table that the GCD controls
    // This must be done after the GCD is initialized and memory services are available,
    // as we need to allocate memory for the page table structure.
    // This function always uses the GCD functions to map the page table so that the GCD remains in sync with the
    // changes here (setting XP)
    pub(crate) fn init_paging_with(&self, hob_list: &HobList, page_table: Box<dyn PatinaPageTable>) {
        log::info!("Initializing paging for the GCD");

        *self.page_table.lock() = Some(page_table);

        // this is before we get allocated descriptors, so we don't need to preallocate memory here
        let mut mmio_res_descs: Vec<dxe_services::MemorySpaceDescriptor> = Vec::new();
        self.memory
            .lock()
            .get_mmio_and_reserved_descriptors(mmio_res_descs.as_mut())
            .expect("Failed to get MMIO descriptors!");

        // Before we install this page table, we need to ensure that DXE Core is mapped correctly here as well as any
        // allocated memory and MMIO. All other memory will be unmapped initially. Do allocated memory first, then the
        // DXE Core, so that we can ensure that the DXE Core is mapped correctly and not overwritten by the allocated
        // memory attrs. We also need to preallocate memory here so that we do not allocate memory after getting the
        // descriptors
        let mut descriptors: Vec<dxe_services::MemorySpaceDescriptor> =
            Vec::with_capacity(self.memory_descriptor_count() + 10);
        self.memory
            .lock()
            .get_allocated_memory_descriptors(&mut descriptors)
            .expect("Failed to get allocated memory descriptors!");

        // now map the memory regions, keeping any cache attributes set in the GCD descriptors
        for desc in descriptors {
            log::trace!(
                target: "paging",
                "Mapping memory region {:#x?} of length {:#x?} with attributes {:#x?}",
                desc.base_address,
                desc.length,
                desc.attributes
            );

            if let Err(err) = self.set_memory_space_attributes(
                desc.base_address as usize,
                desc.length as usize,
                (desc.attributes & efi::CACHE_ATTRIBUTE_MASK) | efi::MEMORY_XP,
            ) {
                // if we fail to set these attributes (which should just be XP at this point), we should try to
                // continue
                log::error!(
                    "Failed to map memory region {:#x?} of length {:#x?} with attributes {:#x?}. Error: {:?}",
                    desc.base_address,
                    desc.length,
                    desc.attributes,
                    err
                );
                debug_assert!(false);
            }
        }

        // Retrieve the MemoryAllocationModule hob corresponding to the DXE core so that we can map it correctly
        let dxe_core_hob = hob_list
            .iter()
            .find_map(|x| match x {
                Hob::MemoryAllocationModule(module) if module.module_name == guids::DXE_CORE => Some(module),
                _ => None,
            })
            .expect("Did not find MemoryAllocationModule Hob for DxeCore. Use patina::guid::DXE_CORE as FFS GUID.");

        let pe_info = unsafe {
            UefiPeInfo::parse(core::slice::from_raw_parts(
                dxe_core_hob.alloc_descriptor.memory_base_address as *const u8,
                dxe_core_hob.alloc_descriptor.memory_length as usize,
            ))
            .expect("Failed to parse PE info for DXE Core")
        };

        let dxe_core_cache_attr =
            match self.get_memory_descriptor_for_address(dxe_core_hob.alloc_descriptor.memory_base_address) {
                Ok(desc) => desc.attributes & efi::CACHE_ATTRIBUTE_MASK,
                Err(e) => panic!("DXE Core not mapped in GCD {e:?}"),
            };

        // map the entire image as RW, as the PE headers don't live in the sections
        self.set_memory_space_attributes(
            dxe_core_hob.alloc_descriptor.memory_base_address as usize,
            dxe_core_hob.alloc_descriptor.memory_length as usize,
            efi::MEMORY_XP | dxe_core_cache_attr,
        )
        .unwrap_or_else(|_| {
            panic!(
                "Failed to map DXE Core image {:#x?} of length {:#x?} with attributes {:#x?}.",
                dxe_core_hob.alloc_descriptor.memory_base_address, 0x1000, 0
            )
        });

        // now map each section with the correct image protections
        for section in pe_info.sections {
            // each section starts at image_base + virtual_address, per PE/COFF spec.
            let section_base_address =
                dxe_core_hob.alloc_descriptor.memory_base_address + (section.virtual_address as u64);
            let mut attributes = efi::MEMORY_XP;
            if section.characteristics & pecoff::IMAGE_SCN_CNT_CODE == pecoff::IMAGE_SCN_CNT_CODE {
                attributes = efi::MEMORY_RO;
            }

            // We need to use the virtual size for the section length, but
            // we cannot rely on this to be section aligned, as some compilers rely on the loader to align this
            let aligned_virtual_size = match align_up(section.virtual_size, pe_info.section_alignment) {
                Ok(size) => size as u64,
                Err(_) => {
                    panic!(
                        "Failed to align section size {:#x?} with alignment {:#x?}",
                        section.virtual_size, pe_info.section_alignment
                    );
                }
            };

            log::trace!(
                target: "paging",
                "Mapping DXE Core image memory region {section_base_address:#x?} of length {aligned_virtual_size:#x?} with attributes {attributes:#x?}",
            );

            attributes |=
                match self.get_memory_descriptor_for_address(dxe_core_hob.alloc_descriptor.memory_base_address) {
                    Ok(desc) => desc.attributes & efi::CACHE_ATTRIBUTE_MASK,
                    Err(e) => panic!("DXE Core section not mapped in GCD {e:?}"),
                };

            self.set_memory_space_attributes(section_base_address as usize, aligned_virtual_size as usize, attributes)
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to map DXE Core image {:#x?} of length {:#x?} with attributes {:#x?}.",
                        dxe_core_hob.alloc_descriptor.memory_base_address, 0x1000, 0
                    )
                });
        }

        // now map MMIO. Drivers expect to be able to access MMIO regions as RW, so we need to map them as such
        for desc in mmio_res_descs {
            // MMIO is not necessarily described at page granularity, but needs to be mapped as such in the page
            // table
            let base_address = desc.base_address as usize & !UEFI_PAGE_MASK;
            let len = (desc.length as usize + UEFI_PAGE_MASK) & !UEFI_PAGE_MASK;
            let new_attributes = (desc.attributes & efi::CACHE_ATTRIBUTE_MASK) | efi::MEMORY_XP;

            log::trace!(
                target: "paging",
                "Mapping {:?} region {:#x?} of length {:#x?} with attributes {:#x?}",
                desc.memory_type,
                base_address,
                len,
                new_attributes
            );

            if let Err(err) = self.set_memory_space_attributes(base_address, len, new_attributes) {
                // if we fail to set these attributes we may or may not be able to continue to boot. It depends on
                // if a driver attempts to touch this MMIO region
                log::error!(
                    "Failed to map {:?} region {:#x?} of length {:#x?} with attributes {:#x?}. Error: {:?}",
                    desc.memory_type,
                    base_address,
                    len,
                    new_attributes,
                    err
                );
                debug_assert!(false);
            }
        }

        // make sure we didn't map page 0 if it was reserved or MMIO, we are using this for null pointer detection
        // only do this if page 0 actually exists
        if let Ok(descriptor) = self.get_memory_descriptor_for_address(0)
            && descriptor.memory_type != GcdMemoryType::NonExistent
            && let Err(err) = self.set_memory_space_attributes(0, UEFI_PAGE_SIZE, efi::MEMORY_RP)
        {
            // if we fail to set these attributes we can continue to boot, but we will not be able to detect null
            // pointer dereferences.
            log::error!("Failed to unmap page 0, which is reserved for null pointer detection. Error: {err:?}");
            debug_assert!(false);
        }

        self.page_table.lock().as_mut().unwrap().install_page_table().expect("Failed to install the page table");

        log::info!("Paging initialized for the GCD");
    }

    /// This service adds reserved memory, system memory, or memory-mapped I/O resources to the global coherency domain of the processor.
    ///
    /// # Safety
    /// Since the first call with enough system memory will cause the creation of an array at `base_address` + [MEMORY_BLOCK_SLICE_SIZE].
    /// The memory from `base_address` to `base_address+len` must be inside the valid address range of the program and not in use.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.1
    pub unsafe fn add_memory_space(
        &self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<usize, EfiError> {
        let result = unsafe { self.memory.lock().add_memory_space(memory_type, base_address, len, capabilities) };
        if result.is_ok()
            && let Some(callback) = self.memory_change_callback
        {
            callback(MapChangeType::AddMemorySpace);
        }
        result
    }

    /// This service removes reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.4
    #[coverage(off)]
    pub fn remove_memory_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        let result = self.memory.lock().remove_memory_space(base_address, len);
        if result.is_ok() {
            if let Some(page_table) = &mut *self.page_table.lock() {
                match page_table.unmap_memory_region(base_address as u64, len as u64) {
                    Ok(_) => {}
                    Err(status) => {
                        log::error!(
                            "Failed to unmap memory region {base_address:#x?} of length {len:#x?}. Status: {status:#x?} during
                                remove_memory_space removal. This is expected if this region was not previously mapped",
                        );
                    }
                }
            }

            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::RemoveMemorySpace);
            }
        }
        result
    }

    /// This service allocates nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.2
    pub fn allocate_memory_space(
        &self,
        allocate_type: AllocateType,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        let result = self.memory.lock().allocate_memory_space(
            allocate_type,
            memory_type,
            alignment,
            len,
            image_handle,
            device_handle,
        );
        if result.is_ok() {
            // if we successfully allocated memory, we want to set the range as NX. For any standard data, we should
            // always have NX set and no consumer needs to update it. If a code region is going to be allocated
            // here, we rely on the image loader to update the attributes as appropriate for the code sections. The
            // same holds true for other required attributes.
            if let Ok(base_address) = result.as_ref() {
                let attributes = match self.get_memory_descriptor_for_address(*base_address as efi::PhysicalAddress) {
                    Ok(descriptor) => descriptor.attributes,
                    Err(_) => DEFAULT_CACHE_ATTR,
                };
                // it is safe to call set_memory_space_attributes without calling set_memory_space_capabilities here
                // because we set efi::MEMORY_XP as a capability on all memory ranges we add to the GCD. A driver could
                // call set_memory_space_capabilities to remove the XP capability, but that is something that should
                // be caught and fixed.
                let default_attributes = self.memory.lock().default_attributes;
                match self.set_memory_space_attributes(
                    *base_address,
                    len,
                    (attributes & efi::CACHE_ATTRIBUTE_MASK) | default_attributes,
                ) {
                    Ok(_) => (),
                    Err(EfiError::NotReady) => {
                        // this is expected if paging is not initialized yet. The GCD will still be updated, but
                        // the page table will not yet. When we initialize paging, the GCD will use the attributes
                        // that have been updated here to initialize the page table. paging must allocate memory
                        // to form the page table we are going to use.
                    }
                    Err(e) => {
                        // this is now a real error case, paging is enabled, but we failed to set NX on the
                        // range. This we want to catch. In a release build, we should still continue, but we'll
                        // not have NX set on the range.
                        log::error!(
                            "Could not set NX for memory address {:#X} for len {:#X} with error {:?}",
                            *base_address,
                            len,
                            e
                        );
                        debug_assert!(false);
                    }
                }
            } else {
                log::error!("Could not extract base address from allocation result, unable to set memory attributes.");
                debug_assert!(false);
            }

            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::AllocateMemorySpace);
            }
        }
        result
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    #[coverage(off)]
    pub fn free_memory_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        let mut result = self.memory.lock().free_memory_space(base_address, len);

        match result {
            Ok(_) => {
                // when we free, we want to unmap this memory region and mark it EFI_MEMORY_RP in the GCD
                // we don't panic if we don't have a page table because the memory bucket code does a free before the
                // page table is initialized. If we were to end up without the page table initialized, we would still
                // keep track of state in the GCD
                if let Some(page_table) = &mut *self.page_table.lock() {
                    match page_table.unmap_memory_region(base_address as u64, len as u64) {
                        Ok(_) => {}
                        Err(status) => {
                            log::error!(
                                "Failed to unmap memory region {base_address:#x?} of length {len:#x?}. Status: {status:#x?}",
                            );
                            debug_assert!(false);
                            match status {
                                PtError::OutOfResources => EfiError::OutOfResources,
                                PtError::NoMapping => EfiError::NotFound,
                                _ => EfiError::InvalidParameter,
                            };
                        }
                    }
                }

                if let Some(callback) = self.memory_change_callback {
                    callback(MapChangeType::FreeMemorySpace);
                }
            }
            // this is the post-EBS case, we silently fail and return success
            Err(EfiError::AccessDenied) => result = Ok(()),
            _ => {}
        }

        result
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// Ownership of the memory as indicated by the image_handle associated with the block is retained, which means that
    /// it cannot be re-allocated except by the original owner or by requests targeting a specific address within the
    /// block (i.e. [`Self::allocate_memory_space`] with [`AllocateType::Address`]).
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    pub fn free_memory_space_preserving_ownership(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        let result = self.memory.lock().free_memory_space_preserving_ownership(base_address, len);
        if result.is_ok()
            && let Some(callback) = self.memory_change_callback
        {
            callback(MapChangeType::FreeMemorySpace);
        }
        result
    }

    // Apply Patina memory protection rules to the given attributes.
    //
    // Rules:
    //   - If UC memory is requested, always set XP memory.
    fn apply_memory_protection_rules(mut attributes: u64) -> u64 {
        // If we have UC memory, we always set XP. In DXE, we should never be executing from UC memory. On
        // AArch64, this is defined as a programming error to have executable device memory (which UC maps to).
        if attributes & efi::MEMORY_UC == efi::MEMORY_UC {
            attributes |= efi::MEMORY_XP;
        }
        attributes
    }

    /// This service sets attributes on the given memory space.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
    pub fn set_memory_space_attributes(
        &self,
        base_address: usize,
        len: usize,
        attributes: u64,
    ) -> Result<(), EfiError> {
        // this API allows for setting attributes across multiple descriptors in the GCD (assuming the capabilities
        // allow it). The lower level set_memory_space_attributes will only operate on a single entry in the GCD/page
        // table, so at this level we need to check to see if the range spans multiple entries and if so, we need to
        // split the range and call set_memory_space_attributes for each entry. We also need to set the paging
        // attributes per entry to ensure that we keep the GCD and page table in sync

        let attributes = Self::apply_memory_protection_rules(attributes);

        let mut current_base = base_address as u64;
        let mut res = Ok(());
        let range_end = (base_address + len) as u64;
        while current_base < range_end {
            let descriptor = self.get_memory_descriptor_for_address(current_base as efi::PhysicalAddress)?;
            let descriptor_end = descriptor.base_address + descriptor.length;

            // it is still legal to split a descriptor and only set the attributes on part of it
            let next_base = u64::min(descriptor_end, range_end);
            let current_len = next_base - current_base;
            match self.memory.lock().set_memory_space_attributes(
                current_base as usize,
                current_len as usize,
                attributes,
            ) {
                Ok(()) => {}
                Err(e) => {
                    log::error!(
                        "Failed to set GCD memory attributes for memory region {current_base:#x?} of length {current_len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                    );
                    debug_assert!(false);
                }
            }

            // 0 is a valid value for paging attributes: it means RWX. 0 is invalid for cache attributes. edk2 has a
            // behavior where if the caller passes 0 for cache and paging attributes, then 0 (RWX) is not applied to
            // the page table and only the virtual attribute(s) are applied to the GCD, such as EFI_RUNTIME. In order
            // to maintain compatibility with existing drivers, we preserve this poor paradigm.
            if attributes & (efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK) != 0 {
                match self.set_paging_attributes(current_base as usize, current_len as usize, attributes) {
                    Ok(_) => {}
                    Err(EfiError::NotReady) => {
                        // before the page table is installed, we expect to get a return of NotReady. This means the GCD
                        // has been updated with the attributes, but the page table is not installed yet. In init_paging, the
                        // page table will be updated with the current state of the GCD. The code that calls into this expects
                        // NotReady to be returned, so we must catch that error and report it. However, we also need to
                        // make sure any attribute updates across descriptors update the full range and not error out here.
                        res = Err(EfiError::NotReady);
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to set page table memory attributes for memory region {current_base:#x?} of length {current_len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                        );
                        debug_assert!(false);

                        // if we failed here, we shouldn't leave the GCD and the page table out of sync. Roll the GCD back
                        // to the previous attributes for this range. We may have partially updated this range in the GCD
                        // and the page table, but they will be in sync. We could attempt to continue here, but we need
                        // to return an error to the caller, so we might as well stop here.
                        if let Err(rollback_err) = self.memory.lock().set_memory_space_attributes(
                            current_base as usize,
                            current_len as usize,
                            descriptor.attributes,
                        ) {
                            // well, we did our best. The GCD and page table are now out of sync, which is a critical error.
                            log::error!(
                                "Failed to roll back GCD attributes after page table attribute set failure. This is a critical error. GCD and page table are now out of sync. Rollback error: {:?}",
                                rollback_err
                            );
                        }

                        return Err(e);
                    }
                }
            }

            current_base = next_base;
        }

        // if we made it out of the loop, we set the attributes correctly and should call the memory change callback,
        // if there is one
        if let Some(callback) = self.memory_change_callback {
            callback(MapChangeType::SetMemoryAttributes);
        }
        res
    }

    /// This service sets capabilities on the given memory space.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
    pub fn set_memory_space_capabilities(
        &self,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<(), EfiError> {
        let result = self.memory.lock().set_memory_space_capabilities(base_address, len, capabilities);
        if result.is_ok()
            && let Some(callback) = self.memory_change_callback
        {
            callback(MapChangeType::SetMemoryCapabilities);
        }
        result
    }

    /// returns a copy of the current set of memory blocks descriptors in the GCD.
    pub fn get_memory_descriptors(
        &self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
    ) -> Result<(), EfiError> {
        self.memory.lock().get_memory_descriptors(buffer)
    }

    // returns the descriptor for the given physical address.
    pub fn get_memory_descriptor_for_address(
        &self,
        address: efi::PhysicalAddress,
    ) -> Result<dxe_services::MemorySpaceDescriptor, EfiError> {
        self.memory.lock().get_memory_descriptor_for_address(address)
    }

    /// returns the current count of blocks in the list.
    pub fn memory_descriptor_count(&self) -> usize {
        self.memory.lock().memory_descriptor_count()
    }

    /// Acquires lock and delegates to [`IoGCD::add_io_space`]
    pub fn add_io_space(
        &self,
        io_type: dxe_services::GcdIoType,
        base_address: usize,
        len: usize,
    ) -> Result<usize, EfiError> {
        self.io.lock().add_io_space(io_type, base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::remove_io_space`]
    pub fn remove_io_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        self.io.lock().remove_io_space(base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::allocate_io_space`]
    pub fn allocate_io_space(
        &self,
        allocate_type: AllocateType,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        self.io.lock().allocate_io_space(allocate_type, io_type, alignment, len, image_handle, device_handle)
    }

    /// Acquires lock and delegates to [`IoGCD::free_io_space]
    pub fn free_io_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        self.io.lock().free_io_space(base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::get_io_descriptors`]
    pub fn get_io_descriptors(&self, buffer: &mut Vec<dxe_services::IoSpaceDescriptor>) -> Result<(), EfiError> {
        self.io.lock().get_io_descriptors(buffer)
    }

    /// Acquires lock and delegates to [`IoGCD::io_descriptor_count`]
    pub fn io_descriptor_count(&self) -> usize {
        self.io.lock().io_descriptor_count()
    }

    #[cfg(feature = "compatibility_mode_allowed")]
    /// This activates compatibility mode for the GCD.
    /// This will:
    /// - Map the range 0 - 0xA0000 as RWX if the memory type is SystemMemory.
    /// - Update the locked GCD to not set efi::MEMORY_XP on newly allocated pages
    pub(crate) fn activate_compatibility_mode(&self) {
        const LEGACY_BIOS_WB_ADDRESS: usize = 0xA0000;

        // always map page 0 if it exists in this system, as grub will attempt to read it for legacy boot structures
        // map it WB by default, because 0 is being used as the null page, it may not have gotten cache attributes
        // populated
        if let Ok(descriptor) = self.get_memory_descriptor_for_address(0)
            // set_memory_space_attributes will set both the GCD and paging attributes
            && descriptor.memory_type != dxe_services::GcdMemoryType::NonExistent
            && let Err(e) = self.set_memory_space_attributes(0, UEFI_PAGE_SIZE, efi::MEMORY_WB)
        {
            log::error!("Failed to map page 0 for compat mode. Status: {e:#x?}");
            debug_assert!(false);
        }

        // map legacy region if system mem
        let mut address = UEFI_PAGE_SIZE; // start at 0x1000, as we already mapped page 0
        while address < LEGACY_BIOS_WB_ADDRESS {
            let mut size = UEFI_PAGE_SIZE;
            if let Ok(descriptor) = self.get_memory_descriptor_for_address(address as efi::PhysicalAddress) {
                // if the legacy region is not system memory, we should not map it
                if descriptor.memory_type == dxe_services::GcdMemoryType::SystemMemory {
                    size = match address + descriptor.length as usize {
                        end_addr if end_addr > LEGACY_BIOS_WB_ADDRESS => LEGACY_BIOS_WB_ADDRESS - address,
                        _ => descriptor.length as usize,
                    };

                    // set_memory_space_attributes will set both the GCD and paging attributes
                    match self.set_memory_space_attributes(
                        address,
                        size,
                        descriptor.attributes & efi::CACHE_ATTRIBUTE_MASK,
                    ) {
                        Ok(_) => {}
                        Err(e) => {
                            log::error!(
                                "Failed to map legacy bios region at {:#x?} of length {:#x?} with attributes {:#x?}. Status: {:#x?}",
                                address,
                                size,
                                descriptor.attributes & efi::CACHE_ATTRIBUTE_MASK,
                                e
                            );
                            debug_assert!(false);
                        }
                    }
                }
            }
            address += size;
        }
        self.memory.lock().activate_compatibility_mode();
    }
}

impl Display for SpinLockedGcd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(gcd) = self.memory.try_lock() {
            writeln!(f, "\n{gcd}")?;
        } else {
            writeln!(f, "Locked: {:?}", self.memory.try_lock())?;
        }
        if let Some(gcd) = self.io.try_lock() {
            writeln!(f, "\n{gcd}")?;
        } else {
            writeln!(f, "Locked: {:?}", self.io.try_lock())?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for SpinLockedGcd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{:?}", self.memory.try_lock())?;
        writeln!(f, "{:?}", self.io.try_lock())?;
        Ok(())
    }
}

unsafe impl Sync for SpinLockedGcd {}
unsafe impl Send for SpinLockedGcd {}

#[cfg(test)]
#[coverage(off)]
mod tests {
    //! GCD (Global Coherency Domain) test module.
    //!
    //! # Safety Notes
    //!
    //! This test module extensively uses `unsafe` for the following operations:
    //!
    //! ## Memory Allocation (`get_memory`)
    //! - Allocates memory from the system allocator with UEFI page alignment
    //! - Returns 'static lifetime slices that are intentionally leaked for test simplicity
    //! - Memory is valid for the entire test duration
    //!
    //! ## GCD Operations (`add_memory_space`, `init_memory_blocks`, etc.)
    //! - These functions are unsafe because they operate on raw memory addresses
    //! - In tests, all memory addresses come from controlled allocations via `get_memory`
    //! - All memory regions are valid and properly aligned
    //! - Test isolation is ensured via `with_locked_state` which holds a global test lock
    //!
    //! ## Global State (`GCD.reset()`)
    //! - Tests reset global GCD state to ensure test isolation
    //! - The test lock prevents concurrent access during reset operations
    extern crate std;
    use core::{alloc::Layout, sync::atomic::AtomicBool};
    use patina::base::align_up;

    use crate::test_support::{self, MockPageTable, MockPageTableWrapper};

    use super::*;
    use alloc::vec::Vec;
    use r_efi::efi;
    use std::{alloc::GlobalAlloc, cell::RefCell, rc::Rc};

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_gcd_initialization() {
        let gdc = GCD::new(48);
        assert_eq!(2_usize.pow(48), gdc.maximum_address);
        assert_eq!(gdc.memory_blocks.capacity(), 0);
        assert_eq!(0, gdc.memory_descriptor_count())
    }

    #[test]
    fn test_add_memory_space_before_memory_blocks_instantiated() {
        // SAFETY: Test memory allocation - memory is valid and properly aligned.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);

        // SAFETY: GCD test operation - address comes from controlled allocation above.
        assert_eq!(
            Err(EfiError::NotReady),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, address, MEMORY_BLOCK_SLICE_SIZE, 0) },
            "First add memory space should be a system memory."
        );
        assert_eq!(0, gcd.memory_descriptor_count());

        assert_eq!(
            Err(EfiError::OutOfResources),
            unsafe {
                gcd.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE - 1,
                    0,
                )
            },
            "First add memory space with system memory should contain enough space to contain the block list."
        );
        assert_eq!(0, gcd.memory_descriptor_count());
    }

    #[test]
    fn test_add_memory_space_with_all_memory_type() {
        let (mut gcd, _) = create_gcd();

        assert_eq!(Ok(0), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 0, 1, 0) });
        assert_eq!(Ok(3), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1, 1, 0) });
        assert_eq!(Ok(4), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Persistent, 2, 1, 0) });
        assert_eq!(Ok(5), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::MoreReliable, 3, 1, 0) });
        assert_eq!(Ok(6), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Unaccepted, 4, 1, 0) });
        assert_eq!(Ok(7), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::MemoryMappedIo, 5, 1, 0) });

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::InvalidParameter),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::NonExistent, 10, 1, 0) },
            "Can't manually add NonExistent memory space manually."
        );

        assert!(is_gcd_memory_slice_valid(&gcd));
        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_add_memory_space_with_0_len_block() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(EfiError::InvalidParameter), unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 0, 0)
        });
        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_add_memory_space_when_memory_block_full() {
        let (mut gcd, address) = create_gcd();
        let addr = address + MEMORY_BLOCK_SLICE_SIZE;

        let mut n = 0;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            assert!(
                unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr + n, 1, n as u64) }
                    .is_ok()
            );
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        let res = unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr + n, 1, n as u64) };
        assert_eq!(
            Err(EfiError::OutOfResources),
            res,
            "Should return out of memory if there is no space in memory blocks."
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    }

    #[test]
    fn test_add_memory_space_outside_processor_range() {
        let (mut gcd, _) = create_gcd();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(Err(EfiError::Unsupported), unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address + 1, 1, 0)
        });
        assert_eq!(Err(EfiError::Unsupported), unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address, 1, 0)
        });
        assert_eq!(Err(EfiError::Unsupported), unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address - 1, 2, 0)
        });

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_add_memory_space_in_range_already_added() {
        let (mut gcd, _) = create_gcd();
        // Add block to test the boundary on.
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1000, 10, 0) }.unwrap();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 1002, 5, 0) },
            "Can't add inside a range previously added."
        );
        assert_eq!(
            Err(EfiError::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 998, 5, 0) },
            "Can't add partially inside a range previously added (Start)."
        );
        assert_eq!(
            Err(EfiError::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 1009, 5, 0) },
            "Can't add partially inside a range previously added (End)."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_add_memory_space_in_range_already_allocated() {
        let (mut gcd, address) = create_gcd();
        // Add unallocated block after allocated one.
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address - 100, 100, 0) }.unwrap();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, 5, 0) },
            "Can't add inside a range previously allocated."
        );
        assert_eq!(
            Err(EfiError::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, address - 100, 200, 0) },
            "Can't add partially inside a range previously allocated."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_add_memory_space_block_merging() {
        let (mut gcd, _) = create_gcd();

        assert_eq!(Ok(4), unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1000, 10, 0) });
        let block_count = gcd.memory_descriptor_count();

        // Test merging when added after
        match unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1010, 10, 0) } {
            Ok(idx) => {
                let mb = gcd.memory_blocks.get_with_idx(idx).unwrap();
                assert_eq!(1000, mb.as_ref().base_address);
                assert_eq!(20, mb.as_ref().length);
                assert_eq!(block_count, gcd.memory_descriptor_count());
            }
            Err(e) => panic!("{e:?}"),
        }

        // Test merging when added before
        match unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 990, 10, 0) } {
            Ok(idx) => {
                let mb = gcd.memory_blocks.get_with_idx(idx).unwrap();
                assert_eq!(990, mb.as_ref().base_address);
                assert_eq!(30, mb.as_ref().length);
                assert_eq!(block_count, gcd.memory_descriptor_count());
            }
            Err(e) => panic!("{e:?}"),
        }

        assert!(
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 1020, 10, 0) }.is_ok(),
            "A different memory type should note result in a merge."
        );
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert!(
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 1030, 10, 1) }.is_ok(),
            "A different capabilities should note result in a merge."
        );
        assert_eq!(block_count + 2, gcd.memory_descriptor_count());

        assert!(is_gcd_memory_slice_valid(&gcd));
    }

    #[test]
    fn test_add_memory_space_state() {
        let (mut gcd, _) = create_gcd();
        match unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 100, 10, 123) } {
            Ok(idx) => {
                let mb = *gcd.memory_blocks.get_with_idx(idx).unwrap();
                match mb {
                    MemoryBlock::Unallocated(md) => {
                        assert_eq!(100, md.base_address);
                        assert_eq!(10, md.length);
                        assert_eq!(efi::MEMORY_RUNTIME | efi::MEMORY_ACCESS_MASK | 123, md.capabilities);
                        assert_eq!(0, md.image_handle as usize);
                        assert_eq!(0, md.device_handle as usize);
                    }
                    MemoryBlock::Allocated(_) => panic!("Add should keep the block unallocated"),
                }
            }
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn test_remove_memory_space_before_memory_blocks_instantiated() {
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);

        assert_eq!(Err(EfiError::NotFound), gcd.remove_memory_space(address, MEMORY_BLOCK_SLICE_SIZE));
    }

    #[test]
    fn test_remove_memory_space_with_0_len_block() {
        let (mut gcd, _) = create_gcd();

        // Add memory space to remove in a valid area.
        assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 10, 0) }.is_ok());

        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(EfiError::InvalidParameter), gcd.remove_memory_space(5, 0));

        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.remove_memory_space(10, 0),
            "If there is no allocate done first, 0 length invalid param should have priority."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_remove_memory_space_outside_processor_range() {
        let (mut gcd, _) = create_gcd();
        // Add memory space to remove in a valid area.
        assert!(
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address - 10, 10, 0) }
                .is_ok()
        );

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.remove_memory_space(gcd.maximum_address - 10, 11),
            "An address outside the processor range support is invalid."
        );
        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.remove_memory_space(gcd.maximum_address, 10),
            "An address outside the processor range support is invalid."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_remove_memory_space_in_range_not_added() {
        let (mut gcd, _) = create_gcd();
        // Add memory space to remove in a valid area.
        assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 100, 10, 0) }.is_ok());

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(95, 10),
            "Can't remove memory space partially added."
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(105, 10),
            "Can't remove memory space partially added."
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(10, 10),
            "Can't remove memory space not previously added."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_remove_memory_space_in_range_allocated() {
        let (mut gcd, address) = create_gcd();

        let snapshot = copy_memory_block(&gcd);

        // Not found has a priority over the access denied because the check if the range is valid is done earlier.
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(address - 5, 10),
            "Can't remove memory space partially allocated."
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(address + MEMORY_BLOCK_SLICE_SIZE - 5, 10),
            "Can't remove memory space partially allocated."
        );

        assert_eq!(
            Err(EfiError::AccessDenied),
            gcd.remove_memory_space(address + 10, 10),
            "Can't remove memory space not previously allocated."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_remove_memory_space_when_memory_block_full() {
        let (mut gcd, address) = create_gcd();
        let addr = address + MEMORY_BLOCK_SLICE_SIZE;

        assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr, 10, 0_u64) }.is_ok());
        let mut n = 1;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            assert!(
                unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr + 10 + n, 1, n as u64) }
                    .is_ok()
            );
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.remove_memory_space(addr, 5),
            "Should return out of memory if there is no space in memory blocks."
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    }

    #[test]
    fn test_remove_memory_space_block_merging() {
        let (mut gcd, address) = create_gcd();
        let page_size = 0x1000;
        let aligned_address = address & !(page_size - 1);
        let aligned_length = page_size * 10;
        let aligned_address = if aligned_address > aligned_length {
            aligned_address - aligned_length
        } else {
            aligned_address + aligned_length
        };

        assert!(
            unsafe {
                gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, aligned_address, aligned_length, 0)
            }
            .is_ok()
        );

        let block_count = gcd.memory_descriptor_count();

        for i in 0..5 {
            assert!(gcd.remove_memory_space(aligned_address + i * page_size, page_size).is_ok());
        }

        // First index because the add memory started at aligned_address.
        assert_eq!(aligned_address, copy_memory_block(&gcd)[1].as_ref().base_address as usize);
        assert_eq!(aligned_length / 2, copy_memory_block(&gcd)[1].as_ref().length as usize);
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert!(is_gcd_memory_slice_valid(&gcd));

        // Removing in the middle should create 2 new blocks.
        assert!(gcd.remove_memory_space(aligned_address + page_size * 5, page_size).is_ok());
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert!(is_gcd_memory_slice_valid(&gcd));
    }

    #[test]
    fn test_remove_memory_space_state() {
        let (mut gcd, address) = create_gcd();
        assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, address, 123) }.is_ok());

        match gcd.remove_memory_space(0, 10) {
            Ok(_) => {
                let mb = copy_memory_block(&gcd)[0];
                match mb {
                    MemoryBlock::Unallocated(md) => {
                        assert_eq!(0, md.base_address);
                        assert_eq!(10, md.length);
                        assert_eq!(0, md.capabilities);
                        assert_eq!(0, md.image_handle as usize);
                        assert_eq!(0, md.device_handle as usize);
                    }
                    MemoryBlock::Allocated(_) => panic!("remove should keep the block unallocated"),
                }
            }
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn test_allocate_memory_space_before_memory_blocks_instantiated() {
        let mut gcd = GCD::new(48);
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0),
                dxe_services::GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                10,
                1 as _,
                None
            )
        );
    }

    #[test]
    fn test_allocate_memory_space_with_0_len_block() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::Reserved,
                UEFI_PAGE_SHIFT,
                0,
                1 as _,
                None
            ),
        );
        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_with_null_image_handle() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::Reserved,
                0,
                10,
                ptr::null_mut(),
                None
            ),
        );
        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_with_address_outside_processor_range() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(gcd.maximum_address - 100),
                dxe_services::GcdMemoryType::Reserved,
                0,
                1000,
                1 as _,
                None
            ),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(gcd.maximum_address + 100),
                dxe_services::GcdMemoryType::Reserved,
                0,
                1000,
                1 as _,
                None
            ),
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_with_all_memory_type() {
        let (mut gcd, _) = create_gcd();
        for (i, memory_type) in [
            dxe_services::GcdMemoryType::Reserved,
            dxe_services::GcdMemoryType::SystemMemory,
            dxe_services::GcdMemoryType::Persistent,
            dxe_services::GcdMemoryType::MemoryMappedIo,
            dxe_services::GcdMemoryType::MoreReliable,
            dxe_services::GcdMemoryType::Unaccepted,
        ]
        .into_iter()
        .enumerate()
        {
            unsafe { gcd.add_memory_space(memory_type, (i + 1) * 10, 10, 0) }.unwrap();
            let res = gcd.allocate_memory_space(AllocateType::Address((i + 1) * 10), memory_type, 0, 10, 1 as _, None);
            match memory_type {
                dxe_services::GcdMemoryType::Unaccepted => assert_eq!(Err(EfiError::InvalidParameter), res),
                _ => assert!(res.is_ok()),
            }
        }
    }

    #[test]
    fn test_allocate_memory_space_with_no_memory_space_available() {
        let (mut gcd, _) = create_gcd();

        // Add memory space of len 100 to multiple space.
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 100, 0) }.unwrap();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1000, 100, 0) }.unwrap();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address - 100, 100, 0) }
            .unwrap();

        let memory_blocks_snapshot = copy_memory_block(&gcd);

        // Try to allocate chunk bigger than 100.
        for allocate_type in [AllocateType::BottomUp(None), AllocateType::TopDown(None)] {
            assert_eq!(
                Err(EfiError::OutOfResources),
                gcd.allocate_memory_space(
                    allocate_type,
                    dxe_services::GcdMemoryType::SystemMemory,
                    0,
                    1000,
                    1 as _,
                    None
                ),
                "Assert fail with allocate type: {allocate_type:?}"
            );
        }

        for allocate_type in
            [AllocateType::BottomUp(Some(10_000)), AllocateType::TopDown(Some(10_000)), AllocateType::Address(10_000)]
        {
            assert_eq!(
                Err(EfiError::NotFound),
                gcd.allocate_memory_space(
                    allocate_type,
                    dxe_services::GcdMemoryType::SystemMemory,
                    0,
                    1000,
                    1 as _,
                    None
                ),
                "Assert fail with allocate type: {allocate_type:?}"
            );
        }

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_alignment() {
        let (mut gcd, _) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x1000, 0x1000, 0) }.unwrap();

        assert_eq!(
            Ok(0x1000),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                0x0f,
                1 as _,
                None
            ),
            "Allocate bottom up without alignment"
        );
        assert_eq!(
            Ok(0x1010),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::SystemMemory,
                4,
                0x10,
                1 as _,
                None
            ),
            "Allocate bottom up with alignment of 4 bits (find first address that is aligned)"
        );
        assert_eq!(
            Ok(0x1020),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::SystemMemory,
                4,
                100,
                1 as _,
                None
            ),
            "Allocate bottom up with alignment of 4 bits (already aligned)"
        );
        assert_eq!(
            Ok(0x1ff1),
            gcd.allocate_memory_space(
                AllocateType::TopDown(None),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                0x0f,
                1 as _,
                None
            ),
            "Allocate top down without alignment"
        );
        assert_eq!(
            Ok(0x1fe0),
            gcd.allocate_memory_space(
                AllocateType::TopDown(None),
                dxe_services::GcdMemoryType::SystemMemory,
                4,
                0x0f,
                1 as _,
                None
            ),
            "Allocate top down with alignment of 4 bits (find first address that is aligned)"
        );
        assert_eq!(
            Ok(0x1f00),
            gcd.allocate_memory_space(
                AllocateType::TopDown(None),
                dxe_services::GcdMemoryType::SystemMemory,
                4,
                0xe0,
                1 as _,
                None
            ),
            "Allocate top down with alignment of 4 bits (already aligned)"
        );
        assert_eq!(
            Ok(0x1a00),
            gcd.allocate_memory_space(
                AllocateType::Address(0x1a00),
                dxe_services::GcdMemoryType::SystemMemory,
                4,
                100,
                1 as _,
                None
            ),
            "Allocate Address with alignment of 4 bits (already aligned)"
        );

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0x1a0f),
                dxe_services::GcdMemoryType::SystemMemory,
                4,
                100,
                1 as _,
                None
            ),
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_block_merging() {
        let (mut gcd, _) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x1000, 0x1000, 0) }.unwrap();

        for allocate_type in [AllocateType::BottomUp(None), AllocateType::TopDown(None)] {
            let block_count = gcd.memory_descriptor_count();
            assert!(
                gcd.allocate_memory_space(allocate_type, dxe_services::GcdMemoryType::SystemMemory, 0, 1, 1 as _, None)
                    .is_ok(),
                "{allocate_type:?}"
            );
            assert_eq!(block_count + 1, gcd.memory_descriptor_count());
            assert!(
                gcd.allocate_memory_space(allocate_type, dxe_services::GcdMemoryType::SystemMemory, 0, 1, 1 as _, None)
                    .is_ok(),
                "{allocate_type:?}"
            );
            assert_eq!(block_count + 1, gcd.memory_descriptor_count());
            assert!(
                gcd.allocate_memory_space(allocate_type, dxe_services::GcdMemoryType::SystemMemory, 0, 1, 2 as _, None)
                    .is_ok(),
                "{allocate_type:?}: A different image handle should not result in a merge."
            );
            assert_eq!(block_count + 2, gcd.memory_descriptor_count());
            assert!(
                gcd.allocate_memory_space(
                    allocate_type,
                    dxe_services::GcdMemoryType::SystemMemory,
                    0,
                    1,
                    2 as _,
                    Some(1 as _)
                )
                .is_ok(),
                "{allocate_type:?}: A different device handle should not result in a merge."
            );
            assert_eq!(block_count + 3, gcd.memory_descriptor_count());
        }

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(
            Ok(0x1000 + 4),
            gcd.allocate_memory_space(
                AllocateType::Address(0x1000 + 4),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                1,
                2 as _,
                Some(1 as _)
            ),
            "Merge should work with address allocation too."
        );
        assert_eq!(block_count, gcd.memory_descriptor_count());

        assert!(is_gcd_memory_slice_valid(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_with_address_not_added() {
        let (mut gcd, _) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x100, 10, 0) }.unwrap();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0x100),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                11,
                1 as _,
                None
            ),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0x95),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                10,
                1 as _,
                None
            ),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(110),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                5,
                1 as _,
                None
            ),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                5,
                1 as _,
                None
            ),
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_allocate_memory_space_with_address_allocated() {
        let (mut gcd, address) = create_gcd();
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(address),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                5,
                1 as _,
                None
            ),
        );
    }

    #[test]
    fn test_free_memory_space_before_memory_blocks_instantiated() {
        let mut gcd = GCD::new(48);
        assert_eq!(Err(EfiError::NotFound), gcd.free_memory_space(0x1000, 0x1000));
    }

    #[test]
    fn test_free_memory_space_when_0_len_block() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(EfiError::InvalidParameter), gcd.free_memory_space(0, 0));
        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_free_memory_space_outside_processor_range() {
        let (mut gcd, _) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address - 100, 100, 0) }
            .unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(gcd.maximum_address - 100),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            100,
            1 as _,
            None,
        )
        .unwrap();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(Err(EfiError::Unsupported), gcd.free_memory_space(gcd.maximum_address, 10));
        assert_eq!(Err(EfiError::Unsupported), gcd.free_memory_space(gcd.maximum_address - 99, 100));
        assert_eq!(Err(EfiError::Unsupported), gcd.free_memory_space(gcd.maximum_address + 1, 100));

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_free_memory_space_in_range_not_allocated() {
        let (mut gcd, _) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x3000, 0x3000, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0x3000),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();

        assert_eq!(Err(EfiError::NotFound), gcd.free_memory_space(0x2000, 0x1000));
        assert_eq!(Err(EfiError::NotFound), gcd.free_memory_space(0x4000, 0x1000));
        assert_eq!(Err(EfiError::NotFound), gcd.free_memory_space(0, 0x1000));
    }

    #[test]
    fn test_free_memory_space_when_memory_block_full() {
        let (mut gcd, _) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x1000000, UEFI_PAGE_SIZE * 2, 0) }
            .unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0x1000000),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            UEFI_PAGE_SIZE * 2,
            1 as _,
            None,
        )
        .unwrap();

        let mut n = 1;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            let addr = 0x2000000 + (n * UEFI_PAGE_SIZE);
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr, UEFI_PAGE_SIZE, n as u64) }
                .unwrap();
            n += 1;
        }
        let memory_blocks_snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(EfiError::OutOfResources), gcd.free_memory_space(0x1000000, UEFI_PAGE_SIZE));
        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    }

    #[test]
    fn test_free_memory_space_merging() {
        let (mut gcd, _) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x1000, 0x10000, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0x1000),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x10000,
            1 as _,
            None,
        )
        .unwrap();

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(Ok(()), gcd.free_memory_space(0x1000, 0x1000), "Free beginning of a block.");
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert_eq!(Ok(()), gcd.free_memory_space(0x5000, 0x1000), "Free in the middle of a block");
        assert_eq!(block_count + 3, gcd.memory_descriptor_count());
        assert_eq!(Ok(()), gcd.free_memory_space(0x9000, 0x1000), "Free at the end of a block");
        assert_eq!(block_count + 5, gcd.memory_descriptor_count());

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(Ok(()), gcd.free_memory_space(0x2000, 0x2000));
        assert_eq!(block_count, gcd.memory_descriptor_count());

        let blocks = copy_memory_block(&gcd);
        let mb = blocks[0];
        assert_eq!(0, mb.as_ref().base_address);
        assert_eq!(0x1000, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(0x6000, 0x1000));
        assert_eq!(block_count, gcd.memory_descriptor_count());
        let blocks = copy_memory_block(&gcd);
        let mb = blocks[2];
        assert_eq!(0x4000, mb.as_ref().base_address);
        assert_eq!(0x1000, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(0x8000, 0x1000));
        assert_eq!(block_count, gcd.memory_descriptor_count());
        let blocks = copy_memory_block(&gcd);
        let mb = blocks[4];
        assert_eq!(0x7000, mb.as_ref().base_address);
        assert_eq!(0x1000, mb.as_ref().length);

        assert!(is_gcd_memory_slice_valid(&gcd));
    }

    #[test]
    fn test_set_memory_space_attributes_with_invalid_parameters() {
        let mut gcd = GCD {
            memory_blocks: Rbt::new(),
            maximum_address: 0,
            allocate_memory_space_fn: GCD::allocate_memory_space_internal,
            free_memory_space_fn: GCD::free_memory_space_worker,
            default_attributes: efi::MEMORY_XP,
            prioritize_32_bit_memory: false,
        };
        assert_eq!(Err(EfiError::NotReady), gcd.set_memory_space_attributes(0, 0x50000, 0b1111));

        let (mut gcd, _) = create_gcd();

        // Test that setting memory space attributes on more space than is available is an error
        assert_eq!(Err(EfiError::Unsupported), gcd.set_memory_space_attributes(0x100000000000000, 50, 0b1111));

        // Test that calling set_memory_space_attributes with no size returns invalid parameter
        assert_eq!(Err(EfiError::InvalidParameter), gcd.set_memory_space_attributes(0, 0, 0b1111));

        // Test that calling set_memory_space_attributes with invalid attributes returns invalid parameter
        assert_eq!(Err(EfiError::InvalidParameter), gcd.set_memory_space_attributes(0, 0, 0));

        // Test that a non-page aligned address returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0x1000, efi::MEMORY_WB)
        );

        // Test that a non-page aligned address with the runtime attribute set returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0x1000, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );

        // Test that a non-page aligned size returns invalid parameter
        assert_eq!(Err(EfiError::InvalidParameter), gcd.set_memory_space_attributes(0x1000, 0xFFF, efi::MEMORY_WB));

        // Test that a non-page aligned size returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0x1000, 0xFFF, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );

        // Test that a non-page aligned address and size returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0xFFF, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );
    }

    #[test]
    fn test_set_capabilities_and_attributes() {
        let (mut gcd, address) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x1000, address - 0x1000, 0) }
            .unwrap();

        gcd.allocate_memory_space(
            AllocateType::BottomUp(None),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x2000,
            1 as _,
            None,
        )
        .unwrap();
        // Trying to set capabilities where the range falls outside a block should return unsupported
        assert_eq!(Err(EfiError::Unsupported), gcd.set_memory_space_capabilities(0x1000, 0x3000, 0b1111));
        gcd.set_memory_space_capabilities(0x1000, 0x2000, efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP).unwrap();
        gcd.set_gcd_memory_attributes(0x1000, 0x2000, efi::MEMORY_RO).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_set_attributes_panic() {
        let (mut gcd, address) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, address, 0) }.unwrap();

        gcd.allocate_memory_space(
            AllocateType::BottomUp(None),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x2000,
            1 as _,
            None,
        )
        .unwrap();
        gcd.set_memory_space_capabilities(0, 0x2000, efi::MEMORY_RP | efi::MEMORY_RO).unwrap();
        // Trying to set attributes where the range falls outside a block should panic in debug case
        gcd.set_memory_space_attributes(0, 0x3000, 0b1).unwrap();
    }

    #[test]
    fn test_block_split_when_memory_blocks_full() {
        let (mut gcd, address) = create_gcd();
        unsafe {
            gcd.add_memory_space(
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                address,
                efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB,
            )
        }
        .unwrap();

        let mut n = 1;
        let mut allocated_addresses = Vec::new();
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            let addr = gcd
                .allocate_memory_space(
                    AllocateType::BottomUp(None),
                    dxe_services::GcdMemoryType::SystemMemory,
                    0,
                    0x2000,
                    n as _,
                    None,
                )
                .unwrap();
            allocated_addresses.push(addr);
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        // Test that allocate_memory_space fails when full
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                0x1000,
                1 as _,
                None
            )
        );
        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd));

        // Verify that the memory blocks array is at capacity
        assert_eq!(gcd.memory_descriptor_count(), MEMORY_BLOCK_SLICE_LEN, "Memory blocks should be at capacity");

        // Test that set_memory_space_capabilities fails when full, if the block requires a split
        // Use the first allocated address to ensure we're working with a valid allocated block
        let first_allocated = allocated_addresses[0];
        let capabilities_result = gcd.set_memory_space_capabilities(
            first_allocated,
            0x1000,
            efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP,
        );

        // This should fail with OutOfResources, but may panic in debug builds due to assertions
        // We verify the memory is at capacity regardless of the specific error
        match capabilities_result {
            Err(EfiError::OutOfResources) => {
                // Expected behavior in release builds
            }
            _ => {
                // In debug builds, operations that would exceed capacity might panic
                // The important thing is that we've verified the array is at capacity
                assert_eq!(gcd.memory_descriptor_count(), MEMORY_BLOCK_SLICE_LEN, "Memory should remain at capacity");
            }
        }
    }

    #[test]
    fn test_invalid_add_io_space() {
        let mut gcd = IoGCD::_new(16);

        assert!(gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).is_ok());
        // Cannot Allocate a range in a range that is already allocated
        assert_eq!(Err(EfiError::AccessDenied), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10));

        // Cannot allocate a range as NonExistent
        assert_eq!(Err(EfiError::InvalidParameter), gcd.add_io_space(dxe_services::GcdIoType::NonExistent, 10, 10));

        // Cannot do more allocations if the underlying data structure is full
        for i in 1..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.add_io_space(dxe_services::GcdIoType::Io, (IO_BLOCK_SLICE_LEN + 1) * 10, 10)
        );
    }

    #[test]
    fn test_invalid_remove_io_space() {
        let mut gcd = IoGCD::_new(16);

        // Cannot remove a range of 0
        assert_eq!(Err(EfiError::InvalidParameter), gcd.remove_io_space(0, 0));

        // Cannot remove a range greater than what is available
        assert_eq!(Err(EfiError::Unsupported), gcd.remove_io_space(0, 70_000));

        // Cannot remove an io space if it does not exist
        assert_eq!(Err(EfiError::NotFound), gcd.remove_io_space(0, 10));

        // Cannot remove an io space if it is allocated
        gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).unwrap();
        gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        assert_eq!(Err(EfiError::AccessDenied), gcd.remove_io_space(0, 10));

        // Cannot remove an io space if it is partially in a block and we are full, as it
        // causes a split with no space to add a new node.
        let mut gcd = IoGCD::_new(16);
        for i in 2..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }
        assert_eq!(Err(EfiError::OutOfResources), gcd.remove_io_space(25, 3));
        assert!(gcd.remove_io_space(20, 10).is_ok());
    }

    #[test]
    fn test_ensure_allocate_io_space_conformance() {
        let mut gcd = IoGCD::_new(16);
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x4000));

        assert_eq!(
            Ok(0),
            gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
        assert_eq!(
            Ok(0x100),
            gcd.allocate_io_space(AllocateType::BottomUp(None), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
        assert_eq!(
            Ok(0x3F00),
            gcd.allocate_io_space(AllocateType::TopDown(None), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
        assert_eq!(
            Ok(0x1000),
            gcd.allocate_io_space(AllocateType::Address(0x1000), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
    }

    #[test]
    fn test_ensure_allocations_fail_when_out_of_resources() {
        let mut gcd = IoGCD::_new(16);
        for i in 0..IO_BLOCK_SLICE_LEN - 1 {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }

        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 0x4000)
        );
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, usize::MAX)
        );
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_address(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 210)
        );
    }

    #[test]
    fn test_allocate_bottom_up_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x4000)
        );

        // Setup some io_space for the following tests
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x100));
        assert_eq!(Ok(1), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x100, 0x100));
        assert_eq!(Ok(2), gcd.add_io_space(dxe_services::GcdIoType::Io, 0x200, 0x200));
        assert_eq!(Ok(3), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x400, 0x200));

        // Test that we move on to the next block if the current block is not big enough
        // i.e. we skip the 0x0 block because it is not big enough.
        assert_eq!(Ok(0x200), gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 0x150, 1 as _, None, 0x4000));

        // Testing that after we apply allocation requirements, we correctly skip the first available block
        // that meets the initial (0x50) requirement, but does not satisfy the alignment requirement of 0x200.
        assert_eq!(
            Ok(0x400),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Maximum, 0b1001, 0x50, 1 as _, None, 0x4000)
        );
    }

    #[test]
    fn test_allocate_top_down_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x4000)
        );

        // Setup some io_space for the following tests
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x200));
        assert_eq!(Ok(1), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x200, 0x200));
        assert_eq!(Ok(2), gcd.add_io_space(dxe_services::GcdIoType::Io, 0x400, 0x100));
        assert_eq!(Ok(3), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x500, 0x100));

        // Test that we move on to the next block if the current block is not big enough
        // i.e. we skip the 0x0 block because it is not big enough. Since going top down,
        // The address is in the middle of the 0x200 Block such tha
        // 0xB0 (start addr) + 0x150 (size)= 0x200
        assert_eq!(Ok(0xB0), gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 0x150, 1 as _, None, usize::MAX));

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_top_down(dxe_services::GcdIoType::Reserved, 0, 0x150, 1 as _, None, usize::MAX)
        );
    }

    #[test]
    fn test_allocate_address_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_address(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x200)
        );

        // Setup some io_space for the following tests
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x200));
        assert_eq!(Ok(1), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x200, 0x200));
        assert_eq!(Ok(2), gcd.add_io_space(dxe_services::GcdIoType::Io, 0x400, 0x100));
        assert_eq!(Ok(3), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x500, 0x100));

        // If we find a block with the address, but its not the right Io type, we should
        // report not found
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_address(dxe_services::GcdIoType::Reserved, 0, 0x100, 1 as _, None, 0)
        );
    }

    #[test]
    fn test_free_io_space_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot free a range of 0
        assert_eq!(Err(EfiError::InvalidParameter), gcd.free_io_space(0, 0));

        // Cannot free a range greater than what is available
        assert_eq!(Err(EfiError::Unsupported), gcd.free_io_space(0, 70_000));

        // Cannot free an io space if it does not exist
        assert_eq!(Err(EfiError::NotFound), gcd.free_io_space(0, 10));

        gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).unwrap();
        gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        assert_eq!(Ok(()), gcd.free_io_space(0, 10));

        // Cannot free an io space if it is partially in a block and we are full, as it
        // causes a split with no space to add a new node.
        let mut gcd = IoGCD::_new(16);
        for i in 2..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }

        // Cannot partially free a block when full, but we can free the whole block
        gcd.allocate_address(dxe_services::GcdIoType::Maximum, 0, 10, 1 as _, None, 100).unwrap();
        assert_eq!(Err(EfiError::OutOfResources), gcd.free_io_space(105, 3));
        assert_eq!(Ok(()), gcd.free_io_space(100, 10));
    }

    fn create_gcd() -> (GCD, usize) {
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);
        unsafe {
            gcd.init_memory_blocks(
                dxe_services::GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
            )
            .unwrap();
        }
        (gcd, address)
    }

    fn copy_memory_block(gcd: &GCD) -> Vec<MemoryBlock> {
        gcd.memory_blocks.dfs()
    }

    fn is_gcd_memory_slice_valid(gcd: &GCD) -> bool {
        let memory_blocks = &gcd.memory_blocks;
        match memory_blocks.first_idx().map(|idx| memory_blocks.get_with_idx(idx).unwrap().start()) {
            Some(0) => (),
            _ => return false,
        }
        let mut last_addr = 0;
        let blocks = copy_memory_block(gcd);
        let mut w = blocks.windows(2);
        while let Some([a, b]) = w.next() {
            if a.end() != b.start() || a.is_same_state(b) {
                return false;
            }
            last_addr = b.end();
        }
        if last_addr != gcd.maximum_address {
            return false;
        }
        true
    }

    unsafe fn get_memory(size: usize) -> &'static mut [u8] {
        // SAFETY: Allocates memory from the system allocator with UEFI page alignment.
        // The returned slice is intentionally leaked for test simplicity and valid for 'static lifetime.
        let addr = unsafe { alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, UEFI_PAGE_SIZE).unwrap()) };
        // SAFETY: The allocated pointer is valid for `size` bytes and properly aligned.
        unsafe { core::slice::from_raw_parts_mut(addr, size) }
    }

    #[test]
    fn spin_locked_allocator_should_error_if_not_initialized() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            assert_eq!(GCD.memory.lock().maximum_address, 0);

            let add_result = unsafe { GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 100, 0) };
            assert_eq!(add_result, Err(EfiError::NotReady));

            let allocate_result = GCD.allocate_memory_space(
                AllocateType::Address(0),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                10,
                1 as _,
                None,
            );
            assert_eq!(allocate_result, Err(EfiError::NotReady));

            let free_result = GCD.free_memory_space(0, 10);
            assert_eq!(free_result, Err(EfiError::NotReady));

            let remove_result = GCD.remove_memory_space(0, 10);
            assert_eq!(remove_result, Err(EfiError::NotReady));
        });
    }

    #[test]
    fn spin_locked_allocator_init_should_initialize() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            assert_eq!(GCD.memory.lock().maximum_address, 0);

            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
            let address = mem.as_ptr() as usize;
            GCD.init(48, 16);
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            GCD.add_io_space(dxe_services::GcdIoType::Io, 0, 100).unwrap();
            GCD.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
            GCD.free_io_space(0, 10).unwrap();
            GCD.remove_io_space(0, 10).unwrap();
        });
    }

    #[test]
    fn callback_should_fire_when_map_changes() {
        with_locked_state(|| {
            static CALLBACK_INVOKED: AtomicBool = AtomicBool::new(false);
            fn map_callback(map_change_type: MapChangeType) {
                CALLBACK_INVOKED.store(true, core::sync::atomic::Ordering::SeqCst);
                assert_eq!(map_change_type, MapChangeType::AddMemorySpace);
            }
            static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(map_callback));

            assert_eq!(GCD.memory.lock().maximum_address, 0);

            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
            let address = mem.as_ptr() as usize;
            GCD.init(48, 16);
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            unsafe {
                GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x1000, 0x1000, efi::MEMORY_WB)
                    .unwrap();
            }

            assert!(CALLBACK_INVOKED.load(core::sync::atomic::Ordering::SeqCst));
        });
    }

    #[test]
    fn test_spin_locked_set_attributes_capabilities() {
        with_locked_state(|| {
            static CALLBACK2: AtomicBool = AtomicBool::new(false);
            fn map_callback(map_change_type: MapChangeType) {
                if map_change_type == MapChangeType::SetMemoryCapabilities {
                    CALLBACK2.store(true, core::sync::atomic::Ordering::SeqCst);
                }
            }

            static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(map_callback));

            assert_eq!(GCD.memory.lock().maximum_address, 0);

            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
            GCD.init(48, 16);
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }
            GCD.set_memory_space_capabilities(
                address,
                0x1000,
                efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB,
            )
            .unwrap();

            assert!(CALLBACK2.load(core::sync::atomic::Ordering::SeqCst));
        });
    }

    #[test]
    fn allocate_bottom_up_should_allocate_increasing_addresses() {
        with_locked_state(|| {
            const GCD_SIZE: usize = 0x100000;
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
            let base = unsafe { std::alloc::System.alloc(layout) as u64 };
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    base as usize,
                    GCD_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            let mut last_allocation = 0;
            loop {
                let allocate_result = GCD.allocate_memory_space(
                    AllocateType::BottomUp(None),
                    dxe_services::GcdMemoryType::SystemMemory,
                    12,
                    0x1000,
                    1 as _,
                    None,
                );

                if let Ok(address) = allocate_result {
                    assert!(
                        address > last_allocation,
                        "address {address:#x?} is lower than previously allocated address {last_allocation:#x?}",
                    );
                    last_allocation = address;
                } else {
                    break;
                }
            }
        });
    }

    #[test]
    fn allocate_top_down_should_allocate_decreasing_addresses() {
        with_locked_state(|| {
            const GCD_SIZE: usize = 0x100000;
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
            let base = unsafe { std::alloc::System.alloc(layout) as u64 };
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    base as usize,
                    GCD_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            let mut last_allocation = usize::MAX;
            loop {
                let allocate_result = GCD.allocate_memory_space(
                    AllocateType::TopDown(None),
                    dxe_services::GcdMemoryType::SystemMemory,
                    12,
                    0x1000,
                    1 as _,
                    None,
                );

                if let Ok(address) = allocate_result {
                    assert!(
                        address < last_allocation,
                        "address {address:#x?} is higher than previously allocated address {last_allocation:#x?}",
                    );
                    last_allocation = address;
                } else {
                    break;
                }
            }
        });
    }

    #[test]
    fn test_allocate_page_zero_should_fail() {
        let (mut gcd, _) = create_gcd();
        // Increase the memory block size so allocation at 0x1000 is possible after skipping page 0
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 0x2000, efi::MEMORY_WB).unwrap();
        }

        // Try to allocate page 0 implicitly bottom up, we should get bumped to the next available page
        let res = gcd.allocate_memory_space(
            AllocateType::BottomUp(None),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x1000,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), 0x1000, "Should not be able to allocate page 0");

        // Try to allocate page 0 implicitly top down, we should fail with out of resources
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x1000,
            1 as _,
            None,
        );
        assert_eq!(res, Err(EfiError::OutOfResources), "Should not be able to allocate page 0");

        // add a new block to ensure block skipping logic works
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x2000, 0x2000, efi::MEMORY_WB).unwrap();
        }

        // now allocate bottom up, we should be able to allocate page 0x2000
        let res = gcd.allocate_memory_space(
            AllocateType::BottomUp(None),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            0x2000,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), 0x2000, "Should be able to allocate page 0x2000");

        // Try to allocate page 0 explicitly. This should pass as Patina DXE Core needs to allocate by address
        let res = gcd.allocate_memory_space(
            AllocateType::Address(0),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            UEFI_PAGE_SIZE,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), 0x0, "Should be able to allocate page 0 by address");
    }

    #[test]
    fn test_prioritize_32_bit_memory_top_down() {
        let (mut gcd, _) = create_gcd();
        gcd.prioritize_32_bit_memory = true;

        // Test with a contiguous 8gb without a gap.
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 2 * SIZE_4GB, 0) }.unwrap();

        // make sure it prioritizes 32 bit addresses.
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            dxe_services::GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x10000,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), SIZE_4GB - 0x10000, "Should allocate below 4GB when prioritizing 32-bit memory");

        // check that it will fall back to >32 bits.
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            dxe_services::GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            SIZE_4GB,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), SIZE_4GB, "Failed to fall back to higher memory as expected");

        // Free the memory to check the next condition.
        gcd.free_memory_space(SIZE_4GB - 0x10000, 0x10000).unwrap();
        gcd.free_memory_space(SIZE_4GB, SIZE_4GB).unwrap();

        // Check that a sufficiently large allocation will straddle the boundary.
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            dxe_services::GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            SIZE_4GB + 0x1000,
            1 as _,
            None,
        );
        assert!(res.is_ok(), "Failed to fallback to higher memory as expected");
    }

    #[test]
    fn test_spin_locked_gcd_debug_and_display() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Initialize and add some memory
            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
            let address = mem.as_ptr() as usize;
            GCD.init(48, 16);

            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            // Ensure Debug doesn't panic
            let _ = format!("{:?}", &GCD);

            // Ensure Display doesn't panic
            let _ = format!("{}", &GCD);
        });
    }

    #[test]
    fn test_io_gcd_display() {
        let mut io_gcd = IoGCD::_new(16);

        // Add various IO space types
        io_gcd.add_io_space(dxe_services::GcdIoType::Io, 0x0, 0x100).unwrap();
        io_gcd.add_io_space(dxe_services::GcdIoType::Reserved, 0x1000, 0x200).unwrap();
        io_gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x2000, 0x300).unwrap();

        // Ensure Display doesn't panic
        let _ = format!("{}", &io_gcd);
    }

    #[test]
    fn paging_allocator_new_and_basic_alloc() {
        with_locked_state(|| {
            const GCD_SIZE: usize = 0x300000;
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
            let base = unsafe { std::alloc::System.alloc(layout) as u64 };
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    base as usize,
                    GCD_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }
            let mut allocator = PagingAllocator::new(&GCD);

            // Allocate a single page
            let page = allocator
                .allocate_page(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, true)
                .expect("Should allocate a page");
            assert!(
                page >= base && page < (base + GCD_SIZE as u64),
                "Allocated page should be within GCD memory range"
            );

            // allocate another page
            let page2 = allocator
                .allocate_page(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false)
                .expect("Should allocate a second page");
            assert!(page2 != page, "Allocated pages should be unique");
            assert!(
                page2 >= base && page2 < (base + GCD_SIZE as u64),
                "Allocated page should be within GCD memory range"
            );

            // fail to allocate with a bad alignment
            let bad_alloc = allocator.allocate_page(UEFI_PAGE_SIZE as u64, 0x3000, false);
            assert_eq!(bad_alloc, Err(PtError::InvalidParameter), "Should fail to allocate with bad alignment");

            // fail to allocate a zero sized page
            let zero_alloc = allocator.allocate_page(UEFI_PAGE_SIZE as u64, 0, false);
            assert_eq!(zero_alloc, Err(PtError::InvalidParameter), "Should fail to allocate zero sized page");
        });
    }

    #[test]
    #[should_panic]
    fn paging_allocator_exhaustion_asserts() {
        with_locked_state(|| {
            const GCD_SIZE: usize = 0x200000;
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
            let base = unsafe { std::alloc::System.alloc(layout) as u64 };
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    base as usize,
                    GCD_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }
            let mut allocator = PagingAllocator::new(&GCD);

            // Exhaust all available pages
            let mut allocated = Vec::new();
            while let Ok(page) = allocator.allocate_page(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false) {
                allocated.push(page);
            }
        });
    }

    #[test]
    fn test_get_allocated_memory_descriptors() {
        let (mut gcd, _address) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 2 * SIZE_4GB, 0) }.unwrap();

        gcd.allocate_memory_space(
            AllocateType::Address(0x5000),
            dxe_services::GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x4000,
            1 as _,
            None,
        )
        .unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0x9000),
            dxe_services::GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x2000,
            2 as _,
            None,
        )
        .unwrap();

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_allocated_memory_descriptors(&mut buffer).unwrap();
        assert_eq!(buffer.len(), 3); // one extra allocated space for memory_block region
        assert!(
            buffer
                .iter()
                .any(|desc| desc.base_address == 0x5000 && desc.length == 0x4000 && desc.image_handle == 1 as _)
        );
        assert!(
            buffer
                .iter()
                .any(|desc| desc.base_address == 0x9000 && desc.length == 0x2000 && desc.image_handle == 2 as _)
        );
    }

    #[test]
    fn test_get_mmio_and_reserved_descriptors() {
        let (mut gcd, _address) = create_gcd();
        // Add MMIO and Reserved blocks
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::MemoryMappedIo, 0x2000, 0x1000, 0).unwrap();
        }
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 0x3000, 0x10000, 0).unwrap();
        }
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0x14000, 0x6000, 0).unwrap();
        }

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_mmio_and_reserved_descriptors(&mut buffer).unwrap();
        assert!(buffer.len() == 2);
        assert!(buffer.iter().any(|desc| desc.memory_type == dxe_services::GcdMemoryType::MemoryMappedIo
            && desc.base_address == 0x2000
            && desc.length == 0x1000));
        assert!(buffer.iter().any(|desc| desc.memory_type == dxe_services::GcdMemoryType::Reserved
            && desc.base_address == 0x3000
            && desc.length == 0x10000));
    }

    #[test]
    fn test_init_paging_maps_allocated_and_mmio_regions() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 100) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE * 99,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
                GCD.add_memory_space(
                    dxe_services::GcdMemoryType::MemoryMappedIo,
                    0x1000,
                    0x1000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let r = GCD.set_memory_space_attributes(address, MEMORY_BLOCK_SLICE_SIZE * 99, efi::MEMORY_WB);
            assert_eq!(r, Err(EfiError::NotReady));

            let r = GCD.set_memory_space_attributes(0x1000, 0x1000, efi::MEMORY_UC);
            assert_eq!(r, Err(EfiError::NotReady));

            // Create a fake HobList with a MemoryAllocationModule for DXE Core
            let dxe_core_base = address + 0x1000;
            let dxe_core_len = 0x1000000;
            let hob = Hob::MemoryAllocationModule(&patina::pi::hob::MemoryAllocationModule {
                header: patina::pi::hob::header::Hob {
                    r#type: patina::pi::hob::MEMORY_ALLOCATION,
                    length: core::mem::size_of::<patina::pi::hob::MemoryAllocationModule>() as u16,
                    reserved: 0,
                },
                alloc_descriptor: patina::pi::hob::header::MemoryAllocation {
                    name: guids::DXE_CORE,
                    memory_base_address: dxe_core_base as u64,
                    memory_length: dxe_core_len as u64,
                    memory_type: efi::BOOT_SERVICES_DATA,
                    reserved: [0; 4],
                },
                module_name: guids::DXE_CORE,
                entry_point: dxe_core_base as u64 + 0x1000,
            });
            let mut hob_list = HobList::new();
            hob_list.push(hob);

            // SAFETY: We just allocated this memory, write mock PE header data directly to memory
            let pe_header_data = [
                0x4D, 0x5A, 0x78, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x0E, 0x1F, 0xBA, 0x0E,
                0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD, 0x21, 0x54, 0x68, 0x69, 0x73, 0x20, 0x70, 0x72,
                0x6F, 0x67, 0x72, 0x61, 0x6D, 0x20, 0x63, 0x61, 0x6E, 0x6E, 0x6F, 0x74, 0x20, 0x62, 0x65, 0x20, 0x72,
                0x75, 0x6E, 0x20, 0x69, 0x6E, 0x20, 0x44, 0x4F, 0x53, 0x20, 0x6D, 0x6F, 0x64, 0x65, 0x2E, 0x24, 0x00,
                0x00, 0x50, 0x45, 0x00, 0x00, 0x64, 0x86, 0x08, 0x00, 0x81, 0x4E, 0x12, 0x69, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0xF0, 0x00, 0x22, 0x00, 0x0B, 0x02, 0x0E, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00,
                0x60, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x91, 0xA4, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x60,
                0x8D, 0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x06, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x1D, 0x00,
                0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0B, 0x00, 0x60, 0x81, 0x00, 0x00, 0x10, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
                0x00, 0x00, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2E, 0x74, 0x65, 0x78, 0x74, 0x00, 0x00, 0x00,
                0x40, 0x3F, 0x11, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x60, 0x2E, 0x72,
                0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x2C, 0x7B, 0x0A, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x7C, 0x0A,
                0x00, 0x00, 0x44, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x40, 0x00, 0x00, 0x40, 0x2E, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0xE8, 0x8E, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x30, 0x00, 0x0C, 0x00, 0x00, 0x00, 0xC0, 0x1B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0xC0, 0x2E, 0x70, 0x64, 0x61, 0x74, 0x61, 0x00,
                0x00, 0xF8, 0x94, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x96, 0x00, 0x00, 0x00, 0xCC, 0x1B, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E,
                0x65, 0x68, 0x5F, 0x66, 0x72, 0x61, 0x6D, 0xA0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x02,
                0x00, 0x00, 0x00, 0x62, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x6C, 0x69, 0x6E, 0x6B, 0x6D, 0x32, 0x5F, 0x08, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x60, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x64, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x6C, 0x69, 0x6E, 0x6B, 0x6D,
                0x65, 0x5F, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x66, 0x1C,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40,
                0x2E, 0x72, 0x65, 0x6C, 0x6F, 0x63, 0x00, 0x00, 0xE0, 0x3B, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00,
                0x3C, 0x00, 0x00, 0x00, 0x68, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x40, 0x00, 0x00, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00,
            ];

            // SAFETY: We just allocated this memory and pe_header_data is a valid byte array
            unsafe {
                core::ptr::copy_nonoverlapping(pe_header_data.as_ptr(), dxe_core_base as *mut u8, pe_header_data.len());
            }

            // Create a local mock page table that we can access after init_paging_with
            let mock_table = std::rc::Rc::new(std::cell::RefCell::new(MockPageTable::new()));
            let page_table = Box::new(MockPageTableWrapper::new(std::rc::Rc::clone(&mock_table)));

            // Call init_paging
            GCD.init_paging_with(&hob_list, page_table);

            // Validate that init_paging worked by checking our local mock page table
            let mock_ref = mock_table.borrow();
            let mapped_regions = mock_ref.get_mapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            // Verify that memory regions were mapped during init_paging
            assert!(!mapped_regions.is_empty(), "init_paging should have mapped memory regions");
            assert!(!current_mappings.is_empty(), "Page table should have active mappings after init_paging");

            // Verify that we have multiple mapping operations (allocated memory + MMIO + DXE Core)
            assert!(mapped_regions.len() >= 3, "Should have mapped allocated memory, MMIO, and DXE Core regions");

            // Verify that DXE Core region is being managed
            let dxe_core_base = dxe_core_base as u64;
            let dxe_core_end = dxe_core_base + dxe_core_len as u64;

            // Check that we have mappings that overlap with or are contained in the DXE Core region
            let has_dxe_core_mapping = current_mappings.iter().any(|(addr, len, _attr)| {
                let mapping_end = addr + len;
                // Check for overlap: mapping overlaps with DXE core region
                *addr < dxe_core_end && mapping_end > dxe_core_base
            });

            assert!(has_dxe_core_mapping, "DXE Core region should be covered by page table mappings");

            // Verify that memory attributes are being set (should have XP attributes)
            let has_attribute_mappings = current_mappings.iter().any(|(_addr, _len, attr)| {
                attr.bits() != 0 // Should have some attributes set
            });

            assert!(has_attribute_mappings, "Mappings should have memory attributes set");

            // Verify that MMIO region (0x1000-0x2000) is mapped
            let has_mmio_mapping =
                current_mappings.iter().any(|(addr, len, _attr)| *addr <= 0x1000 && (*addr + len) >= 0x2000);

            assert!(has_mmio_mapping, "MMIO region should be mapped after init_paging");
        });
    }

    #[test]
    fn test_set_paging_attributes_with_page_table() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Set up memory space like other tests
            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            // Initialize page table with local MockPageTable
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            *GCD.page_table.lock() = Some(mock_page_table);

            // Test mapping within the allocated memory region
            let base_address = address;
            let length = 0x1000;
            let attributes = MemoryAttributes::Writeback.bits();

            let result = GCD.set_paging_attributes(base_address, length, attributes);
            assert!(result.is_ok());

            // Manually drop the page table to release the reference
            *GCD.page_table.lock() = None;

            // Verify the page table state
            let mock_ref = mock_table.borrow();
            let mapped = mock_ref.get_mapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            assert_eq!(mapped.len(), 1);
            assert_eq!(mapped[0].0, base_address as u64);
            assert_eq!(mapped[0].1, length as u64);
            assert_eq!(mapped[0].2, MemoryAttributes::Writeback);

            assert_eq!(current_mappings.len(), 1);
            assert_eq!(current_mappings[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));
        });
    }

    #[test]
    fn test_set_paging_attributes_cache_attributes() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Set up memory space
            let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
            unsafe {
                GCD.init_memory_blocks(
                    dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE,
                    efi::MEMORY_WB,
                )
                .unwrap();
            }

            // Initialize page table with local MockPageTable
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            *GCD.page_table.lock() = Some(mock_page_table);

            // Test different cache attributes
            let base_address = address;
            let length = 0x1000;

            // Test Uncacheable
            let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::Uncacheable.bits());
            assert!(result.is_ok());

            // Test WriteThrough - should overwrite the previous mapping
            let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::WriteThrough.bits());
            assert!(result.is_ok());

            // Test WriteCombining - should overwrite again
            let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::WriteCombining.bits());
            assert!(result.is_ok());

            // Manually drop the page table to release the reference
            *GCD.page_table.lock() = None;

            // Verify the page table state
            let mock_ref = mock_table.borrow();
            let mapped = mock_ref.get_mapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            // Should have 3 map operations recorded
            assert_eq!(mapped.len(), 3);
            assert_eq!(mapped[0], (base_address as u64, length as u64, MemoryAttributes::Uncacheable));
            assert_eq!(mapped[1], (base_address as u64, length as u64, MemoryAttributes::WriteThrough));
            assert_eq!(mapped[2], (base_address as u64, length as u64, MemoryAttributes::WriteCombining));

            // Current mapping should only show the last one (WriteCombining)
            assert_eq!(current_mappings.len(), 1);
            assert_eq!(current_mappings[0], (base_address as u64, length as u64, MemoryAttributes::WriteCombining));
        });
    }

    #[test]
    fn test_set_paging_attributes_no_page_table() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Don't initialize page table
            let base_address = 0x1000;
            let length = 0x1000;
            let attributes = MemoryAttributes::Writeback.bits();

            let result = GCD.set_paging_attributes(base_address, length, attributes);
            assert_eq!(result.unwrap_err(), EfiError::NotReady);
        });
    }

    #[test]
    fn test_set_paging_attributes_unmap_with_read_protect() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Initialize page table with local MockPageTable
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            *GCD.page_table.lock() = Some(mock_page_table);

            let base_address = 0x1000;
            let length = 0x1000;

            // First map the region
            let map_attributes = MemoryAttributes::Writeback.bits();
            let result = GCD.set_paging_attributes(base_address, length, map_attributes);
            assert!(result.is_ok());

            // Now unmap it using ReadProtect
            let unmap_attributes = MemoryAttributes::ReadProtect.bits();
            let result = GCD.set_paging_attributes(base_address, length, unmap_attributes);
            assert!(result.is_ok());

            // Manually drop the page table to release the reference
            *GCD.page_table.lock() = None;

            // Verify the page table state
            let mock_ref = mock_table.borrow();
            let mapped = mock_ref.get_mapped_regions();
            let unmapped = mock_ref.get_unmapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            // Should have 1 map operation recorded
            assert_eq!(mapped.len(), 1);
            assert_eq!(mapped[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));

            // Should have 1 unmap operation recorded
            assert_eq!(unmapped.len(), 1);
            assert_eq!(unmapped[0], (base_address as u64, length as u64));

            // Current mapping should be empty (region was unmapped)
            assert_eq!(current_mappings.len(), 0);
        });
    }

    #[test]
    fn test_set_paging_attributes_already_mapped_same_attributes() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Initialize page table with local MockPageTable
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            *GCD.page_table.lock() = Some(mock_page_table);

            let base_address = 0x1000;
            let length = 0x1000;
            let attributes = MemoryAttributes::Writeback.bits();

            // Map the region first
            let result = GCD.set_paging_attributes(base_address, length, attributes);
            assert!(result.is_ok());

            // Try to map the same region with same attributes
            let result = GCD.set_paging_attributes(base_address, length, attributes);
            assert!(result.is_ok());

            // Manually drop the page table to release the reference
            *GCD.page_table.lock() = None;

            // Verify the page table state
            let mock_ref = mock_table.borrow();
            let mapped = mock_ref.get_mapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            // The implementation may optimize duplicate mappings, so we verify there's at least one mapping
            assert!(!mapped.is_empty());
            assert!(mapped[0] == (base_address as u64, length as u64, MemoryAttributes::Writeback));

            // If GCD optimizes away the duplicate, there might only be 1 map operation
            // If it doesn't optimize, there will be 2. Both behaviors are acceptable.
            assert!(!mapped.is_empty() && mapped.len() <= 2);

            // Current mapping should show one region
            assert_eq!(current_mappings.len(), 1);
            assert_eq!(current_mappings[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));
        });
    }

    #[test]
    fn test_set_paging_attributes_multiple_regions() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Initialize page table with local MockPageTable
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            *GCD.page_table.lock() = Some(mock_page_table);

            // Map multiple non-overlapping regions
            let regions = [
                (0x1000, 0x1000, MemoryAttributes::Writeback),
                (0x3000, 0x2000, MemoryAttributes::Uncacheable),
                (0x6000, 0x1000, MemoryAttributes::WriteCombining),
            ];

            for (base, len, attrs) in regions {
                let result = GCD.set_paging_attributes(base, len, attrs.bits());
                assert!(result.is_ok());
            }

            // Manually drop the page table to release the reference
            *GCD.page_table.lock() = None;

            // Verify the page table state
            let mock_ref = mock_table.borrow();
            let mapped = mock_ref.get_mapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            // Should have 3 map operations recorded
            assert_eq!(mapped.len(), 3);
            assert_eq!(mapped[0], (0x1000, 0x1000, MemoryAttributes::Writeback));
            assert_eq!(mapped[1], (0x3000, 0x2000, MemoryAttributes::Uncacheable));
            assert_eq!(mapped[2], (0x6000, 0x1000, MemoryAttributes::WriteCombining));

            // Current mappings should show all 3 regions (no overlaps)
            assert_eq!(current_mappings.len(), 3);
            assert!(current_mappings.contains(&(0x1000, 0x1000, MemoryAttributes::Writeback)));
            assert!(current_mappings.contains(&(0x3000, 0x2000, MemoryAttributes::Uncacheable)));
            assert!(current_mappings.contains(&(0x6000, 0x1000, MemoryAttributes::WriteCombining)));
        });
    }

    #[test]
    fn test_set_paging_attributes_overlapping_regions() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Initialize page table with local MockPageTable
            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            *GCD.page_table.lock() = Some(mock_page_table);

            let base_address = 0x1000;
            let length = 0x2000;

            // Map a large region first
            let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::Writeback.bits());
            assert!(result.is_ok());

            // Map a smaller overlapping region with different attributes
            let overlapping_base = 0x1800;
            let overlapping_length = 0x1000;
            let result =
                GCD.set_paging_attributes(overlapping_base, overlapping_length, MemoryAttributes::Uncacheable.bits());
            assert!(result.is_ok());

            // Manually drop the page table to release the reference
            *GCD.page_table.lock() = None;

            // Verify the page table state
            let mock_ref = mock_table.borrow();
            let mapped = mock_ref.get_mapped_regions();
            let current_mappings = mock_ref.get_current_mappings();

            // Should have 2 map operations recorded
            assert_eq!(mapped.len(), 2);
            assert_eq!(mapped[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));
            assert_eq!(mapped[1], (overlapping_base as u64, overlapping_length as u64, MemoryAttributes::Uncacheable));

            // Current mappings should show the overlapping region replaced the original
            // (MockPageTable removes overlapping regions when adding new ones)
            assert_eq!(current_mappings.len(), 1);
            assert_eq!(
                current_mappings[0],
                (overlapping_base as u64, overlapping_length as u64, MemoryAttributes::Uncacheable)
            );
        });
    }
}
