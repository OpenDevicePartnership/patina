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
