//! UEFI PE/COFF Relocation Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::vec::Vec;
use scroll::Pread;

#[repr(C)]
#[derive(Debug, Copy, Clone, Pread)]
pub struct BaseRelocationBlockHeader {
    pub page_rva: u32,
    pub block_size: u32,
}
#[repr(C)]
#[derive(Debug, Copy, Clone, Pread)]
pub struct Relocation {
    pub type_and_offset: u16,
    pub value: u64,
}

#[derive(Debug, Clone)]
pub struct RelocationBlock {
    pub block_header: BaseRelocationBlockHeader,
    pub relocations: Vec<Relocation>,
}

pub(crate) fn parse_relocation_blocks(block: &[u8]) -> super::error::Result<Vec<RelocationBlock>> {
    let mut offset: usize = 0;
    let mut blocks = Vec::new();

    while offset < block.len() {
        let block_start = offset;
        let block_header: BaseRelocationBlockHeader = block.gread_with(&mut offset, scroll::LE)?;

        let mut relocations = Vec::new();
        while offset < block_start + block_header.block_size as usize {
            relocations.push(Relocation { type_and_offset: block.gread_with(&mut offset, scroll::LE)?, value: 0 });
        }

        blocks.push(RelocationBlock { block_header, relocations });
        // block start on 32-bit boundary, so align up if needed.
        offset = (offset + 3) & !3;
    }

    Ok(blocks)
}
