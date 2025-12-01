//! Module for crc32 section decompression.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::pi::fw_fs;
use patina_ffs::{
    FirmwareFileSystemError,
    section::{SectionExtractor, SectionHeader},
};

/// Provides extraction for CRC32 sections.
#[derive(Default, Clone, Copy)]
pub struct Crc32SectionExtractor;

impl Crc32SectionExtractor {
    /// Creates a new `Crc32SectionExtractor` instance.
    #[coverage(off)]
    pub const fn new() -> Self {
        Self {}
    }
}

impl SectionExtractor for Crc32SectionExtractor {
    fn extract(&self, section: &patina_ffs::section::Section) -> Result<alloc::vec::Vec<u8>, FirmwareFileSystemError> {
        if let SectionHeader::GuidDefined(guid_header, crc_header, _) = section.header()
            && guid_header.section_definition_guid == fw_fs::guid::CRC32_SECTION
        {
            if crc_header.len() < 4 {
                Err(FirmwareFileSystemError::DataCorrupt)?;
            }
            let crc32 = u32::from_le_bytes((**crc_header).try_into().unwrap());
            let content = section.try_content_as_slice()?;
            if crc32 != crc32fast::hash(content) {
                //TODO: in EDK2 C reference implementation, data is returned along with EFI_AUTH_STATUS_TEST_FAILED.
                //For now, just return an error if the CRC fails to check.
                Err(FirmwareFileSystemError::DataCorrupt)?;
            }
            return Ok(content.to_vec());
        }
        Err(FirmwareFileSystemError::Unsupported)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use crate::tests::create_crc32_section;

    use super::*;
    use patina::pi::fw_fs::ffs::section::header::GuidDefined;
    use patina_ffs::section::Section;
    use r_efi::efi;

    #[test]
    fn test_crc32_extractor_valid() {
        let content = b"Hello, CRC32!";
        let crc32 = crc32fast::hash(content);
        let section = create_crc32_section(content, crc32.to_le_bytes().to_vec());

        let extractor = Crc32SectionExtractor;
        let result = extractor.extract(&section).expect("CRC32 extraction should succeed");

        assert_eq!(result, content);
    }

    #[test]
    fn test_crc32_extractor_invalid_checksum() {
        let content = b"Hello, CRC32!";
        let wrong_crc32 = 0xDEADBEEFu32; // Intentionally wrong CRC
        let section = create_crc32_section(content, wrong_crc32.to_le_bytes().to_vec());

        let extractor = Crc32SectionExtractor;
        let result = extractor.extract(&section);

        assert!(matches!(result, Err(FirmwareFileSystemError::DataCorrupt)));
    }

    #[test]
    fn test_crc32_extractor_empty_content() {
        let content = b"";
        let crc32 = crc32fast::hash(content);
        let section = create_crc32_section(content, crc32.to_le_bytes().to_vec());

        let extractor = Crc32SectionExtractor;
        let result = extractor.extract(&section).expect("Empty content with valid CRC should succeed");

        assert_eq!(result, content);
    }

    #[test]
    fn test_crc32_extractor_unsupported_guid() {
        let wrong_guid =
            efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let content = b"Test data";

        let guid_header = GuidDefined {
            section_definition_guid: wrong_guid,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4 + 4) as u16,
            attributes: 0x01,
        };

        let crc32_bytes = crc32fast::hash(content).to_le_bytes().to_vec();
        let header = SectionHeader::GuidDefined(guid_header, crc32_bytes, content.len() as u32);
        let section =
            Section::new_from_header_with_data(header, content.to_vec()).expect("Failed to create test section");

        let extractor = Crc32SectionExtractor;
        let result = extractor.extract(&section);

        assert!(matches!(result, Err(FirmwareFileSystemError::Unsupported)));
    }
}
