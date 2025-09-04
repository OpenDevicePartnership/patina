//! Module for crc32 section decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use mu_pi::fw_fs;
use patina_ffs::{
    FirmwareFileSystemError,
    section::{SectionExtractor, SectionHeader},
};

/// Provides extraction for CRC32 sections.
#[derive(Default, Clone, Copy)]
pub struct Crc32SectionExtractor {}
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
