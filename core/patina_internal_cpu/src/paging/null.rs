//! Null Paging - For doc tests
//!
//! This module provides an in direction to the external paging crate.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::boxed::Box;
use patina_paging::{MemoryAttributes, PageTable, PtError};

use patina_paging::page_allocator::PageAllocator;
use r_efi::efi;

#[derive(Default)]
pub struct EfiCpuPagingNull<A>
where
    A: PageAllocator,
{
    _allocator: core::marker::PhantomData<A>,
}

impl<A> PageTable for EfiCpuPagingNull<A>
where
    A: PageAllocator,
{
    fn map_memory_region(&mut self, _address: u64, _size: u64, _attributes: MemoryAttributes) -> Result<(), PtError> {
        Ok(())
    }

    fn unmap_memory_region(&mut self, _address: u64, _size: u64) -> Result<(), PtError> {
        Ok(())
    }

    fn remap_memory_region(&mut self, _address: u64, _size: u64, _attributes: MemoryAttributes) -> Result<(), PtError> {
        Ok(())
    }

    fn install_page_table(&mut self) -> Result<(), PtError> {
        Ok(())
    }

    fn query_memory_region(&self, _address: u64, _size: u64) -> Result<MemoryAttributes, PtError> {
        Ok(MemoryAttributes::empty())
    }

    fn dump_page_tables(&self, _address: u64, _size: u64) {}
}

/// Used to specify that this architecture paging implementation is not supported.
pub fn create_cpu_null_paging<A: PageAllocator + 'static>(
    _page_allocator: A,
) -> Result<Box<dyn PageTable>, efi::Status> {
    Err(efi::Status::UNSUPPORTED)
}
