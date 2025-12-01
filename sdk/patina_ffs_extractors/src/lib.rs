//! # Section Extractor Implementations
//!
//! This crate provides a set of Implementations for the `patina::pi::fw_fs::SectionExtractor` trait.
//!
//! ## Features
//!
//! This crate contains the following features, where each feature corresponds to a different
//! implementation of the `SectionExtractorLib` trait. The crate is configured in this manner to
//! reduce compilation times, by only compiling the necessary implementations.
//! - `brotli`: Enables the `SectionExtractorLibBrotli` implementation.
//! - `crc32`: Enables the `Crc32SectionExtractor` implementation to validate CRC32 GUID-defined
//!   sections and return the verified payload.
//! - `lzma`: Enables the `LzmaSectionExtractor` implementation for GUID-defined LZMA compressed
//!   sections.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![feature(coverage_attribute)]
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

#[cfg(feature = "brotli")]
mod brotli;
#[cfg(feature = "brotli")]
pub use brotli::BrotliSectionExtractor;

#[cfg(feature = "crc32")]
mod crc32;
#[cfg(feature = "crc32")]
pub use crc32::Crc32SectionExtractor;

#[cfg(feature = "lzma")]
mod lzma;
#[cfg(feature = "lzma")]
pub use lzma::LzmaSectionExtractor;

mod composite;
pub use composite::CompositeSectionExtractor;

mod null;
pub use null::NullSectionExtractor;

#[cfg(test)]
#[coverage(off)]
mod tests {
    use alloc::{vec, vec::Vec};
    use patina::pi::fw_fs::{
        ffs::section::header::GuidDefined,
        guid::{BROTLI_SECTION, CRC32_SECTION, LZMA_SECTION},
    };
    use patina_ffs::section::{Section, SectionHeader};

    /// Constructs a section with the specified GUID and payload, prepending
    /// the required 16-byte header (out_size + scratch_size) for Brotli sections.
    pub(crate) fn create_brotli_section(payload: &[u8], out_size: u64) -> Section {
        // Brotli section payload format: [out_size: u64, scratch_size: u64, compressed_data...]
        let scratch_size = 0u64;

        let mut content = Vec::new();
        content.extend_from_slice(&out_size.to_le_bytes());
        content.extend_from_slice(&scratch_size.to_le_bytes());
        content.extend_from_slice(payload);

        let guid_header = GuidDefined {
            section_definition_guid: BROTLI_SECTION,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4) as u16, // common header + guid header
            attributes: 0x01,                                              // EFI_GUIDED_SECTION_PROCESSING_REQUIRED
        };

        let header = SectionHeader::GuidDefined(guid_header, vec![], content.len() as u32);
        Section::new_from_header_with_data(header, content).expect("Failed to create test section")
    }

    /// Helper to create an LZMA GUID-defined section for testing.
    /// Constructs a section with the LZMA GUID and the provided compressed payload.
    pub(crate) fn create_lzma_section(compressed_data: &[u8]) -> Section {
        let guid_header = GuidDefined {
            section_definition_guid: LZMA_SECTION,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4) as u16, // common header + guid header
            attributes: 0x01,                                              // EFI_GUIDED_SECTION_PROCESSING_REQUIRED
        };

        let header = SectionHeader::GuidDefined(guid_header, vec![], compressed_data.len() as u32);
        Section::new_from_header_with_data(header, compressed_data.to_vec()).expect("Failed to create test section")
    }

    /// Helper to create a GUID-defined section for testing.
    pub(crate) fn create_crc32_section(content: &[u8], guid_data: Vec<u8>) -> Section {
        let guid_header = GuidDefined {
            section_definition_guid: CRC32_SECTION,
            data_offset: (core::mem::size_of::<GuidDefined>() + 4 + guid_data.len()) as u16,
            attributes: 0x01,
        };

        let header = SectionHeader::GuidDefined(guid_header, guid_data, content.len() as u32);
        Section::new_from_header_with_data(header, content.to_vec()).expect("Failed to create test section")
    }
}
