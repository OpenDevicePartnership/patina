//! Module for crc32 section decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use patina_ffs::{
    section::{SectionExtractor, SectionMetaData},
    FirmwareFileSystemError,
};
use r_efi::efi;

pub const CRC32_SECTION_GUID: efi::Guid =
    efi::Guid::from_fields(0xFC1BCDB0, 0x7D31, 0x49aa, 0x93, 0x6A, &[0xA4, 0x60, 0x0D, 0x9D, 0xD0, 0x83]);

/// Provides extraction for CRC32 sections.
#[derive(Default, Clone, Copy)]
pub struct Crc32SectionExtractor {}
impl SectionExtractor for Crc32SectionExtractor {
    fn extract(&self, section: &patina_ffs::section::Section) -> Result<alloc::vec::Vec<u8>, FirmwareFileSystemError> {
        if let SectionMetaData::GuidDefined(guid_header, crc_header, _) = section.metadata() {
            if guid_header.section_definition_guid == CRC32_SECTION_GUID {
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
        }
        Err(FirmwareFileSystemError::Unsupported)
    }
}
