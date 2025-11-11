//! This module provides type definitions for UEFI Allocation Services
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::ops::{BitOr, BitOrAssign};

use r_efi::efi;

use super::{BootServices, boxed::BootServicesBox};

/// The way to perform a memory allocation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AllocType {
    /// No specific requirements for the allocation, allowing the system to choose the best option.
    AnyPage,
    /// Will allocate at an address no larger than the specified address.
    MaxAddress(usize),
    /// Will allocate at the specified address.
    Address(usize),
}

/// Represents a memory map in the UEFI system, containing an array of memory descriptors and metadata.
#[derive(Debug)]
pub struct MemoryMap<'a, B: BootServices + ?Sized> {
    /// An array of [efi::MemoryDescriptor]s that describe the memory map.
    pub descriptors: BootServicesBox<'a, [efi::MemoryDescriptor], B>,
    /// The key for the current memory map.
    pub map_key: usize,
    /// The version number associated with the [efi::MemoryDescriptor]
    pub descriptor_version: u32,
}

/// Memory attributes as specified in the UEFI specification.
///
/// Used to describe the attributes of a memory region.
///
/// <https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#efi-boot-services-getmemorymap>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAttribute(u64);

impl MemoryAttribute {
    /// Memory cacheability attribute: The memory region is not cacheable.
    pub const UC: MemoryAttribute = MemoryAttribute(efi::MEMORY_UC);
    /// Memory cacheability attribute: The memory region is write combined.
    pub const WC: MemoryAttribute = MemoryAttribute(efi::MEMORY_WC);
    /// Memory cacheability attribute: The memory region is cacheable with a “write through” policy. Writes that hit in
    /// the cache will also be written to main memory.
    pub const WT: MemoryAttribute = MemoryAttribute(efi::MEMORY_WT);
    /// Memory cacheability attribute: The memory region is cacheable with a “write back” policy. Reads and writes that
    /// hit in the cache do not propagate to main memory. Dirty data is written back to main memory when a new cache
    /// line is allocated.
    pub const WB: MemoryAttribute = MemoryAttribute(efi::MEMORY_WB);
    /// Memory cacheability attribute: The memory region is cacheable, exported, and supports the “fetch and add”
    /// semaphore mechanism.
    pub const UCE: MemoryAttribute = MemoryAttribute(efi::MEMORY_UCE);
    /// Physical memory protection attribute: The memory region is write-protected by system hardware. This is
    /// typically used as a cacheability attribute today. The memory region is cacheable with a “write protected”
    /// policy. Reads come from cache lines when possible, and read misses cause cache fills. Writes are propagated to
    /// the system bus and cause corresponding cache lines on all processors on the bus to be invalidated.
    pub const WP: MemoryAttribute = MemoryAttribute(efi::MEMORY_WP);
    /// Physical memory protection attribute: The memory region is read-protected by system hardware.
    pub const RP: MemoryAttribute = MemoryAttribute(efi::MEMORY_RP);
    /// Physical memory protection attribute: The memory region supports is protected by system hardware from executing code.
    pub const XP: MemoryAttribute = MemoryAttribute(efi::MEMORY_XP);
    /// Runtime memory attribute: The memory region refers to persistent memory
    pub const NV: MemoryAttribute = MemoryAttribute(efi::MEMORY_NV);
    /// The memory region provides higher reliability relative to other memory in the system. If all memory has the
    /// same reliability, then this bit is not used.
    pub const MORE_RELIABLE: MemoryAttribute = MemoryAttribute(efi::MEMORY_MORE_RELIABLE);
    /// Physical memory protection attribute: The memory region supports making this memory range read-only by system
    /// hardware.
    pub const RO: MemoryAttribute = MemoryAttribute(efi::MEMORY_RO);
    /// Specific-purpose memory (SPM). The memory is earmarked for specific purposes such as for specific device
    /// drivers or applications. The SPM attribute serves as a hint to the OS to avoid allocating this memory for core
    /// OS data or code that can not be relocated. Prolonged use of this memory for purposes other than the intended
    /// purpose may result in suboptimal platform performance.
    pub const SP: MemoryAttribute = MemoryAttribute(efi::MEMORY_SP);
    /// The memory region is protected with the CPU’s memory cryptographic capabilities. If this flag is clear, the
    /// memory region is not capable of being protected with the CPU’s memory cryptographic capabilities or the CPU
    /// does not support CPU memory cryptographic capabilities.
    pub const CPU_CRYPTO: MemoryAttribute = MemoryAttribute(efi::MEMORY_CPU_CRYPTO);
    /// Runtime memory attribute: The memory region needs to be given a virtual mapping by the operating system when
    /// SetVirtualAddressMap() is called.
    pub const RUNTIME: MemoryAttribute = MemoryAttribute(efi::MEMORY_RUNTIME);
    /// The memory region is described with additional ISA-specific memory attributes as specified in
    /// EFI_MEMORY_ISA_MASK.
    pub const ISA_VALID: MemoryAttribute = MemoryAttribute(efi::MEMORY_ISA_VALID);
    /// Bits reserved for describing optional ISA-specific cacheability attributes that are not covered by
    /// the standard UEFI Memory Attributes cacheability bits (EFI_MEMORY_UC, EFI_MEMORY_WC, EFI_MEMORY_WT,
    /// EFI_MEMORY_WB and EFI_MEMORY_UCE). See Calling Conventions for further ISA-specific enumeration of these bits.
    pub const ISA_MASK: MemoryAttribute = MemoryAttribute(efi::MEMORY_ISA_MASK);
}

impl BitOr for MemoryAttribute {
    type Output = MemoryAttribute;

    fn bitor(self, rhs: Self) -> Self::Output {
        MemoryAttribute(self.0 | rhs.0)
    }
}

impl BitOrAssign for MemoryAttribute {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0
    }
}

impl From<AllocType> for efi::AllocateType {
    fn from(val: AllocType) -> Self {
        match val {
            AllocType::AnyPage => efi::ALLOCATE_ANY_PAGES,
            AllocType::MaxAddress(_) => efi::ALLOCATE_MAX_ADDRESS,
            AllocType::Address(_) => efi::ALLOCATE_ADDRESS,
        }
    }
}

impl From<MemoryAttribute> for u64 {
    fn from(val: MemoryAttribute) -> Self {
        val.0
    }
}
