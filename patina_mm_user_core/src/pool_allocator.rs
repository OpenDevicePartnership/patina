//! Pool Allocator
//!
//! A page allocator abstraction and a generic pool allocator for the MM User Core.
//!
//! [`PageAllocatorBackend`] abstracts page allocation; the user core implements it with
//! supervisor syscalls. [`PoolAllocator`] is a bump allocator on top of any backend and
//! implements [`GlobalAlloc`], so it can be the `#[global_allocator]`.
//!
//! ## Block Management
//!
//! Each block of pages starts with an in-band header, and the headers form an intrusive
//! linked list, so there is no fixed limit on the number of blocks. When every allocation
//! in a block is freed, the block is unlinked and its pages go back to the backend.
//!
//! ## Safety
//!
//! The list is built from `NonNull<PoolBlockHeader>` pointers into pages the allocator owns
//! but Rust does not track. One `spin::Mutex` guards the head pointer and, by convention,
//! the whole list: every traversal, insertion, and removal happens with the lock held, and
//! no list pointer leaves this module.
//!
//! So while the head lock is held, any `NonNull<PoolBlockHeader>` reachable from the head
//! points to a fully initialized header that no other code can free or alias. The
//! `SAFETY:` comments below refer to this invariant.
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
    /// The address was not previously allocated.
    NotAllocated,
    /// A syscall to the supervisor failed.
    SyscallFailed(u64),
}

/// Minimum alignment of every pool allocation. Sizes are not rounded up.
const MIN_POOL_ALIGNMENT: usize = 16;

/// Trait for page-granularity memory allocation.
///
/// The user core implements it with supervisor syscalls.
pub trait PageAllocatorBackend: Send + Sync {
    /// Allocates `num_pages` contiguous pages and returns their physical base address.
    fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError>;

    /// Frees `num_pages` contiguous pages starting at `addr`.
    fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError>;

    /// Returns whether the page allocator is ready for use.
    fn is_initialized(&self) -> bool;
}

/// In-band header at the start of each pool block.
///
/// Blocks form a singly linked list.
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
pub struct PoolAllocator<P: PageAllocatorBackend + 'static> {
    /// The underlying page allocator.
    page_allocator: &'static P,
    /// Head of the block list. The mutex guards the head and every node reachable from it
    /// (see the module-level safety invariant).
    head: Mutex<Option<NonNull<PoolBlockHeader>>>,
}

// SAFETY: `NonNull<PoolBlockHeader>` is `!Send` and `!Sync` only because raw pointers are.
// The list is guarded by the `head` mutex and never exposed outside this module, so sending
// or sharing a `PoolAllocator` across threads cannot cause a data race.
unsafe impl<P: PageAllocatorBackend> Send for PoolAllocator<P> {}
// SAFETY: As above.
unsafe impl<P: PageAllocatorBackend> Sync for PoolAllocator<P> {}

impl<P: PageAllocatorBackend> PoolAllocator<P> {
    /// Creates a new pool allocator backed by the given page allocator.
    pub const fn new(page_allocator: &'static P) -> Self {
        Self { page_allocator, head: Mutex::new(None) }
    }

    /// Allocates a new block large enough for `layout`, links it at the head of the list,
    /// and bump-allocates `layout` from it.
    ///
    /// Returns `None` if the page allocation or the bump allocation fails.
    fn allocate_in_new_block(&self, head: &mut Option<NonNull<PoolBlockHeader>>, layout: Layout) -> Option<*mut u8> {
        let header_size = mem::size_of::<PoolBlockHeader>();
        // The bump pointer starts right after the header and may need up to `align - 1` bytes of
        // padding, so reserve that too or a valid layout cannot fit the new block.
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
        // page-aligned pages owned only by this allocator. `ptr::write` initializes the header
        // without dropping the old contents. The module-level invariant then holds, and
        // `as_mut()` gives the only reference to the header.
        let header = unsafe {
            ptr::write(header_ptr.as_ptr(), PoolBlockHeader { next, num_pages, offset: header_size, alloc_count: 0 });
            header_ptr.as_mut()
        };

        log::trace!("Pool allocator: new block at {base:#018x} ({num_pages} pages)");

        header.try_alloc(layout)
    }
}

// SAFETY: The `GlobalAlloc` contract is upheld:
// * `alloc` returns null or a pointer to at least `layout.size()` bytes aligned to
//   `layout.align()`. `PoolBlockHeader::try_alloc` aligns the bump pointer to
//   `max(layout.align(), MIN_POOL_ALIGNMENT)` and checks that the region fits the block.
// * A returned pointer stays valid until it is passed to `dealloc`: its block counts it in
//   `alloc_count`, so the block cannot be unlinked and freed while it is live.
// * `dealloc` frees a block's pages only when `alloc_count` reaches zero, so it neither
//   invalidates other live allocations nor frees pages twice.
unsafe impl<P: PageAllocatorBackend> GlobalAlloc for PoolAllocator<P> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.page_allocator.is_initialized() {
            return ptr::null_mut();
        }

        let mut head = self.head.lock();

        // Try to bump-allocate from an existing block.
        let mut cursor = *head;
        while let Some(mut node) = cursor {
            // SAFETY: `node` is reachable from the locked head, so by the module-level
            // invariant it points to a live `PoolBlockHeader` with no other reference to it.
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
            // SAFETY: `node` is reachable from the locked head (module-level invariant). The
            // borrow ends at the `continue` below or after the field reads.
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
                // SAFETY: `prev` is a node dereferenced earlier in this walk. It is still linked
                // and the head lock is held. The borrow of `block` ended with the field reads
                // above, so this `as_mut()` does not alias a live reference.
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
    }
}
