//! Hand-Off Blocks (HOB)
//!
//! Hand-Off Blocks provide a standardized method for passing information from
//! the pre-DXE phase to the DXE phase during the platform initialization process.
//!
//! HOBs describe the physical memory layout, CPU information, firmware volumes,
//! and other platform-specific data that DXE needs to continue the boot process.
//!
//! Based on the UEFI Platform Initialization Specification Volume III.
//!
//! ## Example
//! ```
//! use patina::pi::{BootMode, hob, hob::Hob};
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
//!     fv_name: patina::BinaryGuid::from_string("00000001-0002-0003-0405-060708090A0B"),
//!     file_name: patina::BinaryGuid::from_string("00000001-0002-0003-0405-060708090A0B"),
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
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::pi::BootMode;
use core::{
    ffi::c_void,
    marker::PhantomData,
    mem::{self, size_of},
    slice,
};

// if alloc is available, export the hob list module
#[cfg(any(test, feature = "alloc"))]
pub mod hob_list;

// export hob_list::HobList as HobList if alloc is available
#[cfg(any(test, feature = "alloc"))]
pub use hob_list::HobList;

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
        pub name: crate::BinaryGuid,

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
    pub module_name: crate::BinaryGuid,

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
    pub owner: crate::BinaryGuid,

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
    pub name: crate::BinaryGuid,
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
    pub fv_name: crate::BinaryGuid,

    /// The name of the firmware file which contained this firmware volume.
    ///
    pub file_name: crate::BinaryGuid,
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
    pub fv_name: crate::BinaryGuid,

    /// The name GUID of the firmware file which contained this firmware volume.
    /// Valid only if IsExtractedFv is TRUE.
    ///
    pub file_name: crate::BinaryGuid,
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
/// use patina::pi::hob::get_pi_hob_list_size;
/// use core::ffi::c_void;
///
/// // Assuming `hob_list` is a valid pointer to a HOB list
/// # let some_val = 0;
/// # let hob_list = &some_val as *const _ as *const c_void;
/// let hob_list_ptr: *const c_void = hob_list;
/// let size = unsafe { get_pi_hob_list_size(hob_list_ptr) };
/// println!("HOB list size: {}", size);
/// ```
pub unsafe fn get_pi_hob_list_size(hob_list: *const c_void) -> usize {
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
        // SAFETY: hob_ptr points to valid HOB data. The iterator maintains the pointer through the HOB chain.
        let hob_header = unsafe { *(self.hob_ptr) };
        // SAFETY: HOB type determines the specific HOB structure. Each cast is to the appropriate type based
        // on the HOB header type field. as_ref() converts to a reference with the iterator's lifetime.
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
                    Hob::ResourceDescriptorV2((self.hob_ptr as *const ResourceDescriptorV2).as_ref().expect(NOT_NULL))
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
pub const MEMORY_TYPE_INFO_HOB_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("4C19049F-4137-4DD3-9C10-8B97A83FFDFA");

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
pub(crate) mod tests {
    use crate::pi::{
        BootMode, hob,
        hob::{
            Capsule, Cpu, FirmwareVolume, MemoryAllocation, PhaseHandoffInformationTable, ResourceDescriptor,
            get_pi_hob_list_size,
        },
    };

    use core::{mem::size_of, slice::from_raw_parts};

    use std::vec::Vec;

    // Generate a test firmware volume hob
    // # Returns
    // A FirmwareVolume hob
    pub(crate) fn gen_firmware_volume() -> hob::FirmwareVolume {
        let header = hob::header::Hob { r#type: hob::FV, length: size_of::<hob::FirmwareVolume>() as u16, reserved: 0 };

        hob::FirmwareVolume { header, base_address: 0, length: 0x0123456789abcdef }
    }

    // Generate a test firmware volume 2 hob
    // # Returns
    // A FirmwareVolume2 hob
    pub(crate) fn gen_firmware_volume2() -> hob::FirmwareVolume2 {
        let header =
            hob::header::Hob { r#type: hob::FV2, length: size_of::<hob::FirmwareVolume2>() as u16, reserved: 0 };

        hob::FirmwareVolume2 {
            header,
            base_address: 0,
            length: 0x0123456789abcdef,
            fv_name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            file_name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
        }
    }

    // Generate a test firmware volume 3 hob
    // # Returns
    // A FirmwareVolume3 hob
    pub(crate) fn gen_firmware_volume3() -> hob::FirmwareVolume3 {
        let header =
            hob::header::Hob { r#type: hob::FV3, length: size_of::<hob::FirmwareVolume3>() as u16, reserved: 0 };

        hob::FirmwareVolume3 {
            header,
            base_address: 0,
            length: 0x0123456789abcdef,
            authentication_status: 0,
            extracted_fv: false.into(),
            fv_name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            file_name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
        }
    }

    // Generate a test resource descriptor hob
    // # Returns
    // A ResourceDescriptor hob
    pub(crate) fn gen_resource_descriptor() -> hob::ResourceDescriptor {
        let header = hob::header::Hob {
            r#type: hob::RESOURCE_DESCRIPTOR,
            length: size_of::<hob::ResourceDescriptor>() as u16,
            reserved: 0,
        };

        hob::ResourceDescriptor {
            header,
            owner: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
            physical_start: 0,
            resource_length: 0x0123456789abcdef,
        }
    }

    // Generate a test resource descriptor hob
    // # Returns
    // A ResourceDescriptor hob
    pub(crate) fn gen_resource_descriptor_v2() -> hob::ResourceDescriptorV2 {
        let mut v1 = gen_resource_descriptor();
        v1.header.r#type = hob::RESOURCE_DESCRIPTOR2;
        v1.header.length = size_of::<hob::ResourceDescriptorV2>() as u16;

        hob::ResourceDescriptorV2 { v1, attributes: 8 }
    }

    // Generate a test phase handoff information table hob
    // # Returns
    // A MemoryAllocation hob
    pub(crate) fn gen_memory_allocation() -> hob::MemoryAllocation {
        let header = hob::header::Hob {
            r#type: hob::MEMORY_ALLOCATION,
            length: size_of::<hob::MemoryAllocation>() as u16,
            reserved: 0,
        };

        let alloc_descriptor = hob::header::MemoryAllocation {
            name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            memory_base_address: 0,
            memory_length: 0x0123456789abcdef,
            memory_type: 0,
            reserved: [0; 4],
        };

        hob::MemoryAllocation { header, alloc_descriptor }
    }

    pub(crate) fn gen_memory_allocation_module() -> hob::MemoryAllocationModule {
        let header = hob::header::Hob {
            r#type: hob::MEMORY_ALLOCATION,
            length: size_of::<hob::MemoryAllocationModule>() as u16,
            reserved: 0,
        };

        let alloc_descriptor = hob::header::MemoryAllocation {
            name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            memory_base_address: 0,
            memory_length: 0x0123456789abcdef,
            memory_type: 0,
            reserved: [0; 4],
        };

        hob::MemoryAllocationModule {
            header,
            alloc_descriptor,
            module_name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            entry_point: 0,
        }
    }

    pub(crate) fn gen_capsule() -> hob::Capsule {
        let header =
            hob::header::Hob { r#type: hob::UEFI_CAPSULE, length: size_of::<hob::Capsule>() as u16, reserved: 0 };

        hob::Capsule { header, base_address: 0, length: 0x12 }
    }

    /// Generates a test GUID HOB in a contiguous heap buffer.
    ///
    /// A GUID HOB is laid out as [GuidHob header | data bytes] contiguously in memory. The header's
    /// `length` field covers both the struct and the trailing data. This function replicates that layout
    /// so `HobTrait::as_ptr()` + `size()` correctly spans the entire HOB.
    ///
    /// Use `guid_hob_refs()` to extract typed references from the returned buffer.
    pub(crate) fn gen_guid_hob() -> Vec<u8> {
        let data: &[u8] = &[1_u8, 2, 3, 4, 5, 6, 7, 8];
        let hob = hob::GuidHob {
            header: hob::header::Hob {
                r#type: hob::GUID_EXTENSION,
                length: (size_of::<hob::GuidHob>() + data.len()) as u16,
                reserved: 0,
            },
            name: crate::BinaryGuid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
        };

        // Build a contiguous buffer: [GuidHob struct bytes | data bytes]
        let mut buf = Vec::with_capacity(size_of::<hob::GuidHob>() + data.len());
        // SAFETY: Test code - serializing the GuidHob struct into raw bytes for contiguous layout.
        let hob_bytes = unsafe { from_raw_parts(&hob as *const hob::GuidHob as *const u8, size_of::<hob::GuidHob>()) };
        buf.extend_from_slice(hob_bytes);
        buf.extend_from_slice(data);
        buf
    }

    /// Extracts a `(&GuidHob, &[u8])` reference pair from a contiguous GUID HOB buffer.
    ///
    /// # Safety
    ///
    /// The buffer must have been produced by `gen_guid_hob()` and must outlive the returned references.
    pub(crate) fn guid_hob_refs(buf: &[u8]) -> (&hob::GuidHob, &[u8]) {
        assert!(buf.len() >= size_of::<hob::GuidHob>(), "Buffer too small for GuidHob");
        // SAFETY: Test code - the buffer was constructed by gen_guid_hob(), so the buffer layout matches.
        let guid_hob = unsafe { &*(buf.as_ptr() as *const hob::GuidHob) };
        let data = &buf[size_of::<hob::GuidHob>()..];
        (guid_hob, data)
    }

    pub(crate) fn gen_phase_handoff_information_table() -> hob::PhaseHandoffInformationTable {
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
    pub(crate) fn gen_end_of_hoblist() -> hob::PhaseHandoffInformationTable {
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

    pub(crate) fn gen_cpu() -> hob::Cpu {
        let header = hob::header::Hob { r#type: hob::CPU, length: size_of::<hob::Cpu>() as u16, reserved: 0 };

        hob::Cpu { header, size_of_memory_space: 0, size_of_io_space: 0, reserved: [0; 6] }
    }

    #[test]
    fn test_get_pi_hob_list_size_single_hob() {
        use core::ffi::c_void;

        let end_of_list = gen_end_of_hoblist();

        // SAFETY: The list is created in this test with a valid end-of-list marker
        let size = unsafe { get_pi_hob_list_size(&end_of_list as *const _ as *const c_void) };

        assert_eq!(size, size_of::<PhaseHandoffInformationTable>());
    }

    #[test]
    fn test_get_pi_hob_list_size_multiple_hobs() {
        use core::ffi::c_void;

        // Create a HOB list with multiple HOBs in contiguous memory
        let capsule = gen_capsule();
        let firmware_volume = gen_firmware_volume();
        let end_of_list = gen_end_of_hoblist();

        let expected_size =
            size_of::<Capsule>() + size_of::<FirmwareVolume>() + size_of::<PhaseHandoffInformationTable>();

        // This buffer will hold the contiguous HOBs
        let mut buffer = Vec::new();

        // Add a capsule HOB
        // SAFETY: Creating a byte slice from a struct for test purposes.
        let capsule_bytes =
            unsafe { core::slice::from_raw_parts(&capsule as *const Capsule as *const u8, size_of::<Capsule>()) };
        buffer.extend_from_slice(capsule_bytes);

        // Add a firmware volume HOB
        // SAFETY: Creating a byte slice from a struct for test purposes.
        let fv_bytes = unsafe {
            core::slice::from_raw_parts(
                &firmware_volume as *const FirmwareVolume as *const u8,
                size_of::<FirmwareVolume>(),
            )
        };
        buffer.extend_from_slice(fv_bytes);

        // Add an end-of-list HOB
        // SAFETY: Creating a byte slice from a struct for test purposes.
        let end_bytes = unsafe {
            core::slice::from_raw_parts(
                &end_of_list as *const PhaseHandoffInformationTable as *const u8,
                size_of::<PhaseHandoffInformationTable>(),
            )
        };
        buffer.extend_from_slice(end_bytes);

        // SAFETY: The list is created in this test with headers and an end-of-list marker that should be valid
        let size = unsafe { get_pi_hob_list_size(buffer.as_ptr() as *const c_void) };

        assert_eq!(size, expected_size);
    }

    #[test]
    fn test_get_pi_hob_list_size_varied_hob_types() {
        use core::ffi::c_void;

        // Create a HOB list with various HOB types
        let cpu = gen_cpu();
        let resource = gen_resource_descriptor();
        let memory_alloc = gen_memory_allocation();
        let end_of_list = gen_end_of_hoblist();

        let expected_size = size_of::<Cpu>()
            + size_of::<ResourceDescriptor>()
            + size_of::<MemoryAllocation>()
            + size_of::<PhaseHandoffInformationTable>();

        // This buffer will hold the contiguous HOBs
        let mut buffer = Vec::new();

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(&cpu as *const Cpu as *const u8, size_of::<Cpu>())
        });

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &resource as *const ResourceDescriptor as *const u8,
                size_of::<ResourceDescriptor>(),
            )
        });

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &memory_alloc as *const MemoryAllocation as *const u8,
                size_of::<MemoryAllocation>(),
            )
        });

        // SAFETY: Creating a byte slice from a struct for test purposes.
        buffer.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &end_of_list as *const PhaseHandoffInformationTable as *const u8,
                size_of::<PhaseHandoffInformationTable>(),
            )
        });

        // SAFETY: The list is created in this test with headers and an end-of-list marker that should be valid
        let size = unsafe { get_pi_hob_list_size(buffer.as_ptr() as *const c_void) };

        assert_eq!(size, expected_size);
    }
}
