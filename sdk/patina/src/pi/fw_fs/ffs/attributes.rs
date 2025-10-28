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

/// Raw FFS attribute constant definitions
pub mod raw {
    /// Large file attribute
    pub const LARGE_FILE: u8 = 0x01;
    /// 2-byte data alignment
    pub const DATA_ALIGNMENT_2: u8 = 0x02;
    /// File must be at a fixed address
    pub const FIXED: u8 = 0x04;
    /// Data alignment mask
    pub const DATA_ALIGNMENT: u8 = 0x38;
    /// File checksum attribute
    pub const CHECKSUM: u8 = 0x40;
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
/// FFS file attribute enumeration
pub enum Attribute {
    /// Large file variant
    LargeFile = raw::LARGE_FILE,
    /// 2-byte alignment variant
    DataAlignment2 = raw::DATA_ALIGNMENT_2,
    /// Fixed address file
    Fixed = raw::FIXED,
    /// Data alignment variant
    DataAlignment = raw::DATA_ALIGNMENT,
    /// Checksum variant
    Checksum = raw::CHECKSUM,
}
