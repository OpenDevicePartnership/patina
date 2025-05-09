//! Module for Brotli decompression.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{boxed::Box, vec, vec::Vec};
use mu_pi::fw_fs::{SectionExtractor, SectionMetaData};
use r_efi::efi;

use alloc_no_stdlib::{self, define_index_ops_mut, SliceWrapper, SliceWrapperMut};
use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, HuffmanCode};

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

pub const BROTLI_SECTION_GUID: efi::Guid =
    efi::Guid::from_fields(0x3D532050, 0x5CDA, 0x4FD0, 0x87, 0x9E, &[0x0F, 0x7F, 0x63, 0x0D, 0x5A, 0xFB]);

/// Provides decompression for Brotli GUIDed sections.
#[derive(Default, Clone, Copy)]
pub struct BrotliSectionExtractor;
impl SectionExtractor for BrotliSectionExtractor {
    fn extract(&self, section: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, efi::Status> {
        if let SectionMetaData::GuidDefined(guid_header, _) = section.meta_data() {
            if guid_header.section_definition_guid == BROTLI_SECTION_GUID {
                let data = section.section_data();
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
                    return Ok(out_data.into_boxed_slice());
                } else {
                    return Err(efi::Status::VOLUME_CORRUPTED);
                }
            }
        }
        Ok(Box::new([0u8; 0]))
    }
}
