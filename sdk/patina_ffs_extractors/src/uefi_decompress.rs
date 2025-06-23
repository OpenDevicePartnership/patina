//! Module for UEFI decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::vec;
use mu_pi::fw_fs::ffs;
use mu_rust_helpers::uefi_decompress::{decompress_into_with_algo, DecompressionAlgorithm};
use patina_ffs::{
    section::{SectionExtractor, SectionMetaData},
    FirmwareFileSystemError,
};
use r_efi::efi;

pub const TIANO_DECOMPRESS_SECTION_GUID: efi::Guid =
    efi::Guid::from_fields(0xA31280AD, 0x481E, 0x41B6, 0x95, 0xE8, &[0x12, 0x7F, 0x4C, 0x98, 0x47, 0x79]);

/// Provides decompression for sections compressed with UEFI compression algorithm and TianoCompress GUIDed sections.
#[derive(Default, Clone, Copy)]
pub struct UefiDecompressSectionExtractor {}
impl SectionExtractor for UefiDecompressSectionExtractor {
    fn extract(&self, section: &patina_ffs::section::Section) -> Result<vec::Vec<u8>, FirmwareFileSystemError> {
        let (src, algo) = match section.metadata() {
            SectionMetaData::GuidDefined(guid_header, _, _)
                if guid_header.section_definition_guid == TIANO_DECOMPRESS_SECTION_GUID =>
            {
                (section.try_content_as_slice()?, DecompressionAlgorithm::TianoDecompress)
            }
            SectionMetaData::Compression(compression_header, _) => {
                match compression_header.compression_type {
                    ffs::section::header::NOT_COMPRESSED => return Ok(section.try_content_as_slice()?.to_vec()), //not compressed, so just return section data
                    ffs::section::header::STANDARD_COMPRESSION => {
                        (section.try_content_as_slice()?, DecompressionAlgorithm::UefiDecompress)
                    }
                    _ => Err(FirmwareFileSystemError::Unsupported)?,
                }
            }
            _ => return Err(FirmwareFileSystemError::Unsupported),
        };

        //sanity check the src data
        if src.len() < 8 {
            Err(FirmwareFileSystemError::DataCorrupt)?;
        }

        let compressed_size = u32::from_le_bytes(src[0..4].try_into().unwrap()) as usize;
        if compressed_size > src.len() {
            Err(FirmwareFileSystemError::DataCorrupt)?;
        }

        // allocate a buffer to hold the decompressed data
        let decompressed_size = u32::from_le_bytes(src[4..8].try_into().unwrap()) as usize;
        let mut decompressed_buffer = vec![0u8; decompressed_size];

        // execute decompress
        decompress_into_with_algo(src, &mut decompressed_buffer, algo)
            .map_err(|_err| FirmwareFileSystemError::DataCorrupt)?;
        Ok(decompressed_buffer)
    }
}
