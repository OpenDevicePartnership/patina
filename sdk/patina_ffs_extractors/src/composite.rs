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
mod tests {
    use super::*;
    use alloc::{vec, vec::Vec};
    use patina::pi::fw_fs::{
        ffs::section::header::GuidDefined,
        guid::{BROTLI_SECTION, CRC32_SECTION, LZMA_SECTION},
    };
    use patina_ffs::section::SectionHeader;
    use r_efi::efi;

    /// Constructs a section with the specified GUID and payload, prepending
    /// the required 16-byte header (out_size + scratch_size) for Brotli sections.
    fn create_brotli_section(guid: &efi::Guid, payload: &[u8], out_size: u64) -> Section {
        // Brotli section payload format: [out_size: u64, scratch_size: u64, compressed_data...]
        let scratch_size = 0u64;

        let mut content = Vec::new();
        content.extend_from_slice(&out_size.to_le_bytes());
        content.extend_from_slice(&scratch_size.to_le_bytes());
        content.extend_from_slice(payload);

        let guid_header = GuidDefined {
            section_definition_guid: *guid,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4) as u16, // common header + guid header
            attributes: 0x01,                                              // EFI_GUIDED_SECTION_PROCESSING_REQUIRED
        };

        let header = SectionHeader::GuidDefined(guid_header, vec![], content.len() as u32);
        Section::new_from_header_with_data(header, content).expect("Failed to create test section")
    }

    /// Helper to create an LZMA GUID-defined section for testing.
    ///
    /// Constructs a section with the LZMA GUID and the provided compressed payload.
    fn create_lzma_section(compressed_data: &[u8]) -> Section {
        let guid_header = GuidDefined {
            section_definition_guid: LZMA_SECTION,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4) as u16, // common header + guid header
            attributes: 0x01,                                              // EFI_GUIDED_SECTION_PROCESSING_REQUIRED
        };

        let header = SectionHeader::GuidDefined(guid_header, vec![], compressed_data.len() as u32);
        Section::new_from_header_with_data(header, compressed_data.to_vec()).expect("Failed to create test section")
    }

    /// Helper to create a GUID-defined section for testing.
    fn create_crc32_section(guid: &efi::Guid, content: &[u8], guid_data: Vec<u8>) -> Section {
        let guid_header = GuidDefined {
            section_definition_guid: *guid,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4 + guid_data.len()) as u16,
            attributes: 0x01,
        };

        let header = SectionHeader::GuidDefined(guid_header, guid_data, content.len() as u32);
        Section::new_from_header_with_data(header, content.to_vec()).expect("Failed to create test section")
    }

    #[test]
    #[cfg(feature = "crc32")]
    fn test_composite_extracts_crc32() {
        let content = b"Test CRC32 content";
        let crc32 = crc32fast::hash(content);
        let section = create_crc32_section(&CRC32_SECTION, content, crc32.to_le_bytes().to_vec());

        let extractor = CompositeSectionExtractor::default();
        let result = extractor.extract(&section).expect("Should extract CRC32 section");

        assert_eq!(result, content);
    }

    #[test]
    #[cfg(feature = "brotli")]
    fn test_composite_extracts_brotli() {
        // Pre-compressed "Hello, World!" using Brotli
        let brotli_compressed_data: [u8; 18] = [
            0x21, 0x30, 0x00, 0x04, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x2C, 0x20, 0x57, 0x6F, 0x72, 0x6C, 0x64, 0x21, 0x03,
        ];
        let section = create_brotli_section(&BROTLI_SECTION, &brotli_compressed_data, 13);
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
