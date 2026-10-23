//! Fixed-sized block allocator.
//!
//! Implements a fixed-sized block allocator backed by pages from the GCD. Based on the example fixed-sized block
//! allocator presented here: <https://os.phil-opp.com/allocator-designs/#fixed-size-block-allocator>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;
use super::{AllocationStatistics, AllocationStrategy, DEFAULT_ALLOCATION_STRATEGY, PageAllocator};

use crate::{gcd::SpinLockedGcd, tpl_mutex};

use alloc::vec::Vec;
use core::{
    alloc::{AllocError, Allocator, GlobalAlloc, Layout},
    cmp::{max, min},
    debug_assert,
    fmt::{self, Display},
    mem::{align_of, size_of},
    num::NonZeroUsize,
    ops::Range,
    ptr::{NonNull, slice_from_raw_parts_mut},
    result::Result,
};
use patina::standard::efi;
use patina::{
    error::EfiError,
    pi::dxe_services::GcdMemoryType,
    signature, uefi_pages_to_size, uefi_size_to_pages, writelncrlf,
    {UEFI_PAGE_MASK, UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, align_up, page_shift_from_alignment},
};

/// Type for describing errors that this implementation can produce.
#[derive(Debug, PartialEq)]
pub enum FixedSizeBlockAllocatorError {
    /// Could not satisfy allocation request, and expansion failed.
    ///
    /// Specifies the requested block size that could not be satisfied.
    /// The allocator should attempt to expand through [`FixedSizeBlockAllocator::expand()`] in order
    /// to fulfill the requested allocation.
    OutOfMemory(usize),
    /// The memory region provided to extend the allocator was invalid.
    InvalidExpansion,
    /// An internal error occurred.
    InternalError,
    /// The allocation is larger than the largest fixed block size
    ExceedsBlockSize,
}

// List of fixed block sizes supported by the allocator
const BLOCK_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

// Slice::get() is not const on stable Rust
#[allow(clippy::indexing_slicing)]
const fn block_sizes_divide_alignment() -> bool {
    let mut index = 0;
    while index < BLOCK_SIZES.len() {
        if !UEFI_PAGE_SIZE.is_multiple_of(BLOCK_SIZES[index]) {
            return false;
        }
        index += 1;
    }
    true
}

// Returns the index in the block list for the minimum size block that will
// satisfy allocation for the given layout
fn list_index(layout: &Layout) -> Option<usize> {
    let required_block_size = layout.size().max(layout.align());
    BLOCK_SIZES.iter().position(|&s| s >= required_block_size)
}

struct BlockListNode {
    next: Option<NonNull<BlockListNode>>,
}

const _: () = assert!(size_of::<BlockListNode>() <= 8 && align_of::<BlockListNode>() <= 8);

const LARGE_ALLOCATION_NODE_SIGNATURE: u64 = signature!('B', 'I', 'G', 'A', 'L', 'L', 'C');

struct LargeAllocationNode {
    signature: u64,
    base: usize,
    size: usize,
}

impl LargeAllocationNode {
    fn allocation_size(layout: Layout, granularity: usize) -> Option<usize> {
        let user_alignment = max(layout.align(), max(8, align_of::<Self>()));
        layout
            .size()
            .checked_add(size_of::<Self>())
            .and_then(|size| size.checked_add(user_alignment - 1))
            .and_then(|size| align_up(size, granularity).ok())
    }
}

struct AllocatorListNode {
    next: Option<*mut AllocatorListNode>,
    /// Size in bytes of the entire region, including the metadata header.
    region_size: usize,

    /// The number of bytes of each whole page in this region that are currently sitting on the fixed-size block free
    /// lists, along with the retired and retiring page states.
    ///
    /// This is stored in the metadata header at the front of the region for every page in the region. Pages containing
    /// metadata are never retired so that the metadata remains accessible.
    page_free_bytes: &'static mut [u16],
}

const _: () = assert!(AllocatorListNode::RETIRED_PAGE < AllocatorListNode::RETIRING_PAGE);
const _: () = assert!(UEFI_PAGE_SIZE.is_multiple_of(align_of::<AllocatorListNode>()));

impl AllocatorListNode {
    // This flag is used to indicate that a page has been retired and is no longer in use by the allocator.
    // It represents the free bytes in a page. When the free bytes are equal to the page size, the entire page
    // is free and therefore retired. It contrasts with RETIRING_PAGE, which is greater than the page size and used
    // to indicate the page in question is in process of being retired. Any number of free bytes less than this
    // value indicate that the page has bytes that are allocated and in use.
    const RETIRED_PAGE: u16 = UEFI_PAGE_SIZE as u16;

    // A page is marked as retiring while it is in the process of being unmapped so it is not used if the unmap
    // causes allocations. This value is chosen as greater than the RETIRED_PAGE marker so that it is clear that
    // the page is not in use, which would be represented by 0 <= free_bytes <= RETIRED_PAGE.
    const RETIRING_PAGE: u16 = u16::MAX;

    /// Returns the size of the allocator node and page metadata required to describe `free_region_size` bytes.
    fn size_for_free_region(free_region_size: usize) -> usize {
        let page_count = uefi_size_to_pages!(free_region_size);
        Layout::new::<Self>()
            .extend(Layout::array::<u16>(page_count).expect("Failed to create layout for page metadata"))
            .expect("Arithmetic error in allocator node layout")
            .0
            .pad_to_align()
            .size()
    }

    /// Returns the base address of the region containing this node and its page metadata.
    fn region_base(&self) -> usize {
        core::ptr::from_ref(self).addr()
    }

    /// Returns the base address of the first whole page after the metadata header.
    fn first_page_base(&self) -> usize {
        align_up(self.region_base() + self.header_size(), UEFI_PAGE_SIZE)
            .expect("Heap regions should not be at the end of memory")
    }

    /// Returns the size in bytes of the metadata header
    fn header_size(&self) -> usize {
        Self::size_for_free_region(self.region_size)
    }

    /// Returns the index into `page_free_bytes` of the tracked page containing `address`, if any.
    fn page_index(&self, address: usize) -> Option<usize> {
        let index = address.checked_sub(self.region_base())? >> UEFI_PAGE_SHIFT;
        (index < self.page_free_bytes.len()).then_some(index)
    }

    /// Returns the base address of the tracked page containing `address` along with its free byte count.
    fn page_free_bytes_mut(&mut self, address: usize) -> Option<(usize, &mut u16)> {
        let index = self.page_index(address)?;
        let page_base = self.region_base() + uefi_pages_to_size!(index);
        Some((page_base, self.page_free_bytes.get_mut(index)?))
    }
}

struct AllocatorIterator {
    current: Option<*mut AllocatorListNode>,
}

impl AllocatorIterator {
    fn new(start_node: Option<*mut AllocatorListNode>) -> Self {
        AllocatorIterator { current: start_node }
    }
}

impl Iterator for AllocatorIterator {
    type Item = *mut AllocatorListNode;
    fn next(&mut self) -> Option<*mut AllocatorListNode> {
        if let Some(current) = self.current {
            // SAFETY: current is a valid node pointer from the allocator list.
            self.current = unsafe { (*current).next };
            Some(current)
        } else {
            None
        }
    }
}

/// Fixed Size Block Allocator
///
/// Implements an expandable memory allocator using fixed-sized blocks for speed backed by a linked-list allocator
/// implementation when an appropriate sized free block is not available. If more memory is required than can be
/// satisfied by either the block list or the linked-list, more memory is is allocated externally, then passed into
/// the allocator where a new backing linked-list is created.
///
pub struct FixedSizeBlockAllocator {
    /// The memory type this allocator manages.
    memory_type: efi::MemoryType,

    /// The heads of the linked lists for each fixed-size block. Each index corresponds to a block size in
    /// `BLOCK_SIZES`.
    list_heads: [Option<NonNull<BlockListNode>>; BLOCK_SIZES.len()],

    /// The tails of the linked lists for each fixed-size block. Freed blocks are appended here so that blocks are
    /// recycled in FIFO order, which maximizes the time between a block being freed and it being handed out again.
    list_tails: [Option<NonNull<BlockListNode>>; BLOCK_SIZES.len()],

    /// The linked-list of allocators that this allocator uses to back allocations that are larger than the fixed-size
    /// blocks or if the required fixed-size block list is empty.
    allocators: Option<*mut AllocatorListNode>,

    /// Allocator node and page index where the next retired-page search begins. This is maintained to reclaim pages
    /// in FIFO order, maximizing the time between a page being freed and it being reclaimed.
    retired_page_cursor: Option<(NonNull<AllocatorListNode>, usize)>,

    /// The range of memory that is reserved for this allocator. This is used to stabilize the memory map during an
    /// S4 resume.
    pub(crate) reserved_range: Option<Range<efi::PhysicalAddress>>,

    /// Statistics about the allocator's usage.
    stats: AllocationStatistics,

    /// The page allocation granularity used by this allocator. This is expected to be one of the following:
    /// - `SIZE_4KB` for all allocators except AARCH64 runtime memory allocators
    /// - `SIZE_64KB` for AARCH64 runtime memory allocators
    page_allocation_granularity: usize,
}

impl FixedSizeBlockAllocator {
    /// Creates a new empty `FixedSizeBlockAllocator`
    pub const fn new(memory_type: efi::MemoryType, page_allocation_granularity: usize) -> Self {
        const EMPTY: Option<NonNull<BlockListNode>> = None;
        FixedSizeBlockAllocator {
            memory_type,
            list_heads: [EMPTY; BLOCK_SIZES.len()],
            list_tails: [EMPTY; BLOCK_SIZES.len()],
            allocators: None,
            retired_page_cursor: None,
            reserved_range: None,
            stats: AllocationStatistics::new(),
            page_allocation_granularity,
        }
    }

    // This routine resets some aspects of allocator state for testing purposes.
    // Note: this does not change the page_change_callback.
    #[cfg(test)]
    pub fn reset(&mut self) {
        const EMPTY: Option<NonNull<BlockListNode>> = None;
        self.list_heads = [EMPTY; BLOCK_SIZES.len()];
        self.list_tails = [EMPTY; BLOCK_SIZES.len()];
        self.allocators = None;
        self.retired_page_cursor = None;
        self.reserved_range = None;
        self.stats = AllocationStatistics::new();
    }

    /// Expand the memory available to this allocator with a new contiguous region of memory, setting up a new allocator
    /// node to manage this range. The region is partially consumed by an `AllocatorListNode` and the per-page tracking
    /// metadata for the region; only whole pages after the metadata are available to the allocator.
    ///
    /// Every whole page of the new region after the metadata starts retired, so the allocator does not have any memory
    /// mapped that it has not handed out yet. This function returns `Ok(Some(Range))` if the expansion contains any
    /// whole pages that must be unmapped. If the returned range cannot be mapped, it must be handed to
    /// [`Self::restore_pages`].
    ///
    /// ## Errors
    ///
    /// Returns [`FixedSizeBlockAllocatorError::InvalidExpansion`] if the new region is not page aligned or does not
    /// contain at least one whole page after the metadata.
    /// Returns [`FixedSizeBlockAllocatorError::InternalError`] if the whole pages in the new region cannot be reserved
    /// from its backing allocator.
    fn expand(
        &mut self,
        new_region: NonNull<[u8]>,
    ) -> core::result::Result<Option<Range<usize>>, FixedSizeBlockAllocatorError> {
        // Interpret the first part of the provided region as an AllocatorListNode
        let alloc_node_ptr = new_region.as_ptr().cast::<AllocatorListNode>();

        if !alloc_node_ptr.addr().is_multiple_of(UEFI_PAGE_SIZE) || !new_region.len().is_multiple_of(UEFI_PAGE_SIZE) {
            debug_assert!(false, "FSB expanded with memory region that is not page aligned.");
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        // Only whole pages contained within the region are tracked for retirement.
        let page_count = uefi_size_to_pages!(new_region.len());
        let node_size = AllocatorListNode::size_for_free_region(new_region.len());

        // Ensure we're expanding enough to fit the node and its page metadata
        if new_region.len() <= node_size {
            debug_assert!(false, "FSB expanded with insufficiently sized memory region.");
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        if page_count <= uefi_size_to_pages!(node_size) {
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        // SAFETY: the metadata lies within the region, which was validated to be large enough above. The size of
        // AllocatorListNode includes its alignment padding and u16 has no stricter alignment requirement.
        let page_free_bytes = unsafe {
            let meta_ptr = new_region.as_ptr().cast::<u8>().add(size_of::<AllocatorListNode>()).cast::<u16>();
            for index in 0..page_count {
                meta_ptr.add(index).write(0);
            }
            core::slice::from_raw_parts_mut(meta_ptr, page_count)
        };

        //write the allocator node structure into the start of the range, initialize its heap with the remainder of
        //the range, and add the new allocator to the front of the allocator list.
        let node = AllocatorListNode { next: None, region_size: new_region.len(), page_free_bytes };
        // SAFETY: alloc_node_ptr is aligned and points to valid writable memory for an AllocatorListNode.
        unsafe {
            alloc_node_ptr.write(node);
            (*alloc_node_ptr).next = self.allocators;
        }

        self.allocators = Some(alloc_node_ptr);
        if self.retired_page_cursor.is_none() {
            self.retired_page_cursor = NonNull::new(alloc_node_ptr).map(|node| (node, 0));
        }

        if self.in_reserved_range(alloc_node_ptr.addr() as efi::PhysicalAddress) {
            self.stats.reserved_used += new_region.len();
        } else {
            self.stats.claimed_pages += uefi_size_to_pages!(new_region.len());
        }

        self.retire_new_region(alloc_node_ptr)
    }

    /// Takes every whole page of a newly expanded region after its metadata out of its backing allocator and marks it
    /// retired, so that the pages are not handed out until they have been mapped.
    ///
    /// Returns the range covered by those pages, or `None` if the region contains no whole pages after its metadata.
    fn retire_new_region(
        &mut self,
        node: *mut AllocatorListNode,
    ) -> Result<Option<Range<usize>>, FixedSizeBlockAllocatorError> {
        // SAFETY: node was just written by expand() and its heap has been initialized.
        let heap_start = unsafe { (*node).first_page_base() };
        // SAFETY: node points to the start of its region, and region_size was initialized from that region's length.
        let heap_end = unsafe { node.cast::<u8>().add((*node).region_size) as usize };

        if (heap_start & UEFI_PAGE_MASK) != 0 || (heap_end & UEFI_PAGE_MASK) != 0 {
            debug_assert!(heap_start & UEFI_PAGE_MASK == 0, "Heap start is not aligned to UEFI_PAGE_SIZE.");
            debug_assert!(heap_end & UEFI_PAGE_MASK == 0, "Heap end is not aligned to UEFI_PAGE_SIZE.");
            return Err(FixedSizeBlockAllocatorError::InternalError);
        }

        let pages = heap_start..heap_end;
        if pages.is_empty() {
            return Ok(None);
        }

        // SAFETY: node is valid and its heap covers all of the whole pages in the region.
        unsafe {
            for page_base in pages.clone().step_by(UEFI_PAGE_SIZE) {
                if let Some((_, free_bytes)) = (*node).page_free_bytes_mut(page_base) {
                    *free_bytes = AllocatorListNode::RETIRING_PAGE;
                }
            }
        }

        self.stats.retired_pages += uefi_size_to_pages!(pages.len());

        Ok(Some(pages))
    }

    /// Allocates and returns a pointer to a memory buffer for the given layout.
    ///
    ///
    /// Memory allocated by this routine should be deallocated with
    /// [`Self::dealloc`]
    ///
    /// ## Errors
    ///
    /// Returns [`FixedSizeBlockAllocatorError::OutOfMemory`] when the allocator doesn't have enough memory.
    /// Returns [`FixedSizeBlockAllocatorError::InternalError`] when an internal error occurs.
    pub fn alloc(&mut self, layout: Layout) -> Result<NonNull<[u8]>, FixedSizeBlockAllocatorError> {
        self.stats.pool_allocation_calls += 1;
        self.internal_alloc(layout)
    }

    /// Attempts an allocation without updating the stats for caller allocations
    fn internal_alloc(&mut self, layout: Layout) -> Result<NonNull<[u8]>, FixedSizeBlockAllocatorError> {
        match list_index(&layout) {
            Some(index) => {
                let head = self.list_heads.get(index).copied().ok_or(FixedSizeBlockAllocatorError::InternalError)?;
                if let Some(node) = head {
                    // SAFETY: node was written as a BlockListNode by dealloc() and is still on the free list, so the
                    // page containing it is mapped.
                    let next = unsafe { (*node.as_ptr()).next };
                    *self.list_heads.get_mut(index).ok_or(FixedSizeBlockAllocatorError::InternalError)? = next;
                    if next.is_none() {
                        *self.list_tails.get_mut(index).ok_or(FixedSizeBlockAllocatorError::InternalError)? = None;
                    }
                    let block_size = *BLOCK_SIZES.get(index).ok_or(FixedSizeBlockAllocatorError::InternalError)?;
                    let ptr: NonNull<u8> = node.cast();
                    self.track_block_allocated(ptr.addr().get(), block_size);
                    Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
                } else {
                    // no block exists in list => return OOM so caller can allocate new block from
                    // available pages (if any)
                    let block_size = *BLOCK_SIZES.get(index).ok_or(FixedSizeBlockAllocatorError::InternalError)?;
                    Err(FixedSizeBlockAllocatorError::OutOfMemory(block_size))
                }
            }
            // if the allocation request is larger than our largest fixed block, we'll allocate directly from the GCD
            None => Err(FixedSizeBlockAllocatorError::ExceedsBlockSize),
        }
    }

    fn add_blocks_to_fsb(
        &mut self,
        ptr: NonNull<u8>,
        block_size: usize,
        count: usize,
    ) -> Result<(), FixedSizeBlockAllocatorError> {
        // Make sure we have a large enough block to write the BlockListNode into it. If we don't,
        // we have corruption of our data structures and we need to ignore this region.
        if size_of::<BlockListNode>() > block_size || align_of::<BlockListNode>() > block_size {
            debug_assert!(
                (size_of::<BlockListNode>() <= block_size && align_of::<BlockListNode>() <= block_size),
                "FSB deallocating block too small to store BlockListNode."
            );
            return Err(FixedSizeBlockAllocatorError::InternalError);
        }

        for i in 0..count {
            // SAFETY: the caller-provided span contains count blocks of block_size bytes.
            let offset_ptr = unsafe { NonNull::new_unchecked(ptr.as_ptr().add(i * block_size)) };
            let new_node = offset_ptr.cast::<BlockListNode>();
            // SAFETY: new_node points to memory returned by alloc for this layout, which is large enough and
            // aligned for a BlockListNode as asserted above.
            unsafe { new_node.as_ptr().write(BlockListNode { next: None }) };

            let index =
                BLOCK_SIZES.iter().position(|&s| s == block_size).ok_or(FixedSizeBlockAllocatorError::InternalError)?;

            // Append to the tail so that blocks are recycled in FIFO order.
            match self.list_tails.get(index).copied().expect("list_index guarantees valid index") {
                // SAFETY: tail is a node on this free list, so the page containing it is mapped.
                Some(tail) => unsafe { (*tail.as_ptr()).next = Some(new_node) },
                None => {
                    let _ =
                        self.list_heads.get_mut(index).expect("list_index guarantees valid index").replace(new_node);
                }
            }
            *self.list_tails.get_mut(index).expect("list_index guarantees valid index") = Some(new_node);
        }

        Ok(())
    }

    /// Records fixed-size blocks added to a page without applying deallocation or retirement semantics.
    fn track_blocks_added(&mut self, address: usize, footprint: usize) -> Result<(), FixedSizeBlockAllocatorError> {
        let node = self.node_for_address(address).ok_or(FixedSizeBlockAllocatorError::InternalError)?;

        // SAFETY: node_for_address returned a valid node that tracks the page containing `address`.
        let (_, free_bytes) = unsafe {
            (*node.as_ptr()).page_free_bytes_mut(address).ok_or(FixedSizeBlockAllocatorError::InternalError)?
        };
        *free_bytes = free_bytes
            .checked_add(footprint as u16)
            .filter(|free| *free <= AllocatorListNode::RETIRED_PAGE)
            .ok_or(FixedSizeBlockAllocatorError::InternalError)?;

        Ok(())
    }

    /// Splits a mapped page into fixed-size blocks and makes them available for allocation.
    fn add_page_to_fsb(&mut self, page_base: usize, block_size: usize) -> Result<(), FixedSizeBlockAllocatorError> {
        let page_ptr = NonNull::new(page_base as *mut u8).ok_or(FixedSizeBlockAllocatorError::InternalError)?;
        self.add_blocks_to_fsb(page_ptr, block_size, UEFI_PAGE_SIZE / block_size)?;
        self.track_blocks_added(page_base, UEFI_PAGE_SIZE)
    }

    /// Deallocates a buffer allocated by [`Self::alloc`].
    ///
    /// Returns the base address of a page that was fully returned to the free lists and has been retired. The caller
    /// must unmap that page, or return it to circulation with [`Self::restore_page`] if it cannot be unmapped.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that `ptr` was created by a call to [`Self::alloc`] with the same `index`.
    pub unsafe fn dealloc(&mut self, ptr: NonNull<u8>, index: usize) -> Option<usize> {
        self.stats.pool_free_calls += 1;

        let block_size = *BLOCK_SIZES.get(index).expect("dealloc caller guarantees valid index");

        self.add_blocks_to_fsb(ptr, block_size, 1).ok()?;

        self.track_block_freed(ptr.addr().get(), block_size)
    }

    /// Returns the region node that tracks the page containing `address`, if any.
    fn node_for_address(&self, address: usize) -> Option<NonNull<AllocatorListNode>> {
        AllocatorIterator::new(self.allocators)
            .find(|&node| {
                // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
                unsafe { (*node).page_index(address).is_some() }
            })
            .and_then(NonNull::new)
    }

    /// Records that a fixed-size block was taken off of the free lists.
    fn track_block_allocated(&mut self, address: usize, footprint: usize) {
        let Some(node) = self.node_for_address(address) else {
            debug_assert!(
                false,
                "FSB allocated a block belonging to a page that is not tracked by any allocator node."
            );
            return;
        };

        // SAFETY: node_for_address returned a valid node that tracks the page containing `address`.
        unsafe {
            if let Some((_, free_bytes)) = (*node.as_ptr()).page_free_bytes_mut(address) {
                *free_bytes = free_bytes.saturating_sub(footprint as u16);
            }
        }
    }

    /// Records that a fixed-size block was placed on the free lists, retiring the page if it is completely freed.
    /// Returns the base address of the retired page, if any.
    fn track_block_freed(&mut self, address: usize, footprint: usize) -> Option<usize> {
        let node = self.node_for_address(address)?;

        // SAFETY: node_for_address returned a valid node that tracks the page containing `address`.
        let (page_base, page_free_bytes) = unsafe {
            let (page_base, free_bytes) = (*node.as_ptr()).page_free_bytes_mut(address)?;

            // A page that is out of circulation has no blocks on the free lists, so this is a double free.
            if *free_bytes >= AllocatorListNode::RETIRED_PAGE {
                debug_assert!(false, "FSB freed a block belonging to a page that is not in circulation.");
                return None;
            }

            *free_bytes = free_bytes.saturating_add(footprint as u16);
            (page_base, *free_bytes)
        };

        if page_free_bytes < AllocatorListNode::RETIRED_PAGE {
            return None;
        }

        debug_assert!(page_free_bytes == UEFI_PAGE_SIZE as u16, "Page contains more free bytes than the page size.");

        // All blocks within this page are on the free lists, so the free list nodes must be unlinked before it
        // can be unmapped.
        self.unlink_blocks_in_page(page_base);

        // SAFETY: node remains valid and tracks page_base.
        unsafe {
            let (_, free_bytes) = (*node.as_ptr()).page_free_bytes_mut(page_base)?;
            *free_bytes = AllocatorListNode::RETIRING_PAGE;
        }
        self.stats.retired_pages += 1;

        Some(page_base)
    }

    /// Removes every free block that lives within the given page from the fixed-size block free lists.
    fn unlink_blocks_in_page(&mut self, page_base: usize) {
        let page_range = page_base..page_base + UEFI_PAGE_SIZE;

        for index in 0..BLOCK_SIZES.len() {
            let mut previous: Option<NonNull<BlockListNode>> = None;
            let mut current = self.list_heads.get(index).copied().flatten();

            while let Some(node) = current {
                // SAFETY: node is on the free list, so it points to a BlockListNode in mapped memory.
                let next = unsafe { (*node.as_ptr()).next };

                if page_range.contains(&node.addr().get()) {
                    match previous {
                        // SAFETY: previous is on the free list, so it points to a BlockListNode in mapped memory.
                        Some(previous_node) => unsafe { (*previous_node.as_ptr()).next = next },
                        None => {
                            if let Some(head) = self.list_heads.get_mut(index) {
                                *head = next;
                            }
                        }
                    }
                    if self.list_tails.get(index).copied().flatten() == Some(node)
                        && let Some(tail) = self.list_tails.get_mut(index)
                    {
                        *tail = previous;
                    }
                } else {
                    previous = Some(node);
                }

                current = next;
            }
        }
    }

    /// Returns the allocator node that contains up to `count` contiguous retired pages, if the allocator has any.
    /// This function will return the nodes in FIFO order, starting from the given `current` node.
    ///
    /// The pages remain retired until they are handed to [`Self::restore_pages`].
    fn next_allocator_node(&self, current: NonNull<AllocatorListNode>) -> Option<NonNull<AllocatorListNode>> {
        let head = NonNull::new(self.allocators?)?;
        let mut node = head;
        let tail = loop {
            // SAFETY: node is from the allocator list and remains valid for the lifetime of the allocator.
            let next = unsafe { node.as_ref().next }.and_then(NonNull::new);
            if next == Some(current) {
                return Some(node);
            }
            let Some(next) = next else { break node };
            node = next;
        };

        (current == head).then_some(tail)
    }

    // Search through the allocator nodes for a contiguous run of exactly `count` retired pages. If there does not
    // exist a suitable run, None is returned. This function uses the retired_page_cursor to maintain FIFO order of
    // page reclamation.
    fn find_retired_pages(&mut self, count: usize) -> Option<Range<usize>> {
        if count == 0 {
            return None;
        }

        let head = NonNull::new(self.allocators?)?;
        let (start_node, start_index) = self.retired_page_cursor.unwrap_or((head, 0));
        let mut node = Some(start_node);
        let mut wrapped = false;

        while let Some(current) = node {
            // SAFETY: retired_page_cursor and allocators only contain nodes from the allocator list, which remain
            // valid for the lifetime of the allocator.
            let current_ref = unsafe { current.as_ref() };
            let begin =
                if !wrapped && current == start_node { start_index.min(current_ref.page_free_bytes.len()) } else { 0 };
            let end = if wrapped && current == start_node {
                start_index.min(current_ref.page_free_bytes.len())
            } else {
                current_ref.page_free_bytes.len()
            };

            let mut search_index = begin;
            while let Some(relative_first) = current_ref
                .page_free_bytes
                .get(search_index..end)?
                .iter()
                .position(|free_bytes| *free_bytes == AllocatorListNode::RETIRED_PAGE)
            {
                let first = search_index + relative_first;
                let run = current_ref
                    .page_free_bytes
                    .get(first..end)?
                    .iter()
                    .take(count)
                    .take_while(|free_bytes| **free_bytes == AllocatorListNode::RETIRED_PAGE)
                    .count();
                if run == count {
                    let next_index = first + count;
                    self.retired_page_cursor = if next_index < current_ref.page_free_bytes.len() {
                        Some((current, next_index))
                    } else {
                        Some((self.next_allocator_node(current).unwrap_or(head), 0))
                    };
                    return Some(
                        current_ref.region_base() + uefi_pages_to_size!(first)
                            ..current_ref.region_base() + uefi_pages_to_size!(next_index),
                    );
                }

                search_index = first + run;
                if search_index < end {
                    search_index += 1;
                }
            }

            node = self.next_allocator_node(current);
            if node == Some(start_node) {
                wrapped = true;
            }
            if wrapped && current == start_node {
                break;
            }
        }

        None
    }

    /// Marks pages that have been unmapped as retired, making them available to be mapped back in and reused.
    pub(crate) fn mark_pages_retired(&mut self, pages: Range<usize>) {
        if !pages.start.is_multiple_of(UEFI_PAGE_SIZE) || !pages.end.is_multiple_of(UEFI_PAGE_SIZE) {
            debug_assert!(false, "FSB asked to retire a range that is not page aligned.");
            return;
        }

        for page_base in pages.step_by(UEFI_PAGE_SIZE) {
            let Some(node) = self.node_for_address(page_base) else {
                debug_assert!(false, "FSB asked to retire a page it does not own.");
                continue;
            };
            // SAFETY: node_for_address returned a valid node that tracks page_base.
            unsafe {
                if let Some((_, free_bytes)) = (*node.as_ptr()).page_free_bytes_mut(page_base) {
                    *free_bytes = AllocatorListNode::RETIRED_PAGE;
                }
            }
        }
    }

    #[cfg(test)]
    fn pending_retirement(&self, _max_pages: usize) -> Option<Range<usize>> {
        for node in AllocatorIterator::new(self.allocators) {
            // SAFETY: AllocatorIterator yields valid nodes with permanently mapped metadata.
            let node = unsafe { &*node };
            if let Some(index) =
                node.page_free_bytes.iter().position(|state| *state == AllocatorListNode::RETIRING_PAGE)
            {
                let start = node.region_base() + uefi_pages_to_size!(index);
                return Some(start..start + UEFI_PAGE_SIZE);
            }
        }
        None
    }

    /// Returns a range of retired pages to the backing allocators that own them, making them available for allocation
    /// again.
    ///
    /// The caller must ensure the pages are mapped before calling this.
    pub(crate) fn restore_pages(&mut self, pages: Range<usize>) {
        for page_base in pages.step_by(UEFI_PAGE_SIZE) {
            self.restore_page(page_base);
        }
    }

    /// Makes pages from a new, still-mapped expansion available to the backing allocator.
    fn activate_new_pages(&mut self, pages: Range<usize>) {
        let page_count = uefi_size_to_pages!(pages.len());
        self.restore_pages(pages);
        self.stats.retired_pages = self.stats.retired_pages.checked_sub(page_count).unwrap_or_else(|| {
            debug_assert!(false, "Retired pages stats underflow!");
            0
        });
        self.stats.reclaimed_pages = self.stats.reclaimed_pages.checked_sub(page_count).unwrap_or_else(|| {
            debug_assert!(false, "Reclaimed pages stats underflow!");
            0
        });
    }

    /// Returns a retired page to the backing allocator that owns it, making it available for allocation again.
    ///
    /// The caller must ensure the page is mapped before calling this.
    fn restore_page(&mut self, page_base: usize) {
        let Some(node) = self.node_for_address(page_base) else {
            debug_assert!(false, "FSB asked to restore a page it does not own.");
            return;
        };

        // SAFETY: every byte of this page was handed out by this heap as fixed-size blocks and all of those blocks
        // have been freed and unlinked, so returning the page as a single allocation describes exactly the memory the
        // heap considers in use.
        unsafe {
            if let Some((_, free_bytes)) = (*node.as_ptr()).page_free_bytes_mut(page_base) {
                *free_bytes = 0;
            }
        }
        self.stats.reclaimed_pages += 1;
    }

    /// Returns whether the provided address is in the FSB's reserved range
    pub fn in_reserved_range(&self, address: efi::PhysicalAddress) -> bool {
        match &self.reserved_range {
            Some(reserved_range) => reserved_range.contains(&address),
            _ => false,
        }
    }

    /// Sets the reserved memory range (bin range) for this allocator.
    ///
    /// ## Errors
    ///
    /// Returns [`EfiError::AlreadyStarted`] if a reserved range has already been set.
    pub fn set_reserved_range(&mut self, range: Range<efi::PhysicalAddress>) -> Result<(), EfiError> {
        if self.reserved_range.is_some() {
            Err(EfiError::AlreadyStarted)?;
        }

        let size = (range.end - range.start) as usize;
        self.reserved_range = Some(range);
        self.stats.reserved_size = size;
        self.stats.reserved_used = 0;
        self.stats.claimed_pages += uefi_size_to_pages!(size);

        Ok(())
    }

    /// Tracks page allocations for record keeping
    pub fn notify_page_allocation(&mut self, allocation: NonNull<[u8]>) {
        if self.in_reserved_range(allocation.addr().get() as efi::PhysicalAddress) {
            self.stats.reserved_used += allocation.len();
        } else {
            self.stats.claimed_pages += uefi_size_to_pages!(allocation.len());
        }
    }

    /// Tracks page freeing for record keeping
    pub fn notify_pages_freed(&mut self, address: efi::PhysicalAddress, pages: usize) {
        if self.in_reserved_range(address) {
            self.stats.reserved_used = self.stats.reserved_used.saturating_sub(pages * UEFI_PAGE_SIZE);
        } else {
            self.stats.claimed_pages = self.stats.claimed_pages.saturating_sub(pages);
        }
    }

    /// Get the ranges of the memory owned by this allocator
    ///
    /// Returns an iterator of ranges of the memory owned by this allocator.
    /// If the allocator does not own any memory, it will return an empty iterator.
    pub(crate) fn get_memory_ranges(&self) -> impl Iterator<Item = Range<usize>> {
        AllocatorIterator::new(self.allocators).map(|node| {
            let base = node.addr();

            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let end = base + unsafe { (*node).region_size };
            base..end
        })
    }

    /// Returns the metadata pages of a region for patina tests
    #[cfg(target_os = "uefi")]
    pub(crate) fn firmware_test_metadata_range(&self, base: usize) -> Option<Range<usize>> {
        AllocatorIterator::new(self.allocators).find_map(|node| {
            // SAFETY: the iterator yields live nodes owned by this locked allocator.
            let node = unsafe { &*node };
            (node.region_base() == base).then(|| base..node.first_page_base())
        })
    }

    /// Returns the memory type for this allocator
    #[inline(always)]
    pub fn memory_type(&self) -> efi::MemoryType {
        self.memory_type
    }

    /// Returns a reference to the allocation stats for this allocator.
    pub fn stats(&self) -> &AllocationStatistics {
        &self.stats
    }
}

impl FixedSizeBlockAllocator {
    /// Updates pool page attributes while retaining exclusive access to the allocator.
    fn update_page_attributes(
        &mut self,
        gcd: &SpinLockedGcd,
        pages: &Range<usize>,
        apply_free_policy: bool,
    ) -> Result<(), EfiError> {
        match self.apply_page_attributes(gcd, pages, apply_free_policy) {
            Ok(()) => Ok(()),
            Err((err, failed_address)) => {
                if let Err((rollback_err, _)) =
                    self.apply_page_attributes(gcd, &(pages.start..failed_address), !apply_free_policy)
                {
                    log::error!(
                        "Failed to roll back pool page attributes for {:#x}-{:#x}: {rollback_err:?}",
                        pages.start,
                        failed_address
                    );
                }
                Err(err)
            }
        }
    }

    fn apply_page_attributes(
        &mut self,
        gcd: &SpinLockedGcd,
        pages: &Range<usize>,
        apply_free_policy: bool,
    ) -> Result<(), (EfiError, usize)> {
        let mut base = pages.start;
        while base < pages.end {
            let descriptor = gcd
                .get_memory_descriptor_for_address(base as efi::PhysicalAddress, |d, _| {
                    d.memory_type != GcdMemoryType::NonExistent
                })
                .map_err(|err| (err, base))?;

            let descriptor_end = descriptor.base_address + descriptor.length;
            let end = min(pages.end, descriptor_end as usize);
            if end <= base {
                return Err((EfiError::InvalidParameter, base));
            }

            let attributes = if apply_free_policy {
                crate::gcd::MemoryProtectionPolicy::apply_free_memory_policy(
                    descriptor.attributes,
                    GcdMemoryType::SystemMemory,
                )
            } else {
                gcd.memory_protection_policy
                    .apply_allocated_memory_protection_policy(descriptor.attributes, GcdMemoryType::SystemMemory)
            };
            match gcd.set_memory_space_attributes(base, end - base, attributes) {
                Ok(()) | Err(EfiError::NotReady) => base = end,
                Err(err) => return Err((err, base)),
            }
        }
        Ok(())
    }

    /// Completes retirement with exclusive access to the allocator.
    /// Failed unmaps become retired and must be mapped successfully by a future reclaim before being published.
    fn retire_pages(&mut self, gcd: &SpinLockedGcd, pages: Range<usize>) {
        let result = self.update_page_attributes(gcd, &pages, true);
        self.mark_pages_retired(pages.clone());
        if let Err(err) = result {
            log::error!("Failed to unmap pool pages {:#x}-{:#x}: {err:?}", pages.start, pages.end);
        }
    }

    fn free_pages_internal(
        &mut self,
        gcd: &SpinLockedGcd,
        handle: efi::Handle,
        address: usize,
        pages: usize,
    ) -> Result<(), EfiError> {
        if !address.is_multiple_of(self.page_allocation_granularity) {
            return Err(EfiError::InvalidParameter);
        }

        let descriptor = gcd
            .get_memory_descriptor_for_address(address as efi::PhysicalAddress, |d, _| {
                d.memory_type != GcdMemoryType::NonExistent
            })
            .map_err(|err| match err {
                EfiError::NotFound => err,
                _ => EfiError::InvalidParameter,
            })?;

        if descriptor.image_handle != handle {
            Err(EfiError::NotFound)?;
        }

        if self.in_reserved_range(address as efi::PhysicalAddress) {
            gcd.free_memory_space_preserving_ownership(address, uefi_pages_to_size!(pages)).map_err(
                |err| match err {
                    EfiError::NotFound => err,
                    _ => EfiError::InvalidParameter,
                },
            )?;
        } else {
            gcd.free_memory_space(address, uefi_pages_to_size!(pages)).map_err(|err| match err {
                EfiError::NotFound => err,
                _ => EfiError::InvalidParameter,
            })?;
        }

        Ok(())
    }

    /// Attempts to allocate from the GCD, preferring the reserved (bin) range if one exists.
    ///
    /// For strategies that do not exclude the bin range, this first tries to allocate within the
    /// bin range so that special-type pages land in their designated bin. Specifically, bin
    /// preference is attempted for:
    /// - `TopDown(None)` / `BottomUp(None)`: fully unconstrained strategies.
    /// - `TopDown(Some(max))` / `BottomUp(Some(max))`: constrained strategies where `max` is at
    ///   or above the bin range end, meaning the bin is reachable.
    ///
    /// `Address(addr)` strategies are never redirected because the caller requires an exact address.
    ///
    /// If the bin is full or no bin exists, the allocation falls through to the original strategy.
    fn allocate_from_gcd(
        &mut self,
        gcd: &SpinLockedGcd,
        handle: efi::Handle,
        strategy: AllocationStrategy,
        align_shift: usize,
        size: usize,
    ) -> Result<usize, EfiError> {
        let try_bin = match strategy {
            AllocationStrategy::TopDown(None) | AllocationStrategy::BottomUp(None) => true,
            AllocationStrategy::TopDown(Some(max)) | AllocationStrategy::BottomUp(Some(max)) => {
                if let Some(ref reserved) = self.reserved_range {
                    max as u64 >= reserved.start + size as u64
                } else {
                    false
                }
            }
            _ => false,
        };

        if try_bin
            && let Some(ref reserved) = self.reserved_range
            && let Ok(addr) = gcd.allocate_memory_space(
                AllocationStrategy::TopDown(Some(reserved.end as usize)),
                GcdMemoryType::SystemMemory,
                align_shift,
                size,
                handle,
                None,
            )
        {
            if addr >= reserved.start as usize {
                return Ok(addr);
            }
            let _ = gcd.free_memory_space(addr, size);
        }

        gcd.allocate_memory_space(strategy, GcdMemoryType::SystemMemory, align_shift, size, handle, None)
    }
}

impl Display for FixedSizeBlockAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writelncrlf!(f, "Memory Type: {:x?}", self.memory_type())?;
        writelncrlf!(f, "Allocation Ranges:")?;
        for node in AllocatorIterator::new(self.allocators) {
            let base = node.addr();
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let size = unsafe { (*node).region_size };
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let free = unsafe {
                (*node)
                    .page_free_bytes
                    .iter()
                    .map(|free_bytes| {
                        if *free_bytes < AllocatorListNode::RETIRED_PAGE {
                            *free_bytes as usize
                        } else {
                            AllocatorListNode::RETIRED_PAGE as usize
                        }
                    })
                    .sum::<usize>()
            };
            let used = size.saturating_sub(free);
            writelncrlf!(
                f,
                "  PhysRange: {:#x}-{:#x}, Size: {:#x}, Used: {:#x} Free: {:#x}",
                base,
                base + size,
                size,
                used,
                free,
            )?;
        }
        writelncrlf!(f, "Bucket Range: {:x?}", self.reserved_range)?;
        writelncrlf!(f, "Allocation Stats:")?;
        writelncrlf!(f, "  pool_allocation_calls: {}", self.stats.pool_allocation_calls)?;
        writelncrlf!(f, "  large_pool_allocation_calls: {}", self.stats.large_pool_allocation_calls)?;
        writelncrlf!(f, "  pool_free_calls: {}", self.stats.pool_free_calls)?;
        writelncrlf!(f, "  page_allocation_calls: {}", self.stats.page_allocation_calls)?;
        writelncrlf!(f, "  page_free_calls: {}", self.stats.page_free_calls)?;
        writelncrlf!(f, "  reserved_size: {}", self.stats.reserved_size)?;
        writelncrlf!(f, "  reserved_used: {}", self.stats.reserved_used)?;
        writelncrlf!(f, "  claimed_pages: {}", self.stats.claimed_pages)?;
        writelncrlf!(f, "  retired_pages: {}", self.stats.retired_pages)?;
        writelncrlf!(f, "  reclaimed_pages: {}", self.stats.reclaimed_pages)?;
        Ok(())
    }
}

/// Spin Locked Fixed Size Block Allocator
///
/// A wrapper for [`FixedSizeBlockAllocator`] that allocates additional memory as needed from a GCD
/// and provides Sync/Send via means of a spin mutex.
///
/// Note: [`SpinLockedFixedSizeBlockAllocator::alloc()`] and [`SpinLockedFixedSizeBlockAllocator::allocate()`] will call
/// `alloc()` twice when additional memory is required.
pub struct SpinLockedFixedSizeBlockAllocator {
    /// The GCD instance that this allocator uses to allocate additional memory as needed.
    gcd: &'static SpinLockedGcd,

    /// The handle associated with this allocator. It is used to track ownership of the memory allocated
    /// by this allocator in the GCD.
    handle: efi::Handle,

    /// The inner allocator that is protected by a TPL mutex.
    inner: tpl_mutex::TplMutex<FixedSizeBlockAllocator>,

    /// The minimum amount of memory to allocate when expanding the allocator.
    min_expansion: usize,
}

impl SpinLockedFixedSizeBlockAllocator {
    /// Creates a new empty `FixedSizeBlockAllocator` that will request memory from `gcd` as needed to satisfy
    /// requests.
    pub const fn new(
        gcd: &'static SpinLockedGcd,
        allocator_handle: efi::Handle,
        memory_type: efi::MemoryType,
        page_allocation_granularity: usize,
        min_expansion: usize,
    ) -> Self {
        SpinLockedFixedSizeBlockAllocator {
            gcd,
            handle: allocator_handle,
            inner: tpl_mutex::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                FixedSizeBlockAllocator::new(memory_type, page_allocation_granularity),
                "FsbLock",
            ),
            min_expansion,
        }
    }

    // This routine resets some aspects of allocator state for testing purposes.
    // Note: this does not reset the GCD nor change the page_change_callback.
    #[cfg(test)]
    pub fn reset(&self) {
        self.lock().reset();
    }

    /// Locks the allocator
    ///
    /// This can be used to do several actions on the allocator atomically.
    pub fn lock(&self) -> tpl_mutex::TplGuard<'_, FixedSizeBlockAllocator> {
        self.inner.lock()
    }

    /// Attempts to allocate the given number of pages according to the given allocation strategy.
    /// Valid allocation strategies are:
    /// - BottomUp(None): Allocate the block of pages from the lowest available free memory.
    /// - BottomUp(Some(address)): Allocate the block of pages from the lowest available free memory. Fail if memory
    ///   cannot be found below `address`.
    /// - TopDown(None): Allocate the block of pages from the highest available free memory.
    /// - TopDown(Some(address)): Allocate the block of pages from the highest available free memory. Fail if memory
    ///   cannot be found above `address`.
    /// - Address(address): Allocate the block of pages at exactly the given address (or fail).
    ///
    /// If an address is specified as part of a strategy, it must be page-aligned.
    pub fn allocate_pages(
        &self,
        allocation_strategy: AllocationStrategy,
        pages: usize,
        alignment: usize,
    ) -> Result<NonNull<[u8]>, EfiError> {
        // Record this call in the FSB's stats
        let mut allocator = self.lock();
        allocator.stats.page_allocation_calls += 1;
        let granularity = allocator.page_allocation_granularity;

        page_shift_from_alignment(alignment)?;

        // Granularity and alignment both are powers of two, so we can use the max of the two
        let required_alignment = max(granularity, alignment);

        // Ensure that the requested number of pages is a multiple of the granularity
        let required_pages = align_up(pages, uefi_size_to_pages!(granularity))?;

        let align_shift = page_shift_from_alignment(required_alignment)?;

        if let AllocationStrategy::Address(address) = allocation_strategy {
            // validate allocation strategy addresses for direct address allocation is properly aligned.
            // for BottomUp and TopDown strategies, the address parameter doesn't have to be page-aligned, but
            // the resulting allocation will be page-aligned.
            if address % required_alignment != 0 {
                return Err(EfiError::InvalidParameter);
            }
        }

        // Page allocations and pool allocations are disjoint; page allocations are allocated directly from the GCD and are
        // freed straight back to GCD. As such, a tracking allocator structure is not required.
        let start_address = allocator
            .allocate_from_gcd(
                self.gcd,
                self.handle,
                allocation_strategy,
                align_shift,
                uefi_pages_to_size!(required_pages),
            )
            .map_err(|err| match err {
                EfiError::InvalidParameter | EfiError::NotFound => err,
                _ => EfiError::OutOfResources,
            })?;

        let allocation = slice_from_raw_parts_mut(start_address as *mut u8, uefi_pages_to_size!(required_pages));
        let allocation = NonNull::new(allocation).ok_or(EfiError::OutOfResources)?;

        // Notify the FSB that additional pages were allocated for record keeping
        allocator.notify_page_allocation(allocation);

        Ok(allocation)
    }

    /// Frees the block of pages at the given address of the given size.
    ///
    /// ## Safety
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [`Self::allocate_pages`]
    pub unsafe fn free_pages(&self, address: usize, pages: usize) -> Result<(), EfiError> {
        let mut allocator = self.lock();
        allocator.stats.page_free_calls += 1;
        let granularity = allocator.page_allocation_granularity;

        // Ensure that the requested number of pages is a multiple of the granularity
        let required_pages = align_up(pages, uefi_size_to_pages!(granularity))?;

        allocator.free_pages_internal(self.gcd, self.handle, address, required_pages)?;

        // Notify the FSB that pages were freed for record keeping
        allocator.notify_pages_freed(address as efi::PhysicalAddress, required_pages);

        Ok(())
    }

    /// Sets the reserved memory range (bin range) for this allocator.
    ///
    /// See [`FixedSizeBlockAllocator::set_reserved_range()`] for details on the accounting model.
    ///
    /// ## Errors
    ///
    /// Returns [`EfiError::AlreadyStarted`] if a reserved range has already been set.
    pub fn set_reserved_range(&self, range: Range<efi::PhysicalAddress>) -> Result<(), EfiError> {
        self.lock().set_reserved_range(range)
    }

    /// Returns an iterator of the ranges of memory owned by this allocator
    /// Returns an empty iterator if the allocator does not own any memory.
    pub fn get_memory_ranges(&self) -> alloc::vec::IntoIter<Range<usize>> {
        let mut ranges = Vec::new();

        // The vector is grown with the lock released, since this allocator may be the one backing the global
        // allocator, in which case allocating while holding its lock would be a re-entrant lock.
        loop {
            let count = self.lock().get_memory_ranges().count();
            if count < ranges.capacity() {
                break;
            }
            ranges = Vec::with_capacity(count + 1);
        }

        let allocator = self.lock();
        let range_iter = allocator.get_memory_ranges().take(ranges.capacity());

        // Bounded by the reserved capacity so that pushing cannot reallocate while the lock is held.
        for range in range_iter {
            ranges.push(range);
        }
        drop(allocator);

        ranges.into_iter()
    }

    /// Returns the allocator handle associated with this allocator.
    pub fn handle(&self) -> efi::Handle {
        self.handle
    }

    /// Returns the reserved memory range, if any.
    #[cfg_attr(coverage, coverage(off))]
    pub fn reserved_range(&self) -> Option<Range<efi::PhysicalAddress>> {
        self.inner.lock().reserved_range.clone()
    }

    /// Returns the memory type for this allocator.
    #[allow(dead_code)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn memory_type(&self) -> efi::MemoryType {
        self.inner.lock().memory_type()
    }

    /// Returns allocation statistics for this allocator.
    #[allow(dead_code)]
    pub fn stats(&self) -> AllocationStatistics {
        *self.inner.lock().stats()
    }
}

// SAFETY: SpinLockedFixedSizeBlockAllocator serializes access and delegates to the inner allocator.
unsafe impl GlobalAlloc for SpinLockedFixedSizeBlockAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match self.allocate(layout) {
            Ok(alloc) => alloc.as_ptr().cast::<u8>(),
            Err(_) => core::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(ptr) = NonNull::new(ptr) {
            // SAFETY: ptr came from alloc with the same layout.
            unsafe { self.deallocate(ptr, layout) }
        }
    }
}

// SAFETY: SpinLockedFixedSizeBlockAllocator serializes access and delegates to the inner allocator.
unsafe impl Allocator for SpinLockedFixedSizeBlockAllocator {
    fn allocate(&self, layout: Layout) -> core::result::Result<NonNull<[u8]>, AllocError> {
        let mut allocator = self.lock();
        let allocation = allocator.alloc(layout);
        match allocation {
            Ok(alloc) => Ok(alloc),
            Err(FixedSizeBlockAllocatorError::OutOfMemory(block_size)) => {
                // compile time check that validates the assumption used below to add blocks to the free list.
                const _: () = assert!(block_sizes_divide_alignment());

                // Map enough pages to satisfy the request.
                let reclaim_pages = uefi_size_to_pages!(block_size);

                // Bring a sufficient contiguous retired range back into service before claiming more memory from the
                // GCD. Shorter fragmented ranges remain retired.
                let reclaim_range = allocator.find_retired_pages(reclaim_pages);
                if let Some(pages) = reclaim_range {
                    let mapped = allocator.update_page_attributes(self.gcd, &pages, false);
                    if let Err(err) = mapped {
                        allocator.mark_pages_retired(pages.clone());
                        log::error!("Failed to re-map retired pool pages {:#x}: {err:?}", pages.start);
                    } else {
                        allocator.restore_pages(pages.clone());
                        let start_address = pages.start;
                        if allocator.add_page_to_fsb(start_address, block_size).is_err() {
                            // if we failed here, we should still continue and allocate from the GCD
                            debug_assert!(false, "Failed to add blocks to the fixed-size block allocator.");
                        }

                        // this should always succeed now that we have added a proper sized block to the list
                        if let Ok(alloc) = allocator.internal_alloc(layout) {
                            return Ok(alloc);
                        }
                        debug_assert!(false, "Failed to allocate from internal allocator after reclaiming pages.");
                    }
                }

                // As a matter of policy, allocate at least the minimum expansion amount of memory and ensure the
                // size is aligned to `UEFI_PAGE_SIZE`.
                let original_allocation_size = max(block_size, self.min_expansion);
                let required_alignment = allocator.page_allocation_granularity;
                let mut allocation_size = align_up(original_allocation_size, required_alignment).map_err(|_| {
                    debug_assert!(false);
                    AllocError
                })?;

                loop {
                    let required_size = original_allocation_size
                        .checked_add(AllocatorListNode::size_for_free_region(allocation_size))
                        .ok_or_else(|| {
                            debug_assert!(false);
                            AllocError
                        })?;
                    let next_allocation_size = align_up(required_size, required_alignment).map_err(|_| {
                        debug_assert!(false);
                        AllocError
                    })?;
                    if next_allocation_size == allocation_size {
                        break;
                    }
                    allocation_size = next_allocation_size;
                }

                // Ensure that the requested number of pages is a multiple of the granularity
                let required_pages =
                    align_up(uefi_size_to_pages!(allocation_size), uefi_size_to_pages!(required_alignment)).map_err(
                        |_| {
                            debug_assert!(false);
                            AllocError
                        },
                    )?;

                allocation_size = uefi_pages_to_size!(required_pages);

                // Allocate additional memory through the GCD, returning AllocError
                // if the GCD returns an error
                let start_address: usize = allocator
                    .allocate_from_gcd(
                        self.gcd,
                        self.handle,
                        DEFAULT_ALLOCATION_STRATEGY,
                        page_shift_from_alignment(required_alignment).map_err(|_| {
                            debug_assert!(false);
                            AllocError
                        })?,
                        allocation_size,
                    )
                    .map_err(|err| {
                        log::error!(
                            "Allocator Expansion via GCD failed: [{err:?}], {{ Bytes: {allocation_size:#x}, Alignment: {required_alignment:#x}, Page Count: {required_pages:#x} }}",
                        );
                        AllocError
                    })?;

                // Expand the FSB using the allocated memory region
                let allocated_ptr = NonNull::new(start_address as *mut u8).ok_or_else(|| {
                    debug_assert!(false);
                    AllocError
                })?;
                let Ok(new_pages) = allocator.expand(NonNull::slice_from_raw_parts(allocated_ptr, allocation_size))
                else {
                    debug_assert!(false);
                    return Err(AllocError);
                };

                let mut pages_to_retire = None;
                if let Some(new_pages) = new_pages {
                    let active_end = min(new_pages.end, new_pages.start + uefi_pages_to_size!(reclaim_pages));
                    allocator.activate_new_pages(new_pages.start..active_end);

                    // Split the whole page into fixed-size blocks
                    if allocator.add_page_to_fsb(new_pages.start, block_size).is_err() {
                        debug_assert!(false, "Failed to add blocks to the fixed-size block allocator.");
                    }
                    if active_end < new_pages.end {
                        pages_to_retire = Some(active_end..new_pages.end);
                    }
                }

                // The activated portion of the new region was sized to satisfy this allocation.
                let allocation = allocator.internal_alloc(layout).map_err(|_| {
                    debug_assert!(false);
                    AllocError
                });
                if let Some(pages) = pages_to_retire {
                    allocator.retire_pages(self.gcd, pages);
                }
                allocation
            }
            Err(FixedSizeBlockAllocatorError::ExceedsBlockSize) => {
                // if we get a larger allocation requestion than the largest block size, we will satisfy it with pages
                // from the GCD directly. The largest block is already a page in size, so we are handling a very large
                // pool allocation
                allocator.stats.large_pool_allocation_calls += 1;
                let alignment = allocator.page_allocation_granularity;

                // Per UEFI spec, user pointers must be at least 8-byte aligned. Making the user alignment at least the
                // node alignment also ensures the node immediately before the user pointer can be accessed normally.
                let user_ptr_alignment = max(layout.align(), max(8, align_of::<LargeAllocationNode>()));

                let size = LargeAllocationNode::allocation_size(layout, alignment).ok_or_else(|| {
                    debug_assert!(false);
                    AllocError
                })?;

                let start_address: usize = allocator
                    .allocate_from_gcd(
                        self.gcd,
                        self.handle,
                        DEFAULT_ALLOCATION_STRATEGY,
                        page_shift_from_alignment(alignment).map_err(|_| {
                            debug_assert!(false);
                            AllocError
                        })?,
                        size,
                    )
                    .map_err(|err| {
                        log::error!(
                            "Allocator Expansion via GCD failed: [{err:?}], {{ Alignment: {alignment:#x}, Size: {size:#x} }}",
                        );
                        AllocError
                    })?;

                let user_addr = NonNull::new(
                    align_up(start_address + core::mem::size_of::<LargeAllocationNode>(), user_ptr_alignment)
                        .expect("User addresses should not be at the top of memory") as *mut u8,
                )
                .expect("User addresses should not be null");
                let large_allocation_node =
                    LargeAllocationNode { signature: LARGE_ALLOCATION_NODE_SIGNATURE, base: start_address, size };

                // SAFETY: size reserves enough alignment padding for a LargeAllocationNode immediately before the
                // aligned user pointer and the complete user allocation after it.
                unsafe { user_addr.cast::<LargeAllocationNode>().as_ptr().sub(1).write(large_allocation_node) };

                let allocation =
                    NonNull::slice_from_raw_parts(NonNull::new(start_address as *mut u8).ok_or(AllocError)?, size);
                allocator.notify_page_allocation(allocation);

                Ok(NonNull::slice_from_raw_parts(user_addr, layout.size()))
            }
            Err(e) => {
                log::error!(
                    "Allocator failed to expand and allocate new pages for the requested layout: {layout:?} with error {e:?}"
                );
                debug_assert!(false);
                Err(AllocError)
            }
        }
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        let mut allocator = self.lock();
        let Some(index) = list_index(&layout) else {
            allocator.stats.pool_free_calls += 1;
            // if this isn't in the fixed block list, we directly free the memory in the GCD
            // SAFETY: large allocations place a LargeAllocationNode immediately before the returned user pointer.
            let node = unsafe { NonNull::new_unchecked(ptr.cast::<LargeAllocationNode>().as_ptr().sub(1)).as_ref() };
            if node.signature != LARGE_ALLOCATION_NODE_SIGNATURE {
                debug_assert!(false, "Invalid large allocation node signature");
                return;
            }
            if LargeAllocationNode::allocation_size(layout, allocator.page_allocation_granularity) != Some(node.size) {
                debug_assert!(false, "Large allocation size does not match layout");
                return;
            }
            let Some(base) = NonZeroUsize::new(node.base) else {
                debug_assert!(false, "We shouldn't be deallocating page 0");
                return;
            };
            let pages = uefi_size_to_pages!(node.size);

            if let Err(e) = allocator.free_pages_internal(self.gcd, self.handle, base.get(), pages) {
                log::error!("Failed to free pages: {e:?}");
                debug_assert!(false, "Failed to free pages for large allocation");
            } else {
                allocator.notify_pages_freed(base.get() as efi::PhysicalAddress, pages);
            }
            return;
        };
        // SAFETY: the caller guarantees ptr was allocated with this layout, whose list index was resolved above.
        if let Some(page_base) = unsafe { allocator.dealloc(ptr, index) } {
            allocator.retire_pages(self.gcd, page_base..page_base + UEFI_PAGE_SIZE);
        }
    }
}

impl Display for SpinLockedFixedSizeBlockAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.lock().fmt(f)
    }
}

// SAFETY: SpinLockedFixedSizeBlockAllocator protects internal state with a spin lock.
unsafe impl Sync for SpinLockedFixedSizeBlockAllocator {}
// SAFETY: SpinLockedFixedSizeBlockAllocator protects internal state with a spin lock.
unsafe impl Send for SpinLockedFixedSizeBlockAllocator {}

impl PageAllocator for SpinLockedFixedSizeBlockAllocator {
    fn allocate_pages(
        &self,
        allocation_strategy: AllocationStrategy,
        pages: usize,
        alignment: usize,
    ) -> Result<NonNull<[u8]>, EfiError> {
        Self::allocate_pages(self, allocation_strategy, pages, alignment)
    }

    /// Frees the block of pages at the given address of the given size.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [`Self::allocate_pages`].
    unsafe fn free_pages(&self, address: usize, pages: usize) -> Result<(), EfiError> {
        // SAFETY: address/pages must refer to a valid allocation owned by this allocator
        // per the free_pages safety contract.
        unsafe { Self::free_pages(self, address, pages) }
    }

    fn set_reserved_range(&self, range: Range<efi::PhysicalAddress>) -> Result<(), EfiError> {
        Self::set_reserved_range(self, range)
    }

    fn get_memory_ranges(&self) -> alloc::vec::IntoIter<Range<usize>> {
        Self::get_memory_ranges(self)
    }

    fn handle(&self) -> efi::Handle {
        Self::handle(self)
    }

    fn reserved_range(&self) -> Option<Range<efi::PhysicalAddress>> {
        Self::reserved_range(self)
    }

    fn stats(&self) -> AllocationStatistics {
        Self::stats(self)
    }

    #[cfg(test)]
    fn reset(&self) {
        Self::reset(self);
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;
    use crate::{
        allocator::{
            DEFAULT_ALLOCATION_STRATEGY, DEFAULT_PAGE_ALLOCATION_GRANULARITY, HIGH_TRAFFIC_ALLOC_MIN_EXPANSION,
        },
        gcd, test_support,
    };
    use alloc::vec::Vec;
    use core::{alloc::GlobalAlloc, ffi::c_void, panic};
    use std::alloc::System;

    use patina::{
        uefi_pages_to_size, {SIZE_64KB, UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE},
    };

    use super::*;

    fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
        // SAFETY: Resetting the test GCD is safe in a test.
        unsafe { gcd.reset() };

        gcd.init(48, 16);
        let layout = Layout::from_size_align(size, UEFI_PAGE_SIZE).unwrap();
        // SAFETY: System allocator is used with a valid layout for test memory backing.
        let base = unsafe { System.alloc(layout) as u64 };
        // SAFETY: init_memory_blocks is used with a valid backing allocation for tests.
        unsafe {
            gcd.init_memory_blocks(GcdMemoryType::SystemMemory, base as usize, size, efi::MEMORY_WB, efi::MEMORY_WB)
                .unwrap();
        }
        base
    }

    // this runs each test twice, once with 4KB page allocation granularity and once with 64KB page allocation
    // granularity. This is to ensure that the allocator works correctly with both page allocation granularities.
    fn with_granularity_modulation<F: Fn(usize) + std::panic::RefUnwindSafe>(f: F) {
        f(DEFAULT_PAGE_ALLOCATION_GRANULARITY);
        f(SIZE_64KB);
    }

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            test_support::init_test_logger();
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_get_memory_ranges_returns_allocated_region() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                init_gcd(&GCD, 0x400000);

                let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, granularity);

                assert_eq!(fsb.get_memory_ranges().count(), 0);

                let allocation_size = DEFAULT_PAGE_ALLOCATION_GRANULARITY + UEFI_PAGE_SIZE;
                let allocated_address = GCD
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        page_shift_from_alignment(granularity).unwrap(),
                        allocation_size,
                        DUMMY_HANDLE,
                        None,
                    )
                    .unwrap();

                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(allocated_address as *mut u8).unwrap(),
                    allocation_size,
                ))
                .unwrap();

                let ranges: Vec<_> = fsb.get_memory_ranges().collect();
                assert_eq!(ranges.len(), 1);
                assert_eq!(ranges[0], allocated_address..allocated_address + allocation_size);
            });
        });
    }
    const DUMMY_HANDLE: *mut c_void = 0xDEADBEEF as *mut c_void;

    #[test]
    fn allocate_deallocate_test() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD for test.
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                init_gcd(&GCD, 0x400000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    DUMMY_HANDLE,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
            });
        });
    }

    #[test]
    fn test_list_index() {
        let layout = Layout::from_size_align(8, 1).unwrap();
        assert_eq!(list_index(&layout), Some(0));

        let layout = Layout::from_size_align(12, 8).unwrap();
        assert_eq!(list_index(&layout), Some(1));

        let layout = Layout::from_size_align(8, 32).unwrap();
        assert_eq!(list_index(&layout), Some(2));

        let layout = Layout::from_size_align(4096, 32).unwrap();
        assert_eq!(list_index(&layout), Some(9));

        let layout = Layout::from_size_align(1, 4096).unwrap();
        assert_eq!(list_index(&layout), Some(9));

        let layout = Layout::from_size_align(8192, 1).unwrap();
        assert_eq!(list_index(&layout), None);
    }

    #[test]
    fn test_construct_empty_fixed_size_block_allocator() {
        with_locked_state(|| {
            let fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);
            assert!(fsb.list_heads.iter().all(std::option::Option::is_none));
            assert!(fsb.allocators.is_none());
        });
    }

    #[test]
    fn test_allocator_node_metadata_can_span_multiple_pages() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let region_size = uefi_pages_to_size!(UEFI_PAGE_SIZE);
            assert!(AllocatorListNode::size_for_free_region(region_size) > UEFI_PAGE_SIZE);

            init_gcd(&GCD, region_size * 2);
            let region_base = GCD
                .allocate_memory_space(
                    DEFAULT_ALLOCATION_STRATEGY,
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    region_size,
                    DUMMY_HANDLE,
                    None,
                )
                .unwrap();
            let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);

            let retired = fsb
                .expand(NonNull::slice_from_raw_parts(NonNull::new(region_base as *mut u8).unwrap(), region_size))
                .unwrap()
                .unwrap();

            assert_eq!(retired.start, region_base + 3 * UEFI_PAGE_SIZE);
            // SAFETY: expand installed a valid allocator node at region_base.
            let node = unsafe { &*(region_base as *const AllocatorListNode) };
            assert_eq!(node.page_free_bytes[0..3], [0, 0, 0]);
            assert!(node.page_free_bytes[3..].iter().all(|free_bytes| *free_bytes == AllocatorListNode::RETIRING_PAGE));

            for &block_size in BLOCK_SIZES {
                assert!(fsb.internal_alloc(Layout::from_size_align(block_size, block_size).unwrap()).is_err());
            }
        });
    }

    #[test]
    fn test_expand() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                init_gcd(&GCD, 0x4000000);

                //verify no allocators exist before expand.
                let mut fsb = FixedSizeBlockAllocator::new(efi::RUNTIME_SERVICES_DATA, granularity);
                assert!(fsb.allocators.is_none());

                let allocation_size = DEFAULT_PAGE_ALLOCATION_GRANULARITY;

                // Allocate one page to expand by
                let allocated_address = GCD
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        allocation_size,
                        DUMMY_HANDLE,
                        None,
                    )
                    .unwrap();

                // A single page region is entirely consumed by the region header, so there is no whole page to retire.
                assert_eq!(
                    fsb.expand(NonNull::slice_from_raw_parts(
                        NonNull::new(allocated_address as *mut u8).unwrap(),
                        allocation_size,
                    )),
                    Err(FixedSizeBlockAllocatorError::InvalidExpansion)
                );
                assert!(fsb.allocators.is_none());
                assert!(fsb.retired_page_cursor.is_none());
                assert_eq!(fsb.stats.claimed_pages, 0);
                assert_eq!(fsb.stats.reserved_used, 0);
                assert_eq!(fsb.stats.retired_pages, 0);

                for &block_size in BLOCK_SIZES {
                    assert!(fsb.internal_alloc(Layout::from_size_align(block_size, block_size).unwrap()).is_err());
                }

                //expand by larger than the minimum expansion.
                let allocation_size = DEFAULT_PAGE_ALLOCATION_GRANULARITY + 0x1000;

                // Allocate one page to expand by
                let allocated_address = GCD
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        allocation_size,
                        DUMMY_HANDLE,
                        None,
                    )
                    .unwrap();

                // The whole page in this region starts retired, so it is not available to the backing allocator.
                let retired = fsb
                    .expand(NonNull::slice_from_raw_parts(
                        NonNull::new(allocated_address as *mut u8).unwrap(),
                        allocation_size,
                    ))
                    .unwrap();
                assert_eq!(retired, Some(allocated_address + UEFI_PAGE_SIZE..allocated_address + allocation_size));

                assert!(fsb.allocators.is_some());
                // SAFETY: fsb.allocators points to valid list nodes after expand.
                unsafe {
                    let node = &*fsb.allocators.unwrap();
                    assert!(node.next.is_none());
                    assert_eq!(node.region_size, allocation_size);
                    assert_eq!(node.page_free_bytes[0], 0);
                    assert!(
                        node.page_free_bytes[1..]
                            .iter()
                            .all(|free_bytes| *free_bytes == AllocatorListNode::RETIRING_PAGE)
                    );
                }
            });
        });
    }

    #[test]
    fn test_allocation_iterator() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x800000);

            let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);

            const NUM_ALLOCATIONS: usize = 5;

            const ALLOCATION_SIZE: usize = DEFAULT_PAGE_ALLOCATION_GRANULARITY + UEFI_PAGE_SIZE;
            for _ in 0..NUM_ALLOCATIONS {
                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(
                        GCD.allocate_memory_space(
                            DEFAULT_ALLOCATION_STRATEGY,
                            GcdMemoryType::SystemMemory,
                            UEFI_PAGE_SHIFT,
                            ALLOCATION_SIZE,
                            DUMMY_HANDLE,
                            None,
                        )
                        .unwrap() as *mut u8,
                    )
                    .unwrap(),
                    ALLOCATION_SIZE,
                ))
                .unwrap();
            }

            assert_eq!(NUM_ALLOCATIONS, AllocatorIterator::new(fsb.allocators).count());
            assert!(AllocatorIterator::new(fsb.allocators).all(|node| {
                // SAFETY: node pointers come from AllocatorIterator over fsb.allocators.
                unsafe { (*node).region_size == ALLOCATION_SIZE && (*node).page_free_bytes[0] == 0 }
            }));
        });
    }

    #[test]
    fn test_alloc() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                let base = init_gcd(&GCD, 0x400000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
                // SAFETY: fsb is initialized and used with a valid layout in tests.
                let allocation = unsafe { fsb.alloc(layout) };
                assert!(fsb.lock().allocators.is_some());
                assert!((allocation as u64) > base);
                assert!((allocation as u64) < base + 0x400000);
            });
        });
    }

    #[test]
    fn test_allocate() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                let base = init_gcd(&GCD, 0x400000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
                let allocation = fsb.allocate(layout).unwrap().as_ptr().cast::<u8>();
                assert!(fsb.lock().allocators.is_some());
                assert!((allocation as u64) > base);
                assert!((allocation as u64) < base + 0x400000);
            });
        });
    }

    #[test]
    fn test_dealloc() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                init_gcd(&GCD, 0x400000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                // SAFETY: fsb is initialized and used with a valid layout in tests.
                let allocation = unsafe { fsb.alloc(layout) };
                assert!(!allocation.is_null());
                let free_calls = fsb.stats().pool_free_calls;

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.dealloc(allocation, layout) };
                assert_eq!(fsb.stats().pool_free_calls, free_calls + 1);

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                // SAFETY: fsb is initialized and used with a valid layout in tests.
                let allocation = unsafe { fsb.alloc(layout) };
                assert!(!allocation.is_null());
                let free_calls = fsb.stats().pool_free_calls;

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.dealloc(allocation, layout) };
                assert_eq!(fsb.stats().pool_free_calls, free_calls + 1);
            });
        });
    }

    #[test]
    fn test_deallocate() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                init_gcd(&GCD, 0x400000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                let free_calls = fsb.stats().pool_free_calls;

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
                assert_eq!(fsb.stats().pool_free_calls, free_calls + 1);

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                let free_calls = fsb.stats().pool_free_calls;

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
                assert_eq!(fsb.stats().pool_free_calls, free_calls + 1);
            });
        });
    }

    #[test]
    fn test_allocate_pages() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to back the test GCD.
                let address = init_gcd(&GCD, 0x1000000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let pages = 4;

                let allocation =
                    fsb.allocate_pages(gcd::AllocateType::BottomUp(None), pages, UEFI_PAGE_SIZE).unwrap().cast::<u8>();

                assert!(allocation.as_ptr() as u64 >= address);
                assert!((allocation.as_ptr() as u64) < address + 0x1000000);

                // SAFETY: free_pages uses a valid test allocation pointer and page count.
                unsafe {
                    match fsb.free_pages(0, pages) {
                        Err(EfiError::NotFound) => {}
                        _ => panic!("Expected NOT_FOUND"),
                    }
                };

                // SAFETY: allocation and page count come from allocate_pages in this test.
                unsafe {
                    fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
                };
            });
        });
    }

    #[test]
    fn test_allocate_at_address() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to back the test GCD.
                let address = init_gcd(&GCD, 0x1000000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let target_address = (address + 0x400000 - max(8_u64 * UEFI_PAGE_SIZE as u64, granularity as u64))
                    & (!(granularity as u64 - 1_u64));
                let pages = 4;

                let allocation = fsb
                    .allocate_pages(gcd::AllocateType::Address(target_address as usize), pages, UEFI_PAGE_SIZE)
                    .unwrap()
                    .cast::<u8>();

                assert_eq!(allocation.as_ptr() as u64, target_address);

                // SAFETY: free_pages uses a valid test allocation pointer and page count.
                unsafe {
                    fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
                };
            });
        });
    }

    #[test]
    fn test_allocate_below_address_bottom_up() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to back the test GCD.
                let address = init_gcd(&GCD, 0x1000000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let target_address = address + 0x400000 - 8 * (UEFI_PAGE_SIZE as u64);
                let pages = 4;

                let allocation = fsb
                    .allocate_pages(gcd::AllocateType::BottomUp(Some(target_address as usize)), pages, UEFI_PAGE_SIZE)
                    .unwrap()
                    .cast::<u8>();
                assert!((allocation.as_ptr() as u64) < target_address);

                // SAFETY: free_pages uses a valid test allocation pointer and page count.
                unsafe {
                    fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
                };
            });
        });
    }

    #[test]
    fn test_allocate_below_address_top_down() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to back the test GCD.
                let address = init_gcd(&GCD, 0x1000000);

                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                let target_address = address + 0x400000 - 8 * (UEFI_PAGE_SIZE as u64);
                let pages = 4;

                let allocation = fsb
                    .allocate_pages(gcd::AllocateType::TopDown(Some(target_address as usize)), pages, UEFI_PAGE_SIZE)
                    .unwrap()
                    .cast::<u8>();
                assert!((allocation.as_ptr() as usize + uefi_pages_to_size!(pages)) <= target_address as usize);

                // SAFETY: allocation and page count come from allocate_pages in this test.
                unsafe {
                    fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
                };
            });
        });
    }

    #[test]
    fn test_allocator_commands_with_invalid_parameters() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x400000);

            // Test commands with bad handle.
            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                0 as _,
                efi::BOOT_SERVICES_DATA,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            match fsb.allocate_pages(AllocationStrategy::Address(0x1000), 5, UEFI_PAGE_SIZE) {
                Err(EfiError::InvalidParameter) => {}
                _ => panic!("Expected INVALID_PARAMETER"),
            }

            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                1 as _,
                efi::BOOT_SERVICES_DATA,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );

            let allocation_strategy = AllocationStrategy::Address(0x1000);
            match fsb.allocate_pages(allocation_strategy, 5, UEFI_PAGE_SIZE) {
                Err(EfiError::NotFound) => {}
                _ => panic!("Expected NOT_FOUND"),
            }
            // Test invalid alignment
            let allocation_strategy = AllocationStrategy::Address(0x1001);
            match fsb.allocate_pages(allocation_strategy, 5, UEFI_PAGE_SIZE) {
                Err(EfiError::InvalidParameter) => {}
                _ => panic!("Expected INVALID_PARAMETER"),
            }

            for alignment in [0, UEFI_PAGE_SIZE / 2, UEFI_PAGE_SIZE * 3] {
                assert_eq!(
                    fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, alignment),
                    Err(EfiError::InvalidParameter)
                );
            }

            // SAFETY: Invalid parameters are intentionally tested (unit test).
            unsafe {
                match fsb.free_pages(0x1001, 5) {
                    Err(EfiError::InvalidParameter) => {}
                    _ => panic!("Expected INVALID_PARAMETER"),
                }
            }
        });
    }

    #[test]
    fn validate_fsb_display_impl_does_not_panic() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x400000);

            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                1 as _,
                efi::BOOT_SERVICES_DATA,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 5, UEFI_PAGE_SIZE).unwrap();

            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            let _ = fsb.allocate(layout); // Triggers expansion + allocation

            // Call format on the inner FixedSizeBlockAllocator
            let _ = std::format!("{}", fsb.lock());
        });
    }

    #[test]
    fn test_allocation_stats() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            const TEST_MIN_EXPANSION_SIZE: usize = HIGH_TRAFFIC_ALLOC_MIN_EXPANSION;

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x1000000);

            // Make a fixed-sized-block allocator
            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                1 as _,
                efi::BOOT_SERVICES_DATA,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                HIGH_TRAFFIC_ALLOC_MIN_EXPANSION,
            );

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 0);
            assert_eq!(stats.large_pool_allocation_calls, 0);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, 0);

            //test alloc/deallocate and stats
            // SAFETY: fsb is initialized and used with a valid layout for testing.
            let fixed_layout = Layout::from_size_align(0x100, 0x8).unwrap();
            // SAFETY: fsb is initialized and fixed_layout is valid.
            let ptr = unsafe { fsb.alloc(fixed_layout) };

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 1);
            assert_eq!(stats.large_pool_allocation_calls, 0);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            let initial_claimed = stats.claimed_pages;

            // SAFETY: Allocation was returned by fsb for this layout.
            unsafe {
                fsb.dealloc(ptr, fixed_layout);
            }

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 1);
            assert_eq!(stats.large_pool_allocation_calls, 0);
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed);

            //test alloc/deallocate and stats blowing the bucket
            // SAFETY: fsb is initialized and used with a valid layout in tests.
            let ptr = unsafe { fsb.alloc(Layout::from_size_align(TEST_MIN_EXPANSION_SIZE * 3, 0x8).unwrap()) };

            //after this allocate, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.large_pool_allocation_calls, 1);
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);

            // SAFETY: Allocation was returned by fsb for this layout.
            unsafe {
                fsb.dealloc(ptr, Layout::from_size_align(TEST_MIN_EXPANSION_SIZE * 3, 0x8).unwrap());
            }

            //after this free, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.large_pool_allocation_calls, 1);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed);

            // test that a small page allocation fits in the 1MB free reserved region.
            let ptr = fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 0x4, UEFI_PAGE_SIZE).unwrap().as_ptr();

            //after this allocate_pages, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //16K allocated.
            //1MB-16k free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed + 0x4);

            // SAFETY: free_pages uses a valid test allocation pointer and page count.
            unsafe {
                fsb.free_pages(ptr.cast::<u8>() as usize, 0x4).unwrap();
            }

            //after this free, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed);

            //test that a lage page allocation results in more claimed pages.
            let ptr = fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 0x104, UEFI_PAGE_SIZE).unwrap().as_ptr();

            //after this allocate_pages, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.
            //104 pages (1MB+16K) page as a result of allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 2);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed + 0x104);

            // test that a small page allocation fits in the 1MB free reserved region.
            let ptr1 = fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 0x4, UEFI_PAGE_SIZE).unwrap().as_ptr();

            //after this allocate_pages, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //16K allocated.
            //1MB-16k free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.
            //104 pages (1MB+16K) page as a result of allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 3);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed + 0x104 + 0x4);

            // SAFETY: free_pages uses a valid test allocation pointer and page count.
            unsafe {
                fsb.free_pages(ptr1.cast::<u8>() as usize, 0x4).unwrap();
            }
            // SAFETY: free_pages uses a valid test allocation pointer and page count.
            unsafe {
                fsb.free_pages(ptr.cast::<u8>() as usize, 0x104).unwrap();
            }

            //after this free, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 3);
            assert_eq!(stats.page_free_calls, 3);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, initial_claimed);
        });
    }

    #[test]
    fn test_get_memory_ranges() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                let base = init_gcd(&GCD, 0x400000);

                let mut fsb = FixedSizeBlockAllocator::new(efi::RUNTIME_SERVICES_DATA, granularity);

                const NUM_ALLOCATIONS: usize = 3;

                // Expand the FSB multiple times
                for _ in 0..NUM_ALLOCATIONS {
                    fsb.expand(NonNull::slice_from_raw_parts(
                        NonNull::new(
                            GCD.allocate_memory_space(
                                DEFAULT_ALLOCATION_STRATEGY,
                                GcdMemoryType::SystemMemory,
                                UEFI_PAGE_SHIFT,
                                DEFAULT_PAGE_ALLOCATION_GRANULARITY + UEFI_PAGE_SIZE,
                                DUMMY_HANDLE,
                                None,
                            )
                            .unwrap() as *mut u8,
                        )
                        .unwrap(),
                        DEFAULT_PAGE_ALLOCATION_GRANULARITY + UEFI_PAGE_SIZE,
                    ))
                    .unwrap();
                }

                // Collect the memory ranges reported by the allocator
                let memory_ranges: Vec<_> = fsb.get_memory_ranges().collect();

                // Verify that the reported ranges match the expected ranges
                assert_eq!(memory_ranges.len(), NUM_ALLOCATIONS);
                for range in &memory_ranges {
                    assert!(range.start >= base as usize);
                    assert!(range.end <= (base + 0x400000) as usize);
                    assert!(range.start < range.end);
                }

                // Ensure that the ranges do not overlap
                for i in 0..memory_ranges.len() {
                    for j in i + 1..memory_ranges.len() {
                        assert!(
                            memory_ranges[i].end <= memory_ranges[j].start
                                || memory_ranges[j].end <= memory_ranges[i].start
                        );
                    }
                }
            });
        });
    }

    #[test]
    fn test_page_shift_from_alignment() {
        #[derive(Debug)]
        struct TestConfig {
            alignment: usize,
            expected: Result<usize, EfiError>,
        }

        let configs = [
            TestConfig { alignment: 0x1000, expected: Ok(12) },
            TestConfig { alignment: 0x2000, expected: Ok(13) },
            TestConfig { alignment: 0x400000, expected: Ok(22) },
            TestConfig { alignment: 0x6000, expected: Err(EfiError::InvalidParameter) },
            TestConfig { alignment: 0x800, expected: Err(EfiError::InvalidParameter) },
            TestConfig { alignment: 0, expected: Err(EfiError::InvalidParameter) },
        ];

        for config in configs {
            let result = page_shift_from_alignment(config.alignment);
            assert_eq!(result, config.expected, "Test config: {config:?}");
        }
    }

    // Returns a fixed size block allocator backed by a freshly initialized GCD with room to expand.
    fn fsb_with_gcd(gcd: &'static SpinLockedGcd) -> SpinLockedFixedSizeBlockAllocator {
        init_gcd(gcd, 0x400000);
        SpinLockedFixedSizeBlockAllocator::new(
            gcd,
            DUMMY_HANDLE,
            efi::BOOT_SERVICES_DATA,
            DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            HIGH_TRAFFIC_ALLOC_MIN_EXPANSION,
        )
    }

    fn page_attributes(gcd: &SpinLockedGcd, address: usize) -> u64 {
        gcd.get_memory_descriptor_for_address(address as efi::PhysicalAddress, |d, _| {
            d.memory_type != GcdMemoryType::NonExistent
        })
        .unwrap()
        .attributes
    }

    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const GCD_ALLOCATE: usize = 1;
    const GCD_FREE: usize = 2;
    const GCD_ATTRIBUTES: usize = 4;
    static CHECK_GCD_LOCK: AtomicBool = AtomicBool::new(false);
    static GCD_LOCK_OBSERVATIONS: AtomicUsize = AtomicUsize::new(0);
    static GCD_OBSERVED_UNLOCKED: AtomicBool = AtomicBool::new(false);
    static LOCK_CHECK_GCD: SpinLockedGcd = SpinLockedGcd::new(Some(observe_fsb_guard));
    static LOCK_CHECK_FSB: SpinLockedFixedSizeBlockAllocator = SpinLockedFixedSizeBlockAllocator::new(
        &LOCK_CHECK_GCD,
        DUMMY_HANDLE,
        efi::BOOT_SERVICES_DATA,
        DEFAULT_PAGE_ALLOCATION_GRANULARITY,
        HIGH_TRAFFIC_ALLOC_MIN_EXPANSION,
    );

    fn observe_fsb_guard(change: gcd::MapChangeType) {
        if !CHECK_GCD_LOCK.load(Ordering::Relaxed) {
            return;
        }
        let observation = match change {
            gcd::MapChangeType::AllocateMemorySpace => GCD_ALLOCATE,
            gcd::MapChangeType::FreeMemorySpace => GCD_FREE,
            gcd::MapChangeType::SetMemoryAttributes => GCD_ATTRIBUTES,
            _ => return,
        };
        GCD_LOCK_OBSERVATIONS.fetch_or(observation, Ordering::Relaxed);
        if LOCK_CHECK_FSB.inner.try_lock().is_some() {
            GCD_OBSERVED_UNLOCKED.store(true, Ordering::Relaxed);
        }
    }

    fn init_fsb_guard_check(granularity: usize) {
        CHECK_GCD_LOCK.store(false, Ordering::Relaxed);
        LOCK_CHECK_FSB.reset();
        LOCK_CHECK_FSB.lock().page_allocation_granularity = granularity;
        init_gcd(&LOCK_CHECK_GCD, 0x400000);
        GCD_LOCK_OBSERVATIONS.store(0, Ordering::Relaxed);
        GCD_OBSERVED_UNLOCKED.store(false, Ordering::Relaxed);
        CHECK_GCD_LOCK.store(true, Ordering::Relaxed);
    }

    fn assert_fsb_guard_observations(expected: usize) {
        assert_eq!(GCD_LOCK_OBSERVATIONS.swap(0, Ordering::Relaxed), expected);
        assert!(!GCD_OBSERVED_UNLOCKED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_fsb_pool_expansion_retirement_and_reclaim_hold_guard() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                init_fsb_guard_check(granularity);
                let layout = Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
                let allocation = LOCK_CHECK_FSB.allocate(layout).unwrap().cast::<u8>();
                assert_fsb_guard_observations(GCD_ALLOCATE | GCD_ATTRIBUTES);
                assert_eq!(LOCK_CHECK_FSB.stats().pool_allocation_calls, 1);
                assert_eq!(page_attributes(&LOCK_CHECK_GCD, allocation.addr().get()) & efi::MEMORY_RP, 0);
                let retired = LOCK_CHECK_FSB.stats().retired_pages;

                // SAFETY: allocation is live and was returned by this allocator for layout.
                unsafe { LOCK_CHECK_FSB.deallocate(allocation, layout) };
                assert_fsb_guard_observations(GCD_ATTRIBUTES);
                assert_eq!(LOCK_CHECK_FSB.stats().retired_pages, retired + 1);
                assert_eq!(LOCK_CHECK_FSB.lock().pending_retirement(usize::MAX), None);
                assert_eq!(page_attributes(&LOCK_CHECK_GCD, allocation.addr().get()) & efi::MEMORY_RP, efi::MEMORY_RP);

                let reclaimed = LOCK_CHECK_FSB.stats().reclaimed_pages;
                let allocation = LOCK_CHECK_FSB.allocate(layout).unwrap().cast::<u8>();
                assert_fsb_guard_observations(GCD_ATTRIBUTES);
                assert_eq!(LOCK_CHECK_FSB.stats().reclaimed_pages, reclaimed + 1);
                assert_eq!(page_attributes(&LOCK_CHECK_GCD, allocation.addr().get()) & efi::MEMORY_RP, 0);
                // SAFETY: allocation is live and was returned by this allocator for layout.
                unsafe { LOCK_CHECK_FSB.deallocate(allocation, layout) };
                assert_fsb_guard_observations(GCD_ATTRIBUTES);
                CHECK_GCD_LOCK.store(false, Ordering::Relaxed);
            });
        });
    }

    #[test]
    fn test_fsb_large_pool_and_direct_page_calls_hold_guard() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                init_fsb_guard_check(granularity);
                let layout = Layout::from_size_align(2 * UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
                let allocation = LOCK_CHECK_FSB.allocate(layout).unwrap().cast::<u8>();
                assert_fsb_guard_observations(GCD_ALLOCATE | GCD_ATTRIBUTES);
                assert_eq!(LOCK_CHECK_FSB.stats().large_pool_allocation_calls, 1);
                // SAFETY: allocation is live and was returned by this allocator for layout.
                unsafe { LOCK_CHECK_FSB.deallocate(allocation, layout) };
                assert_fsb_guard_observations(GCD_FREE);
                assert_eq!(LOCK_CHECK_FSB.stats().claimed_pages, 0);

                let allocation = LOCK_CHECK_FSB.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                assert_fsb_guard_observations(GCD_ALLOCATE | GCD_ATTRIBUTES);
                assert_eq!(LOCK_CHECK_FSB.stats().claimed_pages, uefi_size_to_pages!(granularity));
                // SAFETY: the live page allocation was returned by this allocator for one page.
                unsafe { LOCK_CHECK_FSB.free_pages(allocation.addr().get(), 1).unwrap() };
                assert_fsb_guard_observations(GCD_FREE);
                assert_eq!(LOCK_CHECK_FSB.stats().claimed_pages, 0);
                CHECK_GCD_LOCK.store(false, Ordering::Relaxed);
            });
        });
    }

    #[test]
    fn test_fsb_memory_map_event_observes_final_state_after_unlock() {
        use crate::events::EVENT_DB;

        struct Observation<'a> {
            fsb: &'a SpinLockedFixedSizeBlockAllocator,
            calls: usize,
            stats: Option<AllocationStatistics>,
            range: Option<Range<usize>>,
            pending_retirement: bool,
            observed_locked: bool,
            tpl: efi::Tpl,
        }

        extern "efiapi" fn observe_final_state(_event: efi::Event, context: *mut c_void) {
            // SAFETY: the registered context points to a live Observation until the event is closed.
            let observation = unsafe { &mut *context.cast::<Observation<'_>>() };
            observation.calls += 1;
            observation.tpl = crate::events::raise_tpl(efi::TPL_HIGH_LEVEL);
            crate::events::restore_tpl(observation.tpl);
            if let Some(allocator) = observation.fsb.inner.try_lock() {
                observation.stats = Some(*allocator.stats());
                observation.range = allocator.get_memory_ranges().next();
                observation.pending_retirement = allocator.pending_retirement(usize::MAX).is_some();
            } else {
                observation.observed_locked = true;
            }
        }

        fn assert_final_state(observation: &Observation<'_>, calls: usize) {
            assert_eq!(observation.calls, calls);
            assert!(!observation.observed_locked);
            assert!(!observation.pending_retirement);
            assert_eq!(observation.tpl, efi::TPL_CALLBACK);
            assert_eq!(std::format!("{:?}", observation.stats.unwrap()), std::format!("{:?}", observation.fsb.stats()));
            assert_eq!(observation.range, observation.fsb.lock().get_memory_ranges().next());
        }

        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(crate::events::gcd_map_change));
                init_gcd(&GCD, 0x400000);
                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    DUMMY_HANDLE,
                    efi::BOOT_SERVICES_DATA,
                    granularity,
                    HIGH_TRAFFIC_ALLOC_MIN_EXPANSION,
                );
                let mut observation = Observation {
                    fsb: &fsb,
                    calls: 0,
                    stats: None,
                    range: None,
                    pending_retirement: false,
                    observed_locked: false,
                    tpl: efi::TPL_APPLICATION,
                };
                let event = EVENT_DB
                    .create_event(
                        efi::EVT_NOTIFY_SIGNAL,
                        efi::TPL_CALLBACK,
                        Some(observe_final_state),
                        Some(core::ptr::from_mut(&mut observation).cast()),
                        Some(efi::EVENT_GROUP_MEMORY_MAP_CHANGE),
                    )
                    .unwrap();

                let allocator = fsb.lock();
                crate::events::gcd_map_change(gcd::MapChangeType::AllocateMemorySpace);
                assert_eq!(observation.calls, 0);
                drop(allocator);
                assert_final_state(&observation, 1);

                let layout = Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                assert_final_state(&observation, 2);
                assert_eq!(observation.stats.unwrap().pool_allocation_calls, 1);
                assert!(observation.stats.unwrap().retired_pages > 0);
                assert!(observation.range.as_ref().unwrap().contains(&allocation.addr().get()));
                // SAFETY: allocation is live and was returned by fsb for layout.
                unsafe { fsb.deallocate(allocation, layout) };
                assert_eq!(observation.calls, 2);

                let layout = Layout::from_size_align(2 * UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                assert_final_state(&observation, 3);
                assert_eq!(observation.stats.unwrap().large_pool_allocation_calls, 1);
                // SAFETY: allocation is live and was returned by fsb for layout.
                unsafe { fsb.deallocate(allocation, layout) };
                assert_final_state(&observation, 4);
                assert_eq!(observation.stats.unwrap().pool_free_calls, 2);

                let allocation = fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                assert_final_state(&observation, 5);
                assert_eq!(observation.stats.unwrap().page_allocation_calls, 1);
                // SAFETY: the live page allocation was returned by fsb for one page.
                unsafe { fsb.free_pages(allocation.addr().get(), 1).unwrap() };
                assert_final_state(&observation, 6);
                assert_eq!(observation.stats.unwrap().page_free_calls, 1);
                EVENT_DB.close_event(event).unwrap();
            });
        });
    }

    #[test]
    fn test_free_list_recycles_blocks_in_first_in_first_out_order() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            let layout = Layout::from_size_align(0x20, 0x20).unwrap();
            let first = fsb.allocate(layout).unwrap().cast::<u8>();
            let second = fsb.allocate(layout).unwrap().cast::<u8>();
            let third = fsb.allocate(layout).unwrap().cast::<u8>();

            // SAFETY: each allocation was returned by fsb for this layout.
            unsafe {
                fsb.deallocate(first, layout);
                fsb.deallocate(second, layout);
                fsb.deallocate(third, layout);
            }

            // Blocks are handed back out in the order they were freed, so the most recently freed block is the last
            // one to be reused.
            assert_eq!(fsb.allocate(layout).unwrap().cast::<u8>(), first);
            assert_eq!(fsb.allocate(layout).unwrap().cast::<u8>(), second);
            assert_eq!(fsb.allocate(layout).unwrap().cast::<u8>(), third);
        });
    }

    #[test]
    fn test_fsb_expansion_never_allocates_from_metadata_pages() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
                init_gcd(&GCD, 0x400000);
                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    DUMMY_HANDLE,
                    efi::BOOT_SERVICES_DATA,
                    granularity,
                    4 * UEFI_PAGE_SIZE,
                );
                let layout = Layout::from_size_align(2048, 2048).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                let (region, whole_pages) = {
                    let allocator = fsb.lock();
                    let node = allocator.allocators.unwrap();
                    // SAFETY: the lock keeps this allocator's permanently mapped region metadata accessible.
                    let node = unsafe { &*node };
                    let region = node.region_base()..node.region_base() + node.region_size;
                    let whole_pages = node.first_page_base()..region.end;
                    assert!(whole_pages.contains(&allocation.addr().get()));
                    assert!(allocation.addr().get().is_multiple_of(layout.align()));
                    (region, whole_pages)
                };
                let before = fsb.stats();
                assert_eq!(before.pool_allocation_calls, 1);
                assert_eq!(before.reclaimed_pages, 0);
                assert_eq!(before.retired_pages, whole_pages.len() / UEFI_PAGE_SIZE - 1);
                assert!(!whole_pages.is_empty());
                assert_eq!(page_attributes(&GCD, region.start) & efi::MEMORY_RP, 0);
                for page in whole_pages.clone().step_by(UEFI_PAGE_SIZE) {
                    let expected = if page == whole_pages.start { 0 } else { efi::MEMORY_RP };
                    assert_eq!(page_attributes(&GCD, page) & efi::MEMORY_RP, expected);
                }
                let page_layout = Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
                let reclaimed = fsb.allocate(page_layout).unwrap().cast::<u8>();
                assert!(whole_pages.contains(&reclaimed.addr().get()));
                assert_eq!(page_attributes(&GCD, reclaimed.addr().get()) & efi::MEMORY_RP, 0);
                assert_eq!(fsb.stats().claimed_pages, before.claimed_pages);
                assert_eq!(fsb.stats().reclaimed_pages, 1);
                assert_eq!(fsb.get_memory_ranges().count(), 1);
                // SAFETY: both pointers are live and were returned by this allocator for their respective layouts.
                unsafe {
                    fsb.deallocate(reclaimed, page_layout);
                    fsb.deallocate(allocation, layout);
                }
                for page in whole_pages.step_by(UEFI_PAGE_SIZE) {
                    assert_eq!(page_attributes(&GCD, page) & efi::MEMORY_RP, efi::MEMORY_RP);
                }
                assert_eq!(page_attributes(&GCD, region.start) & efi::MEMORY_RP, 0);
            });
        });
    }

    #[test]
    fn test_fsb_live_occupancy_matches_page_retirement_across_regions() {
        fn assert_page_occupancy(fsb: &SpinLockedFixedSizeBlockAllocator, live: &[(NonNull<u8>, Layout)]) {
            let allocator = fsb.lock();
            let regions: Vec<_> = allocator.get_memory_ranges().collect();

            for (allocation, layout) in live {
                let start = allocation.addr().get();
                let end = start + layout.size().max(layout.align()).max(8).next_power_of_two();
                assert!(start.is_multiple_of(layout.align()));
                assert!(regions.iter().any(|region| region.start <= start && end <= region.end));
                assert_eq!(live.iter().filter(|(other, _)| other == allocation).count(), 1);
                for (other, other_layout) in live {
                    if other != allocation {
                        let other_start = other.addr().get();
                        let other_end =
                            other_start + other_layout.size().max(other_layout.align()).max(8).next_power_of_two();
                        assert!(end <= other_start || other_end <= start, "live allocations overlap");
                    }
                }
            }

            for region in regions {
                let page_count = region.len() / UEFI_PAGE_SIZE;
                let metadata_size = Layout::new::<AllocatorListNode>()
                    .extend(Layout::array::<u16>(page_count).unwrap())
                    .unwrap()
                    .0
                    .pad_to_align()
                    .size();
                let metadata_end = region.start + metadata_size;
                let node =
                    AllocatorIterator::new(allocator.allocators).find(|node| node.addr() == region.start).unwrap();
                // SAFETY: the locked allocator owns every node yielded by its iterator.
                let node = unsafe { &*node };

                for (page_index, page_base) in region.step_by(UEFI_PAGE_SIZE).enumerate() {
                    let metadata = page_base < metadata_end;
                    let occupied = live.iter().any(|(allocation, layout)| {
                        let start = allocation.addr().get();
                        let end = start + layout.size().max(layout.align()).max(8).next_power_of_two();
                        start < page_base + UEFI_PAGE_SIZE && page_base < end
                    });
                    let should_retire = !metadata && !occupied;
                    let state = node.page_free_bytes[page_index];
                    assert_eq!(
                        page_attributes(fsb.gcd, page_base) & efi::MEMORY_RP,
                        if should_retire { efi::MEMORY_RP } else { 0 },
                        "page {page_base:#x}: metadata={metadata}, occupied={occupied}, state={state}, live={}, granularity={}",
                        live.len(),
                        allocator.page_allocation_granularity
                    );
                    if should_retire {
                        assert_eq!(state, AllocatorListNode::RETIRED_PAGE, "empty page {page_base:#x}");
                    } else {
                        assert!(state < AllocatorListNode::RETIRED_PAGE, "active page {page_base:#x}: {state}");
                    }
                }
            }
        }

        fn allocate_live(
            fsb: &SpinLockedFixedSizeBlockAllocator,
            live: &mut Vec<(NonNull<u8>, Layout)>,
            layout: Layout,
        ) {
            live.push((fsb.allocate(layout).unwrap().cast::<u8>(), layout));
            assert_page_occupancy(fsb, live);
        }

        fn free_live(
            fsb: &SpinLockedFixedSizeBlockAllocator,
            live: &mut Vec<(NonNull<u8>, Layout)>,
            index: usize,
        ) -> (NonNull<u8>, Layout) {
            let (allocation, layout) = live.remove(index);
            // SAFETY: the independent live ledger contains this allocator's still-live pointer and original layout.
            unsafe { fsb.deallocate(allocation, layout) };
            assert_page_occupancy(fsb, live);
            (allocation, layout)
        }

        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
                init_gcd(&GCD, 0x400000);
                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    DUMMY_HANDLE,
                    efi::BOOT_SERVICES_DATA,
                    granularity,
                    4 * UEFI_PAGE_SIZE,
                );
                let layouts: Vec<_> = [
                    (8, 8),
                    (24, 16),
                    (49, 32),
                    (65, 32),
                    (129, 64),
                    (300, 64),
                    (900, 128),
                    (1500, 256),
                    (17, 2048),
                    (2049, 512),
                    (3073, 4096),
                ]
                .into_iter()
                .map(|(size, alignment)| Layout::from_size_align(size, alignment).unwrap())
                .collect();
                let mut live = Vec::new();
                assert_page_occupancy(&fsb, &live);
                for _ in 0..24 {
                    for layout in &layouts {
                        allocate_live(&fsb, &mut live, *layout);
                    }
                }
                let regions: Vec<_> = fsb.get_memory_ranges().collect();
                assert!(regions.len() >= 3, "workload must force multiple expansions");
                assert!(regions.iter().all(|region| live.iter().any(|(ptr, _)| region.contains(&ptr.addr().get()))));

                let mut freed = Vec::new();
                for index in (0..live.len()).rev().step_by(3) {
                    freed.push(free_live(&fsb, &mut live, index));
                }
                for (_, layout) in &freed {
                    allocate_live(&fsb, &mut live, *layout);
                }
                let previously_used: Vec<_> = live.iter().map(|(ptr, _)| ptr.addr().get() & !UEFI_PAGE_MASK).collect();
                while !live.is_empty() {
                    let index = live.len() / 2;
                    free_live(&fsb, &mut live, index);
                }

                let reclaimed_before = fsb.stats().reclaimed_pages;
                for _ in 0..24 {
                    for layout in layouts.iter().rev() {
                        allocate_live(&fsb, &mut live, *layout);
                    }
                }
                assert!(fsb.stats().reclaimed_pages > reclaimed_before);
                assert!(live.iter().any(|(ptr, _)| previously_used.contains(&(ptr.addr().get() & !UEFI_PAGE_MASK))));
                while !live.is_empty() {
                    free_live(&fsb, &mut live, 0);
                }
            });
        });
    }

    #[test]
    fn test_fsb_eight_byte_blocks_fill_and_retire_page() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);
            let layout = Layout::from_size_align(8, 8).unwrap();
            let allocations: Vec<_> =
                (0..UEFI_PAGE_SIZE / 8).map(|_| fsb.allocate(layout).unwrap().cast::<u8>()).collect();
            let page_base = allocations[0].addr().get();
            assert!(page_base.is_multiple_of(UEFI_PAGE_SIZE));
            for (index, allocation) in allocations.iter().enumerate() {
                assert_eq!(allocation.addr().get(), page_base + index * 8);
            }
            let retired = fsb.stats().retired_pages;
            for allocation in allocations {
                // SAFETY: each allocation is live and was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
            }
            assert_eq!(fsb.stats().retired_pages, retired + 1);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, efi::MEMORY_RP);
            let reclaimed = fsb.allocate(layout).unwrap().cast::<u8>();
            assert_eq!(reclaimed.addr().get(), page_base);
            // SAFETY: reclaimed was returned by fsb for this layout and has not been freed.
            unsafe { fsb.deallocate(reclaimed, layout) };
        });
    }

    #[test]
    fn test_fully_freed_page_is_retired_and_unmapped() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            // A page sized block occupies an entire page on its own.
            let layout = Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
            let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
            let page_base = allocation.addr().get();
            assert!(page_base.is_multiple_of(UEFI_PAGE_SIZE));
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);

            let retired = fsb.stats().retired_pages;

            // SAFETY: allocation was returned by fsb for this layout.
            unsafe { fsb.deallocate(allocation, layout) };

            assert_eq!(fsb.stats().retired_pages, retired + 1);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, efi::MEMORY_RP);
        });
    }

    #[test]
    fn test_partially_freed_page_is_not_retired() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            let small = Layout::from_size_align(0x20, 0x20).unwrap();
            let first = fsb.allocate(small).unwrap().cast::<u8>();
            let second = fsb.allocate(small).unwrap().cast::<u8>();
            let page_base = first.addr().get() & !UEFI_PAGE_MASK;
            assert_eq!(second.addr().get() & !UEFI_PAGE_MASK, page_base);
            let retired = fsb.stats().retired_pages;

            // SAFETY: first was returned by fsb for this layout.
            unsafe { fsb.deallocate(first, small) };

            assert_eq!(fsb.stats().retired_pages, retired);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);

            // SAFETY: second was returned by fsb for this layout.
            unsafe { fsb.deallocate(second, small) };
        });
    }

    #[test]
    fn test_update_page_attributes_applies_free_memory_policy() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);
            let allocation = fsb.allocate(Layout::from_size_align(0x20, 0x20).unwrap()).unwrap().cast::<u8>();
            let first_page = allocation.addr().get() & !UEFI_PAGE_MASK;
            let second_page = first_page + UEFI_PAGE_SIZE;

            assert_eq!(
                GCD.set_memory_space_attributes(first_page, UEFI_PAGE_SIZE, efi::MEMORY_WB | efi::MEMORY_XP),
                Err(EfiError::NotReady)
            );
            assert_eq!(
                GCD.set_memory_space_attributes(second_page, UEFI_PAGE_SIZE, efi::MEMORY_WB | efi::MEMORY_RO),
                Err(EfiError::NotReady)
            );

            fsb.lock().update_page_attributes(&GCD, &(first_page..second_page + UEFI_PAGE_SIZE), true).unwrap();

            assert_eq!(page_attributes(&GCD, first_page), efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RP);
            assert_eq!(page_attributes(&GCD, second_page), efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RP);
        });
    }

    #[test]
    fn test_update_page_attributes_rolls_back_whole_range_after_partial_failure() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let size = 0x400000;
            let base = init_gcd(&GCD, size) as usize;
            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                DUMMY_HANDLE,
                efi::BOOT_SERVICES_DATA,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                HIGH_TRAFFIC_ALLOC_MIN_EXPANSION,
            );
            let pages = 2;
            let allocation = base + size - uefi_pages_to_size!(pages);
            GCD.allocate_memory_space(
                AllocationStrategy::Address(allocation),
                GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                uefi_pages_to_size!(pages),
                DUMMY_HANDLE,
                None,
            )
            .unwrap();
            let allocated_attributes =
                GCD.memory_protection_policy.apply_allocated_memory_protection_policy(0, GcdMemoryType::SystemMemory);
            assert_eq!(page_attributes(&GCD, allocation), allocated_attributes);

            let result = fsb.lock().update_page_attributes(
                &GCD,
                &(allocation..allocation + uefi_pages_to_size!(pages + 1)),
                true,
            );

            assert!(result.is_err());
            assert_eq!(page_attributes(&GCD, allocation), allocated_attributes);
        });
    }

    #[test]
    fn test_fsb_large_allocation_size() {
        for (size, alignment, expected) in [
            (UEFI_PAGE_SIZE + 1, 8, 2 * UEFI_PAGE_SIZE),
            (2 * UEFI_PAGE_SIZE, UEFI_PAGE_SIZE, 4 * UEFI_PAGE_SIZE),
            (UEFI_PAGE_SIZE + 1, SIZE_64KB, SIZE_64KB + 2 * UEFI_PAGE_SIZE),
        ] {
            let layout = Layout::from_size_align(size, alignment).unwrap();
            assert_eq!(LargeAllocationNode::allocation_size(layout, UEFI_PAGE_SIZE), Some(expected));
        }
        let layout = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
        assert_eq!(LargeAllocationNode::allocation_size(layout, 1usize << (usize::BITS - 1)), None);
    }

    #[test]
    fn test_fsb_large_allocation_rejects_corrupted_size() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);
            let layout = Layout::from_size_align(2 * UEFI_PAGE_SIZE, 8).unwrap();
            let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
            let claimed = fsb.stats().claimed_pages;
            // SAFETY: this live large allocation has a writable header immediately before its user pointer.
            let node = unsafe { allocation.cast::<LargeAllocationNode>().as_ptr().sub(1) };
            // SAFETY: node points to the live header, and the allocator is not accessed concurrently.
            let original_size = unsafe {
                let original_size = (*node).size;
                (*node).size += UEFI_PAGE_SIZE;
                original_size
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: allocation is live and layout is the original allocation layout.
                unsafe { fsb.deallocate(allocation, layout) };
            }));
            assert_eq!(result.is_err(), cfg!(debug_assertions));
            assert_eq!(fsb.stats().claimed_pages, claimed);
            // SAFETY: rejected deallocation left the allocation and its header live.
            unsafe {
                (*node).size = original_size;
                fsb.deallocate(allocation, layout);
            }
            assert_eq!(fsb.stats().claimed_pages, 0);
        });
    }

    #[test]
    fn test_large_allocation_is_freed_directly_to_gcd() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            // Allocations larger than the largest block size are not tracked for retirement.
            let large = Layout::from_size_align(UEFI_PAGE_SIZE * 2, UEFI_PAGE_SIZE).unwrap();
            let allocation = fsb.allocate(large).unwrap().cast::<u8>();
            let page_base = allocation.addr().get();
            let retired = fsb.stats().retired_pages;
            assert_eq!(fsb.stats().large_pool_allocation_calls, 1);
            assert!(fsb.stats().claimed_pages > 0);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);

            // SAFETY: allocation was returned by fsb for this layout.
            unsafe { fsb.deallocate(allocation, large) };

            assert_eq!(fsb.stats().retired_pages, retired);
            assert_eq!(fsb.stats().claimed_pages, 0);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, efi::MEMORY_RP);
        });
    }

    #[test]
    fn test_large_allocations_support_user_alignment() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            for alignment in [8, UEFI_PAGE_SIZE, SIZE_64KB, SIZE_64KB * 2] {
                let layout = Layout::from_size_align(UEFI_PAGE_SIZE + 1, alignment).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();

                assert!(allocation.addr().get().is_multiple_of(alignment));

                // SAFETY: allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
                assert_eq!(fsb.stats().claimed_pages, 0);
            }

            assert_eq!(fsb.stats().large_pool_allocation_calls, 4);
        });
    }

    #[test]
    fn test_retiring_a_page_unlinks_every_block_it_holds() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            // Enough small blocks to cover several pages so that retirement has to unlink many free list nodes.
            const BLOCK_COUNT: usize = 1024;
            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocations: Vec<_> = (0..BLOCK_COUNT).map(|_| fsb.allocate(layout).unwrap().cast::<u8>()).collect();
            let retired = fsb.stats().retired_pages;

            for allocation in &allocations {
                // SAFETY: each allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(*allocation, layout) };
            }

            assert!(fsb.stats().retired_pages > retired);

            // The free lists are still consistent and never hand out memory that has been unmapped.
            for _ in 0..BLOCK_COUNT / 2 {
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                assert_eq!(page_attributes(&GCD, allocation.addr().get()) & efi::MEMORY_RP, 0);
            }
        });
    }

    #[test]
    fn test_retired_page_is_reclaimed_and_remapped() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            let layout = Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap();
            let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
            let page_base = allocation.addr().get();

            // SAFETY: allocation was returned by fsb for this layout.
            unsafe { fsb.deallocate(allocation, layout) };
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, efi::MEMORY_RP);

            // The freed page is the lowest retired page, so the next allocation brings it back into service.
            let reclaimed = fsb.stats().reclaimed_pages;
            let next = fsb.allocate(layout).unwrap().cast::<u8>();
            assert_eq!(fsb.stats().reclaimed_pages, reclaimed + 1);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);
            assert_eq!(next.addr().get(), page_base);
        });
    }

    #[test]
    fn test_new_region_starts_unmapped() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            let layout = Layout::from_size_align(0x20, 0x20).unwrap();
            let allocation = fsb.allocate(layout).unwrap().cast::<u8>();

            // Only the pages needed to satisfy the allocation are mapped; the rest of the region the allocator
            // claimed from the GCD stays unmapped until it is needed.
            let region = fsb.get_memory_ranges().next().unwrap();
            let mapped = (region.start..region.end)
                .step_by(UEFI_PAGE_SIZE)
                .filter(|page| page_attributes(&GCD, *page) & efi::MEMORY_RP == 0)
                .count();

            assert!(mapped <= uefi_size_to_pages!(layout.size()) + 1, "{mapped} pages mapped");
            assert!(mapped < uefi_size_to_pages!(region.len()));
            assert_eq!(page_attributes(&GCD, allocation.addr().get()) & efi::MEMORY_RP, 0);
        });
    }

    #[test]
    fn test_pages_awaiting_unmap_are_not_reclaimable() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);

            let region_size = 0x4000;
            let region = GCD
                .allocate_memory_space(
                    DEFAULT_ALLOCATION_STRATEGY,
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    region_size,
                    DUMMY_HANDLE,
                    None,
                )
                .unwrap();

            let pages = fsb
                .expand(NonNull::slice_from_raw_parts(NonNull::new(region as *mut u8).unwrap(), region_size))
                .unwrap()
                .unwrap();
            assert_eq!(pages, region + UEFI_PAGE_SIZE..region + region_size);

            // The pages are out of circulation but still mapped, so they must not be brought back into service until
            // the caller reports that they have been unmapped.
            assert_eq!(fsb.find_retired_pages(usize::MAX), None);

            fsb.mark_pages_retired(pages.clone());
            assert_eq!(fsb.find_retired_pages(uefi_size_to_pages!(pages.len())), Some(pages.clone()));

            // Restoring and splitting one page makes it available to the fixed-size allocator again.
            fsb.restore_pages(pages.start..pages.start + UEFI_PAGE_SIZE);
            fsb.add_page_to_fsb(pages.start, UEFI_PAGE_SIZE).unwrap();
            assert_eq!(fsb.find_retired_pages(usize::MAX), None);
            assert!(fsb.alloc(Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap()).is_ok());
        });
    }

    #[test]
    fn test_retired_pages_are_found_in_fifo_order_across_regions() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);
            let region_size = UEFI_PAGE_SIZE * 4;
            let mut retired_regions = Vec::new();

            for _ in 0..3 {
                let region = GCD
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        region_size,
                        DUMMY_HANDLE,
                        None,
                    )
                    .unwrap();
                let retired = fsb
                    .expand(NonNull::slice_from_raw_parts(NonNull::new(region as *mut u8).unwrap(), region_size))
                    .unwrap()
                    .unwrap();
                fsb.mark_pages_retired(retired.clone());
                retired_regions.push(retired);
            }

            let first_region = retired_regions.first().unwrap();
            for page_base in first_region.clone().step_by(UEFI_PAGE_SIZE) {
                assert_eq!(fsb.find_retired_pages(1), Some(page_base..page_base + UEFI_PAGE_SIZE));
            }

            for region in retired_regions.iter().skip(1) {
                for page_base in region.clone().step_by(UEFI_PAGE_SIZE) {
                    assert_eq!(fsb.find_retired_pages(1), Some(page_base..page_base + UEFI_PAGE_SIZE));
                }
            }

            // Since the pages remain retired, the cursor eventually wraps back to the first region.
            assert_eq!(fsb.find_retired_pages(1), Some(first_region.start..first_region.start + UEFI_PAGE_SIZE));
        });
    }

    #[test]
    fn test_find_retired_pages_skips_fragmented_runs() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);
            let region_size = UEFI_PAGE_SIZE * 4;
            let mut retired_regions = Vec::new();

            for _ in 0..2 {
                let region = GCD
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        region_size,
                        DUMMY_HANDLE,
                        None,
                    )
                    .unwrap();
                let retired = fsb
                    .expand(NonNull::slice_from_raw_parts(NonNull::new(region as *mut u8).unwrap(), region_size))
                    .unwrap()
                    .unwrap();
                fsb.mark_pages_retired(retired.clone());
                retired_regions.push(retired);
            }

            let first_region = retired_regions.first().unwrap();
            fsb.restore_page(first_region.start + UEFI_PAGE_SIZE);

            let second_region = retired_regions.get(1).unwrap();
            assert_eq!(fsb.find_retired_pages(2), Some(second_region.start..second_region.start + 2 * UEFI_PAGE_SIZE));
        });
    }
}
