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

/// Type alias for section type identifiers
pub type EfiSectionType = u8;

/// Firmware File System Leaf Section Types
/// Note: Typically called `EFI_SECTION_*` in EDK II code.
pub mod raw_type {
    /// Pseudo type. It is used as a wild card when retrieving sections to match all types.
    pub const ALL: u8 = 0x00;
    /// Encapsulated section type constants
    pub mod encapsulated {
        /// Compression encapsulated section
        pub const COMPRESSION: u8 = 0x01;
        /// GUID-defined encapsulated section
        pub const GUID_DEFINED: u8 = 0x02;
        /// Disposable encapsulated section
        pub const DISPOSABLE: u8 = 0x03;
    }
    /// PE32 executable section
    pub const PE32: u8 = 0x10;
    /// Position-independent code section
    pub const PIC: u8 = 0x11;
    /// Terse executable section
    pub const TE: u8 = 0x12;
    /// DXE dependency expression section
    pub const DXE_DEPEX: u8 = 0x13;
    /// Version information section
    pub const VERSION: u8 = 0x14;
    /// User interface string section
    pub const USER_INTERFACE: u8 = 0x15;
    /// Compatibility16 section
    pub const COMPATIBILITY16: u8 = 0x16;
    /// Firmware volume image section
    pub const FIRMWARE_VOLUME_IMAGE: u8 = 0x17;
    /// Freeform GUID subtype section
    pub const FREEFORM_SUBTYPE_GUID: u8 = 0x18;
    /// Raw data section
    pub const RAW: u8 = 0x19;
    /// PEI dependency expression section
    pub const PEI_DEPEX: u8 = 0x1B;
    /// MM dependency expression section
    pub const MM_DEPEX: u8 = 0x1C;
    /// OEM-defined section type minimum
    pub const OEM_MIN: u8 = 0xC0;
    /// OEM-defined section type maximum
    pub const OEM_MAX: u8 = 0xDF;
    /// Debug section type minimum
    pub const DEBUG_MIN: u8 = 0xE0;
    /// Debug section type maximum
    pub const DEBUG_MAX: u8 = 0xEF;
    /// FFS-defined section type minimum
    pub const FFS_MIN: u8 = 0xF0;
    /// FFS pad section type
    pub const FFS_PAD: u8 = 0xF0;
    /// FFS-defined section type maximum
    pub const FFS_MAX: u8 = 0xFF;
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[cfg_attr(test, derive(serde::Deserialize))]
/// Section type enumeration for firmware file sections
pub enum Type {
    /// All section types
    All = raw_type::ALL,
    /// Compression section
    Compression = raw_type::encapsulated::COMPRESSION,
    /// GUID-defined section
    GuidDefined = raw_type::encapsulated::GUID_DEFINED,
    /// Disposable section
    Disposable = raw_type::encapsulated::DISPOSABLE,
    /// PE32 executable
    Pe32 = raw_type::PE32,
    /// Position-independent code
    Pic = raw_type::PIC,
    /// Terse executable
    Te = raw_type::TE,
    /// DXE dependency expression
    DxeDepex = raw_type::DXE_DEPEX,
    /// Version information
    Version = raw_type::VERSION,
    /// User interface string
    UserInterface = raw_type::USER_INTERFACE,
    /// Compatibility16 binary
    Compatibility16 = raw_type::COMPATIBILITY16,
    /// Firmware volume image
    FirmwareVolumeImage = raw_type::FIRMWARE_VOLUME_IMAGE,
    /// Freeform GUID subtype
    FreeformSubtypeGuid = raw_type::FREEFORM_SUBTYPE_GUID,
    /// Raw data
    Raw = raw_type::RAW,
    /// PEI dependency expression
    PeiDepex = raw_type::PEI_DEPEX,
    /// MM dependency expression
    MmDepex = raw_type::MM_DEPEX,
}

/// EFI_COMMON_SECTION_HEADER per PI spec 1.8A 3.2.4.1
#[repr(C)]
#[derive(Debug)]
pub struct Header {
    /// Section size (24-bit)
    pub size: [u8; 3],
    /// Section type identifier
    pub section_type: u8,
}

/// Section header structures and definitions
pub mod header {
    use r_efi::efi;

    #[repr(C)]
    #[derive(Debug)]
    /// Standard common section header for sections up to 16MB
    pub struct CommonSectionHeaderStandard {
        /// Section size (24-bit)
        pub size: [u8; 3],
        /// Section type identifier
        pub section_type: u8,
    }

    /// EFI_COMMON_SECTION_HEADER2 per PI spec 1.8A 3.2.4.1
    #[repr(C)]
    #[derive(Debug)]
    /// Extended common section header for sections larger than 16MB
    pub struct CommonSectionHeaderExtended {
        /// Section size (24-bit)
        pub size: [u8; 3],
        /// Section type identifier
        pub section_type: u8,
        /// Extended 32-bit section size
        pub extended_size: u32,
    }

    /// EFI_COMPRESSION_SECTION per PI spec 1.8A 3.2.5.2
    #[repr(C, packed)]
    #[derive(Debug, Clone, Copy)]
    /// Compression section header
    pub struct Compression {
        /// Uncompressed data length
        pub uncompressed_length: u32,
        /// Compression algorithm type
        pub compression_type: u8,
    }
    /// No compression applied
    pub const NOT_COMPRESSED: u8 = 0x00;
    /// Standard compression (typically LZMA)
    pub const STANDARD_COMPRESSION: u8 = 0x01;

    /// EFI_GUID_DEFINED_SECTION per PI spec 1.8A 3.2.5.7
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    /// GUID-defined section header
    pub struct GuidDefined {
        /// GUID identifying the section format
        pub section_definition_guid: efi::Guid,
        /// Offset to section data from start of header
        pub data_offset: u16,
        /// Section attributes
        pub attributes: u16,
        // Guid-specific header fields.
    }

    /// EFI_VERSION_SECTION per PI spec 1.8A 3.2.5.15
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    /// Version section header
    pub struct Version {
        /// Build number
        pub build_number: u16,
    }

    /// EFI_FREEFORM_SUBTYPE_GUID_SECTION per PI spec 1.8A 3.2.5.6
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    /// Freeform GUID subtype section header
    pub struct FreeformSubtypeGuid {
        /// Subtype GUID identifier
        pub sub_type_guid: efi::Guid,
    }
}
