//! Null Paging - For doc tests
//!
//! This module provides an in direction to the external paging crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::boxed::Box;
use patina_paging::MemoryAttributes;

use crate::paging::{CacheAttributeValue, PagingError, PatinaPageTable};
use patina_paging::page_allocator::PageAllocator;

#[derive(Default)]
#[allow(dead_code)]
pub struct EfiCpuPagingNull<A>
where
    A: PageAllocator,
{
    _allocator: core::marker::PhantomData<A>,
}

impl<A> PatinaPageTable for EfiCpuPagingNull<A>
where
    A: PageAllocator,
{
    fn map_memory_region(
        &mut self,
        _address: u64,
        _size: u64,
        _attributes: MemoryAttributes,
    ) -> Result<(), PagingError> {
        Ok(())
    }

    fn map_aliased_memory_region(
        &mut self,
        _virtual_address: u64,
        _physical_address: u64,
        _size: u64,
        _attributes: MemoryAttributes,
    ) -> Result<(), PagingError> {
        Ok(())
    }

    fn unmap_memory_region(&mut self, _address: u64, _size: u64) -> Result<(), PagingError> {
        Ok(())
    }

    fn install_page_table(&mut self) -> Result<(), PagingError> {
        Ok(())
    }

    fn query_memory_region(
        &self,
        _address: u64,
        _size: u64,
    ) -> Result<MemoryAttributes, (PagingError, CacheAttributeValue)> {
        Ok(MemoryAttributes::empty())
    }

    fn dump_page_tables(&self, _address: u64, _size: u64) -> Result<(), PagingError> {
        Ok(())
    }

    fn handle_cacheability_change(
        &self,
        _address: u64,
        _size: u64,
        _old_cache_attributes: MemoryAttributes,
        _new_cache_attributes: MemoryAttributes,
    ) {
    }
}

/// Used to specify that this architecture paging implementation is not supported.
pub fn create_cpu_null_paging<A: PageAllocator + 'static>(
    _page_allocator: A,
) -> Result<impl PatinaPageTable, efi::Status> {
    Err(efi::Status::UNSUPPORTED)
}

/// Open the active page table. Not supported on this architecture.
///
/// ## Safety
/// N/A — always returns an error.
pub unsafe fn open_active_cpu_null_paging<A: PageAllocator + 'static>(
    _page_allocator: A,
) -> Result<impl PatinaPageTable, PagingError> {
    Err::<EfiCpuPagingNull<A>, _>(PagingError::Unsupported)
}
