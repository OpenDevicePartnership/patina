//! Module for a composite of brotli, uefi, and crc32 decompression.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina_ffs::{
    FirmwareFileSystemError,
    section::{Section, SectionExtractor},
};

#[cfg(feature = "brotli")]
use crate::BrotliSectionExtractor;
#[cfg(feature = "crc32")]
use crate::Crc32SectionExtractor;
#[cfg(feature = "lzma")]
use crate::LzmaSectionExtractor;

/// Provides a composite section extractor that combines all section extractors based on enabled feature flags.
#[derive(Clone, Copy)]
pub struct CompositeSectionExtractor {
    #[cfg(feature = "brotli")]
    brotli: BrotliSectionExtractor,
    #[cfg(feature = "crc32")]
    crc32: Crc32SectionExtractor,
    #[cfg(feature = "lzma")]
    lzma: LzmaSectionExtractor,
}

impl Default for CompositeSectionExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeSectionExtractor {
    /// Creates a new instance of the composite section extractor.
    pub const fn new() -> Self {
        Self {
            #[cfg(feature = "brotli")]
            brotli: BrotliSectionExtractor {},
            #[cfg(feature = "crc32")]
            crc32: Crc32SectionExtractor {},
            #[cfg(feature = "lzma")]
            lzma: LzmaSectionExtractor {},
        }
    }
}

impl SectionExtractor for CompositeSectionExtractor {
    fn extract(&self, _section: &Section) -> Result<alloc::vec::Vec<u8>, FirmwareFileSystemError> {
        #[cfg(feature = "brotli")]
        {
            match self.brotli.extract(_section) {
                Err(FirmwareFileSystemError::Unsupported) => (),
                Err(err) => return Err(err),
                Ok(buffer) => return Ok(buffer),
            }
        }

        #[cfg(feature = "crc32")]
        {
            match self.crc32.extract(_section) {
                Err(FirmwareFileSystemError::Unsupported) => (),
                Err(err) => return Err(err),
                Ok(buffer) => return Ok(buffer),
            }
        }

        #[cfg(feature = "lzma")]
        {
            match self.lzma.extract(_section) {
                Err(FirmwareFileSystemError::Unsupported) => (),
                Err(err) => return Err(err),
                Ok(buffer) => return Ok(buffer),
            }
        }

        Err(FirmwareFileSystemError::Unsupported)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "crc32")]
    fn test_composite_extracts_crc32() {
        use crate::tests::create_crc32_section;

        let content = b"Test CRC32 content";
        let crc32 = crc32fast::hash(content);
        let section = create_crc32_section(content, crc32.to_le_bytes().to_vec());

        let extractor = CompositeSectionExtractor::default();
        let result = extractor.extract(&section).expect("Should extract CRC32 section");

        assert_eq!(result, content);
    }

    #[test]
    #[cfg(feature = "brotli")]
    fn test_composite_extracts_brotli() {
        // Pre-compressed "Hello, World!" using Brotli

        use crate::tests::create_brotli_section;
        let brotli_compressed_data: [u8; 18] = [
            0x21, 0x30, 0x00, 0x04, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x2C, 0x20, 0x57, 0x6F, 0x72, 0x6C, 0x64, 0x21, 0x03,
        ];
        let section = create_brotli_section(&brotli_compressed_data, 13);
        let extractor = CompositeSectionExtractor::default();
        let result = extractor.extract(&section);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result, b"Hello, World!");
    }

    #[test]
    #[cfg(feature = "lzma")]
    fn test_composite_extracts_lzma() {
        // Pre-compressed "Hello, World!" using LZMA

        use crate::tests::create_lzma_section;
        let lzma_compressed_data: &[u8] = &[
            0x5D, 0x00, 0x00, 0x80, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x24, 0x19, 0x49, 0x98,
            0x6F, 0x16, 0x02, 0x89, 0x0A, 0x98, 0xE7, 0x3F, 0xA8, 0xC3, 0x95, 0x48, 0x4D, 0xFF, 0xFF, 0x75, 0xF0, 0x00,
            0x00,
        ];
        let section = create_lzma_section(lzma_compressed_data);
        let extractor = CompositeSectionExtractor::default();
        let result = extractor.extract(&section).expect("LZMA extraction should succeed");

        assert_eq!(result, b"Hello, World!");
    }
}
