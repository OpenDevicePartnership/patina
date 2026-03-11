//! Paging Page Allocator
//!
//! A dedicated page allocator for the paging subsystem that allocates pages for
//! page table structures (PML4, PDPT, PD, PT entries).
//!
//! ## Design
//!
//! This allocator is separate from the generic PageAllocator for two reasons:
//!
//! 1. **Bootstrap problem**: The paging subsystem needs to allocate pages for page
//!    tables, but the generic PageAllocator wants to call into paging to set page
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
//! After the paging subsystem is fully initialized, the generic PageAllocator can
//! optionally register a callback to apply page table attributes to newly allocated
//! pages via the paging instance.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    base::{UEFI_PAGE_SIZE, align_up},
    uefi_pages_to_size, uefi_size_to_pages,
};
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

        // SAFETY: `init` is an `unsafe fn` whose contract (see `# Safety` above) requires the
        // caller to provide `pool_base`/`pool_pages` describing a valid, exclusively-owned region
        // of at least `pool_pages * UEFI_PAGE_SIZE` bytes (typically reserved SMRAM untouched by
        // other code). `pool_base` was validated non-zero and page-aligned above to mitigate the
        // risk of undefined behavior.
        unsafe {
            core::ptr::write_bytes(pool_base as *mut u8, 0, uefi_pages_to_size!(pool_pages));
        }

        *state = PagingPoolState { pool_base, pool_pages, current_offset: 0, allocated_pages: 0, initialized: true };

        log::info!("Paging allocator initialized: base=0x{:016x}, pages={}", pool_base, pool_pages);

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

        let pages_needed = uefi_size_to_pages!(size as usize);

        // Calculate the aligned address
        let current_addr = state.pool_base + state.current_offset as u64;
        let aligned_addr = align_up(current_addr, align).map_err(|_| PagingAllocError::InvalidAlignment)?;
        let padding = (aligned_addr - current_addr) as usize;
        let total_bytes = padding + uefi_pages_to_size!(pages_needed);

        // Check if we have enough space
        if state.current_offset + total_bytes > uefi_pages_to_size!(state.pool_pages) {
            log::error!(
                "Paging allocator out of memory: need {} bytes, have {} bytes remaining",
                total_bytes,
                uefi_pages_to_size!(state.pool_pages) - state.current_offset
            );
            return Err(PagingAllocError::OutOfMemory);
        }

        // Update the bump pointer and allocation count.
        state.current_offset += total_bytes;
        state.allocated_pages += pages_needed;

        log::trace!(
            "Paging allocator: allocated {} page(s) at 0x{:016x} (align=0x{:x})",
            pages_needed,
            aligned_addr,
            align
        );

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
        state.pool_pages.saturating_sub(state.allocated_pages)
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
        self.allocate_page_internal(align, size, is_root).map_err(|e| {
            log::error!("Paging allocator error: {:?}", e);
            match e {
                PagingAllocError::NotInitialized => PtError::InvalidParameter,
                PagingAllocError::AlreadyInitialized => PtError::InvalidParameter,
                PagingAllocError::OutOfMemory => PtError::OutOfResources,
                PagingAllocError::InvalidAlignment => PtError::InvalidParameter,
                PagingAllocError::PoolTooSmall => PtError::InvalidParameter,
            }
        })
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
        allocator.allocate_page_internal(align, size, is_root).map_err(|e| {
            log::error!("Paging allocator error: {:?}", e);
            match e {
                PagingAllocError::NotInitialized => PtError::InvalidParameter,
                PagingAllocError::AlreadyInitialized => PtError::InvalidParameter,
                PagingAllocError::OutOfMemory => PtError::OutOfResources,
                PagingAllocError::InvalidAlignment => PtError::InvalidParameter,
                PagingAllocError::PoolTooSmall => PtError::InvalidParameter,
            }
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_paging_allocator_not_initialized() {
        let allocator = PagingPoolAllocator::new();
        assert!(!allocator.is_initialized());
        assert_eq!(allocator.free_page_count(), 0);
        assert_eq!(allocator.allocated_page_count(), 0);
    }

    #[test]
    fn test_paging_allocator_init() {
        let allocator = PagingPoolAllocator::new();

        // Create a test buffer
        let mut buffer = vec![0u8; uefi_pages_to_size!(16)];
        let base = buffer.as_mut_ptr() as u64;
        // Align to page boundary
        let aligned_base = align_up(base, UEFI_PAGE_SIZE as u64).unwrap();

        // SAFETY: `aligned_base` is a page-aligned pointer into `buffer`, a live 16-page heap
        // allocation, so it covers `init`'s required 8 valid pages.
        unsafe {
            assert!(allocator.init(aligned_base, 8).is_ok());
        }

        assert!(allocator.is_initialized());
        assert_eq!(allocator.free_page_count(), 8);
        assert_eq!(allocator.allocated_page_count(), 0);
    }

    #[test]
    fn test_paging_allocator_double_init() {
        let allocator = PagingPoolAllocator::new();

        let mut buffer = vec![0u8; uefi_pages_to_size!(16)];
        let base = buffer.as_mut_ptr() as u64;
        let aligned_base = align_up(base, UEFI_PAGE_SIZE as u64).unwrap();

        // SAFETY: `aligned_base` is a page-aligned pointer into `buffer`, a live 16-page heap
        // allocation, so it covers `init`'s required 8 valid pages.
        unsafe {
            assert!(allocator.init(aligned_base, 8).is_ok());
            assert_eq!(allocator.init(aligned_base, 8), Err(PagingAllocError::AlreadyInitialized));
        }
    }

    #[test]
    fn test_paging_allocator_allocate() {
        let allocator = PagingPoolAllocator::new();

        let mut buffer = vec![0u8; uefi_pages_to_size!(32)];
        let base = buffer.as_mut_ptr() as u64;
        let aligned_base = align_up(base, UEFI_PAGE_SIZE as u64).unwrap();

        // SAFETY: `aligned_base` is a page-aligned pointer into `buffer`, a live 32-page heap
        // allocation, so it covers `init`'s required 16 valid pages.
        unsafe {
            allocator.init(aligned_base, 16).unwrap();
        }

        // Allocate a page
        let result = allocator.allocate_page_internal(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false);
        assert!(result.is_ok());
        let addr = result.unwrap();
        assert_eq!(addr, aligned_base);
        assert_eq!(allocator.allocated_page_count(), 1);

        // Allocate another page
        let result2 = allocator.allocate_page_internal(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false);
        assert!(result2.is_ok());
        let addr2 = result2.unwrap();
        assert_eq!(addr2, aligned_base + UEFI_PAGE_SIZE as u64);
        assert_eq!(allocator.allocated_page_count(), 2);
    }
}
