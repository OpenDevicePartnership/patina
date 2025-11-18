//! DXE Services
//!
//! Services used in the DXE boot phase:
//! - **Global Coherency Domain (GCD) Services** - The Global Coherency Domain (GCD) Services are used to manage the
//!   memory and I/O resources visible to the boot processor.
//! - **Dispatcher Services** - Used during preboot to schedule drivers for execution.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_Services_DXE_Services.html#services-dxe-services>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    cmp::{max, min},
    default::Default,
    ffi::c_void,
    ops::Range,
};

use r_efi::{
    efi::{Guid, Handle, PhysicalAddress, Status},
    system::TableHeader,
};

/// DXE Services Table GUID identifier
///
/// This GUID identifies the DXE Services Table in the EFI System Table
/// Configuration Table array. The DXE Services Table provides services for
/// managing the Global Coherency Domain memory and I/O space maps,
/// and dispatcher functions for managing driver execution dependencies.
pub const DXE_SERVICES_TABLE_GUID: Guid =
    Guid::from_fields(0x5ad34ba, 0x6f02, 0x4214, 0x95, 0x2e, &[0x4d, 0xa0, 0x39, 0x8e, 0x2b, 0xb9]);

/// Adds memory or memory-mapped I/O resources to the Global Coherency Domain (GCD)
///
/// This service adds reserved memory, system memory, or memory-mapped I/O resources
/// to the Global Coherency Domain of the processor. The memory space being added
/// must not overlap with any existing memory space in the GCD.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.1
pub type AddMemorySpace = extern "efiapi" fn(GcdMemoryType, PhysicalAddress, u64, u64) -> Status;

/// Allocates memory space from the Global Coherency Domain (GCD)
///
/// This service allocates nonexistent memory, reserved memory, system memory,
/// or memory-mapped I/O resources from the Global Coherency Domain of the processor.
/// The allocation strategy is determined by the GcdAllocateType parameter.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.2
pub type AllocateMemorySpace =
    extern "efiapi" fn(GcdAllocateType, GcdMemoryType, usize, u64, *mut PhysicalAddress, Handle, Handle) -> Status;

/// Frees memory space from the Global Coherency Domain (GCD)
///
/// This service frees nonexistent memory, reserved memory, system memory,
/// or memory-mapped I/O resources from the Global Coherency Domain of the processor.
/// The freed memory becomes available for future allocation.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
pub type FreeMemorySpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// Removes memory space from the Global Coherency Domain (GCD)
///
/// This service removes reserved memory, system memory, or memory-mapped I/O
/// resources from the Global Coherency Domain of the processor. The removed
/// region must not be currently allocated to any image or have any capabilities set.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.4
pub type RemoveMemorySpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// Retrieves the memory space descriptor for a specified address from the
/// Global Coherency Domain (GCD)
///
/// This service retrieves the descriptor for a memory region containing a
/// specified address from the Global Coherency Domain memory space map.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.5
pub type GetMemorySpaceDescriptor = extern "efiapi" fn(PhysicalAddress, *mut MemorySpaceDescriptor) -> Status;

/// Sets memory space attributes in the Global Coherency Domain (GCD)
///
/// This service modifies the attributes for a memory region in the global
/// coherency domain of the processor. Attributes control caching behavior
/// and access permissions for the memory region.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
pub type SetMemorySpaceAttributes = extern "efiapi" fn(PhysicalAddress, u64, u64) -> Status;

/// Sets memory space capabilities in the Global Coherency Domain (GCD)
///
/// This service modifies the capabilities for a memory region in the global
/// coherency domain of the processor. Capabilities define which attributes
/// are allowed to be set for the memory region.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.7
pub type SetMemorySpaceCapabilities = extern "efiapi" fn(PhysicalAddress, u64, u64) -> Status;

/// Returns the Global Coherency Domain (GCD) memory space map
///
/// This service returns a map of all memory resources in the global coherency
/// domain of the processor, including their types, attributes, and allocation status.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.8
pub type GetMemorySpaceMap = extern "efiapi" fn(*mut usize, *mut *mut MemorySpaceDescriptor) -> Status;

/// Adds I/O space to the Global Coherency Domain (GCD)
///
/// This service adds reserved I/O or I/O resources to the global coherency
/// domain of the processor. The I/O space being added must not overlap
/// with any existing I/O space in the Global Coherency Domain.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.9
pub type AddIoSpace = extern "efiapi" fn(GcdIoType, PhysicalAddress, u64) -> Status;

/// Allocates I/O space from the Global Coherency Domain (GCD)
///
/// This service allocates nonexistent I/O, reserved I/O, or I/O resources
/// from the Global Coherency Domain of the processor. The allocation strategy
/// is determined by the GcdAllocateType parameter.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.10
pub type AllocateIoSpace =
    extern "efiapi" fn(GcdAllocateType, GcdIoType, usize, u64, *mut PhysicalAddress, Handle, Handle) -> Status;

/// Frees I/O space from the Global Coherency Domain (GCD)
///
/// This service frees nonexistent I/O, reserved I/O, or I/O resources
/// from the Global Coherency Domain of the processor. The freed I/O space
/// becomes available for future allocation.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.11
pub type FreeIoSpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// Removes I/O space from the Global Coherency Domain (GCD)
///
/// This service removes reserved I/O or I/O resources from the global coherency
/// domain of the processor. The removed I/O region must not be currently allocated.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.12
pub type RemoveIoSpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// This service retrieves the descriptor for an I/O region containing a specified address.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.13
pub type GetIoSpaceDescriptor = extern "efiapi" fn(PhysicalAddress, *mut IoSpaceDescriptor) -> Status;

/// Returns a map of the I/O resources in the Global Coherency Domain (GCD).
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.14
pub type GetIoSpaceMap = extern "efiapi" fn(*mut usize, *mut *mut IoSpaceDescriptor) -> Status;

/// Executes DXE drivers from firmware volumes
///
/// This service loads and executes DXE drivers from firmware volumes.
/// The dispatcher uses dependency expressions to determine driver execution order.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.1
pub type Dispatch = extern "efiapi" fn() -> Status;

/// Schedules a firmware file for dispatch
///
/// This service clears the Schedule on Request (SOR) flag for a component
/// that is stored in a firmware volume, allowing it to be dispatched.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.2
pub type Schedule = extern "efiapi" fn(Handle, *const Guid) -> Status;

/// Promotes a firmware file from untrusted to trusted state
///
/// This service promotes a file stored in a firmware volume from the untrusted
/// to the trusted state. Only the Security Architectural Protocol can place a file
/// in the untrusted state initially.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.3
pub type Trust = extern "efiapi" fn(Handle, *const Guid) -> Status;

/// Creates a firmware volume handle from system memory
///
/// This service creates a firmware volume handle for a firmware volume
/// that is present in system memory, making it available to the DXE dispatcher.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.II-59 (This one does not have a section)
pub type ProcessFirmwareVolume = extern "efiapi" fn(*const c_void, usize, *mut Handle) -> Status;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// Global Coherency Domain (GCD) memory types
///
/// Defines the types of memory regions that can exist in the Global Coherency Domain.
/// Each memory region has a specific type that determines how it can be used by the system.
pub enum GcdMemoryType {
    /// Memory region with no active decoder
    ///
    /// A memory region that is visible to the boot processor but has no system
    /// components currently decoding this memory region.
    #[default]
    NonExistent = 0,
    /// Reserved memory region
    ///
    /// A memory region being decoded by a system component, but not considered
    /// to be either system memory or memory-mapped I/O.
    Reserved,
    /// System memory region
    ///
    /// A memory region decoded by a memory controller that produces tested
    /// system memory available to the memory services.
    SystemMemory,
    /// Memory-mapped I/O region
    ///
    /// A memory region currently being decoded as memory-mapped I/O that can
    /// be used to access I/O devices in the platform.
    MemoryMappedIo,
    /// Persistent memory region
    ///
    /// A memory region that supports byte-addressable non-volatility,
    /// such as non-volatile dual in-line memory modules (NVDIMM).
    Persistent,
    /// High-reliability memory region
    ///
    /// A memory region that provides higher reliability relative to other memory
    /// in the system. Used when memory has varying reliability characteristics.
    MoreReliable,
    /// Unaccepted memory region
    ///
    /// A memory region that is unaccepted and must be accepted before it can
    /// be converted to system memory. Used in confidential computing environments.
    Unaccepted,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
/// Global Coherency Domain (GCD) allocation strategies
///
/// Defines the allocation strategies that can be used when allocating memory
/// or I/O space from the Global Coherency Domain.
pub enum GcdAllocateType {
    #[default]
    /// Allocate any available address searching bottom-up
    ///
    /// Search for available space starting from the lowest addresses
    AnySearchBottomUp,
    /// Allocate below a maximum address searching bottom-up
    ///
    /// Search for available space below a specified maximum address, starting from the bottom
    MaxAddressSearchBottomUp,
    /// Allocate at a specific address
    ///
    /// Allocate at the exact address specified by the caller
    Address,
    /// Search for memory from top down
    AnySearchTopDown,
    /// Search for memory from specified max address top down
    MaxAddressSearchTopDown,
    /// Maximum allocate type value
    MaxAllocateType,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// `EFI_GCD_MEMORY_SPACE_DESCRIPTOR` in specification.
pub struct MemorySpaceDescriptor {
    /// The physical address of the first byte in the memory region.
    pub base_address: PhysicalAddress,
    /// The number of bytes in the memory region.
    pub length: u64,
    /// The bit mask of attributes that the memory region is capable of supporting.
    pub capabilities: u64,
    /// The bit mask of attributes that the memory region is currently using.
    pub attributes: u64,
    /// Type of the memory region.
    pub memory_type: GcdMemoryType,
    /// The image handle of the agent that allocated the memory resource described by PhysicalStart and NumberOfBytes.
    ///
    /// If this field is NULL, then the memory resource is not currently allocated.
    pub image_handle: Handle,
    /// The device handle for which the memory resource has been allocated.
    ///
    /// If ImageHandle is NULL, then the memory resource is not currently allocated.
    ///
    /// If this field is NULL, then the memory resource is not associated with a device that is described by a device handle.
    pub device_handle: Handle,
}

impl MemorySpaceDescriptor {
    /// Returns the overlapping range from a user provided range and the range described by this descriptor.
    /// If there is no overlap, returns an empty range (start == end).
    ///
    /// This is used when iterating over descriptors to determine how much of the descriptor falls within a given range.
    pub fn get_range_overlap_with_desc(&self, range: &Range<PhysicalAddress>) -> Range<PhysicalAddress> {
        let desc_end = self.base_address.saturating_add(self.length);

        if self.base_address >= range.end || desc_end <= range.start {
            return 0..0;
        }

        let overlap_start = max(self.base_address, range.start);
        let overlap_end = min(desc_end, range.end);

        overlap_start..overlap_end
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// Global Coherency Domain (GCD) I/O space types
///
/// Defines the types of I/O regions that can exist in the Global Coherency Domain.
/// Each I/O region has a specific type that determines how it can be accessed.
pub enum GcdIoType {
    /// I/O region with no active decoder
    ///
    /// An I/O region that is visible to the boot processor but has no system
    /// components currently decoding this I/O region.
    #[default]
    NonExistent = 0,
    /// Reserved I/O region
    ///
    /// An I/O region currently being decoded by a system component, but the I/O
    /// region cannot be used to access I/O devices.
    Reserved,
    /// Active I/O region
    ///
    /// An I/O region currently being decoded by a system component that produces
    /// I/O ports that can be used to access I/O devices.
    Io,
    /// Maximum value for GcdIoType enumeration
    Maximum,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// `EFI_GCD_IO_SPACE_DESCRIPTOR` in specification.
pub struct IoSpaceDescriptor {
    /// Physical address of the first byte in the I/O region.
    pub base_address: PhysicalAddress,
    /// Number of bytes in the I/O region.
    pub length: u64,
    /// Type of the I/O region.
    pub io_type: GcdIoType,
    /// The image handle of the agent that allocated the I/O resource described by PhysicalStart and NumberOfBytes.
    ///
    /// If this field is NULL, then the I/O resource is not currently allocated.
    pub image_handle: Handle,
    /// The device handle for which the I/O resource has been allocated.
    ///
    /// If ImageHandle is NULL , then the I/O resource is not currently allocated.
    ///
    /// If this field is NULL, then the I/O resource is not associated with a device that is described by a device handle.
    pub device_handle: Handle,
}

#[repr(C)]
/// Contains a table header and pointers to all of the DXE-specific services.
///
/// See <https://uefi.org/specs/PI/1.8A/V2_UEFI_System_Table.html#dxe-services-table>.
pub struct DxeServicesTable {
    /// Standard UEFI table header
    pub header: TableHeader,

    //
    // Global Coherency Domain (GCD)
    //
    /// Add memory space to GCD
    pub add_memory_space: AddMemorySpace,
    /// Allocate memory space from GCD
    pub allocate_memory_space: AllocateMemorySpace,
    /// Free memory space in GCD
    pub free_memory_space: FreeMemorySpace,
    /// Remove memory space from GCD
    pub remove_memory_space: RemoveMemorySpace,
    /// Get memory space descriptor
    pub get_memory_space_descriptor: GetMemorySpaceDescriptor,
    /// Set memory space attributes
    pub set_memory_space_attributes: SetMemorySpaceAttributes,
    /// Get memory space map
    pub get_memory_space_map: GetMemorySpaceMap,
    /// Add I/O space to GCD
    pub add_io_space: AddIoSpace,
    /// Allocate I/O space from GCD
    pub allocate_io_space: AllocateIoSpace,
    /// Free I/O space in GCD
    pub free_io_space: FreeIoSpace,
    /// Remove I/O space from GCD
    pub remove_io_space: RemoveIoSpace,
    /// Get I/O space descriptor
    pub get_io_space_descriptor: GetIoSpaceDescriptor,
    /// Get I/O space map
    pub get_io_space_map: GetIoSpaceMap,

    //
    // Dispatcher Services
    //
    /// Dispatch drivers
    pub dispatch: Dispatch,
    /// Schedule drivers for execution
    pub schedule: Schedule,
    /// Establish trust for drivers
    pub trust: Trust,

    //
    // Service to process a single firmware volume found in
    // a capsule
    //
    /// Process firmware volume from capsule
    pub process_firmware_volume: ProcessFirmwareVolume,

    //
    // Extension to Global Coherency Domain (GCD) Services
    //
    /// Set memory space capabilities
    pub set_memory_space_capabilities: SetMemorySpaceCapabilities,
}

impl Default for MemorySpaceDescriptor {
    fn default() -> Self {
        Self {
            base_address: Default::default(),
            length: Default::default(),
            capabilities: Default::default(),
            attributes: Default::default(),
            memory_type: Default::default(),
            image_handle: 0 as Handle,
            device_handle: 0 as Handle,
        }
    }
}

impl Default for IoSpaceDescriptor {
    fn default() -> Self {
        Self {
            base_address: Default::default(),
            length: Default::default(),
            io_type: Default::default(),
            image_handle: 0 as Handle,
            device_handle: 0 as Handle,
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_get_range_overlap_with_desc() {
        let desc = MemorySpaceDescriptor { base_address: 0x10000, length: 0x5000, ..Default::default() };

        // No overlap (before)
        let range = 0x0000..0x0FFF;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0..0);

        // No overlap (after)
        let range = 0x20000..0x2FFFF;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0..0);

        // Partial overlap (start)
        let range = 0x9000..0x13000;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0x10000..0x13000);

        // Partial overlap (end)
        let range = 0x11000..0x25000;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0x11000..0x15000);

        // Descriptor fully within range
        let range = 0x10000..0x20000;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0x10000..0x15000);

        // Range fully within descriptor
        let range = 0x11000..0x12000;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0x11000..0x12000);

        // range == descriptor
        let range = 0x10000..0x15000;
        let overlap = desc.get_range_overlap_with_desc(&range);
        assert_eq!(overlap, 0x10000..0x15000);
    }
}
