//! Module for for a NULL implementation of the section extractor.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina_ffs::{
    FirmwareFileSystemError,
    section::{Section, SectionComposer, SectionExtractor, SectionHeader},
};

/// A section extractor/composer implementation that does no extraction or composition.
#[derive(Default, Clone, Copy)]
pub struct NullSectionProcessor;
impl SectionExtractor for NullSectionProcessor {
    fn extract(&self, _section: &Section) -> Result<alloc::vec::Vec<u8>, FirmwareFileSystemError> {
        Err(FirmwareFileSystemError::Unsupported)
    }
}

impl SectionComposer for NullSectionProcessor {
    fn compose(&self, _section: &Section) -> Result<(SectionHeader, alloc::vec::Vec<u8>), FirmwareFileSystemError> {
        Err(FirmwareFileSystemError::Unsupported)
    }
}
