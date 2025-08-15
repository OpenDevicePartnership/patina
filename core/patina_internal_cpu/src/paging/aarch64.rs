//! AArch64 Paging
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
use patina_paging::{MemoryAttributes, PageTable, PagingType, PtError, PtResult, aarch64::AArch64PageTable};

use patina_paging::page_allocator::PageAllocator;
use r_efi::efi;

#[cfg(test)]
use std::alloc::{Layout, dealloc};

/// The aarch64 paging implementation. It acts as a bridge between the EFI CPU
/// Architecture Protocol and the aarch64 paging implementation.
#[derive(Debug)]
pub struct EfiCpuPagingAArch64<P>
where
    P: PageTable,
{
    paging: P,
}

/// The aarch64 paging implementation.
impl<P> PageTable for EfiCpuPagingAArch64<P>
where
    P: PageTable,
{
    fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
        self.paging.map_memory_region(address, size, attributes)
    }

    fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PtError> {
        self.paging.unmap_memory_region(address, size)
    }

    fn remap_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
        self.paging.remap_memory_region(address, size, attributes)
    }

    fn install_page_table(&mut self) -> Result<(), PtError> {
        self.paging.install_page_table()
    }

    fn query_memory_region(&self, address: u64, size: u64) -> Result<MemoryAttributes, PtError> {
        self.paging.query_memory_region(address, size)
    }

    fn dump_page_tables(&self, address: u64, size: u64) -> PtResult<()> {
        self.paging.dump_page_tables(address, size)
    }
}

pub fn create_cpu_aarch64_paging<A: PageAllocator + 'static>(
    page_allocator: A,
) -> Result<Box<dyn PageTable>, efi::Status> {
    Ok(Box::new(EfiCpuPagingAArch64 {
        paging: AArch64PageTable::new(page_allocator, PagingType::Paging4Level).unwrap(),
    }))
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use std::alloc::alloc;

    use super::*;
    use mockall::mock;

    mock! {
        PageAllocator {}
        impl PageAllocator for PageAllocator {
            fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError>;
        }
    }

    mock! {
        PageTable {}
        impl PageTable for PageTable {
            fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError>;
            fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PtError>;
            fn remap_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError>;
            fn install_page_table(&self) -> Result<(), PtError>;
            fn query_memory_region(&self, address: u64, size: u64) -> Result<MemoryAttributes, PtError>;
            fn get_page_table_pages_for_size(&self, base_address: u64, size: u64) -> Result<u64, PtError>;
            fn dump_page_tables(&self, address: u64, size: u64);
        }
    }

    #[test]
    fn test_map_memory_region() {
        let mut mock_page_table = MockPageTable::new();

        mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingAArch64 { paging: mock_page_table };

        let result = paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Uncacheable);
        assert!(result.is_ok());
    }

    #[test]
    fn test_unmap_memory_region() {
        let mut mock_page_table = MockPageTable::new();

        mock_page_table.expect_unmap_memory_region().returning(|_, _| Ok(()));

        let mut paging = EfiCpuPagingAArch64 { paging: mock_page_table };

        let result = paging.unmap_memory_region(0x1000, 0x1000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_remap_memory_region() {
        let mut mock_page_table = MockPageTable::new();

        mock_page_table.expect_remap_memory_region().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingAArch64 { paging: mock_page_table };

        let result = paging.remap_memory_region(0x1000, 0x1000, MemoryAttributes::Uncacheable);
        assert!(result.is_ok());
    }

    #[test]
    fn test_query_memory_region() {
        let mut mock_page_table = MockPageTable::new();

        mock_page_table
            .expect_query_memory_region()
            .returning(|_, _| Ok(MemoryAttributes::Writeback | MemoryAttributes::Uncacheable));

        let paging = EfiCpuPagingAArch64 { paging: mock_page_table };

        let result = paging.query_memory_region(0x1000, 0x1000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MemoryAttributes::Writeback | MemoryAttributes::Uncacheable);
    }

    #[test]
    fn test_create_cpu_aarch64_paging() {
        let mut mock_page_allocator = MockPageAllocator::new();

        // Create a memory layout with the specified size and alignment
        let layout = Layout::from_size_align(4096, 4096).unwrap();
        // Allocate the memory
        let ptr = unsafe { alloc(layout) };
        let ptr_u64 = ptr as u64;

        mock_page_allocator.expect_allocate_page().returning(move |_, _, _| Ok(ptr_u64));

        let res = create_cpu_aarch64_paging(mock_page_allocator);
        assert!(res.is_ok());

        // Deallocate the memory when done unsafe
        unsafe {
            dealloc(ptr, layout);
        }
    }
}
