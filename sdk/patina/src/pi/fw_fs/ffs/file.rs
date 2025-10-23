//! Firmware File System (FFS) File Definitions
//!
//! Based on the values defined in the UEFI Platform Initialization (PI) Specification V1.8A Section 3.2.3.1
//! EFI_FFS_FILE_HEADER.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

/// Raw FFS file constant definitions
pub mod raw {
    /// File State Bits
    pub mod state {
        /// File header is under construction
        pub const HEADER_CONSTRUCTION: u8 = 0x01;
        /// File header is valid
        pub const HEADER_VALID: u8 = 0x02;
        /// File data is valid
        pub const DATA_VALID: u8 = 0x04;
        /// File is marked for update
        pub const MARKED_FOR_UPDATE: u8 = 0x08;
        /// File has been deleted
        pub const DELETED: u8 = 0x10;
        /// File header is invalid
        pub const HEADER_INVALID: u8 = 0x20;
    }

    /// File Type Definitions
    pub mod r#type {
        /// All file types
        pub const ALL: u8 = 0x00;
        /// Raw data file
        pub const RAW: u8 = 0x01;
        /// Freeform file
        pub const FREEFORM: u8 = 0x02;
        /// Security (SEC) core file
        pub const SECURITY_CORE: u8 = 0x03;
        /// PEI core file
        pub const PEI_CORE: u8 = 0x04;
        /// DXE core file
        pub const DXE_CORE: u8 = 0x05;
        /// Pre-EFI module (PEIM) file
        pub const PEIM: u8 = 0x06;
        /// Driver Execution Environment (DXE) driver file
        pub const DRIVER: u8 = 0x07;
        /// Combined PEIM and driver file
        pub const COMBINED_PEIM_DRIVER: u8 = 0x08;
        /// Application file
        pub const APPLICATION: u8 = 0x09;
        /// Management Mode (MM) file
        pub const MM: u8 = 0x0A;
        /// Firmware volume image file
        pub const FIRMWARE_VOLUME_IMAGE: u8 = 0x0B;
        /// Combined MM and DXE file
        pub const COMBINED_MM_DXE: u8 = 0x0C;
        /// MM core file
        pub const MM_CORE: u8 = 0x0D;
        /// MM standalone module file
        pub const MM_STANDALONE: u8 = 0x0E;
        /// MM standalone core file
        pub const MM_CORE_STANDALONE: u8 = 0x0F;
        /// OEM-defined file type minimum value
        pub const OEM_MIN: u8 = 0xc0;
        /// OEM-defined file type maximum value
        pub const OEM_MAX: u8 = 0xdf;
        /// Debug file type minimum value
        pub const DEBUG_MIN: u8 = 0xe0;
        /// Debug file type maximum value
        pub const DEBUG_MAX: u8 = 0xef;
        /// FFS-defined file type minimum value
        pub const FFS_MIN: u8 = 0xf1;
        /// FFS-defined file type maximum value
        pub const FFS_MAX: u8 = 0xff;
        /// FFS pad file type
        pub const FFS_PAD: u8 = 0xf0;
    }
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
/// Firmware file type enumeration
pub enum Type {
    /// All file types
    All = raw::r#type::ALL,
    /// Raw file type
    Raw = raw::r#type::RAW,
    /// Free form file
    FreeForm = raw::r#type::FREEFORM,
    /// Security core file
    SecurityCore = raw::r#type::SECURITY_CORE,
    /// PEI core file
    PeiCore = raw::r#type::PEI_CORE,
    /// DXE core file
    DxeCore = raw::r#type::DXE_CORE,
    /// PEI module file
    Peim = raw::r#type::PEIM,
    /// Driver file
    Driver = raw::r#type::DRIVER,
    /// Combined PEIM driver file
    CombinedPeimDriver = raw::r#type::COMBINED_PEIM_DRIVER,
    /// Application file
    Application = raw::r#type::APPLICATION,
    /// Traditional Management Mode (MM) file
    Mm = raw::r#type::MM,
    /// Firmware volume image file
    FirmwareVolumeImage = raw::r#type::FIRMWARE_VOLUME_IMAGE,
    /// Combined MM/DXE file
    CombinedMmDxe = raw::r#type::COMBINED_MM_DXE,
    /// Traditional Management Mode (MM) core file
    MmCore = raw::r#type::MM_CORE,
    /// Standalone MM driver file
    MmStandalone = raw::r#type::MM_STANDALONE,
    /// Standalone MM Core file
    MmCoreStandalone = raw::r#type::MM_CORE_STANDALONE,
    /// Begininning of the OEM file type range
    OemMin = raw::r#type::OEM_MIN,
    /// Max for the OEM file type range
    OemMax = raw::r#type::OEM_MAX,
    /// Beginning of the debug file type range
    DebugMin = raw::r#type::DEBUG_MIN,
    /// Max for the debug file type range
    DebugMax = raw::r#type::DEBUG_MAX,
    /// A FFS padding file
    FfsPad = raw::r#type::FFS_PAD,
    /// An unknown file
    FfsUnknown = raw::r#type::FFS_MIN,
    /// Max file type value
    FfsMax = raw::r#type::FFS_MAX,
}

/// Firmware File State
///
/// Represents the current state of a firmware file in the Firmware File System.
/// The state tracks the file's validity and lifecycle through various phases of
/// construction, usage, and deletion. Based on PI Specification Volume 3, Section 3.2.3.1.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum State {
    /// File header is under construction and not yet valid
    HeaderConstruction = raw::state::HEADER_CONSTRUCTION,
    /// File header has been constructed and is valid
    HeaderValid = raw::state::HEADER_VALID,
    /// File data has been written and is valid
    DataValid = raw::state::DATA_VALID,
    /// File is marked for update in a future firmware update operation
    MarkedForUpdate = raw::state::MARKED_FOR_UPDATE,
    /// File has been deleted and should not be processed
    Deleted = raw::state::DELETED,
    /// File header is invalid (checksum mismatch or corruption)
    HeaderInvalid = raw::state::HEADER_INVALID,
}

// EFI_FFS_FILE_HEADER
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Firmware file header structure per PI Specification
pub struct Header {
    /// Unique file GUID identifier
    pub name: efi::Guid,
    /// Header checksum value
    pub integrity_check_header: u8,
    /// File checksum value
    pub integrity_check_file: u8,
    /// Type of file (see file type constants)
    pub file_type: u8,
    /// File attributes
    pub attributes: u8,
    /// 24-bit file size in bytes
    pub size: [u8; 3],
    /// File state (see state constants)
    pub state: u8,
}

// EFI_FFS_FILE_HEADER
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
/// Extended firmware file header structure (version 2) for files larger than 16MB
pub struct Header2 {
    /// Standard file header
    pub header: Header,
    /// Extended 64-bit file size for large files
    pub extended_size: u64,
}
