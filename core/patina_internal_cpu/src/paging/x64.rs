//! X64 Paging
//!
//! This module provides an in direction to the external paging/mtrr crates.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::boxed::Box;
use patina_mtrr::create_mtrr_lib;
use patina_mtrr::error::MtrrError;
use patina_mtrr::structs::MtrrMemoryCacheType;
use patina_mtrr::Mtrr;
use patina_paging::page_allocator::PageAllocator;
use patina_paging::x64::X64PageTable;
use patina_paging::MemoryAttributes;
use patina_paging::PageTable;
use patina_paging::PagingType;
use patina_paging::PtError;
use patina_sdk::error::EfiError;
use r_efi::efi;

/// The x86_64 paging implementation. It acts as a bridge between the EFI CPU
/// Architecture Protocol and the x86_64 paging implementation.
#[derive(Debug)]
pub struct EfiCpuPagingX64<P, M>
where
    P: PageTable,
    M: Mtrr,
{
    paging: P,
    mtrr: M,
}

fn efierror_to_pterror(efi_error: EfiError) -> PtError {
    match efi_error {
        EfiError::InvalidParameter => PtError::InvalidParameter,
        EfiError::OutOfResources => PtError::OutOfResources,
        EfiError::NotFound => PtError::NoMapping,
        _ => PtError::InvalidParameter, // Default case for unsupported error codes
    }
}

/// The x86_64 paging implementation.
impl<P, M> PageTable for EfiCpuPagingX64<P, M>
where
    P: PageTable,
    M: Mtrr,
{
    type ALLOCATOR = P::ALLOCATOR;
    fn borrow_allocator(&mut self) -> &mut P::ALLOCATOR {
        self.paging.borrow_allocator()
    }
    // Paging related APIs
    fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
        let cache_attributes = attributes & MemoryAttributes::CacheAttributesMask;
        let memory_attributes = attributes & MemoryAttributes::AccessAttributesMask;

        if attributes != (cache_attributes | memory_attributes) {
            log::error!("Invalid cache attribute: {:#x}", attributes);
            return Err(PtError::InvalidParameter);
        }

        match apply_caching_attributes(address, size, cache_attributes, &mut self.mtrr) {
            Ok(_) => self.paging.map_memory_region(address, size, attributes & MemoryAttributes::AccessAttributesMask),
            Err(status) => Err(efierror_to_pterror(status)),
        }
    }

    fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PtError> {
        self.paging.unmap_memory_region(address, size)
    }

    fn remap_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
        let cache_attributes = attributes & MemoryAttributes::CacheAttributesMask;
        let memory_attributes = attributes & MemoryAttributes::AccessAttributesMask;

        if attributes != (cache_attributes | memory_attributes) {
            return Err(PtError::InvalidParameter);
        }

        match apply_caching_attributes(address, size, cache_attributes, &mut self.mtrr) {
            Ok(_) => {
                self.paging.remap_memory_region(address, size, attributes & MemoryAttributes::AccessAttributesMask)
            }
            Err(status) => Err(efierror_to_pterror(status)),
        }
    }

    fn install_page_table(&mut self) -> Result<(), PtError> {
        self.paging.install_page_table()
    }

    fn query_memory_region(&self, address: u64, size: u64) -> Result<MemoryAttributes, PtError> {
        self.paging.query_memory_region(address, size).map(|attr|
        // We need to add the cache attributes to the memory attributes
        attr | match self.mtrr.get_memory_attribute(address) {
            MtrrMemoryCacheType::Uncacheable => MemoryAttributes::Uncacheable,
            MtrrMemoryCacheType::WriteCombining => MemoryAttributes::WriteCombining,
            MtrrMemoryCacheType::WriteThrough => MemoryAttributes::WriteThrough,
            MtrrMemoryCacheType::WriteProtected => MemoryAttributes::WriteProtect,
            MtrrMemoryCacheType::WriteBack => MemoryAttributes::Writeback,
            _ => MemoryAttributes::empty(),
        })
    }

    fn dump_page_tables(&self, address: u64, size: u64) {
        self.paging.dump_page_tables(address, size)
    }
}

fn apply_caching_attributes<M: Mtrr>(
    base_address: u64,
    length: u64,
    cache_attributes: MemoryAttributes,
    mtrr: &mut M,
) -> Result<(), EfiError> {
    if cache_attributes.bits() != 0 {
        if !mtrr.is_supported() {
            return Err(EfiError::Unsupported);
        }

        let cache_type = match cache_attributes {
            MemoryAttributes::Uncacheable => MtrrMemoryCacheType::Uncacheable,
            MemoryAttributes::WriteCombining => MtrrMemoryCacheType::WriteCombining,
            MemoryAttributes::WriteThrough => MtrrMemoryCacheType::WriteThrough,
            MemoryAttributes::WriteProtect => MtrrMemoryCacheType::WriteProtected,
            MemoryAttributes::Writeback => MtrrMemoryCacheType::WriteBack,
            _ => return Err(EfiError::Unsupported),
        };

        let curr_attribute = mtrr.get_memory_attribute(base_address);
        if curr_attribute != cache_type {
            // cache attributes are not already set
            match mtrr.set_memory_attribute(base_address, length, cache_type) {
                Ok(_) => {
                    // now we need to program the APs with the update, if they are up
                    return Ok(());
                }
                Err(err) => return Err(mtrr_err_to_efi_status(err)),
            }
        }
    }

    Ok(())
}

pub fn create_cpu_x64_paging<A: PageAllocator + 'static>(
    page_allocator: A,
) -> Result<Box<dyn PageTable<ALLOCATOR = A>>, efi::Status> {
    Ok(Box::new(EfiCpuPagingX64 {
        paging: X64PageTable::new(page_allocator, PagingType::Paging4Level).unwrap(),
        mtrr: create_mtrr_lib(0),
    }))
}

fn mtrr_err_to_efi_status(err: MtrrError) -> EfiError {
    match err {
        MtrrError::MtrrNotSupported => EfiError::Unsupported,
        MtrrError::VariableRangeMtrrExhausted => EfiError::OutOfResources,
        MtrrError::FixedRangeMtrrBaseAddressNotAligned => EfiError::InvalidParameter,
        MtrrError::FixedRangeMtrrLengthNotAligned => EfiError::InvalidParameter,
        MtrrError::InvalidParameter => EfiError::InvalidParameter,
        MtrrError::BufferTooSmall => EfiError::BufferTooSmall,
        MtrrError::OutOfResources => EfiError::OutOfResources,
        MtrrError::AlreadyStarted => EfiError::AlreadyStarted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::mock;
    use patina_mtrr::{
        error::MtrrResult,
        structs::{MtrrMemoryRange, MtrrSettings},
    };

    mock! {
        PageAllocator {}
        impl PageAllocator for PageAllocator {
            fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError>;
        }
    }

    mock! {
        PageTable {}
        impl PageTable for PageTable {
            type ALLOCATOR = MockPageAllocator;
            fn borrow_allocator(&mut self) -> &mut MockPageAllocator;
            fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError>;
            fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PtError>;
            fn remap_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError>;
            fn install_page_table(&self) -> Result<(), PtError>;
            fn query_memory_region(&self, address: u64, size: u64) -> Result<MemoryAttributes, PtError>;
            fn get_page_table_pages_for_size(&self, base_address: u64, size: u64) -> Result<u64, PtError>;
            fn dump_page_tables(&self, address: u64, size: u64);
        }
    }

    mock! {
        Mtrr {}
        impl Mtrr for Mtrr {
            fn is_supported(&self) -> bool;
            fn get_all_mtrrs(&self) -> MtrrResult<MtrrSettings>;
            fn set_all_mtrrs(&mut self, mtrr_setting: &MtrrSettings);
            fn get_memory_attribute(&self, address: u64) -> MtrrMemoryCacheType;
            fn set_memory_attribute(
                &mut self,
                base_address: u64,
                length: u64,
                attribute: MtrrMemoryCacheType,
            ) -> MtrrResult<()>;
            fn set_memory_attributes(&mut self, ranges: &[MtrrMemoryRange]) -> MtrrResult<()>;
            fn get_memory_ranges(&self) -> MtrrResult<Vec<MtrrMemoryRange>>;

            fn debug_print_all_mtrrs(&self);
        }
    }

    #[test]
    fn test_map_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));
        mock_mtrr.expect_is_supported().return_const(true);
        mock_mtrr.expect_get_memory_attribute().return_const(MtrrMemoryCacheType::Uncacheable);
        mock_mtrr.expect_set_memory_attribute().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Uncacheable);
        assert!(result.is_ok());
    }

    #[test]
    fn test_unmap_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mock_mtrr = MockMtrr::new();

        mock_page_table.expect_unmap_memory_region().returning(|_, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.unmap_memory_region(0x1000, 0x1000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_remap_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_remap_memory_region().returning(|_, _, _| Ok(()));
        mock_mtrr.expect_is_supported().return_const(true);
        mock_mtrr.expect_get_memory_attribute().return_const(MtrrMemoryCacheType::Uncacheable);
        mock_mtrr.expect_set_memory_attribute().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.remap_memory_region(0x1000, 0x1000, MemoryAttributes::Uncacheable);
        assert!(result.is_ok());
    }

    #[test]
    fn test_query_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::Writeback));
        mock_mtrr.expect_get_memory_attribute().return_const(MtrrMemoryCacheType::Uncacheable);

        let paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.query_memory_region(0x1000, 0x1000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MemoryAttributes::Writeback | MemoryAttributes::Uncacheable);
    }
}
