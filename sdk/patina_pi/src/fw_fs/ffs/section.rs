//! Firmware File System (FFS) Section Definition
//!
//! Based on the values defined in the UEFI Platform Initialization (PI) Specification V1.8A Section 3.2.4
//! Firmware File Section.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub type EfiSectionType = u8;

/// Firmware File System Leaf Section Types
/// Note: Typically called `EFI_SECTION_*` in EDK II code.
pub mod raw_type {
    /// Pseudo type. It is used as a wild card when retrieving sections to match all types.
    pub const ALL: u8 = 0x00;

    pub mod encapsulated {
        pub const COMPRESSION: u8 = 0x01;
        pub const GUID_DEFINED: u8 = 0x02;
        pub const DISPOSABLE: u8 = 0x03;
    }

    pub const PE32: u8 = 0x10;
    pub const PIC: u8 = 0x11;
    pub const TE: u8 = 0x12;
    pub const DXE_DEPEX: u8 = 0x13;
    pub const VERSION: u8 = 0x14;
    pub const USER_INTERFACE: u8 = 0x15;
    pub const COMPATIBILITY16: u8 = 0x16;
    pub const FIRMWARE_VOLUME_IMAGE: u8 = 0x17;
    pub const FREEFORM_SUBTYPE_GUID: u8 = 0x18;
    pub const RAW: u8 = 0x19;
    pub const PEI_DEPEX: u8 = 0x1B;
    pub const MM_DEPEX: u8 = 0x1C;
    pub const OEM_MIN: u8 = 0xC0;
    pub const OEM_MAX: u8 = 0xDF;
    pub const DEBUG_MIN: u8 = 0xE0;
    pub const DEBUG_MAX: u8 = 0xEF;
    pub const FFS_MIN: u8 = 0xF0;
    pub const FFS_PAD: u8 = 0xF0;
    pub const FFS_MAX: u8 = 0xFF;
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub enum Type {
    All = raw_type::ALL,
    Compression = raw_type::encapsulated::COMPRESSION,
    GuidDefined = raw_type::encapsulated::GUID_DEFINED,
    Disposable = raw_type::encapsulated::DISPOSABLE,
    Pe32 = raw_type::PE32,
    Pic = raw_type::PIC,
    Te = raw_type::TE,
    DxeDepex = raw_type::DXE_DEPEX,
    Version = raw_type::VERSION,
    UserInterface = raw_type::USER_INTERFACE,
    Compatibility16 = raw_type::COMPATIBILITY16,
    FirmwareVolumeImage = raw_type::FIRMWARE_VOLUME_IMAGE,
    FreeformSubtypeGuid = raw_type::FREEFORM_SUBTYPE_GUID,
    Raw = raw_type::RAW,
    PeiDepex = raw_type::PEI_DEPEX,
    MmDepex = raw_type::MM_DEPEX,
}

/// EFI_COMMON_SECTION_HEADER per PI spec 1.8A 3.2.4.1
#[repr(C)]
#[derive(Debug)]
pub struct Header {
    pub size: [u8; 3],
    pub section_type: u8,
}

pub mod header {
    use r_efi::base::Guid;

    #[repr(C)]
    #[derive(Debug)]
    pub struct CommonSectionHeaderStandard {
        pub size: [u8; 3],
        pub section_type: u8,
    }

    /// EFI_COMMON_SECTION_HEADER2 per PI spec 1.8A 3.2.4.1
    #[repr(C)]
    #[derive(Debug)]
    pub struct CommonSectionHeaderExtended {
        pub size: [u8; 3],
        pub section_type: u8,
        pub extended_size: u32,
    }

    /// EFI_COMPRESSION_SECTION per PI spec 1.8A 3.2.5.2
    #[repr(C, packed)]
    #[derive(Debug, Clone, Copy)]
    pub struct Compression {
        pub uncompressed_length: u32,
        pub compression_type: u8,
    }
    pub const NOT_COMPRESSED: u8 = 0x00;
    pub const STANDARD_COMPRESSION: u8 = 0x01;

    /// EFI_GUID_DEFINED_SECTION per PI spec 1.8A 3.2.5.7
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct GuidDefined {
        pub section_definition_guid: Guid,
        pub data_offset: u16,
        pub attributes: u16,
        // Guid-specific header fields.
    }

    /// EFI_VERSION_SECTION per PI spec 1.8A 3.2.5.15
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct Version {
        pub build_number: u16,
    }

    /// EFI_FREEFORM_SUBTYPE_GUID_SECTION per PI spec 1.8A 3.2.5.6
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct FreeformSubtypeGuid {
        pub sub_type_guid: Guid,
    }
}
