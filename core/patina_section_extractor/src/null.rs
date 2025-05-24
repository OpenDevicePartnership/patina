//! Module for for a NULL implementation of the section extractor.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::boxed::Box;
use mu_pi::fw_fs::SectionExtractor;
use r_efi::efi;

/// A section extractor implementation that does no decompression.
#[derive(Default, Clone, Copy)]
pub struct NullSectionExtractor;
impl SectionExtractor for NullSectionExtractor {
    fn extract(&self, _section: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, efi::Status> {
        Ok(Box::new([0u8; 0]))
    }
}
