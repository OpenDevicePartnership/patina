//! Module for for a NULL implementation of the section extractor.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use patina_ffs::{
    section::{Section, SectionComposer, SectionExtractor, SectionMetaData},
    FirmwareFileSystemError,
};

/// A section extractor implementation that does no decompression.
#[derive(Default, Clone, Copy)]
pub struct NullSectionProcessor;
impl SectionExtractor for NullSectionProcessor {
    fn extract(&self, _section: &Section) -> Result<alloc::vec::Vec<u8>, FirmwareFileSystemError> {
        Err(FirmwareFileSystemError::Unsupported)
    }
}

impl SectionComposer for NullSectionProcessor {
    fn compose(&self, _section: &Section) -> Result<(SectionMetaData, alloc::vec::Vec<u8>), FirmwareFileSystemError> {
        Err(FirmwareFileSystemError::Unsupported)
    }
}
