//! Pool Allocator
//!
//! This module provides a trait-based page allocator abstraction and a generic pool
//! allocator for the MM User Core.
//!
//! ## Design
//!
//! The [`PageAllocatorBackend`] trait abstracts the page allocation mechanism.
//! The user core implements it by issuing `syscall` instructions that thunk
//! into the supervisor for page allocation.
//!
//! The [`PoolAllocator`] is a bump-allocator built on top of any `PageAllocatorBackend`.
//! It implements [`GlobalAlloc`] so it can be used as `#[global_allocator]`.
//!
//! ## Block Management
//!
//! Block metadata is stored **in-band** at the start of each page allocation, forming
//! an intrusive linked list. This means there is no fixed cap on the number of blocks -
//! the allocator grows dynamically as needed by requesting more pages from the backend.
//! When all allocations within a block are freed, the block is unlinked from the list
//! and the pages are returned to the backend.
//!
//! ## Safety
//!
//! The intrusive list is built from `NonNull<PoolBlockHeader>` pointers into
//! pages we own but Rust does not track. A single `spin::Mutex` guards the
//! head pointer and (by convention) the entire list - every traversal,
//! insertion, and removal is performed while the lock is held, and no list
//! pointer ever escapes this module.
//!
//! As a consequence of that discipline, while holding the head lock,
//! dereferencing any `NonNull<PoolBlockHeader>` reachable from the head is
//! sound: it points to a fully-initialized header that no other code can
//! concurrently free or alias. Individual `SAFETY:` comments below refer back
//! to this invariant rather than restating it in full.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    alloc::{GlobalAlloc, Layout},
    mem,
    ptr::{self, NonNull},
};
use patina::{uefi_pages_to_size, uefi_size_to_pages};
use spin::Mutex;

/// Errors that can occur during page allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAllocError {
    /// The allocator has not been initialized.
    NotInitialized,
    /// No free pages available to satisfy the request.
    OutOfMemory,
    /// The requested address is not aligned to page boundary.
    NotAligned,
    /// The address is not within any known SMRAM region.
    InvalidAddress,
    /// The address was not previously allocated.
    NotAllocated,
    /// Too many regions to track.
    TooManyRegions,
    /// A syscall to the supervisor failed.
    SyscallFailed(u64),
}

/// Minimum allocation size for the pool allocator.
const MIN_POOL_ALLOC_SIZE: usize = 16;

/// Trait for page-granularity memory allocation.
///
/// Implementors provide the actual page allocation mechanism. The user core
/// implements this by issuing syscalls to the supervisor.
pub trait PageAllocatorBackend: Send + Sync {
    /// Allocates `num_pages` contiguous pages.
    ///
    /// Returns the physical base address of the allocated region on success.
    fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError>;

    /// Frees `num_pages` contiguous pages starting at `addr`.
    fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError>;

    /// Returns whether the page allocator has been initialized and is ready for use.
    fn is_initialized(&self) -> bool;
}

/// In-band header stored at the beginning of each pool page block.
///
/// By placing the metadata inside the allocated pages themselves, we avoid
/// any fixed-size bookkeeping array. Blocks form a singly-linked list so
/// traversal, insertion, and removal are straightforward.
#[repr(C)]
struct PoolBlockHeader {
    /// Pointer to the next block in the linked list (`None` if this is the tail).
    next: Option<NonNull<PoolBlockHeader>>,
    /// Number of pages backing this block (includes the header).
    num_pages: usize,
    /// Current bump offset (in bytes from the block base). Starts just past the header.
    offset: usize,
    /// Number of live allocations served from this block.
    alloc_count: usize,
}

impl PoolBlockHeader {
    /// Base address of this block (== address of the header itself).
    fn base(&self) -> usize {
        ptr::from_ref::<Self>(self) as usize
    }

    /// Total usable capacity of this block in bytes.
    fn capacity(&self) -> usize {
        uefi_pages_to_size!(self.num_pages)
    }

    /// Remaining bytes available for bump allocation.
    fn remaining(&self) -> usize {
        self.capacity().saturating_sub(self.offset)
    }

    /// Returns `true` if the given address falls within this block's page range.
    fn contains(&self, addr: usize) -> bool {
        addr >= self.base() && addr < self.base() + self.capacity()
    }

    /// Try to bump-allocate `layout` from this block.
    fn try_alloc(&mut self, layout: Layout) -> Option<*mut u8> {
        let current_ptr = self.base() + self.offset;
        let align = layout.align().max(MIN_POOL_ALLOC_SIZE);
        let aligned_ptr = (current_ptr + align - 1) & !(align - 1);
        let padding = aligned_ptr - current_ptr;
        let total_size = padding + layout.size();

        if total_size > self.remaining() {
            return None;
        }

        self.offset += total_size;
        self.alloc_count += 1;

        Some(aligned_ptr as *mut u8)
    }
}

/// Pool allocator built on top of a [`PageAllocatorBackend`].
///
/// This allocator provides smaller-granularity allocations by requesting
/// full pages from the backend and subdividing them via bump allocation.
pub struct PoolAllocator<P: PageAllocatorBackend + 'static> {
    /// Reference to the underlying page allocator.
    page_allocator: &'static P,
    /// Head of the intrusive linked list of pool blocks. The mutex guards
    /// both the head pointer and (transitively) every node reachable from it
    /// - see the module-level safety invariant.
    head: Mutex<Option<NonNull<PoolBlockHeader>>>,
}

// SAFETY: `NonNull<PoolBlockHeader>` is `!Send`/`!Sync` only because raw
// pointers are. The list it heads is guarded by `head`'s mutex and never
// exposed outside this module, so transferring or sharing a `PoolAllocator`
// across threads cannot create a data race.
unsafe impl<P: PageAllocatorBackend> Send for PoolAllocator<P> {}
// SAFETY: As above.
unsafe impl<P: PageAllocatorBackend> Sync for PoolAllocator<P> {}

impl<P: PageAllocatorBackend> PoolAllocator<P> {
    /// Creates a new pool allocator backed by the given page allocator.
    pub const fn new(page_allocator: &'static P) -> Self {
        Self { page_allocator, head: Mutex::new(None) }
    }

    /// Allocate a fresh page block large enough to satisfy `layout`, link it
    /// at the head of the list, and bump-allocate `layout` from it.
    ///
    /// Returns the pointer to the new allocation, or `None` if either the
    /// page allocation or the subsequent bump allocation fails.
    fn allocate_in_new_block(&self, head: &mut Option<NonNull<PoolBlockHeader>>, layout: Layout) -> Option<*mut u8> {
        let header_size = mem::size_of::<PoolBlockHeader>();
        let needed = layout.size() + header_size;
        let num_pages = uefi_size_to_pages!(needed).max(1);

        let base = match self.page_allocator.allocate_pages(num_pages) {
            Ok(addr) => addr,
            Err(e) => {
                log::warn!("Pool allocator: failed to allocate {num_pages} pages: {e:?}");
                return None;
            }
        };

        let mut header_ptr = NonNull::new(base as *mut PoolBlockHeader)?;
        let next = head.replace(header_ptr);

        // SAFETY: `base` was returned by `allocate_pages`, so it points to
        // `num_pages * UEFI_PAGE_SIZE` page-aligned, writable bytes that we own
        // exclusively. `ptr::write` initializes the header without dropping
        // the prior (uninitialized) contents. After that, the module-level
        // invariant holds and `as_mut()` yields the unique `&mut` we use to
        // perform the first bump allocation.
        let header = unsafe {
            ptr::write(header_ptr.as_ptr(), PoolBlockHeader { next, num_pages, offset: header_size, alloc_count: 0 });
            header_ptr.as_mut()
        };

        log::trace!("Pool allocator: new block at {base:#018x} ({num_pages} pages)");

        header.try_alloc(layout)
    }
}

// SAFETY: Uphold the `GlobalAlloc` contract:
// * `alloc` returns either a null pointer (on failure) or a pointer to a
//   block of at least `layout.size()` bytes, aligned to `layout.align()`.
//   `PoolBlockHeader::try_alloc` enforces this by aligning the bump pointer
//   to `max(layout.align(), MIN_POOL_ALLOC_SIZE)` and checking the resulting
//   region fits in the block.
// * Returned pointers remain valid until passed back to `dealloc`: the page
//   backing them is owned by a `PoolBlockHeader` whose `alloc_count` covers
//   this allocation, so the block cannot be unlinked and freed while live.
// * `dealloc` only frees the backing pages once `alloc_count` reaches zero,
//   so it neither invalidates other live allocations nor double-frees pages.
unsafe impl<P: PageAllocatorBackend> GlobalAlloc for PoolAllocator<P> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.page_allocator.is_initialized() {
            return ptr::null_mut();
        }

        let mut head = self.head.lock();

        // Walk the list and try to bump-allocate from an existing block.
        let mut cursor = *head;
        while let Some(mut node) = cursor {
            // SAFETY: `node` is reachable from the locked head, so by the
            // module-level invariant it points to a live `PoolBlockHeader`
            // we have exclusive access to for the duration of this borrow.
            let block = unsafe { node.as_mut() };
            if let Some(ptr) = block.try_alloc(layout) {
                return ptr;
            }
            cursor = block.next;
        }

        // No block had room; grow the pool and serve from a fresh block.
        self.allocate_in_new_block(&mut head, layout).unwrap_or(ptr::null_mut())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }

        let mut head = self.head.lock();
        let addr = ptr as usize;

        let mut prev: Option<NonNull<PoolBlockHeader>> = None;
        let mut cursor = *head;
        while let Some(mut node) = cursor {
            // SAFETY: `node` is reachable from the locked head, see module
            // invariant. The borrow ends at the `if !contains` early-continue
            // or at the field reads below.
            let block = unsafe { node.as_mut() };
            if !block.contains(addr) {
                prev = Some(node);
                cursor = block.next;
                continue;
            }

            block.alloc_count = block.alloc_count.saturating_sub(1);
            if block.alloc_count > 0 {
                return;
            }

            // Capture what we need before re-borrowing through `prev`.
            let base = block.base() as u64;
            let num_pages = block.num_pages;
            let next = block.next;

            match prev {
                None => *head = next,
                // SAFETY: `prev` was set on a previous iteration to a node we
                // had just dereferenced safely; it is still linked (nothing
                // in this walk has freed it) and we hold the head lock. The
                // borrow of `block` ended after the field reads above, so
                // this fresh `as_mut()` does not alias any live reference.
                Some(mut p) => unsafe { p.as_mut().next = next },
            }

            if let Err(e) = self.page_allocator.free_pages(base, num_pages) {
                log::warn!("Pool allocator: failed to free block at {base:#018x}: {e:?}");
            } else {
                log::trace!("Pool allocator: freed block at {base:#018x} ({num_pages} pages)");
            }
            return;
        }

        log::warn!("Pool allocator: dealloc called with unknown pointer {addr:#018x}");
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn page_size() -> usize {
        uefi_pages_to_size!(1)
    }

    fn header_size() -> usize {
        mem::size_of::<PoolBlockHeader>()
    }

    /// A page backend served from the host heap, so the blocks handed to the pool
    /// allocator are genuinely writable and it can initialize its headers in place.
    struct HeapPages {
        ready: AtomicBool,
        allocations_left: AtomicUsize,
        free_fails: AtomicBool,
        granted: Mutex<Vec<(u64, usize)>>,
        released: Mutex<Vec<(u64, usize)>>,
    }

    impl HeapPages {
        fn new() -> Self {
            Self {
                ready: AtomicBool::new(true),
                allocations_left: AtomicUsize::new(usize::MAX),
                free_fails: AtomicBool::new(false),
                granted: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
            }
        }

        fn layout(num_pages: usize) -> Layout {
            Layout::from_size_align(uefi_pages_to_size!(num_pages), page_size()).expect("valid page layout")
        }

        /// Every page run the pool allocator asked for, as `(base, pages)`.
        fn granted(&self) -> Vec<(u64, usize)> {
            self.granted.lock().clone()
        }

        /// Every page run the pool allocator tried to give back.
        fn released(&self) -> Vec<(u64, usize)> {
            self.released.lock().clone()
        }
    }

    impl PageAllocatorBackend for HeapPages {
        fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError> {
            if self.allocations_left.load(Ordering::Relaxed) == 0 {
                return Err(PageAllocError::OutOfMemory);
            }
            self.allocations_left.fetch_sub(1, Ordering::Relaxed);

            // SAFETY: `layout` has a non-zero size, satisfying the `alloc_zeroed` contract.
            let ptr = unsafe { alloc::alloc::alloc_zeroed(Self::layout(num_pages)) };
            assert!(!ptr.is_null(), "the host heap backs the fake page allocator");
            self.granted.lock().push((ptr as u64, num_pages));
            Ok(ptr as u64)
        }

        fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
            self.released.lock().push((addr, num_pages));
            if self.free_fails.load(Ordering::Relaxed) {
                return Err(PageAllocError::NotAllocated);
            }
            // SAFETY: `addr` was produced by `allocate_pages` with this exact layout and the pool
            // allocator hands a block back exactly once.
            unsafe { alloc::alloc::dealloc(addr as *mut u8, Self::layout(num_pages)) };
            Ok(())
        }

        fn is_initialized(&self) -> bool {
            self.ready.load(Ordering::Relaxed)
        }
    }

    /// The allocator borrows its backend for `'static`, so the backend is leaked per test.
    fn new_pool() -> (&'static HeapPages, PoolAllocator<HeapPages>) {
        let backend: &'static HeapPages = alloc::boxed::Box::leak(alloc::boxed::Box::new(HeapPages::new()));
        (backend, PoolAllocator::new(backend))
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).expect("valid layout")
    }

    /// Allocates and asserts the result is usable, returning the pointer.
    fn alloc_ok(pool: &PoolAllocator<HeapPages>, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` has a non-zero size.
        let ptr = unsafe { pool.alloc(layout) };
        assert!(!ptr.is_null(), "allocation of {layout:?} succeeds");
        ptr
    }

    fn free(pool: &PoolAllocator<HeapPages>, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` on this pool with this layout.
        unsafe { pool.dealloc(ptr, layout) };
    }

    #[test]
    fn test_allocation_fails_until_the_page_backend_is_ready() {
        let (backend, pool) = new_pool();
        backend.ready.store(false, Ordering::Relaxed);

        // SAFETY: `layout` has a non-zero size.
        assert!(unsafe { pool.alloc(layout(32, 8)) }.is_null());
        assert!(backend.granted().is_empty(), "no pages are requested before the backend is ready");

        backend.ready.store(true, Ordering::Relaxed);
        assert!(!alloc_ok(&pool, layout(32, 8)).is_null());
    }

    #[test]
    fn test_an_allocation_is_writable_for_its_full_size() {
        let (_, pool) = new_pool();
        let layout = layout(512, 8);
        let ptr = alloc_ok(&pool, layout);

        // SAFETY: the allocator promised 512 writable bytes here.
        unsafe {
            ptr::write_bytes(ptr, 0xAB, 512);
            assert!((0..512).all(|i| *ptr.add(i) == 0xAB));
        }
    }

    #[test]
    fn test_every_allocation_meets_the_minimum_alignment() {
        let (_, pool) = new_pool();

        // A one-byte, one-align request is still rounded up to the pool's floor.
        let ptr = alloc_ok(&pool, layout(1, 1));
        assert_eq!(ptr as usize % MIN_POOL_ALLOC_SIZE, 0);
    }

    #[test]
    fn test_an_alignment_stricter_than_the_minimum_is_honoured() {
        let (_, pool) = new_pool();

        for align in [32usize, 64, 256] {
            let ptr = alloc_ok(&pool, layout(8, align));
            assert_eq!(ptr as usize % align, 0, "allocation is aligned to {align}");
        }
    }

    #[test]
    fn test_successive_allocations_are_carved_from_one_page_block() {
        let (backend, pool) = new_pool();
        let layout = layout(64, 8);

        let first = alloc_ok(&pool, layout);
        let second = alloc_ok(&pool, layout);

        assert_eq!(backend.granted().len(), 1, "both allocations came from the same block");
        assert!(second as usize >= first as usize + 64, "the allocations do not overlap");
    }

    #[test]
    fn test_a_request_that_does_not_fit_grows_the_pool() {
        let (backend, pool) = new_pool();
        let large = layout(page_size() - header_size() - 64, 8);

        alloc_ok(&pool, large);
        assert_eq!(backend.granted().len(), 1);

        // The first block is nearly full, so this one needs a block of its own.
        alloc_ok(&pool, layout(256, 8));
        assert_eq!(backend.granted().len(), 2, "the pool grew rather than failing");
    }

    #[test]
    fn test_a_request_larger_than_a_page_asks_for_enough_pages() {
        let (backend, pool) = new_pool();
        let size = 3 * page_size();

        alloc_ok(&pool, layout(size, 8));

        let granted = backend.granted();
        assert_eq!(granted.len(), 1);
        assert_eq!(granted[0].1, uefi_size_to_pages!(size + header_size()), "the header is counted in the request");
    }

    #[test]
    fn test_a_block_is_returned_only_after_its_last_allocation_is_freed() {
        let (backend, pool) = new_pool();
        let layout = layout(64, 8);
        let first = alloc_ok(&pool, layout);
        let second = alloc_ok(&pool, layout);
        let block = backend.granted()[0];

        free(&pool, first, layout);
        assert!(backend.released().is_empty(), "the block is still holding a live allocation");

        free(&pool, second, layout);
        assert_eq!(backend.released(), alloc::vec![block], "the block went back once it was empty");
    }

    #[test]
    fn test_freeing_the_newest_block_relinks_the_list_head() {
        let (backend, pool) = new_pool();
        let filling = layout(page_size() - header_size() - 64, 8);

        let older = alloc_ok(&pool, filling);
        let newest = alloc_ok(&pool, filling);
        assert_eq!(backend.granted().len(), 2);

        free(&pool, newest, filling);
        assert_eq!(backend.released().len(), 1);

        // The older block is still linked and still serves its own pointer back.
        free(&pool, older, filling);
        assert_eq!(backend.released().len(), 2);
    }

    #[test]
    fn test_freeing_a_middle_block_keeps_its_neighbors_reachable() {
        let (backend, pool) = new_pool();
        let filling = layout(page_size() - header_size() - 64, 8);

        let oldest = alloc_ok(&pool, filling);
        let middle = alloc_ok(&pool, filling);
        let newest = alloc_ok(&pool, filling);
        assert_eq!(backend.granted().len(), 3);

        free(&pool, middle, filling);
        assert_eq!(backend.released().len(), 1);

        // Both neighbors survive the unlink and can still be found by address.
        free(&pool, newest, filling);
        free(&pool, oldest, filling);
        assert_eq!(backend.released().len(), 3, "the list was still walkable after the middle block left");
    }

    #[test]
    fn test_dealloc_ignores_a_null_pointer() {
        let (backend, pool) = new_pool();
        alloc_ok(&pool, layout(64, 8));

        free(&pool, ptr::null_mut(), layout(64, 8));

        assert!(backend.released().is_empty());
    }

    #[test]
    fn test_dealloc_ignores_a_pointer_outside_every_block() {
        let (backend, pool) = new_pool();
        alloc_ok(&pool, layout(64, 8));
        let mut stray = 0u64;

        free(&pool, ptr::from_mut(&mut stray).cast::<u8>(), layout(8, 8));

        assert!(backend.released().is_empty(), "an unknown pointer does not retire someone else's block");
    }

    #[test]
    fn test_allocation_returns_null_when_the_backend_is_out_of_pages() {
        let (backend, pool) = new_pool();
        backend.allocations_left.store(0, Ordering::Relaxed);

        // SAFETY: `layout` has a non-zero size.
        assert!(unsafe { pool.alloc(layout(64, 8)) }.is_null());
    }

    #[test]
    fn test_a_block_whose_pages_cannot_be_returned_is_still_unlinked() {
        let (backend, pool) = new_pool();
        let layout = layout(64, 8);
        let ptr = alloc_ok(&pool, layout);
        backend.free_fails.store(true, Ordering::Relaxed);

        free(&pool, ptr, layout);
        assert_eq!(backend.released().len(), 1, "the pool tried to hand the pages back");

        // The block is gone from the list even though the backend refused it, so the
        // next allocation has to start a fresh one.
        backend.free_fails.store(false, Ordering::Relaxed);
        alloc_ok(&pool, layout);
        assert_eq!(backend.granted().len(), 2);
    }

    #[test]
    fn test_an_alignment_that_cannot_fit_beside_the_header_is_refused() {
        let (backend, pool) = new_pool();
        // Sized so the block is exactly one page: aligning past the header then leaves
        // no room for the request itself.
        let impossible = layout(page_size() - header_size(), page_size());

        // SAFETY: `impossible` has a non-zero size.
        assert!(unsafe { pool.alloc(impossible) }.is_null());
        assert_eq!(backend.granted().len(), 1, "the refused request still consumed a block");
    }
}
