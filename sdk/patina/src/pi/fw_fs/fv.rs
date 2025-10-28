//! Firmware Volume (FV) Definitions and Support Code
//!
//! Based on the values defined in the UEFI Platform Initialization (PI) Specification V1.8A 3.1 Firmware Storage
//! Code Definitions.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod attributes;
pub mod file;

/// Type alias for firmware volume file types
pub type EfiFvFileType = u8;

/// Firmware File System revision number
pub const FFS_REVISION: u8 = 2;
/// Maximum file size for FFS version 2 (16MB)
pub const FFS_V2_MAX_FILE_SIZE: usize = 0x1000000;

/// Firmware Volume Write Policy bit definitions
/// Note: Typically named `EFI_FV_*` in EDK II code.
mod raw {
    pub(super) mod write_policy {
        pub const UNRELIABLE_WRITE: u32 = 0x00000000;
        pub const RELIABLE_WRITE: u32 = 0x00000001;
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
/// Firmware volume write policy enumeration
pub enum WritePolicy {
    /// Unreliable write - no guarantees on power loss
    UnreliableWrite = raw::write_policy::UNRELIABLE_WRITE,
    /// Reliable write - atomic on power loss
    ReliableWrite = raw::write_policy::RELIABLE_WRITE,
}

/// EFI_FIRMWARE_VOLUME_HEADER
#[repr(C)]
#[derive(Debug, Clone, Copy)]
/// Firmware volume header structure per PI Specification
pub struct Header {
    /// First 16 bytes are zeros for compatibility
    pub zero_vector: [u8; 16],
    /// File system type GUID
    pub file_system_guid: r_efi::efi::Guid,
    /// Total volume length in bytes
    pub fv_length: u64,
    /// Firmware volume signature
    pub signature: u32,
    /// Volume attributes
    pub attributes: u32,
    /// Length of this header structure
    pub header_length: u16,
    /// Header checksum
    pub checksum: u16,
    /// Offset to extended header (0 if none)
    pub ext_header_offset: u16,
    /// Reserved byte (must be 0)
    pub reserved: u8,
    /// Header revision number
    pub revision: u8,
    /// Variable-length block map array
    pub block_map: [BlockMapEntry; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Firmware volume block map entry describing physical layout
pub struct BlockMapEntry {
    /// Number of blocks of this size
    pub num_blocks: u32,
    /// Length of each block
    pub length: u32,
}

/// EFI_FIRMWARE_VOLUME_EXT_HEADER
#[repr(C)]
#[derive(Debug, Clone, Copy)]
/// Extended firmware volume header
pub struct ExtHeader {
    /// Firmware volume name GUID
    pub fv_name: r_efi::efi::Guid,
    /// Size of this extended header
    pub ext_header_size: u32,
}
