//! Paging Page Allocator
//!
//! A dedicated page allocator for the paging subsystem that allocates pages for
//! page table structures (PML4, PDPT, PD, PT entries).
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

/// Default number of pages to reserve for page table allocations.
/// This should be sufficient for most MM environments (128 pages = 512KB).
pub const DEFAULT_PAGING_POOL_PAGES: usize = 128;

/// A dedicated page allocator for the paging subsystem.
///
/// This allocator uses a simple bump allocator from a reserved pool of pages.
/// It implements the `patina_paging::PageAllocator` trait to be used directly
/// by the paging crate for allocating page table structures.
///
/// ## Thread Safety
///
/// This allocator is thread-safe and can be used from multiple CPUs.
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

    /// Initializes the paging allocator with a reserved memory region.
    ///
    /// `pool_base` must be page-aligned.
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

        // SAFETY: `init` is an `unsafe fn` whose contract (see `# Safety` above) requires the
        // caller to provide `pool_base`/`pool_pages` describing a valid, exclusively-owned region
        // of at least `pool_pages * UEFI_PAGE_SIZE` bytes (typically reserved SMRAM untouched by
        // other code). `pool_base` was validated non-zero and page-aligned above to mitigate the
        // risk of undefined behavior.
        unsafe {
            core::ptr::write_bytes(pool_base as *mut u8, 0, pool_bytes);
        }

        *state = PagingPoolState { pool_base, pool_pages, current_offset: 0, allocated_pages: 0, initialized: true };

        log::info!("Paging allocator initialized: base=0x{pool_base:016x}, pages={pool_pages}");

        Ok(())
    }

    /// Allocates a page for page table structures.
    ///
    /// `align` must be a power of 2 and at least `UEFI_PAGE_SIZE`, `size` must be
    /// at least `UEFI_PAGE_SIZE`, and `is_root` indicates whether this is a root
    /// page table (e.g., PML4).
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

        // Check if we have enough space
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
    /// Allocates a page for page table structures.
    ///
    /// This implements the `patina_paging::PageAllocator` trait.
    fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError> {
        self.allocate_page_internal(align, size, is_root).map_err(|error| paging_error_to_pt(error, align, size))
    }
}

/// Maps an allocator failure onto the `PtError` the paging crate expects.
///
/// Only [`AllocError::OutOfMemory`] becomes `OutOfResources`. Everything else describes a request
/// the allocator could not honour, so it becomes `InvalidParameter`. The match is kept total
/// rather than exhaustive over the reachable variants, because panicking here would take down MM
/// if the error set grows.
fn paging_error_to_pt(error: MmSupervisorError, align: u64, size: u64) -> PtError {
    log::error!("Paging allocator failed a request for 0x{size:x} bytes at align 0x{align:x}: {error}");
    match error {
        MmSupervisorError::Alloc(AllocError::OutOfMemory) => PtError::OutOfResources,
        // Everything else the allocator can report is a malformed request rather than exhaustion.
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
