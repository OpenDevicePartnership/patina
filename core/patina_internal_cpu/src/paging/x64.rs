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
use crate::paging::{CacheAttributeSource, PagingError, PatinaPageTable};
use patina::standard::efi;
use patina_mtrr::{Mtrr, create_mtrr_lib, error::MtrrError, structs::MtrrMemoryCacheType};
use patina_paging::{MemoryAttributes, PageTable, PagingType, page_allocator::PageAllocator, x64::X64PageTable};

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
    mtrr_supported: bool,
}

/// The `x86_64` paging implementation.
impl<P, M> PatinaPageTable for EfiCpuPagingX64<P, M>
where
    P: PageTable,
    M: Mtrr,
{
    // Paging related APIs
    fn map_memory_region(&mut self, address: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PagingError> {
        let cache_attributes = attributes & MemoryAttributes::CacheAttributesMask;
        let memory_attributes = attributes & MemoryAttributes::AccessAttributesMask;

        if attributes != (cache_attributes | memory_attributes) {
            log::error!("Invalid cache attribute: {attributes:#x}");
            return Err(PagingError::InvalidParameter);
        }

        apply_caching_attributes(address, size, cache_attributes, &mut self.mtrr, self.mtrr_supported)?;
        self.paging.map_memory_region(address, size, memory_attributes).map_err(Into::into)
    }

    fn map_aliased_memory_region(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        size: u64,
        attributes: MemoryAttributes,
    ) -> Result<(), PagingError> {
        let cache_attributes = attributes & MemoryAttributes::CacheAttributesMask;
        let memory_attributes = attributes & MemoryAttributes::AccessAttributesMask;

        if attributes != (cache_attributes | memory_attributes) {
            log::error!("Invalid cache attribute: {attributes:#x}");
            return Err(PagingError::InvalidParameter);
        }

        apply_caching_attributes(physical_address, size, cache_attributes, &mut self.mtrr, self.mtrr_supported)?;
        self.paging
            .map_aliased_memory_region(virtual_address, physical_address, size, memory_attributes)
            .map_err(Into::into)
    }

    fn unmap_memory_region(&mut self, address: u64, size: u64) -> Result<(), PagingError> {
        self.paging.unmap_memory_region(address, size).map_err(Into::into)
    }

    fn install_page_table(&mut self) -> Result<(), PagingError> {
        self.paging.install_page_table().map_err(Into::into)
    }

    fn query_memory_region(
        &self,
        address: u64,
        size: u64,
    ) -> Result<MemoryAttributes, (PagingError, Option<MemoryAttributes>)> {
        // start by getting the caching attributes as we need to return those even if the page is unmapped in the
        // page table
        let cache_attr = match self.mtrr.get_memory_attribute(address) {
            Ok(MtrrMemoryCacheType::Uncacheable) => Some(MemoryAttributes::Uncached),
            Ok(MtrrMemoryCacheType::WriteCombining) => Some(MemoryAttributes::WriteCombining),
            Ok(MtrrMemoryCacheType::WriteThrough) => Some(MemoryAttributes::WriteThrough),
            Ok(MtrrMemoryCacheType::WriteProtected) => Some(MemoryAttributes::WriteProtect),
            Ok(MtrrMemoryCacheType::WriteBack) => Some(MemoryAttributes::Writeback),
            Ok(MtrrMemoryCacheType::Reserved1 | MtrrMemoryCacheType::Reserved2 | MtrrMemoryCacheType::Invalid) => {
                return Err((PagingError::InvalidParameter, None));
            }
            // Without MTRRs there are no cache attributes to report; the region is uncached per the Intel SDM.
            Err(MtrrError::MtrrNotSupported) => None,
            Err(error) => {
                debug_assert!(false, "Unexpected return: {error:?} while querying MTRR memory attribute");
                return Err((error.into(), None));
            }
        };

        match self.paging.query_memory_region(address, size) {
            Ok(attr) => Ok(attr | cache_attr.unwrap_or(MemoryAttributes::empty())),
            Err(error) => Err((error.into(), cache_attr)),
        }
    }

    fn cache_attribute_source(&self) -> CacheAttributeSource {
        if self.mtrr_supported { CacheAttributeSource::Processor } else { CacheAttributeSource::Unsupported }
    }

    fn dump_page_tables(&self, address: u64, size: u64) -> Result<(), PagingError> {
        self.paging.dump_page_tables(address, size).map_err(Into::into)
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
    mtrr_supported: bool,
) -> Result<(), PagingError> {
    // If no cache attributes were set or MTRRs are unsupported, just return Ok(). The access attributes should still
    // be programmed in the page table.
    if cache_attributes.is_empty() || !mtrr_supported {
        return Ok(());
    }

    let cache_type = match cache_attributes {
        MemoryAttributes::Uncached => MtrrMemoryCacheType::Uncacheable,
        MemoryAttributes::WriteCombining => MtrrMemoryCacheType::WriteCombining,
        MemoryAttributes::WriteThrough => MtrrMemoryCacheType::WriteThrough,
        MemoryAttributes::WriteProtect => MtrrMemoryCacheType::WriteProtected,
        MemoryAttributes::Writeback => MtrrMemoryCacheType::WriteBack,
        _ => return Err(PagingError::Unsupported),
    };

    if mtrr.get_memory_attribute(base_address)? != cache_type {
        mtrr.set_memory_attribute(base_address, length, cache_type)?;
    }

    Ok(())
}

/// Create an `x86_64` paging instance under the general `PatinaPageTable` trait.
#[cfg_attr(coverage, coverage(off))]
pub fn create_cpu_x64_paging<A: PageAllocator + 'static>(
    page_allocator: A,
) -> Result<impl PatinaPageTable, efi::Status> {
    let mtrr = create_mtrr_lib(0);
    Ok(EfiCpuPagingX64 {
        paging: X64PageTable::new(page_allocator, PagingType::Paging4Level)
            .map_err(|_| efi::Status::INVALID_PARAMETER)?,
        mtrr_supported: mtrr.is_supported(),
        mtrr,
    })
}

/// Open the active `x86_64` page table wrapped in the `PatinaPageTable` trait.
///
/// ## Safety
/// The caller must ensure no other entity is concurrently modifying the page tables.
#[cfg_attr(coverage, coverage(off))]
pub unsafe fn open_active_cpu_x64_paging<A: PageAllocator + 'static>(
    page_allocator: A,
) -> Result<impl PatinaPageTable, PagingError> {
    // SAFETY: Caller ensures no concurrent page table modifications.
    let page_table = unsafe { X64PageTable::open_active(page_allocator)? };
    let mtrr = create_mtrr_lib(0);
    Ok(EfiCpuPagingX64 { paging: page_table, mtrr_supported: mtrr.is_supported(), mtrr })
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina_mtrr::MockMtrr;
    use patina_paging::{MockPageTable, PtError};

    #[test]
    fn test_map_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));
        mock_mtrr.expect_get_memory_attribute().returning(|_| Ok(MtrrMemoryCacheType::Uncacheable));
        mock_mtrr.expect_set_memory_attribute().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

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
        mock_mtrr.expect_get_memory_attribute().returning(|address| {
            assert_eq!(address, 0x1000);
            Ok(MtrrMemoryCacheType::Uncacheable)
        });
        mock_mtrr.expect_set_memory_attribute().returning(|address, size, cache_type| {
            assert_eq!(address, 0x1000);
            assert_eq!(size, 0x1000);
            assert_eq!(cache_type, MtrrMemoryCacheType::WriteBack);
            Ok(())
        });

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

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

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

        let result = paging.unmap_memory_region(0x1000, 0x1000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_remap_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));
        mock_mtrr.expect_get_memory_attribute().returning(|_| Ok(MtrrMemoryCacheType::Uncacheable));
        mock_mtrr.expect_set_memory_attribute().returning(|_, _, _| Ok(()));

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

        let result = paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Uncached);
        assert!(result.is_ok());
    }

    #[test]
    fn test_map_memory_region_with_unsupported_mtrrs() {
        let mut mock_page_table = MockPageTable::new();
        let mock_mtrr = MockMtrr::new();

        mock_page_table.expect_map_memory_region().returning(|_, _, attributes| {
            assert_eq!(attributes, MemoryAttributes::ReadOnly);
            Ok(())
        });

        let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: false };

        assert_eq!(
            paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Writeback | MemoryAttributes::ReadOnly),
            Ok(())
        );
    }

    #[test]
    fn test_map_memory_region_programs_each_cache_type() {
        for (attributes, expected) in [
            (MemoryAttributes::Uncached, MtrrMemoryCacheType::Uncacheable),
            (MemoryAttributes::WriteCombining, MtrrMemoryCacheType::WriteCombining),
            (MemoryAttributes::WriteThrough, MtrrMemoryCacheType::WriteThrough),
            (MemoryAttributes::WriteProtect, MtrrMemoryCacheType::WriteProtected),
            (MemoryAttributes::Writeback, MtrrMemoryCacheType::WriteBack),
        ] {
            let mut mock_page_table = MockPageTable::new();
            let mut mock_mtrr = MockMtrr::new();

            mock_page_table.expect_map_memory_region().returning(|_, _, _| Ok(()));
            // Report a type that never matches so the MTRR is always reprogrammed.
            mock_mtrr.expect_get_memory_attribute().returning(|_| Ok(MtrrMemoryCacheType::Invalid));
            mock_mtrr.expect_set_memory_attribute().returning(move |_, _, cache_type| {
                assert_eq!(cache_type, expected);
                Ok(())
            });

            let mut paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

            assert_eq!(paging.map_memory_region(0x1000, 0x1000, attributes), Ok(()));
        }
    }

    #[test]
    fn test_map_memory_region_with_unsupported_cache_type() {
        let paging = &mut EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: MockMtrr::new(), mtrr_supported: true };

        // UncachedExport has no MTRR equivalent.
        assert_eq!(
            paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::UncachedExport),
            Err(PagingError::Unsupported)
        );

        // Multiple cache types cannot be applied at once either.
        assert_eq!(
            paging.map_memory_region(0x1000, 0x1000, MemoryAttributes::Uncached | MemoryAttributes::Writeback),
            Err(PagingError::Unsupported)
        );
    }

    #[test]
    fn test_query_memory_region_reports_each_cache_type() {
        for (cache_type, expected) in [
            (MtrrMemoryCacheType::Uncacheable, MemoryAttributes::Uncached),
            (MtrrMemoryCacheType::WriteCombining, MemoryAttributes::WriteCombining),
            (MtrrMemoryCacheType::WriteThrough, MemoryAttributes::WriteThrough),
            (MtrrMemoryCacheType::WriteProtected, MemoryAttributes::WriteProtect),
            (MtrrMemoryCacheType::WriteBack, MemoryAttributes::Writeback),
        ] {
            let mut mock_page_table = MockPageTable::new();
            let mut mock_mtrr = MockMtrr::new();

            mock_page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::ReadOnly));
            mock_mtrr.expect_get_memory_attribute().returning(move |_| Ok(cache_type));

            let paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

            assert_eq!(paging.query_memory_region(0x1000, 0x1000), Ok(MemoryAttributes::ReadOnly | expected));
        }
    }

    #[test]
    fn test_query_memory_region_with_invalid_cache_type() {
        for cache_type in [MtrrMemoryCacheType::Reserved1, MtrrMemoryCacheType::Reserved2, MtrrMemoryCacheType::Invalid]
        {
            let mut mock_mtrr = MockMtrr::new();
            mock_mtrr.expect_get_memory_attribute().returning(move |_| Ok(cache_type));

            let paging = EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: mock_mtrr, mtrr_supported: true };

            assert_eq!(paging.query_memory_region(0x1000, 0x1000), Err((PagingError::InvalidParameter, None)));
        }
    }

    #[test]
    fn test_query_memory_region_with_unexpected_mtrr_error() {
        let mut mock_mtrr = MockMtrr::new();
        mock_mtrr.expect_get_memory_attribute().returning(|_| Err(MtrrError::OutOfResources));

        let paging = EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: mock_mtrr, mtrr_supported: true };

        assert_eq!(paging.query_memory_region(0x1000, 0x1000), Err((PagingError::OutOfResources, None)));
    }

    #[test]
    fn test_query_memory_region() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::Writeback));
        mock_mtrr.expect_get_memory_attribute().returning(|_| Ok(MtrrMemoryCacheType::Uncacheable));

        let paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

        let result = paging.query_memory_region(0x1000, 0x1000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), MemoryAttributes::Writeback | MemoryAttributes::Uncached);
    }

    #[test]
    fn test_query_unmapped_memory_region_returns_cache_attributes() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_query_memory_region().returning(|_, _| Err(PtError::NoMapping));
        mock_mtrr.expect_get_memory_attribute().returning(|_| Ok(MtrrMemoryCacheType::WriteBack));

        let paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: true };

        assert_eq!(
            paging.query_memory_region(0x1000, 0x1000),
            Err((PagingError::NoMapping, Some(MemoryAttributes::Writeback)))
        );
    }

    #[test]
    fn test_query_memory_region_with_unsupported_mtrrs() {
        let mut mock_page_table = MockPageTable::new();
        let mut mock_mtrr = MockMtrr::new();

        mock_page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::ReadOnly));
        mock_mtrr.expect_get_memory_attribute().returning(|_| Err(MtrrError::MtrrNotSupported));

        let paging = EfiCpuPagingX64 { paging: mock_page_table, mtrr: mock_mtrr, mtrr_supported: false };

        assert_eq!(paging.query_memory_region(0x1000, 0x1000), Ok(MemoryAttributes::ReadOnly));
    }

    #[test]
    fn test_cache_attribute_source() {
        let supported = EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: MockMtrr::new(), mtrr_supported: true };
        assert_eq!(supported.cache_attribute_source(), CacheAttributeSource::Processor);

        let unsupported =
            EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: MockMtrr::new(), mtrr_supported: false };
        assert_eq!(unsupported.cache_attribute_source(), CacheAttributeSource::Unsupported);
    }

    #[test]
    fn test_handle_cacheability_change() {
        let paging = EfiCpuPagingX64 { paging: MockPageTable::new(), mtrr: MockMtrr::new(), mtrr_supported: true };
        paging.handle_cacheability_change(0x1000, 0x1000, MemoryAttributes::Writeback, MemoryAttributes::Uncached);
    }
}
