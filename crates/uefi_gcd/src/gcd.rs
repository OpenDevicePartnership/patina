//! UEFI Global Coherency Domain (GCD)
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::ptr;

use alloc::{boxed::Box, slice, vec, vec::Vec};
use mu_pi::{dxe_services, hob};
use mu_rust_helpers::function;
use r_efi::efi;
use uefi_collections::{node_size, Error as SliceError, Rbt, SliceKey};

use crate::{ensure, error};

use super::{
    io_block::{self, Error as IoBlockError, IoBlock, IoBlockSplit, StateTransition as IoStateTransition},
    memory_block::{
        self, Error as MemoryBlockError, MemoryBlock, MemoryBlockSplit, StateTransition as MemoryStateTransition,
    },
};

// Todo: Move these to a centralized, permanent location
const UEFI_PAGE_SIZE: usize = 0x1000;
const UEFI_PAGE_MASK: usize = UEFI_PAGE_SIZE - 1;

const MEMORY_BLOCK_SLICE_LEN: usize = 4096;
pub const MEMORY_BLOCK_SLICE_SIZE: usize = MEMORY_BLOCK_SLICE_LEN * node_size::<MemoryBlock>();

const IO_BLOCK_SLICE_LEN: usize = 4096;
const IO_BLOCK_SLICE_SIZE: usize = IO_BLOCK_SLICE_LEN * node_size::<IoBlock>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotInitialized,
    InvalidParameter,
    OutOfResources,
    Unsupported,
    AccessDenied,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InternalError {
    MemoryBlock(MemoryBlockError),
    IoBlock(IoBlockError),
    Slice(SliceError),
}

#[derive(Debug, Clone, Copy)]
pub enum AllocateType {
    // Allocate from the lowest address to the highest address or until the specify address is reached (max address).
    BottomUp(Option<usize>),
    // Allocate from the highest address to the lowest address or until the specify address is reached (min address).
    TopDown(Option<usize>),
    // Allocate at this address.
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

#[derive(Debug)]
#[allow(clippy::upper_case_acronyms)]
//The Global Coherency Domain (GCD) Services are used to manage the memory resources visible to the boot processor.
struct GCD {
    maximum_address: usize,
    memory_blocks: Option<Rbt<'static, MemoryBlock>>,
}

impl GCD {
    // Create an instance of the Global Coherency Domain (GCD) for testing.
    #[cfg(test)]
    pub(crate) const fn new(processor_address_bits: u32) -> Self {
        assert!(processor_address_bits > 0);
        Self { memory_blocks: None, maximum_address: 1 << processor_address_bits }
    }

    pub fn init(&mut self, processor_address_bits: u32) {
        self.maximum_address = 1 << processor_address_bits;
    }

    unsafe fn init_memory_blocks(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<usize, Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(
            memory_type == dxe_services::GcdMemoryType::SystemMemory && len >= MEMORY_BLOCK_SLICE_SIZE,
            Error::OutOfResources
        );

        log::trace!(target: "allocations", "[{}] Initializing memory blocks at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Capabilities: {:#x}\n", function!(), capabilities);

        let unallocated_memory_space = MemoryBlock::Unallocated(dxe_services::MemorySpaceDescriptor {
            memory_type: dxe_services::GcdMemoryType::NonExistent,
            base_address: 0,
            length: self.maximum_address as u64,
            ..Default::default()
        });

        let mut memory_blocks =
            Rbt::new(slice::from_raw_parts_mut::<'static>(base_address as *mut u8, MEMORY_BLOCK_SLICE_SIZE));
        memory_blocks.add(unallocated_memory_space).map_err(|_| Error::OutOfResources)?;
        self.memory_blocks.replace(memory_blocks);

        self.add_memory_space(memory_type, base_address, len, capabilities)?;

        self.allocate_memory_space(
            AllocateType::Address(base_address),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            MEMORY_BLOCK_SLICE_SIZE,
            1 as _,
            None,
        )
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
    ) -> Result<usize, Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);

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

        let Some(memory_blocks) = &mut self.memory_blocks else {
            return self.init_memory_blocks(memory_type, base_address, len, capabilities);
        };

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;
        let block = memory_blocks.get_with_idx(idx).ok_or(Error::NotFound)?;

        ensure!(block.as_ref().memory_type == dxe_services::GcdMemoryType::NonExistent, Error::AccessDenied);

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::Add(memory_type, capabilities),
        ) {
            Ok(idx) => Ok(idx),
            Err(InternalError::MemoryBlock(MemoryBlockError::BlockOutsideRange)) => error!(Error::AccessDenied),
            Err(InternalError::MemoryBlock(MemoryBlockError::InvalidStateTransition)) => {
                error!(Error::InvalidParameter)
            }
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service removes reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.4
    pub fn remove_memory_space(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);

        log::trace!(target: "allocations", "[{}] Removing memory space at {:#x} of length {:#x}", function!(), base_address, len);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;
        let block = *memory_blocks.get_with_idx(idx).ok_or(Error::NotFound)?;

        match Self::split_state_transition_at_idx(memory_blocks, idx, base_address, len, MemoryStateTransition::Remove)
        {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(MemoryBlockError::BlockOutsideRange)) => error!(Error::NotFound),
            Err(InternalError::MemoryBlock(MemoryBlockError::InvalidStateTransition)) => match block {
                MemoryBlock::Unallocated(_) => error!(Error::NotFound),
                MemoryBlock::Allocated(_) => error!(Error::AccessDenied),
            },
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service allocates nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.2
    pub fn allocate_memory_space(
        &mut self,
        allocate_type: AllocateType,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(
            len > 0 && image_handle > ptr::null_mut() && memory_type != dxe_services::GcdMemoryType::Unaccepted,
            Error::InvalidParameter
        );

        log::trace!(target: "allocations", "[{}] Allocating memory space: {:x?}", function!(), allocate_type);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        match allocate_type {
            AllocateType::BottomUp(max_address) => self.allocate_bottom_up(
                memory_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::TopDown(min_address) => self.allocate_top_down(
                memory_type,
                alignment,
                len,
                image_handle,
                device_handle,
                min_address.unwrap_or(0),
            ),
            AllocateType::Address(address) => {
                ensure!(address + len <= self.maximum_address, Error::NotFound);
                self.allocate_address(memory_type, alignment, len, image_handle, device_handle, address)
            }
        }
    }

    fn allocate_bottom_up(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, Error> {
        ensure!(len > 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Bottom up GCD allocation: {:#?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let mut current = memory_blocks.first_idx();
        while let Some(idx) = current {
            let mb = memory_blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            if mb.len() < len {
                current = memory_blocks.next_idx(idx);
                continue;
            }
            let address = mb.start();
            let mut addr = address & (usize::MAX << alignment);
            if addr < address {
                addr += 1 << alignment;
            }
            ensure!(addr + len <= max_address, Error::NotFound);
            if mb.as_ref().memory_type != memory_type {
                current = memory_blocks.next_idx(idx);
                continue;
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
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        if max_address == usize::MAX {
            Err(Error::OutOfResources)
        } else {
            Err(Error::NotFound)
        }
    }

    fn allocate_top_down(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        min_address: usize,
    ) -> Result<usize, Error> {
        ensure!(len > 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Top down GCD allocation: {:#?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Min Address: {:#x}", function!(), min_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let mut current = memory_blocks.last_idx();
        while let Some(idx) = current {
            let mb = memory_blocks.get_with_idx(idx).expect("idx is valid from prev_idx");
            if mb.len() < len {
                current = memory_blocks.prev_idx(idx);
                continue;
            }
            let mut addr = mb.end() - len;
            if addr < mb.start() {
                current = memory_blocks.prev_idx(idx);
                continue;
            }
            addr &= usize::MAX << alignment;
            ensure!(addr >= min_address, Error::NotFound);

            if mb.as_ref().memory_type != memory_type {
                current = memory_blocks.prev_idx(idx);
                continue;
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
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        if min_address == 0 {
            Err(Error::OutOfResources)
        } else {
            Err(Error::NotFound)
        }
    }

    fn allocate_address(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        address: usize,
    ) -> Result<usize, Error> {
        ensure!(len > 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Exact address GCD allocation: {:#?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Address: {:#x}", function!(), address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory Type: {:?}", function!(), memory_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(address as u64)).ok_or(Error::NotFound)?;
        let block = memory_blocks.get_with_idx(idx).ok_or(Error::NotFound)?;

        ensure!(
            block.as_ref().memory_type == memory_type && address == address & (usize::MAX << alignment),
            Error::NotFound
        );

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            address,
            len,
            MemoryStateTransition::Allocate(image_handle, device_handle),
        ) {
            Ok(_) => Ok(address),
            Err(InternalError::MemoryBlock(_)) => error!(Error::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    pub fn free_memory_space(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        self.free_memory_space_worker(base_address, len, MemoryStateTransition::Free)
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
    pub fn free_memory_space_preserving_ownership(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        self.free_memory_space_worker(base_address, len, MemoryStateTransition::FreePreservingOwnership)
    }

    fn free_memory_space_worker(
        &mut self,
        base_address: usize,
        len: usize,
        transition: MemoryStateTransition,
    ) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);

        log::trace!(target: "allocations", "[{}] Freeing memory space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Memory State Transition: {:?}\n", function!(), transition);

        // This is temporary until mu-paging is enabled. Free memory in the current scheme has 0 attrs set, so when
        // we free pages, we need to reset the attrs to 0 so that the pages can be merged with other free pages
        // When mu-paging is brought in, unallocated memory will be unmapped, so this logic will look different
        // Don't check the error here, we still want to free the memory if possible
        let _ = self.set_memory_space_attributes(base_address, len, 0);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;

        match Self::split_state_transition_at_idx(memory_blocks, idx, base_address, len, transition) {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(_)) => error!(Error::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
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
    ) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);
        ensure!((base_address & UEFI_PAGE_MASK) == 0 && (len & UEFI_PAGE_MASK) == 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Setting memory space attributes for {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Attributes: {:#x}\n", function!(), attributes);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::SetAttributes(attributes),
        ) {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(_)) => error!(Error::Unsupported),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
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
    ) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);
        ensure!((base_address & UEFI_PAGE_MASK) == 0 && (len & UEFI_PAGE_MASK) == 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Setting memory space capabilities for {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Capabilities: {:#x}\n", function!(), capabilities);

        let memory_blocks = self.memory_blocks.as_mut().ok_or(Error::NotFound)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = memory_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;

        match Self::split_state_transition_at_idx(
            memory_blocks,
            idx,
            base_address,
            len,
            MemoryStateTransition::SetCapabilities(capabilities),
        ) {
            Ok(_) => Ok(()),
            Err(InternalError::MemoryBlock(_)) => error!(Error::Unsupported),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
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
    ) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(buffer.capacity() >= self.memory_descriptor_count(), Error::InvalidParameter);
        ensure!(buffer.is_empty(), Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Enter\n", function!(), );

        if let Some(blocks) = &mut self.memory_blocks {
            let mut current = blocks.first_idx();
            while let Some(idx) = current {
                let mb = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
                match mb {
                    MemoryBlock::Allocated(descriptor) | MemoryBlock::Unallocated(descriptor) => {
                        buffer.push(*descriptor)
                    }
                }
                current = blocks.next_idx(idx);
            }
            Ok(())
        } else {
            Err(Error::NotFound)
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
        self.memory_blocks.as_ref().map(|mbs| mbs.len()).unwrap_or(0)
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
    io_blocks: Option<Rbt<'static, IoBlock>>,
}

impl IoGCD {
    // Create an instance of the Global Coherency Domain (GCD) for testing.
    #[cfg(test)]
    pub(crate) const fn _new(io_address_bits: u32) -> Self {
        assert!(io_address_bits > 0);
        Self { io_blocks: None, maximum_address: 1 << io_address_bits }
    }

    pub fn init(&mut self, io_address_bits: u32) {
        self.maximum_address = 1 << io_address_bits;
    }

    fn init_io_blocks(&mut self) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);

        let mut io_blocks = Rbt::new(unsafe {
            Box::into_raw(vec![0_u8; IO_BLOCK_SLICE_SIZE].into_boxed_slice())
                .as_mut()
                .expect("RBT given null pointer in initialization.")
        });

        io_blocks
            .add(IoBlock::Unallocated(dxe_services::IoSpaceDescriptor {
                io_type: dxe_services::GcdIoType::NonExistent,
                base_address: 0,
                length: self.maximum_address as u64,
                ..Default::default()
            }))
            .map_err(|_| Error::OutOfResources)?;

        self.io_blocks.replace(io_blocks);

        Ok(())
        /*
        ensure!(memory_type == dxe_services::GcdMemoryType::SystemMemory && len >= MEMORY_BLOCK_SLICE_SIZE, Error::OutOfResources);

        let unallocated_memory_space = MemoryBlock::Unallocated(dxe_services::MemorySpaceDescriptor {
          memory_type: dxe_services::GcdMemoryType::NonExistent,
          base_address: 0,
          length: self.maximum_address as u64,
          ..Default::default()
        });

        let mut memory_blocks =
          SortedSlice::new(slice::from_raw_parts_mut::<'static>(base_address as *mut u8, MEMORY_BLOCK_SLICE_SIZE));
        memory_blocks.add(unallocated_memory_space).map_err(|_| Error::OutOfResources)?;
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
    ) -> Result<usize, Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);

        log::trace!(target: "allocations", "[{}] Adding IO space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}\n", function!(), io_type);

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }

        let Some(io_blocks) = &mut self.io_blocks else {
            return Err(Error::NotInitialized);
        };

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;
        let block = io_blocks.get_with_idx(idx).ok_or(Error::NotFound)?;

        ensure!(block.as_ref().io_type == dxe_services::GcdIoType::NonExistent, Error::AccessDenied);

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Add(io_type)) {
            Ok(idx) => Ok(idx),
            Err(InternalError::IoBlock(IoBlockError::BlockOutsideRange)) => error!(Error::AccessDenied),
            Err(InternalError::IoBlock(IoBlockError::InvalidStateTransition)) => error!(Error::InvalidParameter),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service removes reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.12
    pub fn remove_io_space(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);

        log::trace!(target: "allocations", "[{}] Removing IO space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}\n", function!(), len);

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }

        let io_blocks = self.io_blocks.as_mut().ok_or(Error::NotInitialized)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;
        let block = *io_blocks.get_with_idx(idx).expect("Idx valid from get_closest_idx");

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Remove) {
            Ok(_) => Ok(()),
            Err(InternalError::IoBlock(IoBlockError::BlockOutsideRange)) => error!(Error::NotFound),
            Err(InternalError::IoBlock(IoBlockError::InvalidStateTransition)) => match block {
                IoBlock::Unallocated(_) => error!(Error::NotFound),
                IoBlock::Allocated(_) => error!(Error::AccessDenied),
            },
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
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
    ) -> Result<usize, Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0 && image_handle > ptr::null_mut(), Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Allocating IO space: {:x?}", function!(), allocate_type);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        match allocate_type {
            AllocateType::BottomUp(max_address) => self.allocate_bottom_up(
                io_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::TopDown(min_address) => {
                self.allocate_top_down(io_type, alignment, len, image_handle, device_handle, min_address.unwrap_or(0))
            }
            AllocateType::Address(address) => {
                ensure!(address + len <= self.maximum_address, Error::Unsupported);
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
    ) -> Result<usize, Error> {
        ensure!(len > 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Bottom up IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }

        let io_blocks = self.io_blocks.as_mut().ok_or(Error::NotInitialized)?;

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
            ensure!(addr + len <= max_address, Error::NotFound);
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
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(Error::NotFound)
    }

    fn allocate_top_down(
        &mut self,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        min_address: usize,
    ) -> Result<usize, Error> {
        ensure!(len > 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Top dowm IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Min Address: {:#x}", function!(), min_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }

        let io_blocks = self.io_blocks.as_mut().ok_or(Error::NotInitialized)?;

        log::trace!(target: "gcd_measure", "search");
        let mut current = io_blocks.last_idx();
        while let Some(idx) = current {
            let ib = io_blocks.get_with_idx(idx).expect("idx is valid from prev_idx");
            if ib.len() < len {
                current = io_blocks.prev_idx(idx);
                continue;
            }
            let mut addr = ib.end() - len;
            if addr < ib.start() {
                current = io_blocks.prev_idx(idx);
                continue;
            }
            addr &= usize::MAX << alignment;
            ensure!(addr >= min_address, Error::NotFound);

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
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(Error::NotFound)
    }

    fn allocate_address(
        &mut self,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        address: usize,
    ) -> Result<usize, Error> {
        ensure!(len > 0, Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Exact address IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Address: {:#x}", function!(), address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x}\n", function!(), alignment);

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }
        let io_blocks = self.io_blocks.as_mut().ok_or(Error::NotInitialized)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(address as u64)).ok_or(Error::NotFound)?;
        let block = io_blocks.get_with_idx(idx).ok_or(Error::NotFound)?;

        ensure!(block.as_ref().io_type == io_type && address == address & (usize::MAX << alignment), Error::NotFound);

        match Self::split_state_transition_at_idx(
            io_blocks,
            idx,
            address,
            len,
            IoStateTransition::Allocate(image_handle, device_handle),
        ) {
            Ok(_) => Ok(address),
            Err(InternalError::IoBlock(_)) => error!(Error::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service frees reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.11
    pub fn free_io_space(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(len > 0, Error::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, Error::Unsupported);

        log::trace!(target: "allocations", "[{}] Free IO space at {:#?}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}\n", function!(), len);

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }

        let io_blocks = self.io_blocks.as_mut().ok_or(Error::NotInitialized)?;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(Error::NotFound)?;

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Free) {
            Ok(_) => Ok(()),
            Err(InternalError::IoBlock(_)) => error!(Error::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(Error::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service returns a copy of the current set of memory blocks in the GCD.
    /// Since GCD is used to service heap expansion requests and thus should avoid allocations,
    /// Caller is required to initialize a vector of sufficient capacity to hold the descriptors
    /// and provide a mutable reference to it.
    pub fn get_io_descriptors(&mut self, buffer: &mut Vec<dxe_services::IoSpaceDescriptor>) -> Result<(), Error> {
        ensure!(self.maximum_address != 0, Error::NotInitialized);
        ensure!(buffer.capacity() >= self.io_descriptor_count(), Error::InvalidParameter);
        ensure!(buffer.is_empty(), Error::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Enter\n", function!(), );

        if self.io_blocks.is_none() {
            self.init_io_blocks()?;
        }

        if let Some(blocks) = &mut self.io_blocks {
            let mut current = blocks.first_idx();
            while let Some(idx) = current {
                let ib = blocks.get_with_idx(idx).expect("Index comes from dfs and should be valid");
                match ib {
                    IoBlock::Allocated(descriptor) | IoBlock::Unallocated(descriptor) => buffer.push(*descriptor),
                }
                current = blocks.next_idx(idx);
            }
            Ok(())
        } else {
            Err(Error::NotFound)
        }
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
        self.io_blocks.as_ref().map(|ibs| ibs.len()).unwrap_or(0)
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

/// Implements a spin locked GCD  suitable for use as a static global.
#[derive(Debug)]
pub struct SpinLockedGcd {
    memory: tpl_lock::TplMutex<GCD>,
    io: tpl_lock::TplMutex<IoGCD>,
    memory_change_callback: Option<MapChangeCallback>,
}

impl SpinLockedGcd {
    /// Creates a new uninitialized GCD. [`Self::init`] must be invoked before any other functions or they will return
    /// [`Error::NotInitialized`]. An optional callback can be provided which will be invoked whenever an operation
    /// changes the GCD map.
    pub const fn new(memory_change_callback: Option<MapChangeCallback>) -> Self {
        Self {
            memory: tpl_lock::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                GCD { maximum_address: 0, memory_blocks: None },
                "GcdMemLock",
            ),
            io: tpl_lock::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                IoGCD { maximum_address: 0, io_blocks: None },
                "GcdIoLock",
            ),
            memory_change_callback,
        }
    }

    /// Resets the GCD to default state. Intended for test scenarios.
    ///
    /// # Safety
    ///
    /// This call potentially invalidates all allocations made by any allocator on top of this GCD.
    /// Caller is responsible for ensuring that no such allocations exist.
    ///
    pub unsafe fn reset(&self) {
        let (mut mem, mut io) = (self.memory.lock(), self.io.lock());
        mem.maximum_address = 0;
        mem.memory_blocks = None;
        io.maximum_address = 0;
        io.io_blocks = None;
    }

    /// Initializes the underlying memory GCD and I/O GCD with the given address bits.
    pub fn init(&self, memory_address_bits: u32, io_address_bits: u32) {
        self.memory.lock().init(memory_address_bits);
        self.io.lock().init(io_address_bits);
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
    ) -> Result<usize, Error> {
        let result = self.memory.lock().add_memory_space(memory_type, base_address, len, capabilities);
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::AddMemorySpace)
            }
        }
        result
    }

    /// This service removes reserved memory, system memory, or memory-mapped I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.4
    pub fn remove_memory_space(&self, base_address: usize, len: usize) -> Result<(), Error> {
        let result = self.memory.lock().remove_memory_space(base_address, len);
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::RemoveMemorySpace)
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
    ) -> Result<usize, Error> {
        let result = self.memory.lock().allocate_memory_space(
            allocate_type,
            memory_type,
            alignment,
            len,
            image_handle,
            device_handle,
        );
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::AllocateMemorySpace)
            }

            // if we successfully allocated memory, we want to set the range as NX. For any standard data, we should
            // always have NX set and no consumer needs to update it. If a code region is going to be allocated
            // here, we rely on the image loader to update the attributes as appropriate for the code sections. The
            // same holds true for other required attributes.
            if let Ok(base_address) = result.as_ref() {
                // it is safe to call set_memory_space_attributes without calling set_memory_space_capabilities here
                // because we set efi::MEMORY_XP as a capability on all memory ranges we add to the GCD. A driver could
                // call set_memory_space_capabilities to remove the XP capability, but that is something that should
                // be caught and fixed.
                match self.set_memory_space_attributes(*base_address, len, efi::MEMORY_XP) {
                    Ok(_) => (),
                    Err(Error::NotInitialized) => {
                        // this is expected if mu-paging is not initialized yet. The GCD will still be updated, but
                        // the page table will not yet. When we initialize mu-paging, the GCD will use the attributes
                        // that have been updated here to initialize the page table. mu-paging must allocate memory
                        // to form the page table we are going to use.
                    }
                    Err(e) => {
                        // this is now a real error case, mu-paging is enabled, but we failed to set NX on the
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
        }
        result
    }

    /// This service frees nonexistent memory, reserved memory, system memory, or memory-mapped I/O resources from the
    /// global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
    pub fn free_memory_space(&self, base_address: usize, len: usize) -> Result<(), Error> {
        let result = self.memory.lock().free_memory_space(base_address, len);
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::FreeMemorySpace)
            }
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
    pub fn free_memory_space_preserving_ownership(&self, base_address: usize, len: usize) -> Result<(), Error> {
        let result = self.memory.lock().free_memory_space_preserving_ownership(base_address, len);
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::FreeMemorySpace)
            }
        }
        result
    }

    /// This service sets attributes on the given memory space.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
    pub fn set_memory_space_attributes(&self, base_address: usize, len: usize, attributes: u64) -> Result<(), Error> {
        let result = self.memory.lock().set_memory_space_attributes(base_address, len, attributes);
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::SetMemoryAttributes)
            }
        }
        result
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
    ) -> Result<(), Error> {
        let result = self.memory.lock().set_memory_space_capabilities(base_address, len, capabilities);
        if result.is_ok() {
            if let Some(callback) = self.memory_change_callback {
                callback(MapChangeType::SetMemoryCapabilities)
            }
        }
        result
    }

    /// returns a copy of the current set of memory blocks descriptors in the GCD.
    pub fn get_memory_descriptors(&self, buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>) -> Result<(), Error> {
        self.memory.lock().get_memory_descriptors(buffer)
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
    ) -> Result<usize, Error> {
        self.io.lock().add_io_space(io_type, base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::remove_io_space`]
    pub fn remove_io_space(&self, base_address: usize, len: usize) -> Result<(), Error> {
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
    ) -> Result<usize, Error> {
        self.io.lock().allocate_io_space(allocate_type, io_type, alignment, len, image_handle, device_handle)
    }

    /// Acquires lock and delegates to [`IoGCD::free_io_space]
    pub fn free_io_space(&self, base_address: usize, len: usize) -> Result<(), Error> {
        self.io.lock().free_io_space(base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::get_io_descriptors`]
    pub fn get_io_descriptors(&self, buffer: &mut Vec<dxe_services::IoSpaceDescriptor>) -> Result<(), Error> {
        self.io.lock().get_io_descriptors(buffer)
    }

    /// Acquires lock and delegates to [`IoGCD::io_descriptor_count`]
    pub fn io_descriptor_count(&self) -> usize {
        self.io.lock().io_descriptor_count()
    }
}

unsafe impl Sync for SpinLockedGcd {}
unsafe impl Send for SpinLockedGcd {}

#[cfg(test)]
mod tests {
    extern crate std;
    use core::{alloc::Layout, sync::atomic::AtomicBool};

    use super::*;
    use alloc::{vec, vec::Vec};
    use r_efi::efi;

    #[test]
    fn test_gcd_initialization() {
        let gdc = GCD::new(48);
        assert_eq!(2_usize.pow(48), gdc.maximum_address);
        assert!(gdc.memory_blocks.is_none());
        assert_eq!(0, gdc.memory_descriptor_count())
    }

    #[test]
    fn test_add_memory_space_before_memory_blocks_instantiated() {
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);

        assert_eq!(
            Err(Error::OutOfResources),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, address, MEMORY_BLOCK_SLICE_SIZE, 0) },
            "First add memory space should be a system memory."
        );
        assert_eq!(0, gcd.memory_descriptor_count());

        assert_eq!(
            Err(Error::OutOfResources),
            unsafe {
                gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, MEMORY_BLOCK_SLICE_SIZE - 1, 0)
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
            Err(Error::InvalidParameter),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::NonExistent, 10, 1, 0) },
            "Can't manually add NonExistent memory space manually."
        );

        assert!(is_gcd_memory_slice_valid(&gcd));
        assert_eq!(snapshot, copy_memory_block(&mut gcd));
    }

    #[test]
    fn test_add_memory_space_with_0_len_block() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(Error::InvalidParameter), unsafe {
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
            assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr + n, 1, n as u64) }
                .is_ok());
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        let res = unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr + n, 1, n as u64) };
        assert_eq!(
            Err(Error::OutOfResources),
            res,
            "Should return out of memory if there is no space in memory blocks."
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    }

    #[test]
    fn test_add_memory_space_outside_processor_range() {
        let (mut gcd, _) = create_gcd();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(Err(Error::Unsupported), unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address + 1, 1, 0)
        });
        assert_eq!(Err(Error::Unsupported), unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address, 1, 0)
        });
        assert_eq!(Err(Error::Unsupported), unsafe {
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
            Err(Error::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 1002, 5, 0) },
            "Can't add inside a range previously added."
        );
        assert_eq!(
            Err(Error::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::Reserved, 998, 5, 0) },
            "Can't add partially inside a range previously added (Start)."
        );
        assert_eq!(
            Err(Error::AccessDenied),
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
            Err(Error::AccessDenied),
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, 5, 0) },
            "Can't add inside a range previously allocated."
        );
        assert_eq!(
            Err(Error::AccessDenied),
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
                let mb = gcd.memory_blocks.as_ref().unwrap().get_with_idx(idx).unwrap();
                assert_eq!(1000, mb.as_ref().base_address);
                assert_eq!(20, mb.as_ref().length);
                assert_eq!(block_count, gcd.memory_descriptor_count());
            }
            Err(e) => assert!(false, "{e:?}"),
        }

        // Test merging when added before
        match unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 990, 10, 0) } {
            Ok(idx) => {
                let mb = gcd.memory_blocks.as_ref().unwrap().get_with_idx(idx).unwrap();
                assert_eq!(990, mb.as_ref().base_address);
                assert_eq!(30, mb.as_ref().length);
                assert_eq!(block_count, gcd.memory_descriptor_count());
            }
            Err(e) => assert!(false, "{e:?}"),
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
                let mb = *gcd.memory_blocks.unwrap().get_with_idx(idx).unwrap();
                match mb {
                    MemoryBlock::Unallocated(md) => {
                        assert_eq!(100, md.base_address);
                        assert_eq!(10, md.length);
                        assert_eq!(efi::MEMORY_RUNTIME | efi::MEMORY_ACCESS_MASK | 123, md.capabilities);
                        assert_eq!(0, md.image_handle as usize);
                        assert_eq!(0, md.device_handle as usize);
                    }
                    MemoryBlock::Allocated(_) => assert!(false, "Add should keep the block unallocated"),
                }
            }
            Err(e) => assert!(false, "{e:?}"),
        }
    }

    #[test]
    fn test_remove_memory_space_before_memory_blocks_instantiated() {
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);

        assert_eq!(Err(Error::NotFound), gcd.remove_memory_space(address, MEMORY_BLOCK_SLICE_SIZE));
    }

    #[test]
    fn test_remove_memory_space_with_0_len_block() {
        let (mut gcd, _) = create_gcd();

        // Add memory space to remove in a valid area.
        assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 10, 0) }.is_ok());

        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(Error::InvalidParameter), gcd.remove_memory_space(5, 0));

        assert_eq!(
            Err(Error::InvalidParameter),
            gcd.remove_memory_space(10, 0),
            "If there is no allocate done first, 0 length invalid param should have priority."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_remove_memory_space_outside_processor_range() {
        let (mut gcd, _) = create_gcd();
        // Add memory space to remove in a valid area.
        assert!(unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, gcd.maximum_address - 10, 10, 0)
        }
        .is_ok());

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(Error::Unsupported),
            gcd.remove_memory_space(gcd.maximum_address - 10, 11),
            "An address outside the processor range support is invalid."
        );
        assert_eq!(
            Err(Error::Unsupported),
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

        assert_eq!(Err(Error::NotFound), gcd.remove_memory_space(95, 10), "Can't remove memory space partially added.");
        assert_eq!(
            Err(Error::NotFound),
            gcd.remove_memory_space(105, 10),
            "Can't remove memory space partially added."
        );
        assert_eq!(
            Err(Error::NotFound),
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
            Err(Error::NotFound),
            gcd.remove_memory_space(address - 5, 10),
            "Can't remove memory space partially allocated."
        );
        assert_eq!(
            Err(Error::NotFound),
            gcd.remove_memory_space(address + MEMORY_BLOCK_SLICE_SIZE - 5, 10),
            "Can't remove memory space partially allocated."
        );

        assert_eq!(
            Err(Error::AccessDenied),
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
            assert!(unsafe {
                gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, addr + 10 + n, 1, n as u64)
            }
            .is_ok());
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(Error::OutOfResources),
            gcd.remove_memory_space(addr, 5),
            "Should return out of memory if there is no space in memory blocks."
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    }

    #[test]
    fn test_remove_memory_space_block_merging() {
        let (mut gcd, address) = create_gcd();
        assert!(unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1, address - 2, 0) }.is_ok());

        let block_count = gcd.memory_descriptor_count();

        for i in 1..10 {
            assert!(gcd.remove_memory_space(address - 1 - i, 1).is_ok());
        }

        // First index because the add memory started at address 1.
        assert_eq!(address - 10, copy_memory_block(&gcd)[2].as_ref().base_address as usize);
        assert_eq!(10, copy_memory_block(&gcd)[2].as_ref().length as usize);
        assert_eq!(block_count, gcd.memory_descriptor_count());
        assert!(is_gcd_memory_slice_valid(&gcd));

        for i in 1..10 {
            assert!(gcd.remove_memory_space(i, 1).is_ok());
        }
        // First index because the add memory started at address 1.
        assert_eq!(0, copy_memory_block(&gcd)[0].as_ref().base_address as usize);
        assert_eq!(10, copy_memory_block(&gcd)[0].as_ref().length as usize);
        assert_eq!(block_count, gcd.memory_descriptor_count());
        assert!(is_gcd_memory_slice_valid(&gcd));

        // Removing in the middle should create a 2 new block.
        assert!(gcd.remove_memory_space(100, 1).is_ok());
        assert_eq!(block_count + 2, gcd.memory_descriptor_count());
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
                    MemoryBlock::Allocated(_) => assert!(false, "remove should keep the block unallocated"),
                }
            }
            Err(e) => assert!(false, "{e:?}"),
        }
    }

    #[test]
    fn test_allocate_memory_space_before_memory_blocks_instantiated() {
        let mut gcd = GCD::new(48);
        assert_eq!(
            Err(Error::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
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
            Err(Error::InvalidParameter),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::Reserved,
                0,
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
            Err(Error::InvalidParameter),
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
            Err(Error::NotFound),
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
            Err(Error::NotFound),
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
            unsafe { gcd.add_memory_space(memory_type, i * 10, 10, 0) }.unwrap();
            let res = gcd.allocate_memory_space(AllocateType::Address(i * 10), memory_type, 0, 10, 1 as _, None);
            match memory_type {
                dxe_services::GcdMemoryType::Unaccepted => assert_eq!(Err(Error::InvalidParameter), res),
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
                Err(Error::OutOfResources),
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
                Err(Error::NotFound),
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
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 0x1000, 0) }.unwrap();

        assert_eq!(
            Ok(0),
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
            Ok(0x10),
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
            Ok(0x20),
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
            Ok(0xff1),
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
            Ok(0xfe0),
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
            Ok(0xf00),
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
            Ok(0xa00),
            gcd.allocate_memory_space(
                AllocateType::Address(0xa00),
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
            Err(Error::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0xa0f),
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
            Err(Error::NotFound),
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
            Err(Error::NotFound),
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
            Err(Error::NotFound),
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
            Err(Error::NotFound),
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
            Err(Error::NotFound),
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
        assert_eq!(Err(Error::NotFound), gcd.free_memory_space(0, 100));
    }

    #[test]
    fn test_free_memory_space_when_0_len_block() {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(Error::InvalidParameter), gcd.remove_memory_space(0, 0));
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

        assert_eq!(Err(Error::Unsupported), gcd.free_memory_space(gcd.maximum_address, 10));
        assert_eq!(Err(Error::Unsupported), gcd.free_memory_space(gcd.maximum_address - 99, 100));
        assert_eq!(Err(Error::Unsupported), gcd.free_memory_space(gcd.maximum_address + 1, 100));

        assert_eq!(snapshot, copy_memory_block(&gcd));
    }

    #[test]
    fn test_free_memory_space_in_range_not_allocated() {
        let (mut gcd, _) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1000, 100, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(1000),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            100,
            1 as _,
            None,
        )
        .unwrap();

        assert_eq!(Err(Error::NotFound), gcd.free_memory_space(1050, 100));
        assert_eq!(Err(Error::NotFound), gcd.free_memory_space(950, 100));
        assert_eq!(Err(Error::NotFound), gcd.free_memory_space(0, 100));
    }

    #[test]
    fn test_free_memory_space_when_memory_block_full() {
        let (mut gcd, _) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 100, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            100,
            1 as _,
            None,
        )
        .unwrap();

        let mut n = 1;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 1000 + n, 1, n as u64) }.unwrap();
            n += 1;
        }
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        assert_eq!(Err(Error::OutOfResources), gcd.free_memory_space(0, 1));

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    }

    #[test]
    fn test_free_memory_space_merging() {
        let (mut gcd, _) = create_gcd();

        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 1000, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            1000,
            1 as _,
            None,
        )
        .unwrap();

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(Ok(()), gcd.free_memory_space(0, 100), "Free beginning of a block.");
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert_eq!(Ok(()), gcd.free_memory_space(500, 100), "Free in the middle of a block");
        assert_eq!(block_count + 3, gcd.memory_descriptor_count());
        assert_eq!(Ok(()), gcd.free_memory_space(900, 100), "Free at the end of a block");
        assert_eq!(block_count + 4, gcd.memory_descriptor_count());

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(Ok(()), gcd.free_memory_space(100, 100));
        assert_eq!(block_count, gcd.memory_descriptor_count());

        let blocks = copy_memory_block(&gcd);
        let mb = blocks[0];
        assert_eq!(0, mb.as_ref().base_address);
        assert_eq!(200, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(600, 100));
        assert_eq!(block_count, gcd.memory_descriptor_count());
        let blocks = copy_memory_block(&gcd);
        let mb = blocks[2];
        assert_eq!(500, mb.as_ref().base_address);
        assert_eq!(200, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(800, 100));
        assert_eq!(block_count, gcd.memory_descriptor_count());
        let blocks = copy_memory_block(&gcd);
        let mb = blocks[4];
        assert_eq!(800, mb.as_ref().base_address);
        assert_eq!(200, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(750, 10));
        assert_eq!(block_count + 2, gcd.memory_descriptor_count());

        assert!(is_gcd_memory_slice_valid(&gcd));
    }

    #[test]
    fn test_set_memory_space_attributes_with_invalid_parameters() {
        let mut gcd = GCD { memory_blocks: None, maximum_address: 0 };
        assert_eq!(Err(Error::NotInitialized), gcd.set_memory_space_attributes(0, 0x50000, 0b1111));

        let (mut gcd, _) = create_gcd();

        // Test that setting memory space attributes on more space than is available is an error
        assert_eq!(Err(Error::Unsupported), gcd.set_memory_space_attributes(0x100000000000000, 50, 0b1111));

        // Test that calling set_memory_space_attributes with no size returns invalid parameter
        assert_eq!(Err(Error::InvalidParameter), gcd.set_memory_space_attributes(0, 0, 0b1111));

        // Test that calling set_memory_space_attributes with invalid attributes returns invalid parameter
        assert_eq!(Err(Error::InvalidParameter), gcd.set_memory_space_attributes(0, 0, 0));

        // Test that a non-page aligned address returns invalid parameter
        assert_eq!(Err(Error::InvalidParameter), gcd.set_memory_space_attributes(0xFFFFFFFF, 0x1000, efi::MEMORY_WB));

        // Test that a non-page aligned address with the runtime attribute set returns invalid parameter
        assert_eq!(
            Err(Error::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0x1000, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );

        // Test that a non-page aligned size returns invalid parameter
        assert_eq!(Err(Error::InvalidParameter), gcd.set_memory_space_attributes(0x1000, 0xFFF, efi::MEMORY_WB));

        // Test that a non-page aligned size returns invalid parameter
        assert_eq!(
            Err(Error::InvalidParameter),
            gcd.set_memory_space_attributes(0x1000, 0xFFF, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );

        // Test that a non-page aligned address and size returns invalid parameter
        assert_eq!(
            Err(Error::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0xFFF, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );
    }

    #[test]
    fn test_set_capabilities_and_attributes() {
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
        // Trying to set capabilities where the range falls outside a block should return unsupported
        assert_eq!(Err(Error::Unsupported), gcd.set_memory_space_capabilities(0, 0x3000, 0b1111));

        gcd.set_memory_space_capabilities(0, 0x2000, 0b1111).unwrap();

        // Trying to set attributes where the range falls outside a block should return unsupported
        assert_eq!(Err(Error::Unsupported), gcd.set_memory_space_attributes(0, 0x3000, 0b1));
        gcd.set_memory_space_attributes(0, 0x1000, 0b1).unwrap();
    }

    #[test]
    fn test_block_split_when_memory_blocks_full() {
        let (mut gcd, address) = create_gcd();
        unsafe { gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, address, 0) }.unwrap();

        let mut n = 1;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                dxe_services::GcdMemoryType::SystemMemory,
                0,
                0x2000,
                n as _,
                None,
            )
            .unwrap();
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        // Test that allocate_memory_space fails when full
        assert_eq!(
            Err(Error::OutOfResources),
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

        // Test that set_memory_space_attributes fails when full, if the block requires a split
        assert_eq!(Err(Error::OutOfResources), gcd.set_memory_space_capabilities(0x1000, 0x1000, 0b1111));

        // Set capabilities on an exact block so we don't split it, and can test failing set_attributes
        gcd.set_memory_space_capabilities(0x4000, 0x2000, 0b1111).unwrap();
        assert_eq!(Err(Error::OutOfResources), gcd.set_memory_space_attributes(0x5000, 0x1000, 0b1111));
    }

    #[test]
    fn test_invalid_add_io_space() {
        let mut gcd = IoGCD::_new(16);

        assert!(gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).is_ok());
        // Cannot Allocate a range in a range that is already allocated
        assert_eq!(Err(Error::AccessDenied), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10));

        // Cannot allocate a range as NonExistent
        assert_eq!(Err(Error::InvalidParameter), gcd.add_io_space(dxe_services::GcdIoType::NonExistent, 10, 10));

        // Cannot do more allocations if the underlying data structure is full
        for i in 1..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }
        assert_eq!(
            Err(Error::OutOfResources),
            gcd.add_io_space(dxe_services::GcdIoType::Io, (IO_BLOCK_SLICE_LEN + 1) * 10, 10)
        );
    }

    #[test]
    fn test_invalid_remove_io_space() {
        let mut gcd = IoGCD::_new(16);

        // Cannot remove a range of 0
        assert_eq!(Err(Error::InvalidParameter), gcd.remove_io_space(0, 0));

        // Cannot remove a range greater than what is available
        assert_eq!(Err(Error::Unsupported), gcd.remove_io_space(0, 70_000));

        // Cannot remove an io space if it does not exist
        assert_eq!(Err(Error::NotFound), gcd.remove_io_space(0, 10));

        // Cannot remove an io space if it is allocated
        gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).unwrap();
        gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        assert_eq!(Err(Error::AccessDenied), gcd.remove_io_space(0, 10));

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
        assert_eq!(Err(Error::OutOfResources), gcd.remove_io_space(25, 3));
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
            Err(Error::OutOfResources),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 0x4000)
        );
        assert_eq!(
            Err(Error::OutOfResources),
            gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 0)
        );
        assert_eq!(
            Err(Error::OutOfResources),
            gcd.allocate_address(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 210)
        );
    }

    #[test]
    fn test_allocate_bottom_up_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(Error::NotFound),
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
            Err(Error::NotFound),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x4000)
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
        assert_eq!(Ok(0xB0), gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 0x150, 1 as _, None, 0));

        assert_eq!(
            Err(Error::NotFound),
            gcd.allocate_top_down(dxe_services::GcdIoType::Reserved, 0, 0x150, 1 as _, None, 0)
        );
    }

    #[test]
    fn test_allocate_address_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(Error::NotFound),
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
            Err(Error::NotFound),
            gcd.allocate_address(dxe_services::GcdIoType::Reserved, 0, 0x100, 1 as _, None, 0)
        );
    }

    #[test]
    fn test_free_io_space_conformance() {
        let mut gcd = IoGCD::_new(16);

        // Cannot free a range of 0
        assert_eq!(Err(Error::InvalidParameter), gcd.free_io_space(0, 0));

        // Cannot free a range greater than what is available
        assert_eq!(Err(Error::Unsupported), gcd.free_io_space(0, 70_000));

        // Cannot free an io space if it does not exist
        assert_eq!(Err(Error::NotFound), gcd.free_io_space(0, 10));

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
        assert_eq!(Err(Error::OutOfResources), gcd.free_io_space(105, 3));
        assert_eq!(Ok(()), gcd.free_io_space(100, 10));
    }

    fn create_gcd() -> (GCD, usize) {
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, MEMORY_BLOCK_SLICE_SIZE, 0)
                .unwrap();
        }
        (gcd, address)
    }

    fn copy_memory_block(gcd: &GCD) -> Vec<MemoryBlock> {
        let Some(memory_blocks) = &gcd.memory_blocks else {
            return vec![];
        };

        memory_blocks.dfs()
    }

    fn is_gcd_memory_slice_valid(gcd: &GCD) -> bool {
        if let Some(memory_blocks) = gcd.memory_blocks.as_ref() {
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
        }
        true
    }

    unsafe fn get_memory(size: usize) -> &'static mut [u8] {
        let addr = alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, 8).unwrap());
        core::slice::from_raw_parts_mut(addr, size)
    }

    #[test]
    fn spin_locked_allocator_should_error_if_not_initialized() {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        let add_result = unsafe { GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, 0, 100, 0) };
        assert_eq!(add_result, Err(Error::NotInitialized));

        let allocate_result = GCD.allocate_memory_space(
            AllocateType::Address(0),
            dxe_services::GcdMemoryType::SystemMemory,
            0,
            10,
            1 as _,
            None,
        );
        assert_eq!(allocate_result, Err(Error::NotInitialized));

        let free_result = GCD.free_memory_space(0, 10);
        assert_eq!(free_result, Err(Error::NotInitialized));

        let remove_result = GCD.remove_memory_space(0, 10);
        assert_eq!(remove_result, Err(Error::NotInitialized));
    }

    #[test]
    fn spin_locked_allocator_init_should_initialize() {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        GCD.init(48, 16);
        unsafe {
            GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, MEMORY_BLOCK_SLICE_SIZE, 0)
                .unwrap();
        }

        GCD.add_io_space(dxe_services::GcdIoType::Io, 0, 100).unwrap();
        GCD.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        GCD.free_io_space(0, 10).unwrap();
        GCD.remove_io_space(0, 10).unwrap();
    }

    #[test]
    fn callback_should_fire_when_map_changes() {
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
            GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, MEMORY_BLOCK_SLICE_SIZE, 0)
                .unwrap();
        }

        assert!(CALLBACK_INVOKED.load(core::sync::atomic::Ordering::SeqCst));
    }

    fn align_up(address: usize, alignment: usize) -> usize {
        (address + alignment - 1) & !(alignment - 1)
    }

    #[test]
    fn test_spin_locked_set_attributes_capabilities() {
        static CALLBACK1: AtomicBool = AtomicBool::new(false);
        static CALLBACK2: AtomicBool = AtomicBool::new(false);
        fn map_callback(map_change_type: MapChangeType) {
            match map_change_type {
                MapChangeType::SetMemoryAttributes => CALLBACK1.store(true, core::sync::atomic::Ordering::SeqCst),
                MapChangeType::SetMemoryCapabilities => CALLBACK2.store(true, core::sync::atomic::Ordering::SeqCst),
                _ => {}
            }
        }

        static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(map_callback));

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
        let address = align_up(mem.as_ptr() as usize, 0x1000);
        GCD.init(48, 16);
        unsafe {
            GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, address, MEMORY_BLOCK_SLICE_SIZE, 0)
                .unwrap();
        }
        GCD.set_memory_space_capabilities(address, 0x1000, 0b1111).unwrap();
        GCD.set_memory_space_attributes(address, 0x1000, 0b1011).unwrap();

        assert!(CALLBACK1.load(core::sync::atomic::Ordering::SeqCst));
        assert!(CALLBACK2.load(core::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn allocate_bottom_up_should_allocate_increasing_addresses() {
        use std::{alloc::GlobalAlloc, println};
        const GCD_SIZE: usize = 0x100000;
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
        let base = unsafe { std::alloc::System.alloc(layout) as u64 };
        unsafe {
            GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, base as usize, GCD_SIZE, 0).unwrap();
        }

        println!("GCD base: {:#x?}", base);
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
            println!("Allocation result: {:#x?}", allocate_result);
            if let Ok(address) = allocate_result {
                assert!(
                    address > last_allocation,
                    "address {:#x?} is lower than previously allocated address {:#x?}",
                    address,
                    last_allocation
                );
                last_allocation = address;
            } else {
                break;
            }
        }
    }

    #[test]
    fn allocate_top_down_should_allocate_decreasing_addresses() {
        use std::{alloc::GlobalAlloc, println};
        const GCD_SIZE: usize = 0x100000;
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
        let base = unsafe { std::alloc::System.alloc(layout) as u64 };
        unsafe {
            GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, base as usize, GCD_SIZE, 0).unwrap();
        }

        println!("GCD base: {:#x?}", base);
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
            println!("Allocation result: {:#x?}", allocate_result);
            if let Ok(address) = allocate_result {
                assert!(
                    address < last_allocation,
                    "address {:#x?} is higher than previously allocated address {:#x?}",
                    address,
                    last_allocation
                );
                last_allocation = address;
            } else {
                break;
            }
        }
    }
}
