//! Hand-Off Blocks (HOB)
//!
//! Hand-Off Blocks provide a standardized method for passing information from
//! the pre-DXE phase to the DXE phase during the platform initialization process.
//!
//! HOBs describe the physical memory layout, CPU information, firmware volumes,
//! and other platform-specific data that DXE needs to continue the boot process.
//!
//! The HOB list is a contiguous list of HOB structures, each with a common header
//! followed by type-specific data. Typically, the PEI Foundation creates and manages
//! the HOB list during the PEI phase, and it is passed to the DXE Foundation
//! during the PEI-to-DXE handoff.
//!
//! Based on the UEFI Platform Initialization Specification Volume III.
//!
//! ## Example
//! ```
//! use patina::pi::{BootMode, hob, hob::Hob, hob::HobList};
//! use core::mem::size_of;
//!
//! // Generate HOBs to initialize a new HOB list
//! fn gen_capsule() -> hob::Capsule {
//!   let header = hob::header::Hob { r#type: hob::UEFI_CAPSULE, length: size_of::<hob::Capsule>() as u16, reserved: 0 };
//!
//!   hob::Capsule { header, base_address: 0, length: 0x12 }
//! }
//!
//! fn gen_firmware_volume2() -> hob::FirmwareVolume2 {
//!   let header = hob::header::Hob { r#type: hob::FV2, length: size_of::<hob::FirmwareVolume2>() as u16, reserved: 0 };
//!
//!   hob::FirmwareVolume2 {
//!     header,
//!     base_address: 0,
//!     length: 0x0123456789abcdef,
//!     fv_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
//!     file_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
//!   }
//! }
//!
//! fn gen_end_of_hoblist() -> hob::PhaseHandoffInformationTable {
//!   let header = hob::header::Hob {
//!     r#type: hob::END_OF_HOB_LIST,
//!     length: size_of::<hob::PhaseHandoffInformationTable>() as u16,
//!     reserved: 0,
//!   };
//!
//!   hob::PhaseHandoffInformationTable {
//!     header,
//!     version: 0x00010000,
//!     boot_mode: BootMode::BootWithFullConfiguration,
//!     memory_top: 0xdeadbeef,
//!     memory_bottom: 0xdeadc0de,
//!     free_memory_top: 104,
//!     free_memory_bottom: 255,
//!     end_of_hob_list: 0xdeaddeadc0dec0de,
//!   }
//! }
//!
//! // Generate some example HOBs
//! let capsule = gen_capsule();
//! let firmware_volume2 = gen_firmware_volume2();
//! let end_of_hob_list = gen_end_of_hoblist();
//!
//! // Create a new empty HOB list
//! let mut hoblist = HobList::new();
//!
//! // Push the example HOBs onto the HOB list
//! hoblist.push(Hob::Capsule(&capsule));
//! hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
//! hoblist.push(Hob::Handoff(&end_of_hob_list));
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{
    base::{align_down, align_up},
    pi::BootMode,
};
use core::{
    ffi::c_void,
    fmt,
    marker::PhantomData,
    mem::{self, size_of},
    slice,
};
use indoc::indoc;

// Expectation is someone will provide alloc
extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

// If the target is x86_64, then EfiPhysicalAddress is u64
#[cfg(target_arch = "x86_64")]
/// Type for a UEFI physical address.
pub type EfiPhysicalAddress = u64;

// If the target is aarch64, then EfiPhysicalAddress is u64
#[cfg(target_arch = "aarch64")]
/// Type for a UEFI physical address.
pub type EfiPhysicalAddress = u64;

// if the target is x86, then EfiPhysicalAddress is u32
#[cfg(target_arch = "x86")]
/// Type for a UEFI physical address.
pub type EfiPhysicalAddress = u32;

// if the target is not x86, x86_64, or aarch64, then alert the user
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("This crate only (currently) supports x86, x86_64, and aarch64 architectures");

// HOB type field is a UINT16
/// Contains general state information used by the HOB producer phase. This HOB must be the first one in the HOB list.
pub const HANDOFF: u16 = 0x0001;
/// Describes all memory ranges used during the HOB producer phase that exist outside the HOB list. This HOB type
/// describes how memory is used, not the physical attributes of memory.
pub const MEMORY_ALLOCATION: u16 = 0x0002;
/// Describes the resource properties of all fixed, nonrelocatable resource ranges found on the processor host bus
/// during the HOB producer phase.
pub const RESOURCE_DESCRIPTOR: u16 = 0x0003;
/// Allows writers of executable content in the HOB producer phase to maintain and manage HOBs whose types are not
/// included in this specification. Specifically, writers of executable content in the HOB producer phase can generate
/// a GUID and name their own HOB entries using this module-specific value.
pub const GUID_EXTENSION: u16 = 0x0004;
/// Details the location of firmware volumes that contain firmware files.
pub const FV: u16 = 0x0005;
/// Describes processor information, such as address space and I/O space capabilities.
pub const CPU: u16 = 0x0006;
/// Describes pool memory allocations.
pub const MEMORY_POOL: u16 = 0x0007;
/// Details the location of a firmware volume which was extracted from a file within another firmware volume.
pub const FV2: u16 = 0x0009;
/// HOB type for load PEIM (unused).
pub const LOAD_PEIM_UNUSED: u16 = 0x000A;
/// Details the location of coalesced UEFI capsule memory pages.
pub const UEFI_CAPSULE: u16 = 0x000B;
/// Details the location of a firmware volume including authentication information, for both standalone and extracted
/// firmware volumes.
pub const FV3: u16 = 0x000C;
/// HOB type for resource descriptor v2 HOB.
pub const RESOURCE_DESCRIPTOR2: u16 = 0x000D;
/// Indicates that the contents of the HOB can be ignored.
pub const UNUSED: u16 = 0xFFFE;
/// Indicates the end of the HOB list. This HOB must be the last one in the HOB list.
pub const END_OF_HOB_LIST: u16 = 0xFFFF;

/// HOB header structures and definitions.
pub mod header {
    use crate::pi::hob::EfiPhysicalAddress;
    use r_efi::system::MemoryType;

    /// Describes the format and size of the data inside the HOB. All HOBs must contain
    /// this generic HOB header.
    ///
    /// This header provides the foundation for traversing the HOB list by containing
    /// the type identifier and length information. The HOB list is composed of consecutive
    /// HOB structures that allows iteration from one HOB to the next until the end-of-list
    /// marker is encountered.
    ///
    #[repr(C)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct Hob {
        // EFI_HOB_GENERIC_HEADER
        /// Identifies the HOB data structure type.
        /// This field specifies which HOB structure follows this header,
        /// such as memory allocation, resource descriptor, or firmware volume.
        pub r#type: u16,

        /// The length in bytes of the HOB.
        /// This includes the HOB header and all associated data. Used for
        /// traversing to the next HOB in the HOB list.
        pub length: u16,

        /// This field must always be set to zero.
        ///
        pub reserved: u32,
    }

    /// Memory allocation HOB header that describes allocated memory regions.
    ///
    /// This header describes memory that has been allocated during the HOB producer phase
    /// and provides information needed for the HOB consumer phase to incorporate these
    /// allocations into the system memory map. The Name field identifies the purpose
    /// and allows for specific handling by components that understand the allocation type.
    ///
    #[repr(C)]
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct MemoryAllocation {
        // EFI_HOB_MEMORY_ALLOCATION_HEADER
        /// A GUID that defines the memory allocation region's type and purpose.
        /// This GUID identifies the specific type of memory allocation and may
        /// indicate additional data structures that follow this header. Well-known
        /// GUIDs include allocations for stack, BSP store, and module images.
        ///
        pub name: r_efi::base::Guid,

        /// The base address of memory allocated by this HOB.
        /// This is the physical address where the memory allocation begins,
        /// and it will be included in the memory map during DXE phase
        /// memory map construction.
        ///
        pub memory_base_address: EfiPhysicalAddress,

        /// The length in bytes of memory allocated by this HOB.
        /// This specifies the size of the memory region from the base address
        /// that has been allocated and should be reflected in the memory map.
        pub memory_length: u64,

        /// Defines the type of memory allocated by this HOB.
        /// The memory type follows EFI memory type definitions and determines
        /// how this memory region will be treated in the memory map,
        /// such as whether it's available for allocation or reserved.
        ///
        pub memory_type: MemoryType,

        /// This field will always be set to zero.
        ///
        pub reserved: [u8; 4],
    }
}

/// Describes pool memory allocations.
///
/// The memory pool HOB is produced by the HOB producer phase and describes pool
/// memory allocations. The HOB consumer phase should be able to ignore these HOBs.
/// The purpose of this HOB is to allow for the HOB producer phase to have a simple
/// memory allocation mechanism within the HOB list. The size of the memory allocation
/// is stipulated by the HobLength field in the generic HOB header.
///
pub type MemoryPool = header::Hob;

/// Phase Handoff Information Table (PHIT) HOB.
///
/// Contains general state information used by the HOB producer phase. This HOB must
/// be the first one in the HOB list. The PHIT HOB provides essential information
/// for the transition from the HOB producer phase to the HOB consumer phase, including
/// boot mode, memory ranges, and the location of the HOB list end.
///
/// The HOB consumer phase reads the PHIT HOB during its initialization to understand
/// the system state and available resources from the HOB producer phase.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PhaseHandoffInformationTable {
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_HANDOFF.
    ///
    pub header: header::Hob, // EFI_HOB_GENERIC_HEADER

    /// The version number pertaining to the PHIT HOB definition.
    /// This value is four bytes in length to provide an 8-byte aligned entry
    /// when it is combined with the 4-byte BootMode.
    ///
    pub version: u32,

    /// The system boot mode as determined during the HOB producer phase.
    /// This indicates the type of boot being performed (normal boot, S3 resume,
    /// recovery boot, etc.) and affects how the HOB consumer phase initializes the system.
    pub boot_mode: BootMode,

    /// The highest address location of memory allocated for use by the HOB producer phase.
    /// This address must be 4-KB aligned to meet page restrictions and represents
    /// the upper boundary of memory available to the HOB producer phase.
    ///
    pub memory_top: EfiPhysicalAddress,

    /// The lowest address location of memory allocated for use by the HOB producer phase.
    /// This represents the lower boundary of memory available to the HOB producer phase.
    pub memory_bottom: EfiPhysicalAddress,

    /// The highest address location of free memory currently available for allocation.
    /// This address must be 4-KB aligned to meet page restrictions and represents
    /// the upper boundary of memory that can be allocated by the HOB producer phase.
    ///
    pub free_memory_top: EfiPhysicalAddress,

    /// The lowest address location of free memory available for use by the HOB producer phase.
    ///
    pub free_memory_bottom: EfiPhysicalAddress,

    /// The end of the HOB list.
    ///
    pub end_of_hob_list: EfiPhysicalAddress,
}

/// Describes all memory ranges used during the HOB producer
/// phase that exist outside the HOB list. This HOB type
/// describes how memory is used, not the physical attributes of memory.
///
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct MemoryAllocation {
    // EFI_HOB_MEMORY_ALLOCATION
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_MEMORY_ALLOCATION.
    ///
    pub header: header::Hob,

    /// An instance of the EFI_HOB_MEMORY_ALLOCATION_HEADER that describes the
    /// various attributes of the logical memory allocation.
    ///
    pub alloc_descriptor: header::MemoryAllocation,
    // Additional data pertaining to the "Name" Guid memory
    // may go here.
    //
}

// EFI_HOB_MEMORY_ALLOCATION_STACK
/// Describes the memory stack that is produced by the HOB producer phase and upon
/// which all post-memory-installed executable content in the HOB producer phase is executing.
///
/// This HOB describes the memory stack used by the HOB producer phase and is necessary
/// for the hand-off into the HOB consumer phase to know this information so that it can
/// appropriately map this stack into its own execution environment and describe it in
/// any subsequent memory maps. The HOB consumer phase may elect to move or relocate
/// the BSP's stack to meet its own requirements.
///
pub type MemoryAllocationStack = MemoryAllocation;

// EFI_HOB_MEMORY_ALLOCATION_BSP_STORE
/// Defines the location of the boot-strap processor (BSP) BSPStore register overflow store.
///
/// This HOB is valid for the Itanium processor family only and describes the location
/// of the BSP's backing store pointer store register overflow area. This information
/// is needed during the transition to the HOB consumer phase for proper processor
/// state management.
///
pub type MemoryAllocationBspStore = MemoryAllocation;

/// Defines the location and entry point of the HOB consumer phase.
///
/// The HOB consumer phase reads the memory allocation module HOB during its
/// initialization. This HOB describes the memory location of the HOB consumer phase
/// and should be used by the HOB consumer phase to create the image handle for itself.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MemoryAllocationModule {
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_MEMORY_ALLOCATION.
    ///
    pub header: header::Hob,

    /// An instance of the EFI_HOB_MEMORY_ALLOCATION_HEADER that describes the
    /// various attributes of the logical memory allocation. The type field will be
    /// used for subsequent inclusion in the memory map.
    ///
    pub alloc_descriptor: header::MemoryAllocation,

    /// The GUID specifying the values of the firmware file system name
    /// that contains the HOB consumer phase component.
    ///
    pub module_name: r_efi::base::Guid, // EFI_GUID

    /// The address of the memory-mapped firmware volume
    /// that contains the HOB consumer phase firmware file.
    ///
    pub entry_point: u64, // EFI_PHYSICAL_ADDRESS
}

//
// Value of ResourceType in EFI_HOB_RESOURCE_DESCRIPTOR.
//
/// System memory that persists out of the HOB producer phase.
pub const EFI_RESOURCE_SYSTEM_MEMORY: u32 = 0x00000000;
/// Memory-mapped I/O that is programmed in the HOB producer phase.
pub const EFI_RESOURCE_MEMORY_MAPPED_IO: u32 = 0x00000001;
/// Processor I/O space.
pub const EFI_RESOURCE_IO: u32 = 0x00000002;
/// Memory-mapped firmware devices.
pub const EFI_RESOURCE_FIRMWARE_DEVICE: u32 = 0x00000003;
/// Memory that is decoded to produce I/O cycles.
pub const EFI_RESOURCE_MEMORY_MAPPED_IO_PORT: u32 = 0x00000004;
/// Reserved memory address space.
pub const EFI_RESOURCE_MEMORY_RESERVED: u32 = 0x00000005;
/// Reserved I/O address space.
pub const EFI_RESOURCE_IO_RESERVED: u32 = 0x00000006;

//
// BZ3937_EFI_RESOURCE_MEMORY_UNACCEPTED is defined for unaccepted memory.
// But this definition has not been officially in the PI spec. Base
// on the code-first we define BZ3937_EFI_RESOURCE_MEMORY_UNACCEPTED at
// MdeModulePkg/Include/Pi/PrePiHob.h and update EFI_RESOURCE_MAX_MEMORY_TYPE
// to 8. After BZ3937_EFI_RESOURCE_MEMORY_UNACCEPTED is officially published
// in PI spec, we will re-visit here.
//
// #define BZ3937_EFI_RESOURCE_MEMORY_UNACCEPTED      0x00000007
/// Maximum memory type value.
pub const EFI_RESOURCE_MAX_MEMORY_TYPE: u32 = 0x00000007;

//
// These types can be ORed together as needed.
//
// The following attributes are used to describe settings
//
/// Physical memory attribute: The memory region exists.
pub const EFI_RESOURCE_ATTRIBUTE_PRESENT: u32 = 0x00000001;
/// Physical memory attribute: The memory region has been initialized.
pub const EFI_RESOURCE_ATTRIBUTE_INITIALIZED: u32 = 0x00000002;
/// Physical memory attribute: The memory region has been tested.
pub const EFI_RESOURCE_ATTRIBUTE_TESTED: u32 = 0x00000004;
/// Physical memory protection attribute: The memory region is read protected.
pub const EFI_RESOURCE_ATTRIBUTE_READ_PROTECTED: u32 = 0x00000080;

//
// This is typically used as memory cacheability attribute today.
// NOTE: Since PI spec 1.4, please use EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTED
// as Physical write protected attribute, and EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTED
// means Memory cacheability attribute: The memory supports being programmed with
// a writeprotected cacheable attribute.
//
/// Memory cacheability attribute: The memory supports being programmed with a write-protected cacheable attribute.
pub const EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTED: u32 = 0x00000100;
/// Physical memory protection attribute: The memory region is execution protected.
pub const EFI_RESOURCE_ATTRIBUTE_EXECUTION_PROTECTED: u32 = 0x00000200;
/// Physical memory persistence attribute: This memory is configured for byte-addressable non-volatility.
pub const EFI_RESOURCE_ATTRIBUTE_PERSISTENT: u32 = 0x00800000;

//
// Physical memory relative reliability attribute. This
// memory provides higher reliability relative to other
// memory in the system. If all memory has the same
// reliability, then this bit is not used.
//
/// Physical memory relative reliability attribute: This memory provides higher reliability relative to other memory in the system.
pub const EFI_RESOURCE_ATTRIBUTE_MORE_RELIABLE: u32 = 0x02000000;

//
// The rest of the attributes are used to describe capabilities
//
/// Physical memory attribute: The memory region supports single-bit ECC.
pub const EFI_RESOURCE_ATTRIBUTE_SINGLE_BIT_ECC: u32 = 0x00000008;
/// Physical memory attribute: The memory region supports multibit ECC.
pub const EFI_RESOURCE_ATTRIBUTE_MULTIPLE_BIT_ECC: u32 = 0x00000010;
/// Physical memory attribute: The memory region supports reserved ECC.
pub const EFI_RESOURCE_ATTRIBUTE_ECC_RESERVED_1: u32 = 0x00000020;
/// Physical memory attribute: The memory region supports reserved ECC.
pub const EFI_RESOURCE_ATTRIBUTE_ECC_RESERVED_2: u32 = 0x00000040;
/// Memory cacheability attribute: The memory does not support caching.
pub const EFI_RESOURCE_ATTRIBUTE_UNCACHEABLE: u32 = 0x00000400;
/// Memory cacheability attribute: The memory supports a write-combining attribute.
pub const EFI_RESOURCE_ATTRIBUTE_WRITE_COMBINEABLE: u32 = 0x00000800;
/// Memory cacheability attribute: The memory supports being programmed with a write-through cacheable attribute.
pub const EFI_RESOURCE_ATTRIBUTE_WRITE_THROUGH_CACHEABLE: u32 = 0x00001000;
/// Memory cacheability attribute: The memory region supports being configured as cacheable with a write-back policy.
pub const EFI_RESOURCE_ATTRIBUTE_WRITE_BACK_CACHEABLE: u32 = 0x00002000;
/// Memory physical attribute: The memory supports 16-bit I/O.
pub const EFI_RESOURCE_ATTRIBUTE_16_BIT_IO: u32 = 0x00004000;
/// Memory physical attribute: The memory supports 32-bit I/O.
pub const EFI_RESOURCE_ATTRIBUTE_32_BIT_IO: u32 = 0x00008000;
/// Memory physical attribute: The memory supports 64-bit I/O.
pub const EFI_RESOURCE_ATTRIBUTE_64_BIT_IO: u32 = 0x00010000;
/// Memory cacheability attribute: The memory region is uncacheable and exported and supports the fetch and add semaphore mechanism.
pub const EFI_RESOURCE_ATTRIBUTE_UNCACHED_EXPORTED: u32 = 0x00020000;
/// Memory capability attribute: The memory supports being protected from processor reads.
pub const EFI_RESOURCE_ATTRIBUTE_READ_PROTECTABLE: u32 = 0x00100000;

//
// This is typically used as memory cacheability attribute today.
// NOTE: Since PI spec 1.4, please use EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTABLE
// as Memory capability attribute: The memory supports being protected from processor
// writes, and EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTABLE TABLE means Memory cacheability attribute:
// The memory supports being programmed with a writeprotected cacheable attribute.
//
/// Memory cacheability attribute: The memory supports being programmed with a write-protected cacheable attribute.
pub const EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTABLE: u32 = 0x00200000;
/// Memory capability attribute: The memory supports being protected from processor execution.
pub const EFI_RESOURCE_ATTRIBUTE_EXECUTION_PROTECTABLE: u32 = 0x00400000;
/// Memory capability attribute: This memory supports byte-addressable non-volatility.
pub const EFI_RESOURCE_ATTRIBUTE_PERSISTABLE: u32 = 0x01000000;

/// Physical memory protection attribute: The memory region is write protected.
pub const EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTED: u32 = 0x00040000;
/// Memory capability attribute: The memory supports being protected from processor writes.
pub const EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTABLE: u32 = 0x00080000;

/// Mask for memory attributes.
pub const MEMORY_ATTRIBUTE_MASK: u32 = EFI_RESOURCE_ATTRIBUTE_PRESENT
    | EFI_RESOURCE_ATTRIBUTE_INITIALIZED
    | EFI_RESOURCE_ATTRIBUTE_TESTED
    | EFI_RESOURCE_ATTRIBUTE_READ_PROTECTED
    | EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTED
    | EFI_RESOURCE_ATTRIBUTE_EXECUTION_PROTECTED
    | EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTED
    | EFI_RESOURCE_ATTRIBUTE_16_BIT_IO
    | EFI_RESOURCE_ATTRIBUTE_32_BIT_IO
    | EFI_RESOURCE_ATTRIBUTE_64_BIT_IO
    | EFI_RESOURCE_ATTRIBUTE_PERSISTENT;

/// Tested memory attributes mask.
pub const TESTED_MEMORY_ATTRIBUTES: u32 =
    EFI_RESOURCE_ATTRIBUTE_PRESENT | EFI_RESOURCE_ATTRIBUTE_INITIALIZED | EFI_RESOURCE_ATTRIBUTE_TESTED;

/// Initialized memory attributes mask.
pub const INITIALIZED_MEMORY_ATTRIBUTES: u32 = EFI_RESOURCE_ATTRIBUTE_PRESENT | EFI_RESOURCE_ATTRIBUTE_INITIALIZED;

/// Present memory attributes mask.
pub const PRESENT_MEMORY_ATTRIBUTES: u32 = EFI_RESOURCE_ATTRIBUTE_PRESENT;

/// Attributes for reserved memory before it is promoted to system memory
pub const EFI_MEMORY_PRESENT: u64 = 0x0100_0000_0000_0000;
/// Memory initialized attribute flag.
pub const EFI_MEMORY_INITIALIZED: u64 = 0x0200_0000_0000_0000;
/// Memory tested attribute flag.
pub const EFI_MEMORY_TESTED: u64 = 0x0400_0000_0000_0000;

///
/// Physical memory persistence attribute.
/// The memory region supports byte-addressable non-volatility.
///
pub const EFI_MEMORY_NV: u64 = 0x0000_0000_0000_8000;
///
/// The memory region provides higher reliability relative to other memory in the system.
/// If all memory has the same reliability, then this bit is not used.
///
pub const EFI_MEMORY_MORE_RELIABLE: u64 = 0x0000_0000_0001_0000;

/// Describes the resource properties of all fixed, nonrelocatable resource ranges
/// found on the processor host bus during the HOB producer phase.
///
/// The resource descriptor HOB describes the resource properties of all fixed,
/// nonrelocatable resource ranges found on the processor host bus during the HOB
/// producer phase. This HOB type does not describe how memory is used but instead
/// describes the attributes of the physical memory present. The HOB consumer phase
/// reads all resource descriptor HOBs when it establishes the initial Global
/// Coherency Domain (GCD) map.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ResourceDescriptor {
    // EFI_HOB_RESOURCE_DESCRIPTOR
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_RESOURCE_DESCRIPTOR.
    ///
    pub header: header::Hob,

    /// A GUID representing the owner of the resource.
    /// This GUID is used by HOB consumer phase components to correlate device
    /// ownership of a resource.
    ///
    pub owner: r_efi::base::Guid,

    /// Resource type enumeration as defined by EFI_RESOURCE_TYPE.
    /// Identifies whether this resource is system memory, memory-mapped I/O,
    /// I/O ports, firmware device, or other platform-specific resource types.
    pub resource_type: u32,

    /// Resource attributes as defined by EFI_RESOURCE_ATTRIBUTE_TYPE.
    /// Includes information about cacheability, protection attributes,
    /// persistence, reliability, and other characteristics of the resource.
    pub resource_attribute: u32,

    /// Physical start address of the resource region.
    ///
    pub physical_start: EfiPhysicalAddress,

    /// Number of bytes of the resource region.
    ///
    pub resource_length: u64,
}

impl ResourceDescriptor {
    /// Validates resource descriptor attributes.
    pub fn attributes_valid(&self) -> bool {
        (self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_READ_PROTECTED == 0
            || self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTABLE != 0)
            && (self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTED == 0
                || self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_WRITE_PROTECTABLE != 0)
            && (self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_EXECUTION_PROTECTED == 0
                || self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_EXECUTION_PROTECTABLE != 0)
            && (self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTED == 0
                || self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_READ_ONLY_PROTECTABLE != 0)
            && (self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_PERSISTENT == 0
                || self.resource_attribute & EFI_RESOURCE_ATTRIBUTE_PERSISTABLE != 0)
    }
}

/// PI Spec Status: Pending.
/// This change is checked in as a code first approach. The PI spec will be updated
/// to reflect this change in the future.
///
/// Describes the resource properties of all fixed,
/// nonrelocatable resource ranges found on the processor
/// host bus during the HOB producer phase.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ResourceDescriptorV2 {
    // EFI_HOB_RESOURCE_DESCRIPTOR
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_RESOURCE_DESCRIPTOR.
    ///
    pub v1: ResourceDescriptor,

    /// The attributes of the resource described by this HOB.
    ///
    pub attributes: u64,
}

impl From<ResourceDescriptor> for ResourceDescriptorV2 {
    fn from(mut v1: ResourceDescriptor) -> Self {
        v1.header.r#type = RESOURCE_DESCRIPTOR2;
        ResourceDescriptorV2 { v1, attributes: 0 }
    }
}

/// Allows writers of executable content in the HOB producer phase to maintain and
/// manage HOBs whose types are not included in this specification.
///
/// The GUID extension HOB allows code in the HOB producer phase to create custom HOB
/// definitions using a GUID.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct GuidHob {
    // EFI_HOB_GUID_TYPE
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_GUID_EXTENSION.
    ///
    pub header: header::Hob,

    /// A GUID that defines the contents of this HOB.
    ///
    pub name: r_efi::base::Guid,
    // Guid specific data goes here
    //
}

/// Details the location of firmware volumes that contain firmware files.
///
/// The firmware volume HOB details the location of firmware volumes that contain
/// firmware files. It includes a base address and length. In particular, the HOB
/// consumer phase will use these HOBs to discover drivers to execute and the hand-off
/// into the HOB consumer phase will use this HOB to discover the location of the HOB
/// consumer phase firmware file. Firmware volumes described by the firmware volume
/// HOB must have a firmware volume header as described in this specification.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FirmwareVolume {
    // EFI_HOB_FIRMWARE_VOLUME
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_FV.
    ///
    pub header: header::Hob,

    /// The physical memory-mapped base address of the firmware volume.
    ///
    pub base_address: EfiPhysicalAddress,

    /// The length in bytes of the firmware volume.
    ///
    pub length: u64,
}

/// Details the location of a firmware volume which was extracted from a file within
/// another firmware volume.
///
/// The firmware volume HOB details the location of a firmware volume that was
/// extracted prior to the HOB consumer phase from a file within a firmware volume.
/// By recording the volume and file name, the HOB consumer phase can avoid processing
/// the same file again. This HOB is created by a module that has loaded a firmware
/// volume from another file into memory.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FirmwareVolume2 {
    // EFI_HOB_FIRMWARE_VOLUME2
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_FV2.
    ///
    pub header: header::Hob,

    /// The physical memory-mapped base address of the firmware volume.
    ///
    pub base_address: EfiPhysicalAddress,

    /// The length in bytes of the firmware volume.
    ///
    pub length: u64,

    /// The name of the firmware volume.
    ///
    pub fv_name: r_efi::base::Guid,

    /// The name of the firmware file which contained this firmware volume.
    ///
    pub file_name: r_efi::base::Guid,
}

/// Details the location of a firmware volume including authentication information,
/// for both standalone and extracted firmware volumes.
///
/// The firmware volume HOB details the location of firmware volumes that contain
/// firmware files. It includes a base address and length. In particular, the HOB
/// consumer phase will use these HOBs to discover drivers to execute and the hand-off
/// into the HOB consumer phase will use this HOB to discover the location of the HOB
/// consumer phase firmware file. The HOB consumer phase must provide appropriate
/// authentication data reflecting AuthenticationStatus for clients accessing the
/// corresponding firmware volumes.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FirmwareVolume3 {
    // EFI_HOB_FIRMWARE_VOLUME3
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_FV3.
    ///
    pub header: header::Hob,

    /// The physical memory-mapped base address of the firmware volume.
    ///
    pub base_address: EfiPhysicalAddress,

    /// The length in bytes of the firmware volume.
    ///
    pub length: u64,

    /// The authentication status. See Related Definitions of
    /// EFI_PEI_GUIDED_SECTION_EXTRACTION_PPI.ExtractSection() for more information.
    ///
    pub authentication_status: u32,

    /// TRUE if the FV was extracted as a file within another firmware volume.
    /// FALSE otherwise.
    ///
    pub extracted_fv: r_efi::efi::Boolean,

    /// The name GUID of the firmware volume.
    /// Valid only if IsExtractedFv is TRUE.
    ///
    pub fv_name: r_efi::base::Guid,

    /// The name GUID of the firmware file which contained this firmware volume.
    /// Valid only if IsExtractedFv is TRUE.
    ///
    pub file_name: r_efi::base::Guid,
}

/// Describes processor information, such as address space and I/O space capabilities.
///
/// The CPU HOB is produced by the processor executable content in the HOB producer
/// phase. It describes processor information, such as address space and I/O space
/// capabilities. The HOB consumer phase consumes this information to describe the
/// extent of the GCD capabilities.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    // EFI_HOB_CPU
    /// The HOB generic header. Header.HobType = EFI_HOB_TYPE_CPU.
    ///
    pub header: header::Hob,

    /// Identifies the maximum physical memory addressability of the processor.
    ///
    pub size_of_memory_space: u8,

    /// Identifies the maximum physical I/O addressability of the processor.
    ///
    pub size_of_io_space: u8,

    /// For this version of the specification, this field will always be set to zero.
    ///
    pub reserved: [u8; 6],
}

/// Details the location of coalesced UEFI capsule memory pages.
///
/// Each UEFI capsule HOB details the location of a UEFI capsule. It includes a base
/// address and length which is based upon memory blocks with a EFI_CAPSULE_HEADER and
/// the associated CapsuleImageSize-based payloads. These HOBs shall be created by the
/// PEI PI firmware sometime after the UEFI UpdateCapsule service invocation with the
/// CAPSULE_FLAGS_POPULATE_SYSTEM_TABLE flag set in the EFI_CAPSULE_HEADER.
///
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Capsule {
    // EFI_HOB_CAPSULE
    /// The HOB generic header where Header.HobType = EFI_HOB_TYPE_UEFI_CAPSULE.
    ///
    pub header: header::Hob,

    /// The physical memory-mapped base address of a UEFI capsule. This value is set to
    /// point to the base of the contiguous memory of the UEFI capsule.
    ///
    pub base_address: EfiPhysicalAddress,

    /// The length of the contiguous memory in bytes.
    pub length: u64,
}

/// Represents a HOB list.
///
pub struct HobList<'a>(Vec<Hob<'a>>);

impl Default for HobList<'_> {
    fn default() -> Self {
        HobList::new()
    }
}

/// Union of all the possible HOB Types.
///
#[derive(Clone, Debug)]
pub enum Hob<'a> {
    /// Phase handoff information table HOB.
    Handoff(&'a PhaseHandoffInformationTable),
    /// Memory allocation HOB.
    MemoryAllocation(&'a MemoryAllocation),
    /// Memory allocation module HOB.
    MemoryAllocationModule(&'a MemoryAllocationModule),
    /// Capsule HOB.
    Capsule(&'a Capsule),
    /// Resource descriptor HOB.
    ResourceDescriptor(&'a ResourceDescriptor),
    /// GUID extension HOB.
    GuidHob(&'a GuidHob, &'a [u8]),
    /// Firmware volume HOB.
    FirmwareVolume(&'a FirmwareVolume),
    /// Firmware volume v2 HOB.
    FirmwareVolume2(&'a FirmwareVolume2),
    /// Firmware volume v3 HOB.
    FirmwareVolume3(&'a FirmwareVolume3),
    /// CPU information HOB.
    Cpu(&'a Cpu),
    /// Resource descriptor v2 HOB.
    ResourceDescriptorV2(&'a ResourceDescriptorV2),
    /// Miscellaneous HOB type.
    Misc(u16),
}

/// Trait for Hand-Off Block types.
pub trait HobTrait {
    /// Returns the size of the HOB.
    fn size(&self) -> usize;
    /// Returns a pointer to the HOB data.
    fn as_ptr<T>(&self) -> *const T;
}

// HOB Trait implementation.
impl HobTrait for Hob<'_> {
    /// Returns the size of the HOB.
    fn size(&self) -> usize {
        match self {
            Hob::Handoff(_) => size_of::<PhaseHandoffInformationTable>(),
            Hob::MemoryAllocation(_) => size_of::<MemoryAllocation>(),
            Hob::MemoryAllocationModule(_) => size_of::<MemoryAllocationModule>(),
            Hob::Capsule(_) => size_of::<Capsule>(),
            Hob::ResourceDescriptor(_) => size_of::<ResourceDescriptor>(),
            Hob::GuidHob(hob, _) => hob.header.length as usize,
            Hob::FirmwareVolume(_) => size_of::<FirmwareVolume>(),
            Hob::FirmwareVolume2(_) => size_of::<FirmwareVolume2>(),
            Hob::FirmwareVolume3(_) => size_of::<FirmwareVolume3>(),
            Hob::Cpu(_) => size_of::<Cpu>(),
            Hob::ResourceDescriptorV2(_) => size_of::<ResourceDescriptorV2>(),
            Hob::Misc(_) => size_of::<u16>(),
        }
    }

    /// Returns a pointer to the HOB.
    fn as_ptr<T>(&self) -> *const T {
        match self {
            Hob::Handoff(hob) => *hob as *const PhaseHandoffInformationTable as *const _,
            Hob::MemoryAllocation(hob) => *hob as *const MemoryAllocation as *const _,
            Hob::MemoryAllocationModule(hob) => *hob as *const MemoryAllocationModule as *const _,
            Hob::Capsule(hob) => *hob as *const Capsule as *const _,
            Hob::ResourceDescriptor(hob) => *hob as *const ResourceDescriptor as *const _,
            Hob::GuidHob(hob, _) => *hob as *const GuidHob as *const _,
            Hob::FirmwareVolume(hob) => *hob as *const FirmwareVolume as *const _,
            Hob::FirmwareVolume2(hob) => *hob as *const FirmwareVolume2 as *const _,
            Hob::FirmwareVolume3(hob) => *hob as *const FirmwareVolume3 as *const _,
            Hob::Cpu(hob) => *hob as *const Cpu as *const _,
            Hob::ResourceDescriptorV2(hob) => *hob as *const ResourceDescriptorV2 as *const _,
            Hob::Misc(hob) => *hob as *const u16 as *const _,
        }
    }
}

/// Calculates the total size of a HOB list in bytes.
///
/// This function iterates through the HOB list starting from the given pointer,
/// summing up the lengths of each HOB until it reaches the end of the list.
///
/// # Arguments
///
/// * `hob_list` - A pointer to the start of the HOB list as a C structure.
///
/// # Returns
///
/// The total size of the HOB list in bytes.
///
/// # Safety
///
/// This function is unsafe because it uses a raw pointer to traverse memory and read data. The caller
/// must ensure that the pointer is valid and points to a properly formatted HOB list.
///
/// # Example
///
/// ```no_run
/// use patina::pi::hob::get_c_hob_list_size;
/// use core::ffi::c_void;
///
/// // Assuming `hob_list` is a valid pointer to a HOB list
/// # let some_val = 0;
/// # let hob_list = &some_val as *const _ as *const c_void;
/// let hob_list_ptr: *const c_void = hob_list;
/// let size = unsafe { get_c_hob_list_size(hob_list_ptr) };
/// println!("HOB list size: {}", size);
/// ```
pub unsafe fn get_c_hob_list_size(hob_list: *const c_void) -> usize {
    let mut hob_header: *const header::Hob = hob_list as *const header::Hob;
    let mut hob_list_len = 0;

    loop {
        // SAFETY: The caller must ensure that `hob_list` is a valid pointer to a properly formatted HOB list.
        let current_header = unsafe { hob_header.cast::<header::Hob>().as_ref().expect("Could not get hob list len") };
        hob_list_len += current_header.length as usize;
        if current_header.r#type == END_OF_HOB_LIST {
            break;
        }
        let next_hob = hob_header as usize + current_header.length as usize;
        hob_header = next_hob as *const header::Hob;
    }

    hob_list_len
}

impl<'a> HobList<'a> {
    /// Instantiates a Hoblist.
    pub const fn new() -> Self {
        HobList(Vec::new())
    }

    /// Implements iter for Hoblist.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     for hob in the_hob_list.iter() {
    ///         // ... do something with the hob(s)
    ///     }
    /// }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &Hob<'_>> {
        self.0.iter()
    }

    /// Returns a mutable pointer to the underlying data.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     let ptr: *mut c_void = the_hob_list.as_mut_ptr();
    ///     // ... do something with the pointer
    /// }
    /// ```
    pub fn as_mut_ptr<T>(&mut self) -> *mut T {
        self.0.as_mut_ptr() as *mut T
    }

    /// Returns the size of the Hoblist in bytes.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     let length = the_hob_list.size();
    ///     println!("size_of_hobs: {:?}", length);
    /// }
    pub fn size(&self) -> usize {
        let mut size_of_hobs = 0;

        for hob in self.iter() {
            size_of_hobs += hob.size()
        }

        size_of_hobs
    }

    /// Implements len for Hoblist.
    /// Returns the number of hobs in the list.
    ///
    /// # Example(s)
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///    // example discovering and adding hobs to a hob list
    ///    let mut the_hob_list = HobList::default();
    ///    the_hob_list.discover_hobs(hob_list);
    ///
    ///    let length = the_hob_list.len();
    ///    println!("length_of_hobs: {:?}", length);
    /// }
    /// ```
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Implements is_empty for Hoblist.
    /// Returns true if the list is empty.
    ///
    /// # Example(s)
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///    // example discovering and adding hobs to a hob list
    ///    let mut the_hob_list = HobList::default();
    ///    the_hob_list.discover_hobs(hob_list);
    ///
    ///    let is_empty = the_hob_list.is_empty();
    ///    println!("is_empty: {:?}", is_empty);
    /// }
    /// ```
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Implements push for Hoblist.
    ///
    /// Parameters:
    /// * hob: Hob<'a> - the hob to add to the list
    ///
    /// # Example(s)
    /// ```no_run
    /// use core::{ffi::c_void, mem::size_of};
    /// use patina::pi::hob::{HobList, Hob, header, FirmwareVolume, FV};
    ///
    /// fn example(hob_list: *const c_void) {
    ///   // example discovering and adding hobs to a hob list
    ///   let mut the_hob_list = HobList::default();
    ///   the_hob_list.discover_hobs(hob_list);
    ///
    ///   // example pushing a hob onto the list
    ///   let header = header::Hob {
    ///       r#type: FV,
    ///       length: size_of::<FirmwareVolume>() as u16,
    ///       reserved: 0,
    ///   };
    ///
    ///   let firmware_volume = FirmwareVolume {
    ///       header,
    ///       base_address: 0,
    ///       length: 0x0123456789abcdef,
    ///   };
    ///
    ///   let hob = Hob::FirmwareVolume(&firmware_volume);
    ///   the_hob_list.push(hob);
    /// }
    /// ```
    pub fn push(&mut self, hob: Hob<'a>) {
        let cloned_hob = hob.clone();
        self.0.push(cloned_hob);
    }

    /// Discovers hobs from a C style void* and adds them to a rust structure.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    /// }
    /// ```
    pub fn discover_hobs(&mut self, hob_list: *const c_void) {
        const NOT_NULL: &str = "Ptr should not be NULL";
        fn assert_hob_size<T>(hob: &header::Hob) {
            let hob_len = hob.length as usize;
            let hob_size = mem::size_of::<T>();
            assert_eq!(
                hob_len, hob_size,
                "Trying to cast hob of length {hob_len} into a pointer of size {hob_size}. Hob type: {:?}",
                hob.r#type
            );
        }

        let mut hob_header: *const header::Hob = hob_list as *const header::Hob;

        loop {
            let current_header = unsafe { hob_header.cast::<header::Hob>().as_ref().expect(NOT_NULL) };
            match current_header.r#type {
                HANDOFF => {
                    assert_hob_size::<PhaseHandoffInformationTable>(current_header);
                    let phit_hob =
                        unsafe { hob_header.cast::<PhaseHandoffInformationTable>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::Handoff(phit_hob));
                }
                MEMORY_ALLOCATION => {
                    if current_header.length == mem::size_of::<MemoryAllocationModule>() as u16 {
                        let mem_alloc_hob =
                            unsafe { hob_header.cast::<MemoryAllocationModule>().as_ref().expect(NOT_NULL) };
                        self.0.push(Hob::MemoryAllocationModule(mem_alloc_hob));
                    } else {
                        assert_hob_size::<MemoryAllocation>(current_header);
                        let mem_alloc_hob = unsafe { hob_header.cast::<MemoryAllocation>().as_ref().expect(NOT_NULL) };
                        self.0.push(Hob::MemoryAllocation(mem_alloc_hob));
                    }
                }
                RESOURCE_DESCRIPTOR => {
                    assert_hob_size::<ResourceDescriptor>(current_header);
                    let resource_desc_hob =
                        unsafe { hob_header.cast::<ResourceDescriptor>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::ResourceDescriptor(resource_desc_hob));
                }
                GUID_EXTENSION => {
                    let (guid_hob, data) = unsafe {
                        let hob = hob_header.cast::<GuidHob>().as_ref().expect(NOT_NULL);
                        let data_ptr = hob_header.byte_add(mem::size_of::<GuidHob>()) as *mut u8;
                        let data_len = hob.header.length as usize - mem::size_of::<GuidHob>();
                        (hob, slice::from_raw_parts(data_ptr, data_len))
                    };
                    self.0.push(Hob::GuidHob(guid_hob, data));
                }
                FV => {
                    assert_hob_size::<FirmwareVolume>(current_header);
                    let fv_hob = unsafe { hob_header.cast::<FirmwareVolume>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::FirmwareVolume(fv_hob));
                }
                FV2 => {
                    assert_hob_size::<FirmwareVolume2>(current_header);
                    let fv2_hob = unsafe { hob_header.cast::<FirmwareVolume2>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::FirmwareVolume2(fv2_hob));
                }
                FV3 => {
                    assert_hob_size::<FirmwareVolume3>(current_header);
                    let fv3_hob = unsafe { hob_header.cast::<FirmwareVolume3>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::FirmwareVolume3(fv3_hob));
                }
                CPU => {
                    assert_hob_size::<Cpu>(current_header);
                    let cpu_hob = unsafe { hob_header.cast::<Cpu>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::Cpu(cpu_hob));
                }
                UEFI_CAPSULE => {
                    assert_hob_size::<Capsule>(current_header);
                    let capsule_hob = unsafe { hob_header.cast::<Capsule>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::Capsule(capsule_hob));
                }
                RESOURCE_DESCRIPTOR2 => {
                    assert_hob_size::<ResourceDescriptorV2>(current_header);
                    let resource_desc_hob =
                        unsafe { hob_header.cast::<ResourceDescriptorV2>().as_ref().expect(NOT_NULL) };
                    self.0.push(Hob::ResourceDescriptorV2(resource_desc_hob));
                }
                END_OF_HOB_LIST => {
                    break;
                }
                _ => {
                    self.0.push(Hob::Misc(current_header.r#type));
                }
            }
            let next_hob = hob_header as usize + current_header.length as usize;
            hob_header = next_hob as *const header::Hob;
        }
    }

    /// Relocates all HOBs in the list to new memory locations.
    ///
    /// This function creates new instances of each HOB in the list and updates the list to point to these new instances.
    ///
    /// # Example(s)
    ///
    /// ```no_run
    /// use core::ffi::c_void;
    /// use patina::pi::hob::HobList;
    ///
    /// fn example(hob_list: *const c_void) {
    ///     // example discovering and adding hobs to a hob list
    ///     let mut the_hob_list = HobList::default();
    ///     the_hob_list.discover_hobs(hob_list);
    ///
    ///     // relocate hobs to new memory locations
    ///     the_hob_list.relocate_hobs();
    /// }
    /// ```
    pub fn relocate_hobs(&mut self) {
        for hob in self.0.iter_mut() {
            match hob {
                Hob::Handoff(hob) => *hob = Box::leak(Box::new(PhaseHandoffInformationTable::clone(hob))),
                Hob::MemoryAllocation(hob) => *hob = Box::leak(Box::new(MemoryAllocation::clone(hob))),
                Hob::MemoryAllocationModule(hob) => *hob = Box::leak(Box::new(MemoryAllocationModule::clone(hob))),
                Hob::Capsule(hob) => *hob = Box::leak(Box::new(Capsule::clone(hob))),
                Hob::ResourceDescriptor(hob) => *hob = Box::leak(Box::new(ResourceDescriptor::clone(hob))),
                Hob::GuidHob(hob, data) => {
                    *hob = Box::leak(Box::new(GuidHob::clone(hob)));
                    *data = Box::leak(data.to_vec().into_boxed_slice());
                }
                Hob::FirmwareVolume(hob) => *hob = Box::leak(Box::new(FirmwareVolume::clone(hob))),
                Hob::FirmwareVolume2(hob) => *hob = Box::leak(Box::new(FirmwareVolume2::clone(hob))),
                Hob::FirmwareVolume3(hob) => *hob = Box::leak(Box::new(FirmwareVolume3::clone(hob))),
                Hob::Cpu(hob) => *hob = Box::leak(Box::new(Cpu::clone(hob))),
                Hob::ResourceDescriptorV2(hob) => *hob = Box::leak(Box::new(ResourceDescriptorV2::clone(hob))),
                Hob::Misc(_) => (), // Data is owned in Misc (nothing to move),
            };
        }
    }
}

/// Implements IntoIterator for HobList.
///
/// Defines how it will be converted to an iterator.
impl<'a> IntoIterator for HobList<'a> {
    type Item = Hob<'a>;
    type IntoIter = <Vec<Hob<'a>> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a HobList<'a> {
    type Item = &'a Hob<'a>;
    type IntoIter = core::slice::Iter<'a, Hob<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Implements Debug for Hoblist.
///
/// Writes Hoblist debug information to stdio
///
impl fmt::Debug for HobList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for hob in self.0.clone().into_iter() {
            match hob {
                Hob::Handoff(hob) => {
                    write!(
                        f,
                        indoc! {"
                        PHASE HANDOFF INFORMATION TABLE (PHIT) HOB
                          HOB Length: 0x{:x}
                          Version: 0x{:x}
                          Boot Mode: {}
                          Memory Bottom: 0x{:x}
                          Memory Top: 0x{:x}
                          Free Memory Bottom: 0x{:x}
                          Free Memory Top: 0x{:x}
                          End of HOB List: 0x{:x}\n"},
                        hob.header.length,
                        hob.version,
                        hob.boot_mode,
                        align_up(hob.memory_bottom, 0x1000).unwrap_or(0),
                        align_down(hob.memory_top, 0x1000).unwrap_or(0),
                        align_up(hob.free_memory_bottom, 0x1000).unwrap_or(0),
                        align_down(hob.free_memory_top, 0x1000).unwrap_or(0),
                        hob.end_of_hob_list
                    )?;
                }
                Hob::MemoryAllocation(hob) => {
                    write!(
                        f,
                        indoc! {"
                        MEMORY ALLOCATION HOB
                          HOB Length: 0x{:x}
                          Memory Base Address: 0x{:x}
                          Memory Length: 0x{:x}
                          Memory Type: {:?}\n"},
                        hob.header.length,
                        hob.alloc_descriptor.memory_base_address,
                        hob.alloc_descriptor.memory_length,
                        hob.alloc_descriptor.memory_type
                    )?;
                }
                Hob::ResourceDescriptor(hob) => {
                    write!(
                        f,
                        indoc! {"
                        RESOURCE DESCRIPTOR HOB
                          HOB Length: 0x{:x}
                          Resource Type: 0x{:x}
                          Resource Attribute Type: 0x{:x}
                          Resource Start Address: 0x{:x}
                          Resource Length: 0x{:x}\n"},
                        hob.header.length,
                        hob.resource_type,
                        hob.resource_attribute,
                        hob.physical_start,
                        hob.resource_length
                    )?;
                }
                Hob::GuidHob(hob, _data) => {
                    let (f0, f1, f2, f3, f4, &[f5, f6, f7, f8, f9, f10]) = hob.name.as_fields();
                    write!(
                        f,
                        indoc! {"
                        GUID HOB
                          Type: {:#x}
                          Length: {:#x},
                          GUID: {{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}\n"},
                        hob.header.r#type, hob.header.length, f0, f1, f2, f3, f4, f5, f6, f7, f8, f9, f10,
                    )?;
                }
                Hob::FirmwareVolume(hob) => {
                    write!(
                        f,
                        indoc! {"
                        FIRMWARE VOLUME (FV) HOB
                          HOB Length: 0x{:x}
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.header.length, hob.base_address, hob.length
                    )?;
                }
                Hob::FirmwareVolume2(hob) => {
                    write!(
                        f,
                        indoc! {"
                        FIRMWARE VOLUME 2 (FV2) HOB
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.base_address, hob.length
                    )?;
                }
                Hob::FirmwareVolume3(hob) => {
                    write!(
                        f,
                        indoc! {"
                        FIRMWARE VOLUME 3 (FV3) HOB
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.base_address, hob.length
                    )?;
                }
                Hob::Cpu(hob) => {
                    write!(
                        f,
                        indoc! {"
                        CPU HOB
                          Memory Space Size: 0x{:x}
                          IO Space Size: 0x{:x}\n"},
                        hob.size_of_memory_space, hob.size_of_io_space
                    )?;
                }
                Hob::Capsule(hob) => {
                    write!(
                        f,
                        indoc! {"
                        CAPSULE HOB
                          Base Address: 0x{:x}
                          Length: 0x{:x}\n"},
                        hob.base_address, hob.length
                    )?;
                }
                Hob::ResourceDescriptorV2(hob) => {
                    write!(
                        f,
                        indoc! {"
                        RESOURCE DESCRIPTOR 2 HOB
                          HOB Length: 0x{:x}
                          Resource Type: 0x{:x}
                          Resource Attribute Type: 0x{:x}
                          Resource Start Address: 0x{:x}
                          Resource Length: 0x{:x}
                          Attributes: 0x{:x}\n"},
                        hob.v1.header.length,
                        hob.v1.resource_type,
                        hob.v1.resource_attribute,
                        hob.v1.physical_start,
                        hob.v1.resource_length,
                        hob.attributes
                    )?;
                }
                _ => (),
            }
        }
        write!(f, "Parsed HOBs")
    }
}

impl Hob<'_> {
    /// Returns the HOB header for this Hand-Off Block
    pub fn header(&self) -> header::Hob {
        match self {
            Hob::Handoff(hob) => hob.header,
            Hob::MemoryAllocation(hob) => hob.header,
            Hob::MemoryAllocationModule(hob) => hob.header,
            Hob::Capsule(hob) => hob.header,
            Hob::ResourceDescriptor(hob) => hob.header,
            Hob::GuidHob(hob, _) => hob.header,
            Hob::FirmwareVolume(hob) => hob.header,
            Hob::FirmwareVolume2(hob) => hob.header,
            Hob::FirmwareVolume3(hob) => hob.header,
            Hob::Cpu(hob) => hob.header,
            Hob::ResourceDescriptorV2(hob) => hob.v1.header,
            Hob::Misc(hob_type) => {
                header::Hob { r#type: *hob_type, length: mem::size_of::<header::Hob>() as u16, reserved: 0 }
            }
        }
    }
}

/// A HOB iterator.
///
pub struct HobIter<'a> {
    hob_ptr: *const header::Hob,
    _a: PhantomData<&'a ()>,
}

impl<'a> IntoIterator for &Hob<'a> {
    type Item = Hob<'a>;

    type IntoIter = HobIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        HobIter { hob_ptr: self.as_ptr(), _a: PhantomData }
    }
}

impl<'a> Iterator for HobIter<'a> {
    type Item = Hob<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        const NOT_NULL: &str = "Ptr should not be NULL";
        let hob_header = unsafe { *(self.hob_ptr) };
        let hob = unsafe {
            match hob_header.r#type {
                HANDOFF => {
                    Hob::Handoff((self.hob_ptr as *const PhaseHandoffInformationTable).as_ref().expect(NOT_NULL))
                }
                MEMORY_ALLOCATION if hob_header.length as usize == mem::size_of::<MemoryAllocationModule>() => {
                    Hob::MemoryAllocationModule(
                        (self.hob_ptr as *const MemoryAllocationModule).as_ref().expect(NOT_NULL),
                    )
                }
                MEMORY_ALLOCATION => {
                    Hob::MemoryAllocation((self.hob_ptr as *const MemoryAllocation).as_ref().expect(NOT_NULL))
                }
                RESOURCE_DESCRIPTOR => {
                    Hob::ResourceDescriptor((self.hob_ptr as *const ResourceDescriptor).as_ref().expect(NOT_NULL))
                }
                GUID_EXTENSION => {
                    let hob = (self.hob_ptr as *const GuidHob).as_ref().expect(NOT_NULL);
                    let data_ptr = self.hob_ptr.byte_add(mem::size_of::<GuidHob>()) as *const u8;
                    let data_len = hob.header.length as usize - mem::size_of::<GuidHob>();
                    Hob::GuidHob(hob, slice::from_raw_parts(data_ptr, data_len))
                }
                FV => Hob::FirmwareVolume((self.hob_ptr as *const FirmwareVolume).as_ref().expect(NOT_NULL)),
                FV2 => Hob::FirmwareVolume2((self.hob_ptr as *const FirmwareVolume2).as_ref().expect(NOT_NULL)),
                FV3 => Hob::FirmwareVolume3((self.hob_ptr as *const FirmwareVolume3).as_ref().expect(NOT_NULL)),
                CPU => Hob::Cpu((self.hob_ptr as *const Cpu).as_ref().expect(NOT_NULL)),
                UEFI_CAPSULE => Hob::Capsule((self.hob_ptr as *const Capsule).as_ref().expect(NOT_NULL)),
                RESOURCE_DESCRIPTOR2 => {
                    Hob::ResourceDescriptorV2((self.hob_ptr as *const &ResourceDescriptorV2).as_ref().expect(NOT_NULL))
                }
                END_OF_HOB_LIST => return None,
                hob_type => Hob::Misc(hob_type),
            }
        };
        self.hob_ptr = (self.hob_ptr as usize + hob_header.length as usize) as *const header::Hob;
        Some(hob)
    }
}

// Well-known GUID Extension HOB type definitions

/// Memory Type Information GUID Extension Hob GUID.
pub const MEMORY_TYPE_INFO_HOB_GUID: r_efi::efi::Guid =
    r_efi::efi::Guid::from_fields(0x4c19049f, 0x4137, 0x4dd3, 0x9c, 0x10, &[0x8b, 0x97, 0xa8, 0x3f, 0xfd, 0xfa]);

/// Memory Type Information GUID Extension Hob structure definition.
#[derive(Debug)]
#[repr(C)]
pub struct EFiMemoryTypeInformation {
    /// Type of memory being described.
    pub memory_type: r_efi::efi::MemoryType,
    /// Number of pages in this allocation.
    pub number_of_pages: u32,
}

#[cfg(test)]
mod tests {
    use crate::pi::{
        BootMode, hob,
        hob::{Hob, HobList, HobTrait},
    };

    use core::{
        ffi::c_void,
        mem::{drop, forget, size_of},
        ptr,
        slice::from_raw_parts,
    };

    // Expectation is someone will provide alloc
    extern crate alloc;
    use alloc::vec::Vec;

    // Generate a test firmware volume hob
    // # Returns
    // A FirmwareVolume hob
    fn gen_firmware_volume() -> hob::FirmwareVolume {
        let header = hob::header::Hob { r#type: hob::FV, length: size_of::<hob::FirmwareVolume>() as u16, reserved: 0 };

        hob::FirmwareVolume { header, base_address: 0, length: 0x0123456789abcdef }
    }

    // Generate a test firmware volume 2 hob
    // # Returns
    // A FirmwareVolume2 hob
    fn gen_firmware_volume2() -> hob::FirmwareVolume2 {
        let header =
            hob::header::Hob { r#type: hob::FV2, length: size_of::<hob::FirmwareVolume2>() as u16, reserved: 0 };

        hob::FirmwareVolume2 {
            header,
            base_address: 0,
            length: 0x0123456789abcdef,
            fv_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            file_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
        }
    }

    // Generate a test firmware volume 3 hob
    // # Returns
    // A FirmwareVolume3 hob
    fn gen_firmware_volume3() -> hob::FirmwareVolume3 {
        let header =
            hob::header::Hob { r#type: hob::FV3, length: size_of::<hob::FirmwareVolume3>() as u16, reserved: 0 };

        hob::FirmwareVolume3 {
            header,
            base_address: 0,
            length: 0x0123456789abcdef,
            authentication_status: 0,
            extracted_fv: false.into(),
            fv_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            file_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
        }
    }

    // Generate a test resource descriptor hob
    // # Returns
    // A ResourceDescriptor hob
    fn gen_resource_descriptor() -> hob::ResourceDescriptor {
        let header = hob::header::Hob {
            r#type: hob::RESOURCE_DESCRIPTOR,
            length: size_of::<hob::ResourceDescriptor>() as u16,
            reserved: 0,
        };

        hob::ResourceDescriptor {
            header,
            owner: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
            physical_start: 0,
            resource_length: 0x0123456789abcdef,
        }
    }

    // Generate a test resource descriptor hob
    // # Returns
    // A ResourceDescriptor hob
    fn gen_resource_descriptor_v2() -> hob::ResourceDescriptorV2 {
        let mut v1 = gen_resource_descriptor();
        v1.header.r#type = hob::RESOURCE_DESCRIPTOR2;
        v1.header.length = size_of::<hob::ResourceDescriptorV2>() as u16;

        hob::ResourceDescriptorV2 { v1, attributes: 8 }
    }

    // Generate a test phase handoff information table hob
    // # Returns
    // A MemoryAllocation hob
    fn gen_memory_allocation() -> hob::MemoryAllocation {
        let header = hob::header::Hob {
            r#type: hob::MEMORY_ALLOCATION,
            length: size_of::<hob::MemoryAllocation>() as u16,
            reserved: 0,
        };

        let alloc_descriptor = hob::header::MemoryAllocation {
            name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            memory_base_address: 0,
            memory_length: 0x0123456789abcdef,
            memory_type: 0,
            reserved: [0; 4],
        };

        hob::MemoryAllocation { header, alloc_descriptor }
    }

    fn gen_memory_allocation_module() -> hob::MemoryAllocationModule {
        let header = hob::header::Hob {
            r#type: hob::MEMORY_ALLOCATION,
            length: size_of::<hob::MemoryAllocationModule>() as u16,
            reserved: 0,
        };

        let alloc_descriptor = hob::header::MemoryAllocation {
            name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            memory_base_address: 0,
            memory_length: 0x0123456789abcdef,
            memory_type: 0,
            reserved: [0; 4],
        };

        hob::MemoryAllocationModule {
            header,
            alloc_descriptor,
            module_name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            entry_point: 0,
        }
    }

    fn gen_capsule() -> hob::Capsule {
        let header =
            hob::header::Hob { r#type: hob::UEFI_CAPSULE, length: size_of::<hob::Capsule>() as u16, reserved: 0 };

        hob::Capsule { header, base_address: 0, length: 0x12 }
    }

    fn gen_guid_hob() -> (hob::GuidHob, Box<[u8]>) {
        let data = Box::new([1_u8, 2, 3, 4, 5, 6, 7, 8]);
        (
            hob::GuidHob {
                header: hob::header::Hob {
                    r#type: hob::GUID_EXTENSION,
                    length: (size_of::<hob::GuidHob>() + data.len()) as u16,
                    reserved: 0,
                },
                name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            },
            data,
        )
    }

    fn gen_phase_handoff_information_table() -> hob::PhaseHandoffInformationTable {
        let header = hob::header::Hob {
            r#type: hob::HANDOFF,
            length: size_of::<hob::PhaseHandoffInformationTable>() as u16,
            reserved: 0,
        };

        hob::PhaseHandoffInformationTable {
            header,
            version: 0x00010000,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0xdeadbeef,
            memory_bottom: 0xdeadc0de,
            free_memory_top: 104,
            free_memory_bottom: 255,
            end_of_hob_list: 0xdeaddeadc0dec0de,
        }
    }

    // Generate a test end of hoblist hob
    // # Returns
    // A PhaseHandoffInformationTable hob
    fn gen_end_of_hoblist() -> hob::PhaseHandoffInformationTable {
        let header = hob::header::Hob {
            r#type: hob::END_OF_HOB_LIST,
            length: size_of::<hob::PhaseHandoffInformationTable>() as u16,
            reserved: 0,
        };

        hob::PhaseHandoffInformationTable {
            header,
            version: 0x00010000,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0xdeadbeef,
            memory_bottom: 0xdeadc0de,
            free_memory_top: 104,
            free_memory_bottom: 255,
            end_of_hob_list: 0xdeaddeadc0dec0de,
        }
    }

    fn gen_cpu() -> hob::Cpu {
        let header = hob::header::Hob { r#type: hob::CPU, length: size_of::<hob::Cpu>() as u16, reserved: 0 };

        hob::Cpu { header, size_of_memory_space: 0, size_of_io_space: 0, reserved: [0; 6] }
    }

    // Converts the Hoblist to a C array.
    // # Arguments
    // * `hob_list` - A reference to the HobList.
    //
    // # Returns
    // A tuple containing a pointer to the C array and the length of the C array.
    pub fn to_c_array(hob_list: &hob::HobList) -> (*const c_void, usize) {
        let size = hob_list.size();
        let mut c_array: Vec<u8> = Vec::with_capacity(size);

        for hob in hob_list.iter() {
            let slice = unsafe { from_raw_parts(hob.as_ptr(), hob.size()) };
            c_array.extend_from_slice(slice);
        }

        let void_ptr = c_array.as_ptr() as *const c_void;

        // in order to not call the destructor on the Vec at the end of this function, we need to forget it
        forget(c_array);

        (void_ptr, size)
    }

    // Implements a function to manually free a C array.
    //
    // # Arguments
    // * `c_array_ptr` - A pointer to the C array.
    // * `len` - The length of the C array.
    //
    // # Safety
    //
    // The caller must ensure that the pointer and length match a Vec originally created by to_c_array.
    pub fn manually_free_c_array(c_array_ptr: *const c_void, len: usize) {
        let ptr = c_array_ptr as *mut u8;
        // SAFETY: Caller is responsible for ensuring the pointer and length are valid per the function contract.
        unsafe {
            drop(Vec::from_raw_parts(ptr, len, len));
        }
    }

    #[test]
    fn test_hoblist_empty() {
        let hoblist = HobList::new();
        assert_eq!(hoblist.len(), 0);
        assert!(hoblist.is_empty());
    }

    #[test]
    fn test_hoblist_push() {
        let mut hoblist = HobList::new();
        let resource = gen_resource_descriptor();
        hoblist.push(Hob::ResourceDescriptor(&resource));
        assert_eq!(hoblist.len(), 1);

        let firmware_volume = gen_firmware_volume();
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));

        assert_eq!(hoblist.len(), 2);

        let resource_v2 = gen_resource_descriptor_v2();
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));

        assert_eq!(hoblist.len(), 3);
    }

    #[test]
    fn test_hoblist_iterate() {
        let mut hoblist = HobList::default();
        let resource = gen_resource_descriptor();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let end_of_hob_list = gen_end_of_hoblist();
        let capsule = gen_capsule();
        let (guid_hob, guid_hob_data) = gen_guid_hob();
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();

        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(&guid_hob, guid_hob_data.as_ref()));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        let mut count = 0;
        hoblist.iter().for_each(|hob| {
            match hob {
                Hob::ResourceDescriptor(resource) => {
                    assert_eq!(resource.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::MemoryAllocation(memory_allocation) => {
                    assert_eq!(memory_allocation.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::MemoryAllocationModule(memory_allocation_module) => {
                    assert_eq!(memory_allocation_module.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::Capsule(capsule) => {
                    assert_eq!(capsule.base_address, 0);
                }
                Hob::GuidHob(guid_hob, data) => {
                    assert_eq!(guid_hob.name, r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]));
                    assert_eq!(*data, &[1_u8, 2, 3, 4, 5, 6, 7, 8]);
                }
                Hob::FirmwareVolume(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume2(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume3(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::Handoff(handoff) => {
                    assert_eq!(handoff.memory_top, 0xdeadbeef);
                }
                _ => {
                    panic!("Unexpected hob type");
                }
            }
            count += 1;
        });
        assert_eq!(count, 9);
    }

    #[test]
    fn test_hoblist_discover() {
        // generate some test hobs
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let (guid_hob, guid_hob_data) = gen_guid_hob();
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let resource_v2 = gen_resource_descriptor_v2();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(&guid_hob, guid_hob_data.as_ref()));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        // assert that the hoblist has 3 hobs and they are of the correct type

        let mut count = 0;
        hoblist.iter().for_each(|hob| {
            match hob {
                Hob::ResourceDescriptor(resource) => {
                    assert_eq!(resource.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::MemoryAllocation(memory_allocation) => {
                    assert_eq!(memory_allocation.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::MemoryAllocationModule(memory_allocation_module) => {
                    assert_eq!(memory_allocation_module.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::Capsule(capsule) => {
                    assert_eq!(capsule.base_address, 0);
                }
                Hob::GuidHob(guid_hob, data) => {
                    assert_eq!(guid_hob.name, r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]));
                    assert_eq!(&data[..], &guid_hob_data[..]);
                }
                Hob::FirmwareVolume(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume2(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume3(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::Handoff(handoff) => {
                    assert_eq!(handoff.memory_top, 0xdeadbeef);
                }
                Hob::Cpu(cpu) => {
                    assert_eq!(cpu.size_of_memory_space, 0);
                }
                Hob::ResourceDescriptorV2(resource) => {
                    assert_eq!(resource.v1.header.r#type, hob::RESOURCE_DESCRIPTOR2);
                    assert_eq!(resource.v1.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                _ => {
                    panic!("Unexpected hob type");
                }
            }
            count += 1;
        });

        assert_eq!(count, 12);

        // c_hoblist is a pointer to the hoblist - we need to manually free it later
        let (c_array_hoblist, length) = to_c_array(&hoblist);

        // create a new hoblist
        let mut cloned_hoblist = HobList::new();
        cloned_hoblist.discover_hobs(c_array_hoblist);

        // assert that the hoblist has 2 hobs and they are of the correct type
        // we don't need to check the end of hoblist hob as it will not be 'discovered'
        // by the discover_hobs function and simply end the iteration
        count = 0;
        hoblist.into_iter().for_each(|hob| {
            match hob {
                Hob::ResourceDescriptor(resource) => {
                    assert_eq!(resource.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::MemoryAllocation(memory_allocation) => {
                    assert_eq!(memory_allocation.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::MemoryAllocationModule(memory_allocation_module) => {
                    assert_eq!(memory_allocation_module.alloc_descriptor.memory_length, 0x0123456789abcdef);
                }
                Hob::Capsule(capsule) => {
                    assert_eq!(capsule.base_address, 0);
                }
                Hob::GuidHob(guid_hob, data) => {
                    assert_eq!(guid_hob.name, r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]));
                    assert_eq!(data, &[1_u8, 2, 3, 4, 5, 6, 7, 8]);
                }
                Hob::FirmwareVolume(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume2(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::FirmwareVolume3(firmware_volume) => {
                    assert_eq!(firmware_volume.length, 0x0123456789abcdef);
                }
                Hob::Handoff(handoff) => {
                    assert_eq!(handoff.memory_top, 0xdeadbeef);
                }
                Hob::ResourceDescriptorV2(resource) => {
                    assert_eq!(resource.v1.header.r#type, hob::RESOURCE_DESCRIPTOR2);
                    assert_eq!(resource.v1.resource_type, hob::EFI_RESOURCE_SYSTEM_MEMORY);
                }
                Hob::Cpu(cpu) => {
                    assert_eq!(cpu.size_of_memory_space, 0);
                }
                _ => {
                    panic!("Unexpected hob type");
                }
            }
            count += 1;
        });

        assert_eq!(count, 12);

        // free the c array
        manually_free_c_array(c_array_hoblist, length);
    }

    #[test]
    fn test_hob_iterator() {
        // generate some test hobs
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let (guid_hob, guid_hob_data) = gen_guid_hob();
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(&guid_hob, guid_hob_data.as_ref()));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        let (c_array_hoblist, length) = to_c_array(&hoblist);

        let hob = Hob::ResourceDescriptor(unsafe {
            (c_array_hoblist as *const hob::ResourceDescriptor).as_ref::<'static>().unwrap()
        });
        for h in &hob {
            println!("{:?}", h.header());
        }

        manually_free_c_array(c_array_hoblist, length);
    }

    #[test]
    fn test_hob_iterator2() {
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let (guid_hob, guid_hob_data) = gen_guid_hob();
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let resource_v2 = gen_resource_descriptor_v2();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(&guid_hob, guid_hob_data.as_ref()));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        // Make sure we can iterate over a reference to a HobList without
        // consuming it.
        for hob in &hoblist {
            println!("{:?}", hob.header());
        }

        for hob in hoblist {
            println!("{:?}", hob.header());
        }
    }

    #[test]
    fn test_relocate_hobs() {
        // generate some test hobs
        let resource = gen_resource_descriptor();
        let handoff = gen_phase_handoff_information_table();
        let firmware_volume = gen_firmware_volume();
        let firmware_volume2 = gen_firmware_volume2();
        let firmware_volume3 = gen_firmware_volume3();
        let capsule = gen_capsule();
        let (guid_hob, guid_hob_data) = gen_guid_hob();
        let memory_allocation = gen_memory_allocation();
        let memory_allocation_module = gen_memory_allocation_module();
        let cpu = gen_cpu();
        let resource_v2 = gen_resource_descriptor_v2();
        let end_of_hob_list = gen_end_of_hoblist();

        // create a new hoblist
        let mut hoblist = HobList::new();

        // Push the resource descriptor to the hoblist
        hoblist.push(Hob::ResourceDescriptor(&resource));
        hoblist.push(Hob::Handoff(&handoff));
        hoblist.push(Hob::FirmwareVolume(&firmware_volume));
        hoblist.push(Hob::FirmwareVolume2(&firmware_volume2));
        hoblist.push(Hob::FirmwareVolume3(&firmware_volume3));
        hoblist.push(Hob::Capsule(&capsule));
        hoblist.push(Hob::GuidHob(&guid_hob, guid_hob_data.as_ref()));
        hoblist.push(Hob::MemoryAllocation(&memory_allocation));
        hoblist.push(Hob::MemoryAllocationModule(&memory_allocation_module));
        hoblist.push(Hob::Cpu(&cpu));
        hoblist.push(Hob::Misc(12345));
        hoblist.push(Hob::ResourceDescriptorV2(&resource_v2));
        hoblist.push(Hob::Handoff(&end_of_hob_list));

        let hoblist_address = hoblist.as_mut_ptr::<()>() as usize;
        let hoblist_len = hoblist.len();
        hoblist.relocate_hobs();
        assert_eq!(
            hoblist_address,
            hoblist.as_mut_ptr::<()>() as usize,
            "Only hobs need to be relocated, not the vector."
        );
        assert_eq!(hoblist_len, hoblist.len());

        for (i, hob) in hoblist.into_iter().enumerate() {
            match hob {
                Hob::ResourceDescriptor(hob) if i == 0 => {
                    assert_ne!(ptr::addr_of!(resource), hob);
                    assert_eq!(resource, *hob);
                }
                Hob::Handoff(hob) if i == 1 => {
                    assert_ne!(ptr::addr_of!(handoff), hob);
                    assert_eq!(handoff, *hob);
                }
                Hob::FirmwareVolume(hob) if i == 2 => {
                    assert_ne!(ptr::addr_of!(firmware_volume), hob);
                    assert_eq!(firmware_volume, *hob);
                }
                Hob::FirmwareVolume2(hob) if i == 3 => {
                    assert_ne!(ptr::addr_of!(firmware_volume2), hob);
                    assert_eq!(firmware_volume2, *hob);
                }
                Hob::FirmwareVolume3(hob) if i == 4 => {
                    assert_ne!(ptr::addr_of!(firmware_volume3), hob);
                    assert_eq!(firmware_volume3, *hob);
                }
                Hob::Capsule(hob) if i == 5 => {
                    assert_ne!(ptr::addr_of!(capsule), hob);
                    assert_eq!(capsule, *hob);
                }
                Hob::GuidHob(hob, hob_data) if i == 6 => {
                    assert_ne!(ptr::addr_of!(guid_hob), hob);
                    assert_ne!(guid_hob_data.as_ptr(), hob_data.as_ptr());
                    assert_eq!(guid_hob.header, hob.header);
                    assert_eq!(guid_hob.name, hob.name);
                    assert_eq!(&guid_hob_data[..], hob_data);
                }
                Hob::MemoryAllocation(hob) if i == 7 => {
                    assert_ne!(ptr::addr_of!(memory_allocation), hob);
                    assert_eq!(memory_allocation.header, hob.header);
                    assert_eq!(memory_allocation.alloc_descriptor, hob.alloc_descriptor);
                }
                Hob::MemoryAllocationModule(hob) if i == 8 => {
                    assert_ne!(ptr::addr_of!(memory_allocation_module), hob);
                    assert_eq!(memory_allocation_module, *hob);
                }
                Hob::Cpu(hob) if i == 9 => {
                    assert_ne!(ptr::addr_of!(cpu), hob);
                    assert_eq!(cpu, *hob);
                }
                Hob::Misc(hob) if i == 10 => {
                    assert_eq!(12345, hob);
                }
                Hob::ResourceDescriptorV2(hob) if i == 11 => {
                    assert_ne!(ptr::addr_of!(resource_v2), hob);
                    assert_eq!(resource_v2, *hob);
                }
                Hob::Handoff(hob) if i == 12 => {
                    assert_ne!(ptr::addr_of!(end_of_hob_list), hob);
                    assert_eq!(end_of_hob_list, *hob);
                }
                _ => panic!("Hob at index: {i}."),
            }
        }
    }

    #[test]
    fn test_hoblist_debug_display() {
        use alloc::format;

        let mut hoblist = HobList::new();
        let handoff = gen_phase_handoff_information_table();
        hoblist.push(Hob::Handoff(&handoff));

        let debug_output = format!("{:?}", hoblist);

        assert!(debug_output.contains("PHASE HANDOFF INFORMATION TABLE"));
        assert!(debug_output.contains("HOB Length:"));
        assert!(debug_output.contains("Version:"));
        assert!(debug_output.contains("Boot Mode:"));
        assert!(debug_output.contains("Memory Bottom:"));
        assert!(debug_output.contains("Memory Top:"));
        assert!(debug_output.contains("Free Memory Bottom:"));
        assert!(debug_output.contains("Free Memory Top:"));
        assert!(debug_output.contains("End of HOB List:"));
    }
}
