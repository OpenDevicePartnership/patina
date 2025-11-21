//! Module for Brotli decompression.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::{boxed::Box, vec, vec::Vec};
use alloc_no_stdlib::{self, SliceWrapper, SliceWrapperMut, define_index_ops_mut};
use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, HuffmanCode};
use patina::pi::fw_fs;
use patina_ffs::{
    FirmwareFileSystemError,
    section::{Section, SectionExtractor, SectionHeader},
};

//Rebox and HeapAllocator exist to satisfy BrotliDecompress custom allocation requirements.
//They essentially wrap Box for heap allocations.
struct Rebox<T>(Box<[T]>);

impl<T> core::default::Default for Rebox<T> {
    fn default() -> Self {
        Rebox(Vec::new().into_boxed_slice())
    }
}
define_index_ops_mut!(T, Rebox<T>);

impl<T> alloc_no_stdlib::SliceWrapper<T> for Rebox<T> {
    fn slice(&self) -> &[T] {
        &self.0
    }
}

impl<T> alloc_no_stdlib::SliceWrapperMut<T> for Rebox<T> {
    fn slice_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

struct HeapAllocator<T: Clone> {
    pub default_value: T,
}

impl<T: Clone> alloc_no_stdlib::Allocator<T> for HeapAllocator<T> {
    type AllocatedMemory = Rebox<T>;
    fn alloc_cell(self: &mut HeapAllocator<T>, len: usize) -> Rebox<T> {
        Rebox(vec![self.default_value.clone(); len].into_boxed_slice())
    }
    fn free_cell(self: &mut HeapAllocator<T>, _data: Rebox<T>) {}
}

/// Provides decompression for Brotli GUIDed sections.
#[derive(Default, Clone, Copy)]
pub struct BrotliSectionExtractor;

impl BrotliSectionExtractor {
    /// Creates a new `BrotliSectionExtractor` instance.
    #[coverage(off)]
    pub const fn new() -> Self {
        Self {}
    }
}

impl SectionExtractor for BrotliSectionExtractor {
    fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
        if let SectionHeader::GuidDefined(guid_header, _, _) = section.header()
            && guid_header.section_definition_guid == fw_fs::guid::BROTLI_SECTION
        {
            let data = section.try_content_as_slice()?;
            let out_size = u64::from_le_bytes(data[0..8].try_into().unwrap());
            let _scratch_size = u64::from_le_bytes(data[8..16].try_into().unwrap());

            let mut brotli_state = BrotliState::new(
                HeapAllocator::<u8> { default_value: 0 },
                HeapAllocator::<u32> { default_value: 0 },
                HeapAllocator::<HuffmanCode> { default_value: Default::default() },
            );
            let in_data = &data[16..];
            let mut out_data = vec![0u8; out_size as usize];
            let mut out_data_size = 0;
            let result = BrotliDecompressStream(
                &mut in_data.len(),
                &mut 0,
                &data[16..],
                &mut out_data.len(),
                &mut 0,
                out_data.as_mut_slice(),
                &mut out_data_size,
                &mut brotli_state,
            );

            if matches!(result, BrotliResult::ResultSuccess) {
                return Ok(out_data);
            } else {
                return Err(FirmwareFileSystemError::DataCorrupt);
            }
        }
        Err(FirmwareFileSystemError::Unsupported)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use crate::tests::create_brotli_section;

    use super::*;

    #[test]
    fn test_brotli_extractor() {
        // Pre-compressed "Hello, World!" using Brotli
        let brotli_compressed_data: [u8; 18] = [
            0x21, 0x30, 0x00, 0x04, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x2C, 0x20, 0x57, 0x6F, 0x72, 0x6C, 0x64, 0x21, 0x03,
        ];
        let section = create_brotli_section(&brotli_compressed_data, 13);
        let extractor = BrotliSectionExtractor;
        let result = extractor.extract(&section);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result, b"Hello, World!");
    }
}
