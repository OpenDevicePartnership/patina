//! Pool Allocator
//!
//! [`PageAllocatorBackend`] abstracts page allocation; the user core implements it with
//! supervisor syscalls. [`PoolAllocator`] bump-allocates from blocks of pages taken from a
//! backend and implements [`GlobalAlloc`].
//!
//! Each block starts with an in-band header, and the headers form an intrusive linked list.
//! When every allocation in a block is freed, the block's pages go back to the backend.
//!
//! ## Safety
//!
//! One `spin::Mutex` guards the list head and, by convention, every node: all traversal,
//! insertion, and removal happens with the lock held, and no list pointer leaves this
//! module. So while the lock is held, every `NonNull<PoolBlockHeader>` reachable from the
//! head points to an initialized header that nothing else can free or alias. The `SAFETY:`
//! comments below rely on this invariant.
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
pub(crate) enum PageAllocError {
    /// The allocator has not been initialized.
    NotInitialized,
    /// No free pages available to satisfy the request.
    OutOfMemory,
    /// A syscall to the supervisor failed.
    SyscallFailed(u64),
}

/// Minimum alignment of every pool allocation. Sizes are not rounded up.
const MIN_POOL_ALIGNMENT: usize = 16;

/// Trait for page-granularity memory allocation.
pub(crate) trait PageAllocatorBackend: Send + Sync {
    /// Allocates `num_pages` contiguous pages and returns their physical base address.
    fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError>;

    /// Frees `num_pages` contiguous pages starting at `addr`.
    fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError>;

    /// Returns whether the page allocator is ready for use.
    fn is_initialized(&self) -> bool;
}

/// In-band header at the start of each pool block.
#[repr(C)]
struct PoolBlockHeader {
    /// The next block in the list, or `None` at the tail.
    next: Option<NonNull<PoolBlockHeader>>,
    /// Number of pages in this block, including the header.
    num_pages: usize,
    /// Bump offset in bytes from the block base. Starts right after the header.
    offset: usize,
    /// Number of live allocations in this block.
    alloc_count: usize,
}

impl PoolBlockHeader {
    /// Base address of this block, which is the address of the header.
    fn base(&self) -> usize {
        ptr::from_ref::<Self>(self) as usize
    }

    /// Size of this block in bytes, including the header.
    fn capacity(&self) -> usize {
        uefi_pages_to_size!(self.num_pages)
    }

    /// Bytes left for bump allocation.
    fn remaining(&self) -> usize {
        self.capacity().saturating_sub(self.offset)
    }

    /// Returns `true` if `addr` falls within this block.
    fn contains(&self, addr: usize) -> bool {
        addr >= self.base() && addr < self.base() + self.capacity()
    }

    /// Bump-allocates `layout` from this block, or returns `None` if it does not fit.
    fn try_alloc(&mut self, layout: Layout) -> Option<*mut u8> {
        let current_ptr = self.base() + self.offset;
        let align = layout.align().max(MIN_POOL_ALIGNMENT);
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

/// Pool allocator on top of a [`PageAllocatorBackend`].
///
/// Requests whole pages from the backend and bump-allocates smaller pieces from them.
pub(crate) struct PoolAllocator<P: PageAllocatorBackend + 'static> {
    /// The underlying page allocator.
    page_allocator: &'static P,
    /// Head of the block list. The mutex guards every node reachable from it.
    head: Mutex<Option<NonNull<PoolBlockHeader>>>,
}

// SAFETY: Only the raw list pointers make this type `!Send` and `!Sync`. The list is guarded by
// the `head` mutex and never leaves this module, so sharing the allocator cannot race.
unsafe impl<P: PageAllocatorBackend> Send for PoolAllocator<P> {}
// SAFETY: As above.
unsafe impl<P: PageAllocatorBackend> Sync for PoolAllocator<P> {}

impl<P: PageAllocatorBackend> PoolAllocator<P> {
    /// Creates a new pool allocator backed by the given page allocator.
    pub(crate) const fn new(page_allocator: &'static P) -> Self {
        Self { page_allocator, head: Mutex::new(None) }
    }

    /// Allocates a new block large enough for `layout`, links it at the head of the list,
    /// and bump-allocates `layout` from it.
    ///
    /// Returns `None` if the page allocation or the bump allocation fails.
    fn allocate_in_new_block(&self, head: &mut Option<NonNull<PoolBlockHeader>>, layout: Layout) -> Option<*mut u8> {
        let header_size = mem::size_of::<PoolBlockHeader>();
        // Reserve up to `align - 1` bytes of padding after the header, or a valid layout may
        // not fit the new block.
        let align = layout.align().max(MIN_POOL_ALIGNMENT);
        let needed = header_size.checked_add(align - 1)?.checked_add(layout.size())?;
        let num_pages = uefi_size_to_pages!(needed).max(1);

        let base = match self.page_allocator.allocate_pages(num_pages) {
            Ok(addr) => addr,
            Err(error) => {
                log::warn!("Pool allocator: failed to allocate {num_pages} pages: {error:?}");
                return None;
            }
        };

        let mut header_ptr = NonNull::new(base as *mut PoolBlockHeader)?;
        let next = head.replace(header_ptr);

        // SAFETY: `base` came from `allocate_pages`, so it points to `num_pages` writable,
        // page-aligned pages owned only by this allocator. Writing the header establishes the
        // module invariant, and `as_mut()` gives the only reference to it.
        let header = unsafe {
            ptr::write(header_ptr.as_ptr(), PoolBlockHeader { next, num_pages, offset: header_size, alloc_count: 0 });
            header_ptr.as_mut()
        };

        log::trace!("Pool allocator: new block at {base:#018x} ({num_pages} pages)");

        header.try_alloc(layout)
    }
}

// SAFETY: The `GlobalAlloc` contract is upheld:
// * `try_alloc` aligns to `max(layout.align(), MIN_POOL_ALIGNMENT)` and checks that
//   `layout.size()` bytes fit the block, so `alloc` returns null or a valid pointer.
// * A live allocation is counted in its block's `alloc_count`, and `dealloc` frees a block
//   only when that count reaches zero, so no live allocation is invalidated or freed twice.
unsafe impl<P: PageAllocatorBackend> GlobalAlloc for PoolAllocator<P> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.page_allocator.is_initialized() {
            return ptr::null_mut();
        }

        let mut head = self.head.lock();

        let mut cursor = *head;
        while let Some(mut node) = cursor {
            // SAFETY: `node` is reachable from the locked head (module invariant).
            let block = unsafe { node.as_mut() };
            if let Some(ptr) = block.try_alloc(layout) {
                return ptr;
            }
            cursor = block.next;
        }

        // No block had room, so grow the pool.
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
            // SAFETY: `node` is reachable from the locked head (module invariant). The borrow
            // ends at the `continue` below or after the field reads.
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

            // Read these before borrowing through `prev`.
            let base = block.base() as u64;
            let num_pages = block.num_pages;
            let next = block.next;

            match prev {
                None => *head = next,
                // SAFETY: `prev` is still linked and the head lock is held. The borrow of `block`
                // ended with the field reads above, so this does not alias it.
                Some(mut previous) => unsafe { previous.as_mut().next = next },
            }

            if let Err(error) = self.page_allocator.free_pages(base, num_pages) {
                log::warn!("Pool allocator: failed to free block at {base:#018x}: {error:?}");
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
                return Err(PageAllocError::SyscallFailed(patina::standard::efi::Status::NOT_FOUND.as_usize() as u64));
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
            assert!((0..512).all(|offset| *ptr.add(offset) == 0xAB));
        }
    }

    #[test]
    fn test_every_allocation_meets_the_minimum_alignment() {
        let (_, pool) = new_pool();

        // A one-byte, one-align request is still rounded up to the pool's floor.
        let ptr = alloc_ok(&pool, layout(1, 1));
        assert_eq!(ptr as usize % MIN_POOL_ALIGNMENT, 0);
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
    fn test_an_alignment_that_needs_padding_after_the_header_gets_a_larger_block() {
        let (backend, pool) = new_pool();
        // Without room for the padding past the header, this request would get exactly one page
        // and then not fit in it.
        let padded = layout(page_size() - header_size(), page_size());

        let ptr = alloc_ok(&pool, padded);
        assert_eq!(ptr as usize % page_size(), 0, "allocation is aligned to a page");
        assert_eq!(backend.granted().len(), 1);
        assert_eq!(backend.granted()[0].1, 2, "the block includes room for the alignment padding");
        free(&pool, ptr, padded);
    }
}
