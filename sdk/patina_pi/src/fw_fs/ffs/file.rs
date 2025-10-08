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

pub mod raw {
    /// File State Bits
    pub mod state {
        pub const HEADER_CONSTRUCTION: u8 = 0x01;
        pub const HEADER_VALID: u8 = 0x02;
        pub const DATA_VALID: u8 = 0x04;
        pub const MARKED_FOR_UPDATE: u8 = 0x08;
        pub const DELETED: u8 = 0x10;
        pub const HEADER_INVALID: u8 = 0x20;
    }

    /// File Type Definitions
    pub mod r#type {
        pub const ALL: u8 = 0x00;
        pub const RAW: u8 = 0x01;
        pub const FREEFORM: u8 = 0x02;
        pub const SECURITY_CORE: u8 = 0x03;
        pub const PEI_CORE: u8 = 0x04;
        pub const DXE_CORE: u8 = 0x05;
        pub const PEIM: u8 = 0x06;
        pub const DRIVER: u8 = 0x07;
        pub const COMBINED_PEIM_DRIVER: u8 = 0x08;
        pub const APPLICATION: u8 = 0x09;
        pub const MM: u8 = 0x0A;
        pub const FIRMWARE_VOLUME_IMAGE: u8 = 0x0B;
        pub const COMBINED_MM_DXE: u8 = 0x0C;
        pub const MM_CORE: u8 = 0x0D;
        pub const MM_STANDALONE: u8 = 0x0E;
        pub const MM_CORE_STANDALONE: u8 = 0x0F;
        pub const OEM_MIN: u8 = 0xc0;
        pub const OEM_MAX: u8 = 0xdf;
        pub const DEBUG_MIN: u8 = 0xe0;
        pub const DEBUG_MAX: u8 = 0xef;
        pub const FFS_MIN: u8 = 0xf1;
        pub const FFS_MAX: u8 = 0xff;
        pub const FFS_PAD: u8 = 0xf0;
    }
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Type {
    All = raw::r#type::ALL,
    Raw = raw::r#type::RAW,
    FreeForm = raw::r#type::FREEFORM,
    SecurityCore = raw::r#type::SECURITY_CORE,
    PeiCore = raw::r#type::PEI_CORE,
    DxeCore = raw::r#type::DXE_CORE,
    Peim = raw::r#type::PEIM,
    Driver = raw::r#type::DRIVER,
    CombinedPeimDriver = raw::r#type::COMBINED_PEIM_DRIVER,
    Application = raw::r#type::APPLICATION,
    Mm = raw::r#type::MM,
    FirmwareVolumeImage = raw::r#type::FIRMWARE_VOLUME_IMAGE,
    CombinedMmDxe = raw::r#type::COMBINED_MM_DXE,
    MmCore = raw::r#type::MM_CORE,
    MmStandalone = raw::r#type::MM_STANDALONE,
    MmCoreStandalone = raw::r#type::MM_CORE_STANDALONE,
    OemMin = raw::r#type::OEM_MIN,
    OemMax = raw::r#type::OEM_MAX,
    DebugMin = raw::r#type::DEBUG_MIN,
    DebugMax = raw::r#type::DEBUG_MAX,
    FfsPad = raw::r#type::FFS_PAD,
    FfsUnknown = raw::r#type::FFS_MIN,
    FfsMax = raw::r#type::FFS_MAX,
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum State {
    HeaderConstruction = raw::state::HEADER_CONSTRUCTION,
    HeaderValid = raw::state::HEADER_VALID,
    DataValid = raw::state::DATA_VALID,
    MarkedForUpdate = raw::state::MARKED_FOR_UPDATE,
    Deleted = raw::state::DELETED,
    HeaderInvalid = raw::state::HEADER_INVALID,
}

// EFI_FFS_FILE_HEADER
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub name: efi::Guid,
    pub integrity_check_header: u8,
    pub integrity_check_file: u8,
    pub file_type: u8,
    pub attributes: u8,
    pub size: [u8; 3],
    pub state: u8,
}

// EFI_FFS_FILE_HEADER
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Header2 {
    pub header: Header,
    pub extended_size: u64,
}
