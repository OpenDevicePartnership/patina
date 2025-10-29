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

/// Type alias for firmware volume file attributes as defined in the PI Specification
pub type EfiFvFileAttributes = u32;

/// Raw constant definitions for firmware volume file attributes
pub mod raw {
    /// Firmware File Volume File Attributes
    /// Note: Typically named `EFI_FV_FILE_ATTRIB_*` in EDK II code.
    /// Firmware volume file attribute constants
    pub mod attribute {
        /// File alignment requirement mask
        pub const ALIGNMENT: u32 = 0x0000001F;
        /// File must be loaded at a fixed address
        pub const FIXED: u32 = 0x00000100;
        /// File can be memory-mapped
        pub const MEMORY_MAPPED: u32 = 0x00000200;
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
/// Firmware volume file attribute enumeration
pub enum Attribute {
    /// Alignment requirement attribute
    Alignment = raw::attribute::ALIGNMENT,
    /// Fixed address loading attribute
    Fixed = raw::attribute::FIXED,
    /// Memory-mapped file attribute
    MemoryMapped = raw::attribute::MEMORY_MAPPED,
}
