//! Paging Page Allocator
//!
//! A dedicated page allocator for the paging subsystem that allocates pages for
//! page table structures (PML4, PDPT, PD, PT entries).
//!
//! ## Design
//!
//! This allocator is separate from the generic `PageAllocator` for two reasons:
//!
//! 1. **Bootstrap problem**: The paging subsystem needs to allocate pages for page
//!    tables, but the generic `PageAllocator` wants to call into paging to set page
//!    attributes for newly allocated pages. This creates a circular dependency.
//!
//! 2. **Security**: Page table pages require special attributes (Supervisor, RW,
//!    non-executable) and should be tracked separately from general allocations.
//!
//! ## Initialization
//!
//! The paging allocator is initialized with a reserved memory region from SMRAM.
//! This region is exclusively used for page table allocations.
//!
//! ## Integration with Paging
//!
//! After the paging subsystem is fully initialized, the generic `PageAllocator` can
//! optionally register a callback to apply page table attributes to newly allocated
//! pages via the paging instance.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{UEFI_PAGE_SIZE, align_up};
use patina_paging::{PtError, page_allocator::PageAllocator as PagingPageAllocator};
use spin::Mutex;

/// Default number of pages to reserve for page table allocations.
/// This should be sufficient for most MM environments (128 pages = 512KB).
pub const DEFAULT_PAGING_POOL_PAGES: usize = 128;

/// Errors that can occur during paging allocator operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingAllocError {
    /// The allocator has not been initialized.
    NotInitialized,
    /// Already initialized.
    AlreadyInitialized,
    /// No free pages available to satisfy the request.
    OutOfMemory,
    /// Invalid alignment requested.
    InvalidAlignment,
    /// Invalid allocation size requested.
    InvalidSize,
    /// The pool region is too small.
    PoolTooSmall,
}

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
    ///
    /// ## Errors
    ///
    /// Returns an error if already initialized or if parameters are invalid.
    pub unsafe fn init(&self, pool_base: u64, pool_pages: usize) -> Result<(), PagingAllocError> {
        let mut state = self.state.lock();
        if state.initialized {
            return Err(PagingAllocError::AlreadyInitialized);
        }

        if pool_base == 0 || pool_pages == 0 {
            return Err(PagingAllocError::PoolTooSmall);
        }

        if !pool_base.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            return Err(PagingAllocError::InvalidAlignment);
        }

        let pool_bytes = Self::page_bytes(pool_pages).ok_or(PagingAllocError::PoolTooSmall)?;
        let pool_bytes_u64 = u64::try_from(pool_bytes).map_err(|_| PagingAllocError::PoolTooSmall)?;
        pool_base.checked_add(pool_bytes_u64).ok_or(PagingAllocError::PoolTooSmall)?;

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
    pub fn allocate_page_internal(&self, align: u64, size: u64, _is_root: bool) -> Result<u64, PagingAllocError> {
        let mut state = self.state.lock();
        if !state.initialized {
            return Err(PagingAllocError::NotInitialized);
        }

        if align < UEFI_PAGE_SIZE as u64 || !align.is_power_of_two() {
            return Err(PagingAllocError::InvalidAlignment);
        }
        if size < UEFI_PAGE_SIZE as u64 {
            return Err(PagingAllocError::InvalidSize);
        }

        let size = usize::try_from(size).map_err(|_| PagingAllocError::InvalidSize)?;
        let pages_needed = size.div_ceil(UEFI_PAGE_SIZE);
        let allocation_bytes = Self::page_bytes(pages_needed).ok_or(PagingAllocError::InvalidSize)?;

        // Calculate the aligned address
        let current_offset = u64::try_from(state.current_offset).map_err(|_| PagingAllocError::OutOfMemory)?;
        let current_addr = state.pool_base.checked_add(current_offset).ok_or(PagingAllocError::OutOfMemory)?;
        let aligned_addr = align_up(current_addr, align).map_err(|_| PagingAllocError::InvalidAlignment)?;
        let padding = usize::try_from(aligned_addr - current_addr).map_err(|_| PagingAllocError::OutOfMemory)?;
        let total_bytes = padding.checked_add(allocation_bytes).ok_or(PagingAllocError::OutOfMemory)?;
        let new_offset = state.current_offset.checked_add(total_bytes).ok_or(PagingAllocError::OutOfMemory)?;
        let pool_bytes = Self::page_bytes(state.pool_pages).ok_or(PagingAllocError::OutOfMemory)?;

        // Check if we have enough space
        if new_offset > pool_bytes {
            log::error!(
                "Paging allocator out of memory: need {} bytes, have {} bytes remaining",
                total_bytes,
                pool_bytes - state.current_offset
            );
            return Err(PagingAllocError::OutOfMemory);
        }

        // Update the bump pointer and allocation count.
        let allocated_pages = state.allocated_pages.checked_add(pages_needed).ok_or(PagingAllocError::OutOfMemory)?;
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
        self.allocate_page_internal(align, size, is_root).map_err(paging_error_to_pt)
    }
}

fn paging_error_to_pt(error: PagingAllocError) -> PtError {
    log::error!("Paging allocator error: {error:?}");
    match error {
        PagingAllocError::NotInitialized
        | PagingAllocError::AlreadyInitialized
        | PagingAllocError::InvalidAlignment
        | PagingAllocError::InvalidSize
        | PagingAllocError::PoolTooSmall => PtError::InvalidParameter,
        PagingAllocError::OutOfMemory => PtError::OutOfResources,
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
        allocator.allocate_page_internal(align, size, is_root).map_err(paging_error_to_pt)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    const TEST_BUFFER_PAGES: usize = 32;
    const TEST_BUFFER_BYTES: usize = TEST_BUFFER_PAGES * UEFI_PAGE_SIZE;

    #[repr(align(4096))]
    struct AlignedBuffer([u8; TEST_BUFFER_BYTES]);

    fn initialized_allocator(pool_pages: usize) -> (PagingPoolAllocator, Box<AlignedBuffer>) {
        assert!(pool_pages <= TEST_BUFFER_PAGES);
        let mut buffer = Box::new(AlignedBuffer([0xA5; TEST_BUFFER_BYTES]));
        let allocator = PagingPoolAllocator::new();

        // SAFETY: `buffer` is page-aligned, remains live in the returned tuple, and
        // contains at least `pool_pages` pages.
        unsafe {
            allocator.init(buffer.0.as_mut_ptr() as u64, pool_pages).unwrap();
        }

        (allocator, buffer)
    }

    #[test]
    fn test_paging_allocator_not_initialized() {
        let allocator = PagingPoolAllocator::new();
        assert!(!allocator.is_initialized());
        assert_eq!(allocator.free_page_count(), 0);
        assert_eq!(allocator.allocated_page_count(), 0);
        assert_eq!(
            allocator.allocate_page_internal(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false),
            Err(PagingAllocError::NotInitialized)
        );
    }

    #[test]
    fn test_paging_allocator_init_validates_parameters() {
        let mut buffer = Box::new(AlignedBuffer([0xA5; TEST_BUFFER_BYTES]));
        let base = buffer.0.as_mut_ptr() as u64;

        // SAFETY: each non-null address points into the live test buffer. Invalid
        // parameters are rejected before the allocator writes to it.
        unsafe {
            assert_eq!(PagingPoolAllocator::new().init(base, 0), Err(PagingAllocError::PoolTooSmall));
            assert_eq!(PagingPoolAllocator::new().init(base + 1, 1), Err(PagingAllocError::InvalidAlignment));
        }
    }

    #[test]
    fn test_paging_allocator_init_zeroes_only_the_pool() {
        let pool_pages = 8;
        let (allocator, buffer) = initialized_allocator(pool_pages);
        let pool_bytes = pool_pages * UEFI_PAGE_SIZE;

        assert!(allocator.is_initialized());
        assert_eq!(allocator.free_page_count(), pool_pages);
        assert_eq!(allocator.allocated_page_count(), 0);
        assert!(buffer.0[..pool_bytes].iter().all(|&byte| byte == 0));
        assert!(buffer.0[pool_bytes..].iter().all(|&byte| byte == 0xA5));
    }

    #[test]
    fn test_paging_allocator_double_init_preserves_state() {
        let (allocator, mut buffer) = initialized_allocator(8);
        let base = buffer.0.as_mut_ptr() as u64;

        // SAFETY: `buffer` is page-aligned and contains the requested eight pages.
        unsafe {
            assert_eq!(allocator.init(base, 8), Err(PagingAllocError::AlreadyInitialized));
        }
        assert_eq!(allocator.free_page_count(), 8);
        assert_eq!(allocator.allocated_page_count(), 0);
    }

    #[test]
    fn test_paging_allocator_rejects_invalid_requests_without_consuming_space() {
        let (allocator, _buffer) = initialized_allocator(8);
        let page_size = UEFI_PAGE_SIZE as u64;

        for align in [0, page_size / 2, page_size + 1] {
            assert_eq!(
                allocator.allocate_page_internal(align, page_size, false),
                Err(PagingAllocError::InvalidAlignment)
            );
        }
        for size in [0, page_size - 1, u64::MAX] {
            assert_eq!(allocator.allocate_page_internal(page_size, size, false), Err(PagingAllocError::InvalidSize));
        }

        assert_eq!(PagingPoolAllocator::page_bytes(usize::MAX), None);
        assert_eq!(allocator.free_page_count(), 8);
        assert_eq!(allocator.allocated_page_count(), 0);
    }

    #[test]
    fn test_paging_allocator_allocates_and_rounds_up_to_pages() {
        let (allocator, mut buffer) = initialized_allocator(8);
        let base = buffer.0.as_mut_ptr() as u64;
        let page_size = UEFI_PAGE_SIZE as u64;

        assert_eq!(allocator.allocate_page_internal(page_size, page_size, false), Ok(base));
        assert_eq!(allocator.allocate_page_internal(page_size, page_size + 1, false), Ok(base + page_size));
        assert_eq!(allocator.allocated_page_count(), 3);
        assert_eq!(allocator.free_page_count(), 5);
    }

    #[test]
    fn test_paging_allocator_counts_alignment_padding_as_consumed() {
        let (allocator, mut buffer) = initialized_allocator(12);
        let base = buffer.0.as_mut_ptr() as u64;
        let page_size = UEFI_PAGE_SIZE as u64;
        let double_page = page_size * 2;

        // Move the bump pointer to an odd page relative to a two-page boundary.
        let initial_pages = if base.is_multiple_of(double_page) { 1 } else { 2 };
        allocator.allocate_page_internal(page_size, initial_pages * page_size, false).unwrap();

        let aligned = allocator.allocate_page_internal(double_page, page_size, false).unwrap();
        assert!(aligned.is_multiple_of(double_page));
        assert_eq!(allocator.allocated_page_count(), initial_pages as usize + 1);
        assert_eq!(allocator.free_page_count(), 12 - initial_pages as usize - 2);
    }

    #[test]
    fn test_paging_allocator_out_of_memory_does_not_advance_state() {
        let (allocator, _buffer) = initialized_allocator(2);
        let page_size = UEFI_PAGE_SIZE as u64;

        assert!(allocator.allocate_page_internal(page_size, page_size * 2, false).is_ok());
        assert_eq!(allocator.allocate_page_internal(page_size, page_size, false), Err(PagingAllocError::OutOfMemory));
        assert_eq!(allocator.allocated_page_count(), 2);
        assert_eq!(allocator.free_page_count(), 0);
    }

    #[test]
    fn test_paging_allocator_maps_trait_errors() {
        assert_eq!(paging_error_to_pt(PagingAllocError::InvalidSize), PtError::InvalidParameter);
        assert_eq!(paging_error_to_pt(PagingAllocError::OutOfMemory), PtError::OutOfResources);
    }

    #[test]
    fn test_paging_allocator_trait_adapters() {
        let mut allocator = PagingPoolAllocator::new();
        assert_eq!(
            PagingPageAllocator::allocate_page(&mut allocator, UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false),
            Err(PtError::InvalidParameter)
        );

        let shared_allocator = Box::leak(Box::new(PagingPoolAllocator::new()));
        let mut shared = SharedPagingAllocator::new(shared_allocator);
        assert_eq!(
            PagingPageAllocator::allocate_page(&mut shared, UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, true),
            Err(PtError::InvalidParameter)
        );
    }
}
