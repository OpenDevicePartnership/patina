//! Module for LZMA decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{boxed::Box, vec::Vec};
use core::result::Result;
use mu_pi::fw_fs::{SectionExtractor, SectionMetaData};
use r_efi::efi;

use patina_lzma_rs::io::Cursor;

pub const LZMA_SECTION_GUID: efi::Guid =
    efi::Guid::from_fields(0xEE4E5898, 0x3914, 0x4259, 0x9D, 0x6E, &[0xDC, 0x7B, 0xD7, 0x94, 0x03, 0xCF]);

pub const LZMA_UNKNOWN_UNPACKED_SIZE_MAGIC_VALUE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

/// Provides decompression for LZMA GUIDed sections.
#[derive(Default, Clone, Copy)]
pub struct LzmaSectionExtractor;

impl SectionExtractor for LzmaSectionExtractor {
    fn extract(&self, section: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, efi::Status> {
        if let SectionMetaData::GuidDefined(guid_header, _) = section.meta_data()
            && guid_header.section_definition_guid == LZMA_SECTION_GUID
        {
            let data = section.section_data();

            // Get unpacked size to pre-allocate vector, if available
            // See https://github.com/tukaani-project/xz/blob/dd4a1b259936880e04669b43e778828b60619860/doc/lzma-file-format.txt#L131
            let unpacked_size = u64::from_le_bytes(data[5..13].try_into().unwrap());
            let mut decompressed = if unpacked_size == LZMA_UNKNOWN_UNPACKED_SIZE_MAGIC_VALUE {
                Vec::<u8>::new()
            } else {
                Vec::<u8>::with_capacity(unpacked_size as usize)
            };

            patina_lzma_rs::lzma_decompress(&mut Cursor::new(data), &mut decompressed)
                .map_err(|_| efi::Status::VOLUME_CORRUPTED)?;

            return Ok(decompressed.into_boxed_slice());
        }
        Ok(Box::new([0u8; 0])) // Means "no support for this data section"
    }
}
