//! Paging Page Allocator
//!
//! A bump allocator that hands out pages for page table structures (PML4, PDPT, PD and PT)
//! from a pool reserved at initialization.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{
    error::{MmSupervisorError, MmSupervisorResult},
    memory::AllocError,
};
use patina::{MemoryRange, UEFI_PAGE_SIZE, align_up};
use patina_paging::{PtError, page_allocator::PageAllocator as PagingPageAllocator};
use spin::Mutex;

/// Number of pages reserved for page table structures: 128 pages (512 KiB).
///
/// The supervisor adopts the page table that is active at entry, so this pool only backs tables
/// created afterwards. Those come from splitting a 2 MiB or 1 GiB mapping when a 4 KiB attribute
/// change falls inside it, which takes one or two pages per split. The pool is a bump allocator
/// that never reclaims pages and cannot grow after init, so it must cover every split for the
/// supervisor's lifetime. 128 pages covers splits in 128 distinct 2 MiB regions. When it runs
/// out, the attribute change fails with `PtError::OutOfResources`.
pub const DEFAULT_PAGING_POOL_PAGES: usize = 128;

/// A bump allocator over a reserved pool of pages, used for page table structures.
///
/// It implements `patina_paging::PageAllocator` so the paging crate can allocate from it
/// directly. One lock serializes every operation, so it can be shared across CPUs.
pub struct PagingPoolAllocator {
    /// All mutable state, guarded by a single lock that serializes every operation.
    state: Mutex<PagingPoolState>,
}

/// Mutable state of the paging pool allocator, guarded by [`PagingPoolAllocator`]'s lock.
struct PagingPoolState {
    /// Base address of the pool (0 until initialized).
    pool_base: u64,
    /// Total number of pages in the pool.
    pool_pages: usize,
    /// Current allocation offset (bump pointer) in bytes.
    current_offset: usize,
    /// Number of pages allocated.
    allocated_pages: usize,
    /// Whether the allocator has been initialized.
    initialized: bool,
}

impl PagingPoolAllocator {
    /// Creates a new uninitialized paging page allocator.
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(PagingPoolState {
                pool_base: 0,
                pool_pages: 0,
                current_offset: 0,
                allocated_pages: 0,
                initialized: false,
            }),
        }
    }

    /// Converts a page count to bytes without overflowing.
    fn page_bytes(pages: usize) -> Option<usize> {
        pages.checked_mul(UEFI_PAGE_SIZE)
    }

    /// Initializes the allocator over a reserved, page-aligned region and zeroes it.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that:
    /// - `pool_base` points to a valid memory region in SMRAM
    /// - The region is not used by any other allocator
    /// - The region has at least `pool_pages * UEFI_PAGE_SIZE` bytes available
    pub unsafe fn init(&self, pool_base: u64, pool_pages: usize) -> MmSupervisorResult<()> {
        let mut state = self.state.lock();
        if state.initialized {
            log::error!("Paging allocator init rejected: already initialized");
            return Err(AllocError::AlreadyInitialized.into());
        }

        if pool_base == 0 || pool_pages == 0 {
            log::error!("Paging allocator init rejected: base=0x{pool_base:016x}, pages={pool_pages}");
            return Err(AllocError::PoolTooSmall.into());
        }

        if !pool_base.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("Paging allocator init rejected: base 0x{pool_base:016x} is not page aligned");
            return Err(AllocError::InvalidAlignment.into());
        }

        let pool = MemoryRange::from_pages(pool_base, pool_pages as u64).map_err(|err| {
            log::error!("Paging allocator init rejected: {pool_pages} pages at 0x{pool_base:016x}: {err}");
            AllocError::PoolTooSmall
        })?;
        let pool_bytes = usize::try_from(pool.size()).map_err(|_| {
            log::error!("Paging allocator init rejected: pool {pool} does not fit a usize");
            AllocError::PoolTooSmall
        })?;

        // SAFETY: the caller guarantees `pool_base` and `pool_pages` describe a valid, exclusively
        // owned region of at least `pool_pages * UEFI_PAGE_SIZE` bytes, which is `pool_bytes`.
        // `pool_base` was checked non-zero and page-aligned above.
        unsafe {
            core::ptr::write_bytes(pool_base as *mut u8, 0, pool_bytes);
        }

        *state = PagingPoolState { pool_base, pool_pages, current_offset: 0, allocated_pages: 0, initialized: true };

        log::info!("Paging allocator initialized: base=0x{pool_base:016x}, pages={pool_pages}");

        Ok(())
    }

    /// Allocates pages for a page table structure.
    ///
    /// `align` must be a power of 2 and at least `UEFI_PAGE_SIZE`, and `size` must be
    /// at least `UEFI_PAGE_SIZE`. `_is_root` comes from the paging crate's trait and is unused.
    pub fn allocate_page_internal(&self, align: u64, size: u64, _is_root: bool) -> MmSupervisorResult<u64> {
        let mut state = self.state.lock();
        if !state.initialized {
            return Err(AllocError::NotInitialized.into());
        }

        if align < UEFI_PAGE_SIZE as u64 || !align.is_power_of_two() {
            return Err(AllocError::InvalidAlignment.into());
        }
        if size < UEFI_PAGE_SIZE as u64 {
            return Err(AllocError::InvalidSize.into());
        }

        let size = usize::try_from(size).map_err(|_| AllocError::InvalidSize)?;
        // `div_ceil` instead of `uefi_size_to_pages!`: the paging crate passes `size` unchecked,
        // and the macro's `size + UEFI_PAGE_MASK` overflows for sizes near `usize::MAX`.
        let pages_needed = size.div_ceil(UEFI_PAGE_SIZE);
        let allocation_bytes = Self::page_bytes(pages_needed).ok_or(AllocError::InvalidSize)?;

        // Calculate the aligned address
        let current_offset = u64::try_from(state.current_offset).map_err(|_| AllocError::OutOfMemory)?;
        let current_addr = state.pool_base.checked_add(current_offset).ok_or(AllocError::OutOfMemory)?;
        let aligned_addr = align_up(current_addr, align).map_err(|_| AllocError::InvalidAlignment)?;
        let padding = usize::try_from(aligned_addr - current_addr).map_err(|_| AllocError::OutOfMemory)?;
        let total_bytes = padding.checked_add(allocation_bytes).ok_or(AllocError::OutOfMemory)?;
        let new_offset = state.current_offset.checked_add(total_bytes).ok_or(AllocError::OutOfMemory)?;
        let pool_bytes = Self::page_bytes(state.pool_pages).ok_or(AllocError::OutOfMemory)?;

        if new_offset > pool_bytes {
            // `paging_error_to_pt` reports the failed request; this adds how much the pool
            // had left, which is what matters when sizing the pool.
            log::error!(
                "Paging allocator out of memory: need {} bytes, have {} bytes remaining",
                total_bytes,
                pool_bytes - state.current_offset
            );
            return Err(AllocError::OutOfMemory.into());
        }

        // Update the bump pointer and allocation count.
        let allocated_pages = state.allocated_pages.checked_add(pages_needed).ok_or(AllocError::OutOfMemory)?;
        state.current_offset = new_offset;
        state.allocated_pages = allocated_pages;

        log::trace!("Paging allocator: allocated {pages_needed} page(s) at 0x{aligned_addr:016x} (align=0x{align:x})");

        Ok(aligned_addr)
    }

    /// Returns whether the allocator has been initialized.
    #[cfg(test)]
    pub fn is_initialized(&self) -> bool {
        self.state.lock().initialized
    }

    /// Returns the number of pages still available in the pool.
    #[cfg(test)]
    pub fn free_page_count(&self) -> usize {
        let state = self.state.lock();
        Self::page_bytes(state.pool_pages)
            .map_or(0, |pool_bytes| pool_bytes.saturating_sub(state.current_offset) / UEFI_PAGE_SIZE)
    }

    /// Returns the number of pages allocated so far.
    #[cfg(test)]
    pub fn allocated_page_count(&self) -> usize {
        self.state.lock().allocated_pages
    }
}

impl PagingPageAllocator for PagingPoolAllocator {
    fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError> {
        self.allocate_page_internal(align, size, is_root).map_err(|error| paging_error_to_pt(error, align, size))
    }
}

/// Maps an allocator failure onto the `PtError` the paging crate expects.
///
/// Only [`AllocError::OutOfMemory`] becomes `OutOfResources`. Everything else describes a request
/// the allocator could not honour, so it becomes `InvalidParameter`. A wildcard arm covers those
/// rather than listing them, so a new error variant cannot make this panic and take down MM.
fn paging_error_to_pt(error: MmSupervisorError, align: u64, size: u64) -> PtError {
    log::error!("Paging allocator failed a request for 0x{size:x} bytes at align 0x{align:x}: {error}");
    match error {
        MmSupervisorError::Alloc(AllocError::OutOfMemory) => PtError::OutOfResources,
        _ => PtError::InvalidParameter,
    }
}

/// A wrapper around [`PagingPoolAllocator`] that implements the
/// `patina_paging::PageAllocator` trait over a shared `&'static` reference.
pub struct SharedPagingAllocator {
    /// The underlying allocator.
    inner: &'static PagingPoolAllocator,
}

impl SharedPagingAllocator {
    /// Creates a new shared paging allocator wrapper.
    pub const fn new(allocator: &'static PagingPoolAllocator) -> Self {
        Self { inner: allocator }
    }
}

impl PagingPageAllocator for SharedPagingAllocator {
    fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError> {
        let allocator = self.inner;
        allocator.allocate_page_internal(align, size, is_root).map_err(|error| paging_error_to_pt(error, align, size))
    }
}
