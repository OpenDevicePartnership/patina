//! X64 Paging
//!
//! This module provides an in direction to the external paging/mtrr crates.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::paging::{CacheAttributeValue, PatinaPageTable};
use patina::{error::EfiError, standard::efi};
use patina_mtrr::{Mtrr, create_mtrr_lib, error::MtrrError, structs::MtrrMemoryCacheType};
use patina_paging::{
    MemoryAttributes, PageTable, PagingType, PtError, page_allocator::PageAllocator, x64::X64PageTable,
};

/// The `x86_64` paging implementation. It acts as a bridge between the EFI CPU
/// Architecture Protocol and the `x86_64` paging implementation.
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
        EfiError::OutOfResources => PtError::OutOfResources,
        EfiError::NotFound => PtError::NoMapping,
        _ => PtError::InvalidParameter, // Default case for unsupported error codes
    }
}

/// The `x86_64` paging implementation.
impl<P, M> PatinaPageTable for EfiCpuPagingX64<P, M>
where
    P: PageTable,
    M: Mtrr,
{
    // Paging related APIs
    fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
        let cache_attributes = attributes & MemoryAttributes::CacheAttributesMask;
        let memory_attributes = attributes & MemoryAttributes::AccessAttributesMask;

        if attributes != (cache_attributes | memory_attributes) {
            log::error!("Invalid cache attribute: {attributes:#x}");
            return Err(PtError::InvalidParameter);
        }

        match apply_caching_attributes(address, size, cache_attributes, &mut self.mtrr) {
            Ok(()) | Err(EfiError::Unsupported) => {
                self.paging.map_memory_region(address, size, attributes & MemoryAttributes::AccessAttributesMask)
            }
            Err(status) => Err(efierror_to_pterror(status)),
        }
    }

    fn map_aliased_memory_region(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        size: u64,
        attributes: MemoryAttributes,
    ) -> Result<(), PtError> {
        let cache_attributes = attributes & MemoryAttributes::CacheAttributesMask;
        let memory_attributes = attributes & MemoryAttributes::AccessAttributesMask;

        if attributes != (cache_attributes | memory_attributes) {
            log::error!("Invalid cache attribute: {attributes:#x}");
            return Err(PtError::InvalidParameter);
        }

        match apply_caching_attributes(physical_address, size, cache_attributes, &mut self.mtrr) {
            Ok(()) | Err(EfiError::Unsupported) => {
                self.paging.map_aliased_memory_region(virtual_address, physical_address, size, memory_attributes)
            }
            Err(status) => Err(efierror_to_pterror(status)),
        }
    }

    fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PtError> {
        self.paging.unmap_memory_region(address, size)
    }

    fn install_page_table(&mut self) -> Result<(), PtError> {
        self.paging.install_page_table()
    }

    fn query_memory_region(&self, address: u64, size: u64) -> Result<MemoryAttributes, (PtError, CacheAttributeValue)> {
        // start by getting the caching attributes as we need to return those even if the page is unmapped in the
        // page table
        let cache_attr = match self.mtrr.get_memory_attribute(address) {
            MtrrMemoryCacheType::Uncacheable => CacheAttributeValue::Valid(MemoryAttributes::Uncached),
            MtrrMemoryCacheType::WriteCombining => CacheAttributeValue::Valid(MemoryAttributes::WriteCombining),
            MtrrMemoryCacheType::WriteThrough => CacheAttributeValue::Valid(MemoryAttributes::WriteThrough),
            MtrrMemoryCacheType::WriteProtected => CacheAttributeValue::Valid(MemoryAttributes::WriteProtect),
            MtrrMemoryCacheType::WriteBack => CacheAttributeValue::Valid(MemoryAttributes::Writeback),
            _ => CacheAttributeValue::Unmapped,
        };

        match self.paging.query_memory_region(address, size) {
            Ok(attr) => {
                if let CacheAttributeValue::Valid(cache_attr_val) = cache_attr {
                    Ok(attr | cache_attr_val)
                } else {
                    debug_assert!(false, "Cache attributes should be valid for mapped region");
                    Ok(attr)
                }
            }
            Err(err) => Err((err, cache_attr)),
        }
    }

    fn dump_page_tables(&self, address: u64, size: u64) -> Result<(), PtError> {
        self.paging.dump_page_tables(address, size)
    }

    fn handle_cacheability_change(
        &self,
        _address: u64,
        _size: u64,
        _old_cache_attributes: MemoryAttributes,
        _new_cache_attributes: MemoryAttributes,
    ) {
        // Cache consistency is already handled by the MTRR library. No further action is needed.
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
            MemoryAttributes::Uncached => MtrrMemoryCacheType::Uncacheable,
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
                Ok(()) => {
                    // now we need to program the APs with the update, if they are up
                    return Ok(());
                }
                Err(err) => return Err(mtrr_err_to_efi_status(err)),
            }
        }
    }

    Ok(())
}

/// Create an `x86_64` paging instance under the general `PatinaPageTable` trait.
#[cfg_attr(coverage, coverage(off))]
pub fn create_cpu_x64_paging<A: PageAllocator + 'static>(
    page_allocator: A,
) -> Result<impl PatinaPageTable, efi::Status> {
    Ok(EfiCpuPagingX64 {
        paging: X64PageTable::new(page_allocator, PagingType::Paging4Level)
            .map_err(|_| efi::Status::INVALID_PARAMETER)?,
        mtrr: create_mtrr_lib(0),
    })
}

/// Open the active `x86_64` page table wrapped in the `PatinaPageTable` trait.
///
/// ## Safety
/// The caller must ensure no other entity is concurrently modifying the page tables.
#[cfg_attr(coverage, coverage(off))]
pub unsafe fn open_active_cpu_x64_paging<A: PageAllocator + 'static>(
    page_allocator: A,
) -> Result<impl PatinaPageTable, PtError> {
    // SAFETY: Caller ensures no concurrent page table modifications.
    let page_table = unsafe { X64PageTable::open_active(page_allocator)? };
    Ok(EfiCpuPagingX64 { paging: page_table, mtrr: create_mtrr_lib(0) })
}

fn mtrr_err_to_efi_status(err: MtrrError) -> EfiError {
    match err {
        MtrrError::AlreadyStarted => EfiError::AlreadyStarted,
        MtrrError::BufferTooSmall => EfiError::BufferTooSmall,
        MtrrError::FixedRangeMtrrBaseAddressNotAligned
        | MtrrError::FixedRangeMtrrLengthNotAligned
        | MtrrError::InvalidParameter => EfiError::InvalidParameter,
        MtrrError::MtrrNotSupported => EfiError::Unsupported,
        MtrrError::OutOfResources | MtrrError::VariableRangeMtrrExhausted => EfiError::OutOfResources,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina_mtrr::MockMtrr;
    use patina_paging::MockPageTable;

    #[test]
    fn test_map_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));
        mock_mtrr.expect_is_supported().return_const(true);
        mock_mtrr.expect_get_memory_attribute().return_const(MtrrMemoryCacheType::Uncacheable);
        mock_mtrr.expect_set_memory_attribute().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Uncached);
        assert!(result.is_ok());
    }

    #[test]
    fn test_map_aliased_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_map_aliased_memory_region().returning(
            |virtual_address, physical_address, size, attributes| {
                assert_eq!(virtual_address, 0x2000);
                assert_eq!(physical_address, 0x1000);
                assert_eq!(size, 0x1000);
                assert_eq!(attributes, MemoryAttributes::ReadOnly);
                Ok(())
            },
        );
        mock_mtrr.expect_is_supported().return_const(true);
        mock_mtrr.expect_get_memory_attribute().returning(|address| {
            assert_eq!(address, 0x1000);
            MtrrMemoryCacheType::Uncacheable
        });
        mock_mtrr.expect_set_memory_attribute().returning(|address, size, cache_type| {
            assert_eq!(address, 0x1000);
            assert_eq!(size, 0x1000);
            assert_eq!(cache_type, MtrrMemoryCacheType::WriteBack);
            Ok(())
        });

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.map_aliased_memory_region(
            0x2000,
            0x1000,
            0x1000,
            MemoryAttributes::Writeback | MemoryAttributes::ReadOnly,
        );
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

        mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));
        mock_mtrr.expect_is_supported().return_const(true);
        mock_mtrr.expect_get_memory_attribute().return_const(MtrrMemoryCacheType::Uncacheable);
        mock_mtrr.expect_set_memory_attribute().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr };

        let result = paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Uncached);
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
        assert_eq!(result.unwrap(), MemoryAttributes::Writeback | MemoryAttributes::Uncached);
    }

    #[test]
    fn test_handle_cacheability_change() {
        let paging = EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: MockMtrr::new() };
        paging.handle_cacheability_change(0x1000, 0x1000, MemoryAttributes::Writeback, MemoryAttributes::Uncached);
    }
}
