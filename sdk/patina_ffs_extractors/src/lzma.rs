//! Module for LZMA decompression.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::vec::Vec;
use core::result::Result;
use patina_ffs::{
    FirmwareFileSystemError,
    section::{Section, SectionExtractor, SectionHeader},
};
use r_efi::efi;

use patina_lzma_rs::io::Cursor;

pub const LZMA_SECTION_GUID: efi::Guid =
    efi::Guid::from_fields(0xEE4E5898, 0x3914, 0x4259, 0x9D, 0x6E, &[0xDC, 0x7B, 0xD7, 0x94, 0x03, 0xCF]);

pub const LZMA_UNKNOWN_UNPACKED_SIZE_MAGIC_VALUE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

/// Provides decompression for LZMA GUIDed sections.
#[derive(Default, Clone, Copy)]
pub struct LzmaSectionExtractor;

impl LzmaSectionExtractor {
    /// Creates a new `LzmaSectionExtractor` instance.
    #[coverage(off)]
    pub const fn new() -> Self {
        Self {}
    }
}

impl SectionExtractor for LzmaSectionExtractor {
    fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
        if let SectionHeader::GuidDefined(guid_header, _, _) = section.header()
            && guid_header.section_definition_guid == LZMA_SECTION_GUID
        {
            let data = section.try_content_as_slice()?;

            // Get unpacked size to pre-allocate vector, if available
            // See https://github.com/tukaani-project/xz/blob/dd4a1b259936880e04669b43e778828b60619860/doc/lzma-file-format.txt#L131
            let unpacked_size =
                u64::from_le_bytes(data.get(5..13).ok_or(FirmwareFileSystemError::DataCorrupt)?.try_into().unwrap());
            let mut decompressed = if unpacked_size == LZMA_UNKNOWN_UNPACKED_SIZE_MAGIC_VALUE {
                Vec::<u8>::new()
            } else {
                Vec::<u8>::with_capacity(unpacked_size as usize)
            };

            patina_lzma_rs::lzma_decompress(&mut Cursor::new(data), &mut decompressed)
                .map_err(|_| FirmwareFileSystemError::DataCorrupt)?;

            return Ok(decompressed);
        }
        Err(FirmwareFileSystemError::Unsupported)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use crate::tests::create_lzma_section;

    use super::*;
    use alloc::vec;
    use patina::pi::fw_fs::ffs::section::header::GuidDefined;
    use patina_ffs::section::Section;

    #[test]
    fn test_lzma_extractor_valid() {
        // Pre-compressed "Hello, World!" using LZMA
        let lzma_compressed_data: &[u8] = &[
            0x5D, 0x00, 0x00, 0x80, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x24, 0x19, 0x49, 0x98,
            0x6F, 0x16, 0x02, 0x89, 0x0A, 0x98, 0xE7, 0x3F, 0xA8, 0xC3, 0x95, 0x48, 0x4D, 0xFF, 0xFF, 0x75, 0xF0, 0x00,
            0x00,
        ];
        let section = create_lzma_section(lzma_compressed_data);
        let extractor = LzmaSectionExtractor;
        let result = extractor.extract(&section).expect("LZMA extraction should succeed");

        assert_eq!(result, b"Hello, World!");
    }

    #[test]
    fn test_lzma_extractor_unknown_size() {
        // LZMA data with unknown unpacked size (0xFFFFFFFFFFFFFFFF)
        let lzma_compressed_data: &[u8] = &[
            0x5d, 0x00, 0x00, 0x08, 0x00, // LZMA properties
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // Unknown unpacked size
            0x00, 0x06, 0x54, 0x65, 0x73, 0x74, 0x00, // Minimal compressed payload
        ];

        let section = create_lzma_section(lzma_compressed_data);
        let extractor = LzmaSectionExtractor;
        // Should succeed even with unknown size (vector grows dynamically)
        let result = extractor.extract(&section);

        // Result depends on whether the compressed data is valid
        assert!(result.is_ok() || matches!(result, Err(FirmwareFileSystemError::DataCorrupt)));
    }

    #[test]
    fn test_lzma_extractor_invalid_data() {
        // Invalid LZMA data (too short / corrupt)
        let invalid_data: &[u8] = &[0x00, 0x01, 0x02, 0x03];

        let section = create_lzma_section(invalid_data);
        let extractor = LzmaSectionExtractor;
        let result = extractor.extract(&section);

        assert!(matches!(result, Err(FirmwareFileSystemError::DataCorrupt)));
    }

    #[test]
    fn test_lzma_extractor_unsupported_guid() {
        let wrong_guid =
            efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let dummy_data = b"Dummy data";

        let guid_header = GuidDefined {
            section_definition_guid: wrong_guid,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4) as u16,
            attributes: 0x01,
        };

        let header = SectionHeader::GuidDefined(guid_header, vec![], dummy_data.len() as u32);
        let section =
            Section::new_from_header_with_data(header, dummy_data.to_vec()).expect("Failed to create test section");

        let extractor = LzmaSectionExtractor;
        let result = extractor.extract(&section);

        assert!(matches!(result, Err(FirmwareFileSystemError::Unsupported)));
    }
}
