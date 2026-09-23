//! UEFI Global Coherency Domain (GCD)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod io_gcd;
mod paging;
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests;

use crate::pecoff::UefiPeInfo;
use alloc::{boxed::Box, slice, vec, vec::Vec};
use core::{fmt::Display, ptr};
#[cfg(test)]
use io_gcd::IO_BLOCK_SLICE_LEN;
use io_gcd::IoGCD;
#[cfg(any(test, feature = "confidential_compute"))]
use paging::AliasedMapping;
pub(crate) use paging::PagingAllocator;
use patina::{DEFAULT_CACHE_ATTR, crc32, error::EfiError, log_debug_assert};

use patina::standard::efi;
use patina::{
    function, guid as base_guids,
    pi::{
        dxe_services::{self, GcdMemoryType, MemorySpaceDescriptor},
        guid as pi_guids, hob,
    },
    uefi::event::CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID,
    uefi_pages_to_size, uefi_size_to_pages, writelncrlf,
    {SIZE_4GB, UEFI_PAGE_MASK, UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, align_up},
};
use patina_internal_core::collections::{Error as SliceError, Rbt, SliceKey, node_size};

use crate::{
    GCD,
    allocator::{DEFAULT_ALLOCATION_STRATEGY, memory_type_for_handle},
    ensure, error,
    events::EVENT_DB,
    gcd::MemoryProtectionPolicy,
    protocol_db,
    protocol_db::INVALID_HANDLE,
    tpl_mutex,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InternalError {
    MemoryBlock(MemoryBlockError),
    IoBlock(IoBlockError),
    Slice(SliceError),
}

impl From<InternalError> for EfiError {
    fn from(err: InternalError) -> Self {
        match err {
            InternalError::MemoryBlock(e) => match e {
                MemoryBlockError::BlockOutsideRange => EfiError::NotFound,
                MemoryBlockError::InvalidStateTransition => EfiError::AccessDenied,
            },
            InternalError::IoBlock(e) => match e {
                IoBlockError::BlockOutsideRange => EfiError::NotFound,
                IoBlockError::InvalidStateTransition => EfiError::AccessDenied,
            },
            InternalError::Slice(e) => match e {
                SliceError::OutOfSpace => EfiError::OutOfResources,
                SliceError::AlreadyExists => EfiError::AlreadyStarted,
                SliceError::NotFound => EfiError::NotFound,
                SliceError::NotSorted => EfiError::Unsupported,
            },
        }
    }
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

pub fn get_capabilities(gcd_mem_type: GcdMemoryType, attributes: u64) -> u64 {
    let mut capabilities = 0;

    for conversion in &ATTRIBUTE_CONVERSION_TABLE {
        if conversion.attribute == 0 {
            break;
        }

        if (conversion.memory
            || (gcd_mem_type != GcdMemoryType::SystemMemory && gcd_mem_type != GcdMemoryType::MoreReliable))
            && (attributes & u64::from(conversion.attribute) != 0)
        {
            capabilities |= conversion.capability;
        }
    }

    capabilities
}

type GcdAllocateFn = fn(
    gcd: &mut GCD,
    allocate_type: AllocateType,
    memory_type: GcdMemoryType,
    alignment: usize,
    len: usize,
    image_handle: efi::Handle,
    device_handle: Option<efi::Handle>,
) -> Result<usize, EfiError>;
type GcdFreeFn =
    fn(gcd: &mut GCD, base_address: usize, len: usize, transition: MemoryStateTransition) -> Result<(), EfiError>;

#[allow(clippy::upper_case_acronyms)]
//The Global Coherency Domain (GCD) Services are used to manage the memory resources visible to the boot processor.
struct GCD {
    maximum_address: usize,
    memory_blocks: Rbt<'static, MemoryBlock>,
    allocate_memory_space_fn: GcdAllocateFn,
    free_memory_space_fn: GcdFreeFn,
    /// Whether to prioritize 32-bit memory allocations
    prioritize_32_bit_memory: bool,
}

impl GCD {
    /// Returns true if the GCD is initialized and ready for use.
    pub fn is_ready(&self) -> bool {
        self.maximum_address != 0
    }
}

#[allow(clippy::missing_fields_in_debug)] // The function pointers are excluded from debug prints.
impl core::fmt::Debug for GCD {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GCD")
            .field("maximum_address", &self.maximum_address)
            .field("memory_blocks", &self.memory_blocks)
            .field("prioritize_32_bit_memory", &self.prioritize_32_bit_memory)
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
            free_memory_space_fn: Self::free_memory_space,
            prioritize_32_bit_memory: false,
        }
    }

    pub fn lock_memory_space(&mut self) {
        self.allocate_memory_space_fn = Self::allocate_memory_space_null;
        self.free_memory_space_fn = Self::free_memory_space_null;
        log::info!("Disallowing alloc/free during ExitBootServices.");
    }

    pub fn unlock_memory_space(&mut self) {
        self.allocate_memory_space_fn = Self::allocate_memory_space_internal;
        self.free_memory_space_fn = Self::free_memory_space;
    }

    pub fn init(&mut self, processor_address_bits: u32) {
        self.maximum_address = 1 << processor_address_bits;
    }

    pub(crate) unsafe fn init_memory_blocks(
        &mut self,
        memory_type: GcdMemoryType,
        base_address: usize,
        len: usize,
        attributes: u64,
        capabilities: u64,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(memory_type == GcdMemoryType::SystemMemory && len >= MEMORY_BLOCK_SLICE_SIZE, EfiError::OutOfResources);

        log::trace!(target: "allocations", "[{}] Initializing memory blocks at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Attributes: {:#x}", function!(), attributes);
        log::trace!(target: "allocations", "[{}]   Capabilities: {:#x}", function!(), capabilities);

        let unallocated_memory_space = MemoryBlock::Unallocated(dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::NonExistent,
            base_address: 0,
            length: self.maximum_address as u64,
            ..Default::default()
        });

        self.memory_blocks.expand(
            // SAFETY: base_address/size refer to a reserved backing allocation for memory blocks.
            unsafe { slice::from_raw_parts_mut::<'static>(base_address as *mut u8, MEMORY_BLOCK_SLICE_SIZE) },
        );

        self.memory_blocks.add(unallocated_memory_space).map_err(|_| EfiError::OutOfResources)?;
        // SAFETY: add_memory_space is called during initialization with validated parameters.
        let idx = unsafe { self.add_memory_space(memory_type, base_address, len, capabilities) }?;

        // Initialize attributes on the first block to WB + XP
        match self.set_memory_space_attributes(
            base_address,
            len,
            GCD.memory_protection_policy
                .apply_allocated_memory_protection_policy(attributes, GcdMemoryType::SystemMemory),
        ) {
            Ok(()) | Err(EfiError::NotReady) => Ok(()),
            Err(err) => Err(err),
        }?;

        // Allocate a chunk of the block to hold the actual first GCD slice
        self.allocate_memory_space(
            AllocateType::Address(base_address),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            MEMORY_BLOCK_SLICE_SIZE,
            protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
            None,
        )?;

        // Apply free memory policy on the remaining free block.
        if len > MEMORY_BLOCK_SLICE_SIZE {
            match self.set_memory_space_attributes(
                base_address + MEMORY_BLOCK_SLICE_SIZE,
                len - MEMORY_BLOCK_SLICE_SIZE,
                MemoryProtectionPolicy::apply_free_memory_policy(attributes, GcdMemoryType::SystemMemory),
            ) {
                Ok(()) | Err(EfiError::NotReady) => Ok(()),
                Err(err) => Err(err),
            }?;
        }

        Ok(idx)
    }

    /// This service adds reserved memory, system memory, or memory-mapped I/O resources to the global coherency domain of the processor.
    ///
    /// # Safety
    /// Since the first call with enough system memory will cause the creation of an array at `base_address` + [`MEMORY_BLOCK_SLICE_SIZE`].
    /// The memory from `base_address` to `base_address+len` must be inside the valid address range of the program and not in use.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.1
    pub unsafe fn add_memory_space(
        &mut self,
        memory_type: GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
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
        let (capabilities, attributes) = MemoryProtectionPolicy::apply_add_memory_policy(capabilities, memory_type);

        let memory_blocks = &mut self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = memory_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(block.as_ref().memory_type == GcdMemoryType::NonExistent, EfiError::AccessDenied);

        // all newly added memory is marked as RP
        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::Add(memory_type, capabilities, attributes),
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
        memory_type: GcdMemoryType,
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
        memory_type: GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        ensure!(gcd.maximum_address != 0, EfiError::NotReady);
        ensure!(
            len > 0 && image_handle > ptr::null_mut() && memory_type != GcdMemoryType::Unaccepted,
            EfiError::InvalidParameter
        );

        log::trace!(target: "allocations", "[{}] Allocating memory space: {:x?}", function!(), allocate_type);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

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

    #[cfg_attr(coverage, coverage(off))]
    fn allocate_memory_space_null(
        _gcd: &mut GCD,
        _allocate_type: AllocateType,
        _memory_type: GcdMemoryType,
        _alignment: usize,
        _len: usize,
        _image_handle: efi::Handle,
        _device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        log_debug_assert!("GCD not allowed to allocate after EBS has started!");
        Err(EfiError::AccessDenied)
    }

    // This function checks if allocated memory blocks exist for the entire specified address range.
    // It returns Ok(()) only if every part of the range is covered by an Allocated block.
    fn get_memory_block_allocation_state(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        let memory_blocks = &self.memory_blocks;

        let mut current_base = base_address as u64;
        let range_end = (base_address + len) as u64;

        while current_base < range_end {
            log::trace!(target: "gcd_measure", "search");
            let idx = memory_blocks.get_closest_idx(&current_base).ok_or(EfiError::NotFound)?;
            let block = memory_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

            // Check that the block covers the current base
            if (current_base < block.start() as u64)
                || (range_end > block.end() as u64 && block.end() as u64 <= current_base)
            {
                return Err(EfiError::NotFound);
            }

            match block {
                MemoryBlock::Unallocated(_) => return Err(EfiError::NotFound),
                MemoryBlock::Allocated(_) => {}
            }

            // Advance to the end of this block or the end of the requested range
            current_base = u64::min(block.end() as u64, range_end);
        }

        Ok(())
    }

    fn free_memory_space(
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

        Self::split_state_transition_at_idx(memory_blocks, idx, base_address, len, transition)
            .map(|_| ())
            .map_err(core::convert::Into::into)
    }

    #[cfg_attr(coverage, coverage(off))]
    fn free_memory_space_null(
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
        memory_type: GcdMemoryType,
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
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

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
        memory_type: GcdMemoryType,
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
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

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
        memory_type: GcdMemoryType,
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
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

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
    ///
    /// # Arguments
    /// * `buffer` - A mutable reference to a vector to hold the descriptors.
    /// * `filter` - A closure invoked with each descriptor and a boolean indicating whether the
    ///   descriptor's block is allocated. Returns `true` if the descriptor should be included.
    ///
    /// # Returns
    /// * `Ok(())` if successful.
    /// * `Err(EfiError::NotReady)` if the GCD is not initialized.
    /// * `Err(EfiError::InvalidParameter)` if the buffer capacity is insufficient or not empty.
    pub fn get_memory_descriptors(
        &self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
        filter: fn(&dxe_services::MemorySpaceDescriptor, bool) -> bool,
    ) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(buffer.capacity() >= self.memory_descriptor_count(), EfiError::InvalidParameter);
        ensure!(buffer.is_empty(), EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Enter\n", function!());

        let blocks = &self.memory_blocks;

        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            let (descriptor, allocated) = match mb {
                MemoryBlock::Allocated(descriptor) => (descriptor, true),
                MemoryBlock::Unallocated(descriptor) => (descriptor, false),
            };
            if filter(descriptor, allocated) {
                buffer.push(*descriptor);
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }

    /// This service returns the descriptor for the given physical address.
    ///
    /// # Arguments
    /// * `address` - The physical address to look up.
    /// * `filter` - A closure invoked with the descriptor and a boolean indicating whether the
    ///   descriptor's block is allocated. Returns `true` if the descriptor should be included.
    pub fn get_memory_descriptor_for_address(
        &self,
        address: efi::PhysicalAddress,
        mut filter: impl FnMut(&dxe_services::MemorySpaceDescriptor, bool) -> bool,
    ) -> Result<dxe_services::MemorySpaceDescriptor, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);

        let memory_blocks = &self.memory_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(address)).ok_or(EfiError::NotFound)?;
        let mb = memory_blocks.get_with_idx(idx).expect("idx is valid from get_closest_idx");
        let (descriptor, allocated) = match mb {
            MemoryBlock::Allocated(descriptor) => (descriptor, true),
            MemoryBlock::Unallocated(descriptor) => (descriptor, false),
        };
        if filter(descriptor, allocated) { Ok(*descriptor) } else { Err(EfiError::NotFound) }
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

        // SAFETY: split_state_transition does not update the key for this block.
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
                // SAFETY: restoring the prior block state does not change the base_address key.
                unsafe {
                    *memory_blocks.get_with_idx_mut(idx).expect("idx valid above") = mb_before_split;
                }
                error!(e);
            }
        };

        // Lets see if we can merge the block with the next block
        if let Some(next_idx) = memory_blocks.next_idx(idx) {
            let mut next = *memory_blocks.get_with_idx(next_idx).expect("idx valid from insert");

            // SAFETY: merge does not update the base_address key for this block.
            unsafe {
                if memory_blocks.get_with_idx_mut(idx).expect("idx valid from insert").merge(&mut next) {
                    memory_blocks.delete_with_idx(next_idx).expect("Index already verified.");
                }
            }
        }

        // Lets see if we can merge the block with the previous block
        if let Some(prev_idx) = memory_blocks.prev_idx(idx) {
            let mut block = *memory_blocks.get_with_idx(idx).expect("idx valid from insert");

            // SAFETY: merge does not update the base_address key for this block.
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

    /// Merges adjacent EFI memory descriptors in place.
    ///
    /// # Arguments
    /// * `descriptors` - A mutable slice of EFI memory descriptors to be merged.
    ///
    /// Returns
    /// * `usize` - The new count of descriptors after merging.
    fn merge_blocks_in_place(&self, descriptors: &mut [efi::MemoryDescriptor]) -> usize {
        if descriptors.is_empty() {
            return 0;
        }

        let mut write_idx = 0;

        for read_idx in 0..descriptors.len() {
            let current = *descriptors.get(read_idx).expect("read_idx < descriptors.len()");

            // Try to merge with the previous descriptor
            if write_idx > 0 {
                let prev = descriptors.get_mut(write_idx - 1).expect("write_idx <= read_idx < descriptors.len()");
                if prev.r#type == current.r#type
                    && prev.attribute == current.attribute
                    && prev.physical_start + uefi_pages_to_size!(prev.number_of_pages as usize) as u64
                        == current.physical_start
                {
                    // Free memory shouldn't even need to be merged because it should already be consistent and coalesced.
                    // If this fails to be true it can cause odd behavior if applications try to allocate blocks of free
                    // memory by address, which is a common pattern for OS loaders.
                    if prev.r#type == efi::CONVENTIONAL_MEMORY {
                        let prev_gcd = self.get_memory_descriptor_for_address(prev.physical_start, |_, _| true);
                        let curr_gcd = self.get_memory_descriptor_for_address(current.physical_start, |_, _| true);
                        log::error!(
                            "Free memory is fragmented in memory descriptors!\r\nprev: {:?}\r\ncurr: {:?}\r\nprev_gcd: {:?}\r\ncurr_gcd: {:?}",
                            crate::allocator::MemoryDescriptorRef(prev),
                            crate::allocator::MemoryDescriptorRef(&current),
                            prev_gcd.unwrap_or_default(),
                            curr_gcd.unwrap_or_default()
                        );
                        debug_assert!(false);
                    }
                    // Merge by extending the previous descriptor
                    prev.number_of_pages += current.number_of_pages;
                    continue;
                }
            }

            if write_idx != read_idx {
                *descriptors.get_mut(write_idx).expect("write_idx <= read_idx < descriptors.len()") = current;
            }
            write_idx += 1;
        }

        write_idx
    }

    /// Determines if a GCD memory descriptor should be included in the EFI memory map.
    ///
    /// Adjusts memory descriptor attributes for the EFI memory map.
    ///
    /// ## Arguments
    ///
    /// * `descriptor` - The GCD memory space descriptor
    /// * `memory_type` - The EFI memory type for this descriptor
    /// * `active_attributes` - If true, use active attributes; if false, use capabilities
    ///
    /// Returns
    /// * `u64` - The adjusted attributes for the EFI memory descriptor.
    fn adjust_efi_memory_map_descriptor(
        descriptor: &MemorySpaceDescriptor,
        memory_type: efi::MemoryType,
        active_attributes: bool,
    ) -> u64 {
        if active_attributes {
            descriptor.attributes
        } else {
            // when we are building the EFI memory map, follow edk2 conventions as OSes will expect that.
            // When using the capabilities, drop the runtime attribute and
            // pick it up from the active attributes. We also drop the access attributes because
            // some OSes think the EFI_MEMORY_MAP attribute field is actually set attributes, not
            // capabilities.
            MemoryProtectionPolicy::apply_efi_memory_map_policy(
                descriptor.attributes,
                descriptor.capabilities,
                descriptor.memory_type,
                memory_type,
            )
        }
    }

    /// Determines if this memory descriptor should be included in the EFI memory map.
    ///
    /// Only descriptors that meet UEFI requirements and represent allocatable or special memory
    /// types are included in the EFI memory map.
    ///
    /// # Returns
    /// * `Some(memory_type)` if the descriptor should be included in the EFI memory map
    /// * `None` if the descriptor should be excluded
    fn is_efi_memory_map_descriptor(descriptor: &MemorySpaceDescriptor) -> Option<efi::MemoryType> {
        // Validate page alignment and size
        if !descriptor.length.is_multiple_of(UEFI_PAGE_SIZE as u64) || descriptor.length == 0 {
            debug_assert!(false, "GCD returned a non-page aligned memory descriptor.");
            return None; // skip entries for non-page aligned entries
        }
        if !descriptor.base_address.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::warn!("GCD returned a non-page-aligned memory descriptor.");
            return None; // skip entries not page aligned
        }

        // first check if an allocator owns this memory, if so, we can use the allocator's memory type for
        // the EFI memory map
        if let Some(memory_type) = memory_type_for_handle(descriptor.image_handle) {
            return Some(memory_type);
        }

        match descriptor.memory_type {
            GcdMemoryType::SystemMemory => {
                if descriptor.image_handle.is_null() {
                    // Free memory not tracked by any allocator or directly allocated in the GCD
                    Some(efi::CONVENTIONAL_MEMORY)
                } else if descriptor.attributes & efi::MEMORY_RUNTIME == efi::MEMORY_RUNTIME {
                    // Directly allocated in the GCD and has the runtime attribute set. Mark it as reserved so it is
                    // preserved into runtime but doesn't have expectations about being in the MAT
                    Some(efi::RESERVED_MEMORY_TYPE)
                } else {
                    // Directly allocated in the GCD and does not have the runtime attribute set. Mark it as boot
                    // services data so it is reflected correctly but doesn't get preserved to runtime
                    Some(efi::BOOT_SERVICES_DATA)
                }
            }

            // Note: there could also be MMIO tracked by the allocators which would not hit this case.
            GcdMemoryType::MemoryMappedIo => {
                // we should only be returning runtime MMIO here
                if descriptor.attributes & efi::MEMORY_RUNTIME == 0 { None } else { Some(efi::MEMORY_MAPPED_IO) }
            }

            // Persistent. Note: this type is not allocatable, but might be created by agents other than the core directly
            // in the GCD.
            GcdMemoryType::Persistent => Some(efi::PERSISTENT_MEMORY),

            // Unaccepted. Note: this type is not allocatable, but might be created by agents other than the core directly
            // in the GCD.
            GcdMemoryType::Unaccepted => Some(efi::UNACCEPTED_MEMORY_TYPE),

            // Reserved.
            GcdMemoryType::Reserved => Some(efi::RESERVED_MEMORY_TYPE),

            // Other memory types are ignored for purposes of the memory map
            _ => None,
        }
    }

    /// Counts the number of EFI memory map descriptors needed.
    ///
    /// Returns
    /// * `usize` - The count of EFI memory map descriptors.
    pub fn memory_descriptor_count_for_efi_memory_map(&self) -> usize {
        let blocks = &self.memory_blocks;
        let mut count = 0;

        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            let descriptor = match mb {
                MemoryBlock::Allocated(descriptor) | MemoryBlock::Unallocated(descriptor) => descriptor,
            };

            if Self::is_efi_memory_map_descriptor(descriptor).is_some() {
                count += 1;
            }
            current = blocks.next_idx(idx);
        }

        count
    }

    /// Populates a caller-provided buffer with EFI memory map descriptors.
    ///
    /// This function iterates through GCD memory blocks, filters them for inclusion in the
    /// EFI memory map, converts them to EFI memory descriptors, and writes them directly
    /// into the provided buffer. Consecutive descriptors with the same type and attributes
    /// are merged to minimize the memory map size.
    ///
    /// ## Arguments
    ///
    /// * `buffer` - Mutable slice to populate with EFI memory descriptors. Must have sufficient
    ///   capacity to hold all descriptors.
    /// * `active_attributes` - If `true`, use active attributes; if `false`, use capabilities
    ///   as required by the UEFI specification.
    ///
    /// ## Returns
    ///
    /// Returns `Ok(count)` with the actual number of descriptors written to the buffer after merging,
    /// or `Err(EfiError::BufferTooSmall)` if the buffer size is too small.
    pub fn populate_efi_memory_map(
        &self,
        buffer: &mut [efi::MemoryDescriptor],
        active_attributes: bool,
    ) -> Result<usize, EfiError> {
        let blocks = &self.memory_blocks;
        let mut write_idx = 0;

        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            let descriptor = match mb {
                MemoryBlock::Allocated(descriptor) | MemoryBlock::Unallocated(descriptor) => descriptor,
            };

            if let Some(memory_type) = Self::is_efi_memory_map_descriptor(descriptor) {
                let number_of_pages = uefi_size_to_pages!(descriptor.length as usize) as u64;
                let attributes = Self::adjust_efi_memory_map_descriptor(descriptor, memory_type, active_attributes);

                let new_descriptor = efi::MemoryDescriptor {
                    r#type: memory_type,
                    physical_start: descriptor.base_address,
                    virtual_start: 0,
                    number_of_pages,
                    attribute: attributes,
                };

                *buffer.get_mut(write_idx).ok_or(EfiError::BufferTooSmall)? = new_descriptor;
                write_idx += 1;
            }
            current = blocks.next_idx(idx);
        }

        // Merge consecutive descriptors with the same type and attributes
        Ok(self.merge_blocks_in_place(buffer.get_mut(..write_idx).ok_or(EfiError::BufferTooSmall)?))
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
        writelncrlf!(
            f,
            "GCDMemType Range                             Capabilities     Attributes       ImageHandle      DeviceHandle"
        )?;
        writelncrlf!(
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
                    writelncrlf!(
                        f,
                        "{}  {:016x?}-{:016x?} {:016x?} {:016x?} {:016x?} {:016x?}",
                        GCD::GCD_MEMORY_TYPE_NAMES.get(mem_type_str_idx).expect("mem_type_str_idx bounded by min()"),
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
    page_table: tpl_mutex::TplMutex<Option<Box<dyn PatinaPageTable>>>,
    #[cfg(any(test, feature = "confidential_compute"))]
    aliased_mappings: tpl_mutex::TplMutex<Vec<AliasedMapping>>,
    /// Contains the current memory protection policy
    pub(crate) memory_protection_policy: MemoryProtectionPolicy,
    last_efi_memory_map_key: tpl_mutex::TplMutex<Option<usize>>,
}

impl SpinLockedGcd {
    /// Returns true if the underlying GCD is initialized and ready for use.
    pub fn is_ready(&self) -> bool {
        self.memory.lock().is_ready()
    }

    /// Creates a new uninitialized GCD. [`Self::init`] must be invoked before any other functions or they will return
    /// [`EfiError::NotReady`]. An optional callback can be provided which will be invoked whenever an operation
    /// changes the GCD map.
    #[cfg_attr(coverage, coverage(off))]
    pub const fn new(memory_change_callback: Option<MapChangeCallback>) -> Self {
        Self {
            memory: tpl_mutex::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                GCD {
                    maximum_address: 0,
                    memory_blocks: Rbt::new(),
                    allocate_memory_space_fn: GCD::allocate_memory_space_internal,
                    free_memory_space_fn: GCD::free_memory_space,
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
            page_table: tpl_mutex::TplMutex::new(efi::TPL_HIGH_LEVEL, None, "GcdPageTableLock"),
            #[cfg(any(test, feature = "confidential_compute"))]
            aliased_mappings: tpl_mutex::TplMutex::new(efi::TPL_HIGH_LEVEL, Vec::new(), "GcdAliasedMappingsLock"),
            memory_protection_policy: MemoryProtectionPolicy::new(),
            last_efi_memory_map_key: tpl_mutex::TplMutex::new(efi::TPL_HIGH_LEVEL, None, "LastEfiMemoryMapKeyLock"),
        }
    }

    /// Initializes the memory blocks in the GCD.
    ///
    /// # Safety
    /// The caller must ensure that the memory region specified by `base_address` and `len` is freely usable RAM and
    /// will never be used by any other part of the system at any time.
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) unsafe fn init_memory_blocks(
        &self,
        memory_type: GcdMemoryType,
        base_address: usize,
        len: usize,
        attributes: u64,
        capabilities: u64,
    ) -> Result<usize, EfiError> {
        // SAFETY: Caller must uphold the safety contract of init_memory_blocks
        unsafe { self.memory.lock().init_memory_blocks(memory_type, base_address, len, attributes, capabilities) }
    }

    #[cfg_attr(coverage, coverage(off))]
    pub fn prioritize_32_bit_memory(&self, value: bool) {
        self.memory.lock().prioritize_32_bit_memory = value;
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
        self.page_table.lock().take();
        self.aliased_mappings.lock().clear();
        // Reset memory protection policy to default state
        self.memory_protection_policy.memory_allocation_default_attributes.set(efi::MEMORY_XP);
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

    /// Returns an iterator over GCD descriptors in the given range.
    ///
    /// Arguments:
    /// - `base_address`: The starting address of the range.
    /// - `len`: The length of the range.
    ///
    /// Returns:
    /// - An iterator that yields `MemorySpaceDescriptor`s without allocating memory.
    pub(crate) fn iter(
        &self,
        base_address: usize,
        len: usize,
    ) -> impl Iterator<Item = Result<MemorySpaceDescriptor, EfiError>> {
        DescRangeIterator::new(self, base_address, len)
    }

    /// This service adds reserved memory, system memory, or memory-mapped I/O resources to the global coherency domain of the processor.
    ///
    /// # Safety
    /// Since the first call with enough system memory will cause the creation of an array at `base_address` + [`MEMORY_BLOCK_SLICE_SIZE`].
    /// The memory from `base_address` to `base_address+len` must be inside the valid address range of the program and not in use.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.1
    pub unsafe fn add_memory_space(
        &self,
        memory_type: GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<usize, EfiError> {
        // SAFETY: caller upholds the contract for add_memory_space.
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
    #[cfg_attr(coverage, coverage(off))]
    pub fn remove_memory_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        let result = self.memory.lock().remove_memory_space(base_address, len);
        if result.is_ok() {
            if let Some(page_table) = &mut *self.page_table.lock() {
                match page_table.unmap_memory_region(base_address as u64, len as u64) {
                    Ok(()) => {}
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
        memory_type: GcdMemoryType,
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
                let mut attributes = match self
                    .get_memory_descriptor_for_address(*base_address as efi::PhysicalAddress, |d, _| {
                        d.memory_type != GcdMemoryType::NonExistent
                    }) {
                    Ok(descriptor) => descriptor.attributes,
                    Err(_) => DEFAULT_CACHE_ATTR,
                };
                // it is safe to call set_memory_space_attributes without calling set_memory_space_capabilities here
                // because we set efi::MEMORY_XP as a capability on all memory ranges we add to the GCD. A driver could
                // call set_memory_space_capabilities to remove the XP capability, but that is something that should
                // be caught and fixed.
                attributes =
                    self.memory_protection_policy.apply_allocated_memory_protection_policy(attributes, memory_type);
                match self.set_memory_space_attributes(*base_address, len, attributes) {
                    Ok(()) => (),
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

    // Internal worker for freeing memory space with different transition types
    fn free_memory_space_internal(
        &self,
        base_address: usize,
        len: usize,
        transition: MemoryStateTransition,
    ) -> Result<(), EfiError> {
        // check if this block is actually allocated by us and bail out if not, since we need to set the attributes
        // to coalesce the memory blocks before attempting to free them
        self.memory.lock().get_memory_block_allocation_state(base_address, len)?;

        let range = base_address as u64..base_address.checked_add(len).ok_or(EfiError::InvalidParameter)? as u64;

        // Set the attributes before freeing the memory space so that the memory blocks are merged together and we
        // can free the range. It is valid to call free pages on memory which has different attributes. If we fail the
        // free, the memory will be unmapped, but still marked allocated in the memory blocks. This is acceptable as it
        // will not be used again, we will return a failure to the caller and they can ignore this memory (which can
        // not be used after the failed free anyway).
        for desc_result in self.iter(base_address, len) {
            let desc = desc_result?;
            let current_range = desc.get_range_overlap_with_desc(&range);
            // we call the worker here because we want to ensure we are getting the caching attribute from the
            // correct descriptor. It is possible the caching attribute is different across descriptors.
            if let Err(e) = self.set_memory_space_attributes_worker(
                current_range.start as usize,
                (current_range.end - current_range.start) as usize,
                MemoryProtectionPolicy::apply_free_memory_policy(desc.attributes, desc.memory_type),
                desc.attributes,
            ) && e != EfiError::NotReady
            {
                // if we failed to set the attributes in the GCD, we want to catch it, but should still try to go
                // down and free the memory space. NotReady is ignored here because the memory bucket code will
                // call this before paging is initialized.
                log::error!(
                    "Failed to set free memory attributes for {:#x?} of length {:#x?} Status: {:#x?}",
                    current_range.start,
                    (current_range.end - current_range.start),
                    e
                );
                debug_assert!(false);
                return Err(e);
            }
        }

        match self.memory.lock().free_memory_space(base_address, len, transition) {
            Ok(()) => {
                if let Some(callback) = self.memory_change_callback {
                    callback(MapChangeType::FreeMemorySpace);
                }
                Ok(())
            }
            // During EBS case, just ignore
            Err(EfiError::AccessDenied) => Ok(()),
            other => other,
        }
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    #[cfg_attr(coverage, coverage(off))]
    pub fn free_memory_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        self.free_memory_space_internal(base_address, len, MemoryStateTransition::Free)
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// Ownership of the memory as indicated by the `image_handle` associated with the block is retained, which means that
    /// it cannot be re-allocated except by the original owner or by requests targeting a specific address within the
    /// block (i.e. [`Self::allocate_memory_space`] with [`AllocateType::Address`]).
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    #[cfg_attr(coverage, coverage(off))]
    pub fn free_memory_space_preserving_ownership(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        self.free_memory_space_internal(base_address, len, MemoryStateTransition::FreePreservingOwnership)
    }

    // This function is the per descriptor worker for set_memory_space_attributes. It assumes that the range being
    // passed to it fits entirely within a single GCD descriptor. The wrapper functions of this must guarantee this or
    // it will fail gracefully when splitting memory blocks.
    fn set_memory_space_attributes_worker(
        &self,
        base_address: usize,
        len: usize,
        attributes: u64,
        original_attributes: u64,
    ) -> Result<(), EfiError> {
        // this API allows for setting attributes across multiple descriptors in the GCD (assuming the capabilities
        // allow it). The lower level set_memory_space_attributes will only operate on a single entry in the GCD/page
        // table, so at this level we need to check to see if the range spans multiple entries and if so, we need to
        // split the range and call set_memory_space_attributes for each entry. We also need to set the paging
        // attributes per entry to ensure that we keep the GCD and page table in sync
        let attributes = MemoryProtectionPolicy::apply_nx_to_uc_policy(attributes);

        match self.memory.lock().set_memory_space_attributes(base_address, len, attributes) {
            Ok(()) => {}
            Err(e) => {
                log::error!(
                    "Failed to set GCD memory attributes for memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                );
                debug_assert!(false);
                return Err(e);
            }
        }

        // 0 is a valid value for paging attributes: it means RWX. 0 is invalid for cache attributes. edk2 has a
        // behavior where if the caller passes 0 for cache and paging attributes, then 0 (RWX) is not applied to
        // the page table and only the virtual attribute(s) are applied to the GCD, such as EFI_RUNTIME. In order
        // to maintain compatibility with existing drivers, we preserve this poor paradigm.
        if attributes & (efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK) != 0 {
            match self.set_paging_attributes(base_address, len, attributes) {
                Ok(()) => {}
                Err(EfiError::NotReady) => {
                    // before the page table is installed, we expect to get a return of NotReady. This means the GCD
                    // has been updated with the attributes, but the page table is not installed yet. In init_paging, the
                    // page table will be updated with the current state of the GCD. The code that calls into this expects
                    // NotReady to be returned, so we must catch that error and report it. However, we also need to
                    // make sure any attribute updates across descriptors update the full range and not error out here.
                    return Err(EfiError::NotReady);
                }
                Err(e) => {
                    log::error!(
                        "Failed to set page table memory attributes for memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                    );
                    debug_assert!(false);

                    // if we failed here, we shouldn't leave the GCD and the page table out of sync. Roll the GCD back
                    // to the previous attributes for this range. We may have partially updated this range in the GCD
                    // and the page table, but they will be in sync. We could attempt to continue here, but we need
                    // to return an error to the caller, so we might as well stop here.
                    if let Err(rollback_err) =
                        self.memory.lock().set_memory_space_attributes(base_address, len, original_attributes)
                    {
                        // well, we did our best. The GCD and page table are now out of sync, which is a critical error.
                        log::error!(
                            "Failed to roll back GCD attributes after page table attribute set failure. This is a critical error. GCD and page table are now out of sync. Rollback error: {rollback_err:?}"
                        );
                    }

                    return Err(e);
                }
            }
        }

        Ok(())
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
        let mut res = Ok(());
        let range = base_address as u64..base_address.checked_add(len).ok_or(EfiError::InvalidParameter)? as u64;

        for desc_result in self.iter(base_address, len) {
            let desc = desc_result?;
            let current_range = desc.get_range_overlap_with_desc(&range);

            match self.set_memory_space_attributes_worker(
                current_range.start as usize,
                (current_range.end - current_range.start) as usize,
                attributes,
                desc.attributes,
            ) {
                Ok(()) => {}
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
                        "Failed to set memory attributes for memory region {:#x?} of length {:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                        current_range.start,
                        (current_range.end - current_range.start),
                    );
                    debug_assert!(false);
                    return Err(e);
                }
            }
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
    ///
    /// # Arguments
    /// * `buffer` - A mutable reference to a vector to hold the descriptors.
    /// * `filter` - A closure invoked with each descriptor and a boolean indicating whether the
    ///   descriptor's block is allocated. Returns `true` if the descriptor should be included.
    pub fn get_memory_descriptors(
        &self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
        filter: fn(&dxe_services::MemorySpaceDescriptor, bool) -> bool,
    ) -> Result<(), EfiError> {
        self.memory.lock().get_memory_descriptors(buffer, filter)
    }

    // returns the descriptor for the given physical address.
    pub fn get_memory_descriptor_for_address(
        &self,
        address: efi::PhysicalAddress,
        filter: fn(&dxe_services::MemorySpaceDescriptor, bool) -> bool,
    ) -> Result<dxe_services::MemorySpaceDescriptor, EfiError> {
        self.memory.lock().get_memory_descriptor_for_address(address, filter)
    }

    /// returns the current count of blocks in the list.
    pub fn memory_descriptor_count(&self) -> usize {
        self.memory.lock().memory_descriptor_count()
    }

    // returns the current count of efi memory map relevant blocks in the list.
    pub fn memory_descriptor_count_for_efi_memory_map(&self) -> usize {
        self.memory.lock().memory_descriptor_count_for_efi_memory_map()
    }

    /// Populates a caller-provided buffer with EFI memory map descriptors.
    ///
    /// This function writes EFI memory descriptors directly into the provided buffer,
    /// merging consecutive regions with identical type and attributes.
    ///
    /// ## Arguments
    ///
    /// * `buffer` - Mutable slice to populate with EFI memory descriptors
    /// * `active_attributes` - If `true`, use active attributes; if `false`, use capabilities
    ///
    /// ## Returns
    ///
    /// The actual number of descriptors written to the buffer.
    pub fn populate_efi_memory_map(
        &self,
        buffer: &mut [efi::MemoryDescriptor],
        active_attributes: bool,
    ) -> Result<usize, EfiError> {
        self.memory.lock().populate_efi_memory_map(buffer, active_attributes)
    }

    /// Gets the last EFI memory map key (CRC32 hash).
    ///
    /// Returns `None` if no memory map key has been set.
    pub fn get_last_efi_memory_map_key(&self) -> Option<usize> {
        *self.last_efi_memory_map_key.lock()
    }

    /// Sets the last EFI memory map key by computing the CRC32 hash of the provided memory map bytes.
    ///
    /// # Arguments
    ///
    /// * `memory_map_bytes` - The byte slice representing the EFI memory map
    pub fn set_last_efi_memory_map_key(&self, memory_map_bytes: &[u8]) {
        let key = crc32::calculate_crc32(memory_map_bytes) as usize;
        *self.last_efi_memory_map_key.lock() = Some(key);
    }
}

impl Display for SpinLockedGcd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(gcd) = self.memory.try_lock() {
            writelncrlf!(f, "\n{gcd}")?;
        } else {
            writelncrlf!(f, "Locked: {:?}", self.memory.try_lock())?;
        }
        if let Some(gcd) = self.io.try_lock() {
            writelncrlf!(f, "\n{gcd}")?;
        } else {
            writelncrlf!(f, "Locked: {:?}", self.io.try_lock())?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for SpinLockedGcd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writelncrlf!(f, "{:?}", self.memory.try_lock())?;
        writelncrlf!(f, "{:?}", self.io.try_lock())?;
        Ok(())
    }
}

// SAFETY: SpinLockedGcd uses internal locks to serialize access to shared state.
unsafe impl Sync for SpinLockedGcd {}
// SAFETY: SpinLockedGcd is safe to move between threads because it owns thread-safe synchronization.
unsafe impl Send for SpinLockedGcd {}

/// Iterator over GCD memory descriptors within a specified range.
/// This iterator yields descriptors lazily to avoid allocating memory because this iterator is used before
/// all of memory is available.
pub(crate) struct DescRangeIterator<'a> {
    gcd: &'a SpinLockedGcd,
    current_base: u64,
    range_end: u64,
}

impl<'a> DescRangeIterator<'a> {
    fn new(gcd: &'a SpinLockedGcd, base_address: usize, len: usize) -> Self {
        Self { gcd, current_base: base_address as u64, range_end: (base_address + len) as u64 }
    }
}

impl Iterator for DescRangeIterator<'_> {
    type Item = Result<MemorySpaceDescriptor, EfiError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_base >= self.range_end {
            return None;
        }

        let descriptor =
            match self.gcd.get_memory_descriptor_for_address(self.current_base as efi::PhysicalAddress, |_, _| true) {
                Ok(desc) => desc,
                Err(e) => return Some(Err(e)),
            };

        let descriptor_end = descriptor.base_address + descriptor.length;
        let next_base = u64::min(descriptor_end, self.range_end);

        self.current_base = next_base;

        Some(Ok(descriptor))
    }
}
