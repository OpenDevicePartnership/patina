//! Firmware File System (FFS) File Attribute Definitions
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

pub mod raw {
    pub const LARGE_FILE: u8 = 0x01;
    pub const DATA_ALIGNMENT_2: u8 = 0x02;
    pub const FIXED: u8 = 0x04;
    pub const DATA_ALIGNMENT: u8 = 0x38;
    pub const CHECKSUM: u8 = 0x40;
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Attribute {
    LargeFile = raw::LARGE_FILE,
    DataAlignment2 = raw::DATA_ALIGNMENT_2,
    Fixed = raw::FIXED,
    DataAlignment = raw::DATA_ALIGNMENT,
    Checksum = raw::CHECKSUM,
}
