//! Module for a composite of brotli, uefi, and crc32 decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::boxed::Box;
use mu_pi::fw_fs::SectionExtractor;

use crate::{BrotliSectionExtractor, Crc32SectionExtractor, UefiDecompressSectionExtractor};

/// Provides a composite section extractor that combines all section extractors based on enabled feature flags.
#[derive(Clone, Copy)]
pub struct CompositeSectionExtractor {
    #[cfg(feature = "uefi_decompress")]
    uefi_decompress: UefiDecompressSectionExtractor,
    #[cfg(feature = "brotli")]
    brotli: BrotliSectionExtractor,
    #[cfg(feature = "crc32")]
    crc32: Crc32SectionExtractor,
}

impl Default for CompositeSectionExtractor {
    fn default() -> Self {
        Self {
            #[cfg(feature = "uefi_decompress")]
            uefi_decompress: UefiDecompressSectionExtractor {},
            #[cfg(feature = "brotli")]
            brotli: BrotliSectionExtractor {},
            #[cfg(feature = "crc32")]
            crc32: Crc32SectionExtractor {},
        }
    }
}

impl SectionExtractor for CompositeSectionExtractor {
    fn extract(&self, section: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::efi::Status> {
        #[cfg(feature = "uefi_decompress")]
        {
            match self.uefi_decompress.extract(section) {
                Err(err) => return Err(err),
                Ok(buffer) => {
                    if buffer.len() > 0 {
                        return Ok(buffer);
                    }
                }
            }
        }
        #[cfg(feature = "brotli")]
        {
            match self.brotli.extract(section) {
                Err(err) => return Err(err),
                Ok(buffer) => {
                    if buffer.len() > 0 {
                        return Ok(buffer);
                    }
                }
            }
        }
        #[cfg(feature = "crc32")]
        {
            match self.crc32.extract(section) {
                Err(err) => return Err(err),
                Ok(buffer) => {
                    if buffer.len() > 0 {
                        return Ok(buffer);
                    }
                }
            }
        }
        Ok(Box::new([0u8; 0]))
    }
}
