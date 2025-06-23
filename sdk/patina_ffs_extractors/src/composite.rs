//! Module for a composite of brotli, uefi, and crc32 decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use patina_ffs::{FirmwareFileSystemError, section::{Section, SectionExtractor}};

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
    fn extract(&self, section: &Section) -> Result<alloc::vec::Vec<u8>, FirmwareFileSystemError> {
        #[cfg(feature = "uefi_decompress")]
        {
            match self.uefi_decompress.extract(section) {
                Err(FirmwareFileSystemError::Unsupported) => (),
                Err(err) => return Err(err),
                Ok(buffer) => return Ok(buffer),
            }
        }

        #[cfg(feature = "brotli")]
        {
            match self.brotli.extract(section) {
                Err(FirmwareFileSystemError::Unsupported) => (),
                Err(err) => return Err(err),
                Ok(buffer) => return Ok(buffer),
            }
        }

        #[cfg(feature = "crc32")]
        {
            match self.crc32.extract(section) {
                Err(FirmwareFileSystemError::Unsupported) => (),
                Err(err) => return Err(err),
                Ok(buffer) => return Ok(buffer),
            }
        }

        Err(FirmwareFileSystemError::Unsupported)
    }
}
