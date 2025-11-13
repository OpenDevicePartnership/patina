//! Module for a null section extractor that always returns Unsupported.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::vec::Vec;
use patina_ffs::{FirmwareFileSystemError, section::SectionExtractor};

/// A section extractor that always returns Unsupported.
#[derive(Default, Clone, Copy)]
pub struct NullSectionExtractor;

impl NullSectionExtractor {
    /// Creates a new `NullSectionExtractor` instance.
    #[coverage(off)]
    pub const fn new() -> Self {
        Self {}
    }
}

impl SectionExtractor for NullSectionExtractor {
    #[coverage(off)]
    fn extract(&self, _: &patina_ffs::section::Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
        Err(FirmwareFileSystemError::Unsupported)
    }
}
