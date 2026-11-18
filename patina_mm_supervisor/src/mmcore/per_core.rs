//! Per-Core Storage
//!
//! Allocates the per-core arrays of the [`CpuManager`](super::cpu::CpuManager) and the
//! [`MailboxManager`](super::mailbox::MailboxManager) from supervisor pages during BSP
//! initialization. The pages stay allocated for the supervisor's lifetime.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::mem::{align_of, size_of};

use patina::UEFI_PAGE_SIZE;

use crate::{
    error::MmSupervisorResult,
    memory::{AllocError, PageAllocator},
};

/// Allocates supervisor pages for `count` values of `T`, sets each to `T::default()`, and returns
/// them as a slice. The pages are never freed, and once the page table is installed they are
/// mapped supervisor-only, read/write and non-executable.
///
/// ## Errors
///
/// Returns [`AllocError::InvalidSize`] when `count` is zero or its byte size overflows, and the
/// page allocator's error when no run of free pages is large enough.
pub(crate) fn allocate_per_core_slice<T: Default>(
    page_allocator: &PageAllocator,
    count: usize,
) -> MmSupervisorResult<&'static [T]> {
    const { assert!(align_of::<T>() <= UEFI_PAGE_SIZE, "per-core storage starts on a page boundary") };

    let bytes = count.checked_mul(size_of::<T>()).ok_or(AllocError::InvalidSize)?;
    // `uefi_size_to_pages!` adds `UEFI_PAGE_MASK` first, which can overflow here.
    let base = page_allocator.allocate_pages(bytes.div_ceil(UEFI_PAGE_SIZE))?;
    let address = usize::try_from(base).map_err(|_| AllocError::InvalidAddress)?;
    let first = core::ptr::with_exposed_provenance_mut::<T>(address);

    // SAFETY: `allocate_pages` returned an unowned, page-aligned run of at least `bytes` bytes,
    // and a page boundary satisfies `T`'s alignment as asserted above. `write` does not read or
    // drop the old contents. Every element is written before the slice is formed, and the pages
    // are never freed, so the slice can be `'static`.
    unsafe {
        for index in 0..count {
            first.add(index).write(T::default());
        }
        Ok(core::slice::from_raw_parts(first, count))
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{PageAlignedMemory, scan_regions, smram_hob_list};

    fn page_allocator_over(memory: &PageAlignedMemory) -> PageAllocator {
        let allocator = PageAllocator::new();
        let hob_list = smram_hob_list(memory);
        // SAFETY: the scanned descriptor references the live, exclusively owned `memory`.
        unsafe { allocator.init_from_regions(&scan_regions(&hob_list)) }.expect("the region supports the allocator");
        allocator
    }

    /// One cache line, the size of an `ApMailbox`.
    #[derive(Default)]
    #[repr(C, align(64))]
    struct CacheLine(u8);

    /// One page.
    #[derive(Default)]
    #[repr(C, align(4096))]
    struct Page(u8);

    /// A value whose default is not zero, so a written element can be told from fresh memory.
    #[derive(Debug, PartialEq)]
    struct Marker(u32);

    impl Default for Marker {
        fn default() -> Self {
            Self(7)
        }
    }

    #[test]
    fn test_per_core_slice_rounds_up_to_whole_pages() {
        let memory = Box::leak(Box::new(PageAlignedMemory::new(8)));
        let allocator = page_allocator_over(memory);
        let free_before = allocator.free_page_count();

        // 65 cache lines are one line more than a page holds, so two pages are taken.
        let slice = allocate_per_core_slice::<CacheLine>(&allocator, 65).expect("two pages are free");

        assert_eq!(slice.len(), 65);
        assert_eq!(slice.as_ptr().addr() % UEFI_PAGE_SIZE, 0);
        assert_eq!(free_before - allocator.free_page_count(), 2);
    }

    #[test]
    fn test_per_core_slice_takes_one_page_for_an_exact_fit() {
        let memory = Box::leak(Box::new(PageAlignedMemory::new(8)));
        let allocator = page_allocator_over(memory);
        let free_before = allocator.free_page_count();

        // 64 cache lines fill one page exactly, so no second page is taken.
        let slice = allocate_per_core_slice::<CacheLine>(&allocator, 64).expect("a page is free");

        assert_eq!(slice.len(), 64);
        assert_eq!(free_before - allocator.free_page_count(), 1);
    }

    #[test]
    fn test_per_core_slice_sets_every_element_to_its_default() {
        let memory = Box::leak(Box::new(PageAlignedMemory::new(8)));
        let allocator = page_allocator_over(memory);

        let slice = allocate_per_core_slice::<Marker>(&allocator, 3).expect("a page is free");

        assert_eq!(slice, [Marker(7), Marker(7), Marker(7)]);
    }

    #[test]
    fn test_per_core_slice_rejects_an_empty_or_overflowing_count() {
        let memory = Box::leak(Box::new(PageAlignedMemory::new(8)));
        let allocator = page_allocator_over(memory);

        assert_eq!(allocate_per_core_slice::<u64>(&allocator, 0).err(), Some(AllocError::InvalidSize.into()));
        assert_eq!(allocate_per_core_slice::<u64>(&allocator, usize::MAX).err(), Some(AllocError::InvalidSize.into()));
    }

    #[test]
    fn test_per_core_slice_reports_a_count_the_allocator_cannot_hold() {
        let memory = Box::leak(Box::new(PageAlignedMemory::new(4)));
        let allocator = page_allocator_over(memory);

        assert!(allocate_per_core_slice::<Page>(&allocator, 64).is_err());
    }
}
