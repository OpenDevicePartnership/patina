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

use core::{default::Default, ffi::c_void};

use r_efi::{
    efi::{Guid, Handle, PhysicalAddress, Status},
    system::TableHeader,
};

pub const DXE_SERVICES_TABLE_GUID: Guid =
    Guid::from_fields(0x5ad34ba, 0x6f02, 0x4214, 0x95, 0x2e, &[0x4d, 0xa0, 0x39, 0x8e, 0x2b, 0xb9]);

/// This service adds reserved memory system memory or memory mapped IO resources to the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.1
pub type AddMemorySpace = extern "efiapi" fn(GcdMemoryType, PhysicalAddress, u64, u64) -> Status;

/// This service allocates nonexistent memory reserved memory system memory or memory-mapped IO resources
/// from the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.2
pub type AllocateMemorySpace =
    extern "efiapi" fn(GcdAllocateType, GcdMemoryType, usize, u64, *mut PhysicalAddress, Handle, Handle) -> Status;

/// This service frees nonexistent memory, reserved memory, system memory,
/// or memory-mapped I/O resources from the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.3
pub type FreeMemorySpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// This service removes reserved memory, system memory,
/// or memory-mapped I/O resources from the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.4
pub type RemoveMemorySpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// This service retrieves the descriptor for a memory region containing a specified address.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.5
pub type GetMemorySpaceDescriptor = extern "efiapi" fn(PhysicalAddress, *mut MemorySpaceDescriptor) -> Status;

/// This service modifies the attributes for a memory region in the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.6
pub type SetMemorySpaceAttributes = extern "efiapi" fn(PhysicalAddress, u64, u64) -> Status;

/// This service modifies the capabilities for a memory region in the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.7
pub type SetMemorySpaceCapabilities = extern "efiapi" fn(PhysicalAddress, u64, u64) -> Status;

/// Returns a map of the memory resources in the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.8
pub type GetMemorySpaceMap = extern "efiapi" fn(*mut usize, *mut *mut MemorySpaceDescriptor) -> Status;

/// This service adds reserved I/O, or I/O resources to the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.9
pub type AddIoSpace = extern "efiapi" fn(GcdIoType, PhysicalAddress, u64) -> Status;

/// This service allocates nonexistent I/O, reserved I/O, or I/O resources from the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.10
pub type AllocateIoSpace =
    extern "efiapi" fn(GcdAllocateType, GcdIoType, usize, u64, *mut PhysicalAddress, Handle, Handle) -> Status;

/// This service frees nonexistent I/O, reserved I/O, or I/O resources from the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.11
pub type FreeIoSpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// This service removes reserved I/O, or I/O resources from the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.12
pub type RemoveIoSpace = extern "efiapi" fn(PhysicalAddress, u64) -> Status;

/// This service retrieves the descriptor for an I/O region containing a specified address.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.13
pub type GetIoSpaceDescriptor = extern "efiapi" fn(PhysicalAddress, *mut IoSpaceDescriptor) -> Status;

/// Returns a map of the I/O resources in the global coherency domain of the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.14
pub type GetIoSpaceMap = extern "efiapi" fn(*mut usize, *mut *mut IoSpaceDescriptor) -> Status;

/// Loads and executes DXE drivers from firmware volumes.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.1
pub type Dispatch = extern "efiapi" fn() -> Status;

/// Clears the Schedule on Request (SOR) flag for a component that is stored in a firmware volume.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.2
pub type Schedule = extern "efiapi" fn(Handle, *const Guid) -> Status;

/// Promotes a file stored in a firmware volume from the untrusted to the trusted state.
/// Only the Security Architectural Protocol can place a file in the untrusted state.
/// A platform specific component may choose to use this service to promote a previously untrusted file to the trusted state.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3.3
pub type Trust = extern "efiapi" fn(Handle, *const Guid) -> Status;

/// Creates a firmware volume handle for a firmware volume that is present in system memory.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-7.3. II-59 (This one does not have a section)
pub type ProcessFirmwareVolume = extern "efiapi" fn(*const c_void, usize, *mut Handle) -> Status;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// `EFI_GCD_MEMORY_TYPE` in specification.
pub enum GcdMemoryType {
    /// A memory region that is visible to the boot processor.
    /// However, there are no system components that are currently decoding this memory region.
    #[default]
    NonExistent = 0,
    /// A memory region that is visible to the boot processor.
    /// This memory region is being decoded by a system component,
    /// but the memory region is not considered to be either system memory or memory-mapped I/O.
    Reserved,
    /// A memory region that is visible to the boot processor.
    /// A memory controller is currently decoding this memory region
    /// and the memory controller is producing a tested system memory region that is available to the memory services.
    SystemMemory,
    /// A memory region that is visible to the boot processor. This memory region is currently being decoded by a
    /// component as memory-mapped I/O that can be used to access I/O devices in the platform.
    MemoryMappedIo,
    /// A memory region that is visible to the boot processor. This memory supports byte-addressable non-volatility.
    Persistent,
    /// A memory region that provides higher reliability relative to other memory in the system.
    /// If all memory has the same reliability, then this bit is not used.
    MoreReliable,
    /// A memory region that is unaccepted. This region must be accepted before it can be converted to system memory.
    Unaccepted,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
/// `EFI_GCD_ALLOCATE_TYPE` in specification.
pub enum GcdAllocateType {
    #[default]
    AnySearchBottomUp,
    MaxAddressSearchBottomUp,
    Address,
    AnySearchTopDown,
    MaxAddressSearchTopDown,
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

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// `EFI_GCD_IO_TYPE` in specification.
pub enum GcdIoType {
    /// An I/O region that is visible to the boot processor.
    /// However, there are no system components that are currently decoding this I/O region.
    #[default]
    NonExistent = 0,
    /// An I/O region that is visible to the boot processor.
    /// This I/O region is currently being decoded by a system component,
    ///  but the I/O region cannot be used to access I/O devices.
    Reserved,
    /// An I/O region that is visible to the boot processor.
    /// This I/O region is currently being decoded by a system component
    /// that is producing I/O ports that can be used to access I/O devices.
    Io,
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
    pub header: TableHeader,

    //
    // Global Coherency
    //
    pub add_memory_space: AddMemorySpace,
    pub allocate_memory_space: AllocateMemorySpace,
    pub free_memory_space: FreeMemorySpace,
    pub remove_memory_space: RemoveMemorySpace,
    pub get_memory_space_descriptor: GetMemorySpaceDescriptor,
    pub set_memory_space_attributes: SetMemorySpaceAttributes,
    pub get_memory_space_map: GetMemorySpaceMap,
    pub add_io_space: AddIoSpace,
    pub allocate_io_space: AllocateIoSpace,
    pub free_io_space: FreeIoSpace,
    pub remove_io_space: RemoveIoSpace,
    pub get_io_space_descriptor: GetIoSpaceDescriptor,
    pub get_io_space_map: GetIoSpaceMap,

    //
    // Dispatcher Services
    //
    pub dispatch: Dispatch,
    pub schedule: Schedule,
    pub trust: Trust,

    //
    // Service to process a single firmware volume found in
    // a capsule
    //
    pub process_firmware_volume: ProcessFirmwareVolume,

    //
    // Extension to Global Coherency Domain Services
    //
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
