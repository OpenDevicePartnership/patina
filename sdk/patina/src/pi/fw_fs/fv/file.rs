//! Firmware Volume File Definitions
//!
//! Based on the bindings and definitions in the UEFI Platform Initialization (PI)
//! Specification V1.8A 3.1 Firmware Storage Code Definitions.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub type EfiFvFileAttributes = u32;

pub mod raw {
    /// Firmware File Volume File Attributes
    /// Note: Typically named `EFI_FV_FILE_ATTRIB_*` in EDK II code.
    pub mod attribute {
        pub const ALIGNMENT: u32 = 0x0000001F;
        pub const FIXED: u32 = 0x00000100;
        pub const MEMORY_MAPPED: u32 = 0x00000200;
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Attribute {
    Alignment = raw::attribute::ALIGNMENT,
    Fixed = raw::attribute::FIXED,
    MemoryMapped = raw::attribute::MEMORY_MAPPED,
}
