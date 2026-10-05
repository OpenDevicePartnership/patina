//! Fixed-sized block allocator.
//!
//! Implements a fixed-sized block allocator backed by a linked list allocator. Based on the example fixed-sized block
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
    ops::Range,
    ptr::{NonNull, slice_from_raw_parts_mut},
    result::Result,
    sync::atomic::{AtomicBool, Ordering},
};
use linked_list_allocator::{align_down_size, align_up_size};
use patina::standard::efi;
use patina::{
    error::EfiError,
    pi::dxe_services::GcdMemoryType,
    uefi_pages_to_size, uefi_size_to_pages, writelncrlf,
    {UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, align_up, page_shift_from_alignment},
};

/// Type for describing errors that this implementation can produce.
#[derive(Debug, PartialEq)]
pub enum FixedSizeBlockAllocatorError {
    /// Could not satisfy allocation request, and expansion failed.
    ///
    /// Specifies how much additional memory is required to be added to the allocator through
    /// [`FixedSizeBlockAllocator::expand()`] in order to fulfill the attempted allocation.
    OutOfMemory(usize),
    /// The provided layout was invalid.
    InvalidLayout,
    /// The memory region provided to extend the allocator was invalid.
    InvalidExpansion,
    /// An internal error occurred.
    InternalError,
}

const ALIGNMENT: usize = 0x1000;

// The block sizes that the fixed-size block allocator will manage. 16 bytes is the smallest size due to the
// restriction of the fallback allocator that hands out the backing memory for these blocks. That allocator
// has 16 bytes of metadata it stores in free regions and so at a minimum can hand out that size of memory, in
// order for us to keep track of how many free bytes are available in each page.
const BLOCK_SIZES: &[usize] = &[16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

// Returns the index in the block list for the minimum size block that will
// satisfy allocation for the given layout
fn list_index(layout: &Layout) -> Option<usize> {
    let required_block_size = layout.size().max(layout.align());
    BLOCK_SIZES.iter().position(|&s| s >= required_block_size)
}

struct BlockListNode {
    next: Option<NonNull<BlockListNode>>,
}

struct AllocatorListNode {
    next: Option<*mut AllocatorListNode>,
    allocator: linked_list_allocator::Heap,

    /// The number of bytes of each whole page in this region that are currently sitting on the fixed-size block free
    /// lists, along with the [`AllocatorListNode::RETIRED_PAGE`] and [`AllocatorListNode::RETIRING_PAGE`] states.
    ///
    /// This is stored in the metadata header at the front of the region for every page in the region. Pages containing
    /// metadata are never retired so that the metadata remains accessible.
    page_free_bytes: &'static mut [u16],
}

const _: () = assert!(AllocatorListNode::RETIRED_PAGE < AllocatorListNode::RETIRING_PAGE);

impl AllocatorListNode {
    // This flag is used to indicate that a page has been retired and is no longer in use by the allocator.
    // It represents the free bytes in a page. When the free bytes are equal to the page size, the entire page
    // is free and therefore retired. It contrasts with RETIRING_PAGE, which is greater than the page size and used
    // to indicate the page in question is in process of being retired. Any number of free bytes less than this
    // value indicate that the page has
    const RETIRED_PAGE: u16 = UEFI_PAGE_SIZE as u16;

    // A page is marked as retiring while it is in the process of being unmapped so it is not used if the unmap
    // causes allocations. This value is chosen as greater than the RETIRED_PAGE marker so that it is clear that
    // the page is not in use, which would be represented by 0 <= free_bytes <= RETIRED_PAGE.
    const RETIRING_PAGE: u16 = u16::MAX;

    /// Returns the base address of the region containing this node and its page metadata.
    fn region_base(&self) -> usize {
        core::ptr::from_ref(self).addr()
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

/// Describes the header that [`FixedSizeBlockAllocator::expand`] reserves at the front of an expansion region.
struct RegionHeader {
    /// The total size of the header. The backing heap for the region starts immediately after it.
    size: usize,
    /// The offset of the per-page metadata array within the region.
    meta_offset: usize,
    /// The number of pages contained in the region, including the pages that contain the metadata.
    page_count: usize,
}

impl RegionHeader {
    fn new(page_count: usize) -> Option<Self> {
        let (layout, meta_offset) =
            Layout::new::<AllocatorListNode>().extend(Layout::array::<u16>(page_count).ok()?).ok()?;

        Some(Self { size: layout.pad_to_align().size(), meta_offset, page_count })
    }

    /// Returns the page-aligned region size needed to hold `payload_size` bytes after the region header.
    fn region_size_for_payload(payload_size: usize) -> Option<usize> {
        let mut page_count = 1;

        loop {
            let header = Self::new(page_count)?;
            let region_size = align_up(payload_size.checked_add(header.size)?, UEFI_PAGE_SIZE).ok()?;
            let required_page_count = region_size / UEFI_PAGE_SIZE;

            if required_page_count == page_count {
                return Some(region_size);
            }

            page_count = required_page_count;
        }
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

    /// The tails of the linked lists for each fixed-size block, maintained so page retirement can unlink blocks.
    list_tails: [Option<NonNull<BlockListNode>>; BLOCK_SIZES.len()],

    /// The linked-list of allocators that this allocator uses to back allocations that are larger than the fixed-size
    /// blocks or if the required fixed-size block list is empty.
    allocators: Option<*mut AllocatorListNode>,

    /// Most recently used allocator node for per-page accounting.
    page_accounting_node: Option<NonNull<AllocatorListNode>>,

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
            page_accounting_node: None,
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
        self.page_accounting_node = None;
        self.reserved_range = None;
        self.stats = AllocationStatistics::new();
    }

    /// Expand the memory available to this allocator with a new contiguous region of memory, setting up a new allocator
    /// node to manage this range. The region is partially consumed by an `AllocatorListNode` and the per-page tracking
    /// metadata for the region; the remainder is available to the allocator.
    ///
    /// Every whole page of the new region after the metadata starts retired, so the allocator does not have any memory
    /// mapped that it has not handed out yet. The returned range is the memory the caller must unmap, or must hand to
    /// [`Self::restore_pages`] if it cannot be unmapped.
    ///
    /// ## Errors
    ///
    /// Returns [`FixedSizeBlockAllocatorError::InvalidExpansion`] if the new region is not larger than and aligned to
    /// `AllocatorListNode`.
    /// Returns [`FixedSizeBlockAllocatorError::InternalError`] if the whole pages in the new region cannot be reserved
    /// from its backing allocator.
    pub fn expand(
        &mut self,
        new_region: NonNull<[u8]>,
    ) -> core::result::Result<Option<Range<usize>>, FixedSizeBlockAllocatorError> {
        // Interpret the first part of the provided region as an AllocatorListNode
        let alloc_node_ptr = new_region.as_ptr().cast::<AllocatorListNode>();

        if !alloc_node_ptr.addr().is_multiple_of(UEFI_PAGE_SIZE) {
            debug_assert!(false, "FSB expanded with memory region that is not page aligned.");
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        // Only whole pages contained within the region are tracked for retirement.
        let header = RegionHeader::new(uefi_size_to_pages!(new_region.len())).ok_or_else(|| {
            debug_assert!(false, "FSB expanded with insufficiently sized memory region.");
            FixedSizeBlockAllocatorError::InvalidExpansion
        })?;

        // Ensure we're expanding enough to fit the node and its page metadata
        if new_region.len() <= header.size {
            debug_assert!(false, "FSB expanded with insufficiently sized memory region.");
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        // SAFETY: the header lies within the region, which was validated to be large enough above, and meta_offset is
        // aligned for the page metadata by Layout::extend.
        let page_free_bytes = unsafe {
            let meta_ptr = new_region.as_ptr().cast::<u8>().add(header.meta_offset).cast::<u16>();
            for index in 0..header.page_count {
                meta_ptr.add(index).write(0);
            }
            core::slice::from_raw_parts_mut(meta_ptr, header.page_count)
        };

        let heap_region: NonNull<[u8]> = NonNull::slice_from_raw_parts(
            // SAFETY: header.size is less than the region length, as validated above.
            NonNull::new(unsafe { new_region.as_ptr().cast::<u8>().add(header.size) }).ok_or_else(|| {
                debug_assert!(false, "FSB expanded with insufficiently sized memory region.");
                FixedSizeBlockAllocatorError::InvalidExpansion
            })?,
            new_region.len() - header.size,
        );

        //write the allocator node structure into the start of the range, initialize its heap with the remainder of
        //the range, and add the new allocator to the front of the allocator list.
        let node = AllocatorListNode { next: None, allocator: linked_list_allocator::Heap::empty(), page_free_bytes };
        // SAFETY: alloc_node_ptr is aligned and points to valid writable memory for an AllocatorListNode.
        unsafe {
            alloc_node_ptr.write(node);
            (*alloc_node_ptr).allocator.init(heap_region.cast::<u8>().as_ptr(), heap_region.len());
            (*alloc_node_ptr).next = self.allocators;
        }

        self.allocators = Some(alloc_node_ptr);
        self.page_accounting_node = NonNull::new(alloc_node_ptr);

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
        let (heap_start, heap_end) = unsafe { ((*node).allocator.bottom() as usize, (*node).allocator.top() as usize) };

        let pages = align_up_size(heap_start, UEFI_PAGE_SIZE)..align_down_size(heap_end, UEFI_PAGE_SIZE);
        if pages.is_empty() {
            // The region contains no whole heap pages after its metadata, but its partial page remains usable.
            return Ok(None);
        }

        let layout = Layout::from_size_align(pages.len(), UEFI_PAGE_SIZE)
            .map_err(|_| FixedSizeBlockAllocatorError::InternalError)?;

        // SAFETY: node is valid and its heap covers all of the whole pages in the region.
        unsafe {
            (*node).allocator.allocate_first_fit(layout).map_err(|_| FixedSizeBlockAllocatorError::InternalError)?;
            for page_base in pages.clone().step_by(UEFI_PAGE_SIZE) {
                if let Some((_, free_bytes)) = (*node).page_free_bytes_mut(page_base) {
                    *free_bytes = AllocatorListNode::RETIRING_PAGE;
                }
            }
        }

        self.stats.retired_pages += uefi_size_to_pages!(pages.len());

        Ok(Some(pages))
    }

    // allocates from the linked-list backing allocator if a free block of the
    // appropriate size is not available.
    fn fallback_alloc(&mut self, layout: Layout) -> Result<NonNull<[u8]>, FixedSizeBlockAllocatorError> {
        for node in AllocatorIterator::new(self.allocators) {
            // SAFETY: node is a valid allocator list node pointer from the iterator.
            let allocator = unsafe { &mut (*node).allocator };
            if let Ok(ptr) = allocator.allocate_first_fit(layout) {
                return Ok(NonNull::slice_from_raw_parts(ptr, layout.size()));
            }
        }

        // Determine how much additional memory is required
        //
        // Per the `linked_list_allocator::hole::HoleList::new` documentation, depending on the alignment of the
        // hole_addr pointer, the minimum size for storing required metadata is between 2 * size_of::<usize> and
        //  3 * size_of::<usize>. The size reservation for `additional_mem_required` assumed the largest size.
        let allocation_size = layout.pad_to_align().size() + 3 * size_of::<usize>();

        let additional_mem_required = RegionHeader::region_size_for_payload(allocation_size).ok_or_else(|| {
            debug_assert!(false, "FSB expanded with insufficiently sized memory region.");
            FixedSizeBlockAllocatorError::InvalidExpansion
        })?;

        Err(FixedSizeBlockAllocatorError::OutOfMemory(additional_mem_required))
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
    /// Returns [`FixedSizeBlockAllocatorError::InvalidLayout`] when the layout provided is invalid.
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
                    // no block exists in list => allocate new block
                    let block_size = *BLOCK_SIZES.get(index).ok_or(FixedSizeBlockAllocatorError::InternalError)?;
                    // only works if all block sizes are a power of 2
                    let block_align = block_size;
                    let layout = match Layout::from_size_align(block_size, block_align) {
                        Ok(layout) => layout,
                        Err(_) => return Err(FixedSizeBlockAllocatorError::InvalidLayout),
                    };
                    self.fallback_alloc(layout)
                }
            }
            None => self.fallback_alloc(layout),
        }
    }

    // deallocates back to the linked-list backing allocator if the size of
    // layout being freed is too big to be tracked as a fixed-size free block.
    fn fallback_dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        for node in AllocatorIterator::new(self.allocators) {
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let allocator = unsafe { &mut (*node).allocator };
            if (allocator.bottom() <= ptr.as_ptr()) && (ptr.as_ptr() < allocator.top()) {
                // SAFETY: ptr was allocated by this allocator for the given layout.
                unsafe { allocator.deallocate(ptr, layout) };
                return;
            }
        }
    }

    /// Deallocates a buffer allocated by [`Self::alloc`].
    ///
    /// Returns the base address of a page that was fully returned to the free lists and has been retired. The caller
    /// must unmap that page, or return it to circulation with [`Self::restore_page`] if it cannot be unmapped.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that `ptr` was created by a call to [`Self::alloc`] with the same `layout`.
    pub unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) -> Option<usize> {
        self.stats.pool_free_calls += 1;

        let Some(index) = list_index(&layout) else {
            self.fallback_dealloc(ptr, layout);
            return None;
        };

        let block_size = *BLOCK_SIZES.get(index).expect("list_index guarantees valid index");

        // Make sure we have a large enough block to write the BlockListNode into it. If we don't,
        // we have corruption of our data structures and we should just free this region.
        if size_of::<BlockListNode>() > block_size || align_of::<BlockListNode>() > block_size {
            debug_assert!(
                (size_of::<BlockListNode>() <= block_size && align_of::<BlockListNode>() <= block_size),
                "FSB deallocating block too small to store BlockListNode."
            );
            return None;
        }

        let head = self.list_heads.get_mut(index).expect("list_index guarantees valid index");
        let new_node = ptr.cast::<BlockListNode>();
        // SAFETY: new_node points to memory returned by alloc for this layout, which is large enough and
        // aligned for a BlockListNode as asserted above.
        unsafe { new_node.as_ptr().write(BlockListNode { next: *head }) };

        if head.is_none() {
            let _ = self.list_tails.get_mut(index).expect("list_index guarantees valid index").replace(new_node);
        }
        *head = Some(new_node);

        self.track_block_freed(ptr.addr().get(), block_size)
    }

    /// Returns the region node that tracks the page containing `address`, if any.
    fn node_for_address(&mut self, address: usize) -> Option<NonNull<AllocatorListNode>> {
        if let Some(node) = self.page_accounting_node {
            // SAFETY: allocator nodes remain valid for the lifetime of the allocator.
            if unsafe { (*node.as_ptr()).page_index(address).is_some() } {
                return Some(node);
            }
        }

        let node = AllocatorIterator::new(self.allocators)
            .find(|&node| {
                // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
                unsafe { (*node).page_index(address).is_some() }
            })
            .and_then(NonNull::new);
        self.page_accounting_node = node;
        node
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

    /// Returns the address range of up to `count` contiguous retired pages, if the allocator has any.
    ///
    /// The pages remain retired until they are handed to [`Self::restore_pages`].
    fn find_retired_pages(&self, count: usize) -> Option<Range<usize>> {
        AllocatorIterator::new(self.allocators).find_map(|node| {
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let node = unsafe { &*node };
            let first =
                node.page_free_bytes.iter().position(|free_bytes| *free_bytes == AllocatorListNode::RETIRED_PAGE)?;
            let run = node
                .page_free_bytes
                .get(first..)?
                .iter()
                .take(count)
                .take_while(|free_bytes| **free_bytes == AllocatorListNode::RETIRED_PAGE)
                .count();
            Some(node.region_base() + uefi_pages_to_size!(first)..node.region_base() + uefi_pages_to_size!(first + run))
        })
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
        let (Some(ptr), Ok(layout)) =
            (NonNull::new(page_base as *mut u8), Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE))
        else {
            debug_assert!(false, "FSB asked to restore an invalid page.");
            return;
        };

        // SAFETY: every byte of this page was handed out by this heap as fixed-size blocks and all of those blocks
        // have been freed and unlinked, so returning the page as a single allocation describes exactly the memory the
        // heap considers in use.
        unsafe {
            if let Some((_, free_bytes)) = (*node.as_ptr()).page_free_bytes_mut(page_base) {
                *free_bytes = 0;
            }
            (*node.as_ptr()).allocator.deallocate(ptr, layout);
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

    /// Indicates whether the given pointer falls within a memory region managed by this allocator.
    ///
    /// Note: `true` does not indicate that the pointer corresponds to an active allocation - it may be in either
    /// allocated or freed memory. `true` just means that the pointer falls within a memory region that this allocator
    /// manages.
    pub fn contains(&self, ptr: *mut u8) -> bool {
        AllocatorIterator::new(self.allocators).any(|node| {
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let allocator = unsafe { &mut (*node).allocator };
            (allocator.bottom() <= ptr) && (ptr < allocator.top())
        })
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
            self.stats.reserved_used = self.stats.reserved_used.saturating_sub(pages * ALIGNMENT);
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
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let allocator = unsafe { &(*node).allocator };
            allocator.bottom() as usize..allocator.top() as usize
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

impl Display for FixedSizeBlockAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writelncrlf!(f, "Memory Type: {:x?}", self.memory_type())?;
        writelncrlf!(f, "Allocation Ranges:")?;
        for node in AllocatorIterator::new(self.allocators) {
            // SAFETY: node is produced by AllocatorIterator and points to a valid AllocatorListNode.
            let (region_base, allocator) = unsafe { ((*node).region_base(), &mut (*node).allocator) };
            writelncrlf!(
                f,
                "  PhysRange: {:#x}-{:#x}, Size: {:#x}, Used: {:#x} Free: {:#x}",
                region_base,
                allocator.top() as usize,
                allocator.top() as usize - region_base,
                allocator.used(),
                allocator.free(),
            )?;
        }
        writelncrlf!(f, "Bucket Range: {:x?}", self.reserved_range)?;
        writelncrlf!(f, "Allocation Stats:")?;
        writelncrlf!(f, "  pool_allocation_calls: {}", self.stats.pool_allocation_calls)?;
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

    /// Set while this allocator is updating page attributes through the GCD. The GCD may allocate and free pool
    /// memory, so this prevents an attribute update from recursing into another attribute update.
    updating_page_attributes: AtomicBool,
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
            updating_page_attributes: AtomicBool::new(false),
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

    /// Updates the attributes of a range of pool pages through the GCD, preserving the attributes that are not being
    /// changed.
    ///
    /// Note: the FSB lock must not be held when calling this, as the GCD allocates and frees pool memory.
    fn update_page_attributes(&self, pages: &Range<usize>, apply_free_policy: bool) -> Result<(), EfiError> {
        match self.apply_page_attributes(pages, apply_free_policy) {
            Ok(()) => Ok(()),
            Err((err, failed_address)) => {
                if let Err((rollback_err, _)) =
                    self.apply_page_attributes(&(pages.start..failed_address), !apply_free_policy)
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

    fn apply_page_attributes(&self, pages: &Range<usize>, apply_free_policy: bool) -> Result<(), (EfiError, usize)> {
        let mut base = pages.start;
        while base < pages.end {
            let descriptor = self
                .gcd
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
                self.gcd
                    .memory_protection_policy
                    .apply_allocated_memory_protection_policy(descriptor.attributes, GcdMemoryType::SystemMemory)
            };
            match self.gcd.set_memory_space_attributes(base, end - base, attributes) {
                Ok(()) | Err(EfiError::NotReady) => base = end,
                Err(err) => return Err((err, base)),
            }
        }
        Ok(())
    }

    /// Unmaps pages that are not currently used by pool allocations.
    /// If they cannot be unmapped they are placed back into circulation instead.
    fn retire_pages(&self, pages: Range<usize>) {
        if self.updating_page_attributes.swap(true, Ordering::Acquire) {
            // Re-entered from a GCD attribute update; leave the pages in circulation rather than recursing.
            self.lock().restore_pages(pages);
            return;
        }

        let result = self.update_page_attributes(&pages, true);
        if result.is_ok() {
            self.lock().mark_pages_retired(pages.clone());
        }
        self.updating_page_attributes.store(false, Ordering::Release);

        if let Err(err) = result {
            log::error!("Failed to unmap pool pages {:#x}-{:#x}: {err:?}", pages.start, pages.end);
            self.lock().restore_pages(pages);
        }
    }

    /// Maps `count` contiguous retired pages and returns them to the backing allocator.
    ///
    /// Returns `true` if all pages were returned to service, false if no suitable contiguous range was found.
    fn reclaim_pages(&self, count: usize) -> bool {
        if self.updating_page_attributes.swap(true, Ordering::Acquire) {
            return false;
        }

        // Note: the lock is released before updating attributes, as the GCD allocates and frees pool memory.
        let retired_pages = self.lock().find_retired_pages(count);

        let mut reclaimed = false;
        if let Some(pages) = retired_pages {
            match self.update_page_attributes(&pages, false) {
                Ok(()) => {
                    self.lock().restore_pages(pages);
                    reclaimed = true;
                }
                Err(err) => log::error!("Failed to re-map retired pool pages {:#x}: {err:?}", pages.start),
            }
        }

        self.updating_page_attributes.store(false, Ordering::Release);
        reclaimed
    }

    /// Indicates whether the given pointer falls within a memory region managed by this allocator.
    ///
    /// See [`FixedSizeBlockAllocator::contains()`]
    pub fn contains(&self, ptr: NonNull<u8>) -> bool {
        self.lock().contains(ptr.as_ptr())
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
        self.lock().stats.page_allocation_calls += 1;
        let granularity = self.lock().page_allocation_granularity;

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
        let start_address = self
            .allocate_from_gcd(allocation_strategy, align_shift, uefi_pages_to_size!(required_pages))
            .map_err(|err| match err {
                EfiError::InvalidParameter | EfiError::NotFound => err,
                _ => EfiError::OutOfResources,
            })?;

        let allocation = slice_from_raw_parts_mut(start_address as *mut u8, uefi_pages_to_size!(required_pages));
        let allocation = NonNull::new(allocation).ok_or(EfiError::OutOfResources)?;

        // Notify the FSB that additional pages were allocated for record keeping
        self.lock().notify_page_allocation(allocation);

        Ok(allocation)
    }

    /// Frees the block of pages at the given address of the given size.
    ///
    /// ## Safety
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [`Self::allocate_pages`]
    pub unsafe fn free_pages(&self, address: usize, pages: usize) -> Result<(), EfiError> {
        self.lock().stats.page_free_calls += 1;

        let granularity = self.lock().page_allocation_granularity;

        // Ensure that the requested number of pages is a multiple of the granularity
        let required_pages = align_up(pages, uefi_size_to_pages!(granularity))?;

        if !address.is_multiple_of(granularity) {
            return Err(EfiError::InvalidParameter);
        }

        let descriptor = self
            .gcd
            .get_memory_descriptor_for_address(address as efi::PhysicalAddress, |d, _| {
                d.memory_type != GcdMemoryType::NonExistent
            })
            .map_err(|err| match err {
                EfiError::NotFound => err,
                _ => EfiError::InvalidParameter,
            })?;

        if descriptor.image_handle != self.handle {
            Err(EfiError::NotFound)?;
        }

        if self.lock().in_reserved_range(address as efi::PhysicalAddress) {
            self.gcd.free_memory_space_preserving_ownership(address, uefi_pages_to_size!(required_pages)).map_err(
                |err| match err {
                    EfiError::NotFound => err,
                    _ => EfiError::InvalidParameter,
                },
            )?;
        } else {
            self.gcd.free_memory_space(address, uefi_pages_to_size!(required_pages)).map_err(|err| match err {
                EfiError::NotFound => err,
                _ => EfiError::InvalidParameter,
            })?;
        }

        // Notify the FSB that pages were freed for record keeping
        self.lock().notify_pages_freed(address as efi::PhysicalAddress, required_pages);

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
        &self,
        strategy: AllocationStrategy,
        align_shift: usize,
        size: usize,
    ) -> Result<usize, EfiError> {
        let reserved_range = self.lock().reserved_range.clone();

        // Determine whether to attempt bin-preference allocation.
        let try_bin = match strategy {
            AllocationStrategy::TopDown(None) | AllocationStrategy::BottomUp(None) => true,
            AllocationStrategy::TopDown(Some(max)) | AllocationStrategy::BottomUp(Some(max)) => {
                if let Some(ref reserved) = reserved_range { max as u64 >= reserved.start + size as u64 } else { false }
            }
            _ => false,
        };

        if try_bin
            && let Some(ref reserved) = reserved_range
            && let Ok(addr) = self.gcd.allocate_memory_space(
                AllocationStrategy::TopDown(Some(reserved.end as usize)),
                GcdMemoryType::SystemMemory,
                align_shift,
                size,
                self.handle,
                None,
            )
        {
            if addr >= reserved.start as usize {
                return Ok(addr);
            }
            // Landed below the bin, free and fall through.
            let _ = self.gcd.free_memory_space(addr, size);
        }

        // Normal allocation path.
        self.gcd.allocate_memory_space(strategy, GcdMemoryType::SystemMemory, align_shift, size, self.handle, None)
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
        let allocation = self.lock().alloc(layout);
        match allocation {
            Ok(alloc) => Ok(alloc),
            Err(FixedSizeBlockAllocatorError::OutOfMemory(additional_mem_required)) => {
                // Compile-time check to ensure ALIGNMENT is compatible with the alignment requirements
                // of `expand()` and `page_shift_from_alignment()`
                const _: () = assert!(ALIGNMENT.is_multiple_of(align_of::<AllocatorListNode>()));
                const _: () = assert!(ALIGNMENT.is_multiple_of(UEFI_PAGE_SIZE) && ALIGNMENT > 0);

                // Map enough pages to satisfy the request.
                let reclaim_pages = uefi_size_to_pages!(additional_mem_required);

                // Bring a sufficient contiguous retired range back into service before claiming more memory from the
                // GCD. Shorter fragmented ranges remain retired.
                if self.reclaim_pages(reclaim_pages)
                    && let Ok(alloc) = self.lock().internal_alloc(layout)
                {
                    return Ok(alloc);
                }

                // As a matter of policy, allocate at least the minimum expansion amount of memory and ensure the
                // size is aligned to `ALIGNMENT`.
                let mut allocation_size = max(additional_mem_required, self.min_expansion);
                let required_alignment = self.lock().page_allocation_granularity;

                // Ensure that the requested number of pages is a multiple of the granularity
                let required_pages =
                    align_up(uefi_size_to_pages!(allocation_size), uefi_size_to_pages!(required_alignment)).map_err(
                        |_| {
                            debug_assert!(false);
                            AllocError
                        },
                    )?;

                if RegionHeader::new(required_pages).is_none() {
                    log::error!("Allocator expansion metadata is invalid for {required_pages:#x} pages.");
                    return Err(AllocError);
                }

                allocation_size = uefi_pages_to_size!(required_pages);

                // Allocate additional memory through the GCD, returning AllocError
                // if the GCD returns an error
                let start_address: usize = self
                    .allocate_from_gcd(
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
                let Ok(new_pages) = self.lock().expand(NonNull::slice_from_raw_parts(allocated_ptr, allocation_size))
                else {
                    debug_assert!(false);
                    return Err(AllocError);
                };

                // We may only have free space in a partial page after the metadata, in which case we can allocate but
                // do not have any whole pages to activate.
                if let Some(new_pages) = new_pages {
                    let active_end = min(new_pages.end, new_pages.start + uefi_pages_to_size!(reclaim_pages));
                    self.lock().activate_new_pages(new_pages.start..active_end);

                    // Done outside of the lock: the GCD allocates and frees pool memory while updating attributes.
                    if active_end < new_pages.end {
                        self.retire_pages(active_end..new_pages.end);
                    }
                }

                // The activated portion of the new region was sized to satisfy this allocation.
                self.lock().internal_alloc(layout).map_err(|_| {
                    debug_assert!(false);
                    AllocError
                })
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
        // SAFETY: ptr came from allocate with the same layout.
        let retired_page = unsafe { self.lock().dealloc(ptr, layout) };

        // Done outside of the lock: the GCD allocates and frees pool memory while updating attributes.
        if let Some(page_base) = retired_page {
            self.retire_pages(page_base..page_base + UEFI_PAGE_SIZE);
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

    fn contains(&self, ptr: NonNull<u8>) -> bool {
        Self::contains(self, ptr)
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

                let allocation_size = DEFAULT_PAGE_ALLOCATION_GRANULARITY;
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

                let header_size = RegionHeader::new(uefi_size_to_pages!(allocation_size)).unwrap().size;
                let expected_start = allocated_address + header_size;
                let expected_end = expected_start + allocation_size - header_size;
                assert_eq!(ranges[0], expected_start..expected_end);
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
        assert_eq!(list_index(&layout), Some(0));

        let layout = Layout::from_size_align(8, 32).unwrap();
        assert_eq!(list_index(&layout), Some(1));

        let layout = Layout::from_size_align(4096, 32).unwrap();
        assert_eq!(list_index(&layout), Some(8));

        let layout = Layout::from_size_align(1, 4096).unwrap();
        assert_eq!(list_index(&layout), Some(8));

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
    fn test_region_header_supports_metadata_larger_than_one_page() {
        let page_count = UEFI_PAGE_SIZE;
        let header = RegionHeader::new(page_count).unwrap();

        assert!(header.size > UEFI_PAGE_SIZE);
        assert_eq!(header.page_count, page_count);
    }

    #[test]
    fn test_region_size_accounts_for_all_page_metadata() {
        const PAYLOAD_SIZE: usize = 0x1000000;

        let region_size = RegionHeader::region_size_for_payload(PAYLOAD_SIZE).unwrap();
        let page_count = region_size / UEFI_PAGE_SIZE;
        let header = RegionHeader::new(page_count).unwrap();

        assert!(header.size > UEFI_PAGE_SIZE);
        assert!(region_size - header.size >= PAYLOAD_SIZE);

        let smaller_region_size = region_size - UEFI_PAGE_SIZE;
        let smaller_header = RegionHeader::new(page_count - 1).unwrap();
        assert!(smaller_region_size - smaller_header.size < PAYLOAD_SIZE);
    }

    #[test]
    fn test_expand() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                let base = init_gcd(&GCD, 0x4000000);

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
                let retired = fsb
                    .expand(NonNull::slice_from_raw_parts(
                        NonNull::new(allocated_address as *mut u8).unwrap(),
                        allocation_size,
                    ))
                    .unwrap();
                assert_eq!(retired, None);

                assert!(fsb.allocators.is_some());
                // SAFETY: fsb.allocators points to valid list nodes after expand.
                unsafe {
                    assert!((*fsb.allocators.unwrap()).next.is_none());
                    assert!((*fsb.allocators.unwrap()).allocator.bottom() as usize > base as usize);
                    assert_eq!(
                        (*fsb.allocators.unwrap()).allocator.free(),
                        allocation_size - RegionHeader::new(uefi_size_to_pages!(allocation_size)).unwrap().size
                    );
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
                    assert!((*fsb.allocators.unwrap()).next.is_some());
                    assert!((*(*fsb.allocators.unwrap()).next.unwrap()).next.is_none());
                    assert!((*fsb.allocators.unwrap()).allocator.bottom() as usize > base as usize);
                    assert_eq!(
                        (*fsb.allocators.unwrap()).allocator.free(),
                        //expected free: size of the region less the region header and the retired page.
                        allocation_size
                            - RegionHeader::new(uefi_size_to_pages!(allocation_size)).unwrap().size
                            - UEFI_PAGE_SIZE
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

            const ALLOCATION_SIZE: usize = DEFAULT_PAGE_ALLOCATION_GRANULARITY;
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
                unsafe {
                    let expected_free =
                        ALLOCATION_SIZE - RegionHeader::new(uefi_size_to_pages!(ALLOCATION_SIZE)).unwrap().size;
                    (*node).allocator.free() == expected_free
                }
            }));
        });
    }

    #[test]
    fn test_fallback_alloc() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                let _ = init_gcd(&GCD, 0x400000);

                let mut fsb = FixedSizeBlockAllocator::new(efi::RUNTIME_SERVICES_DATA, granularity);

                // Test fallback_alloc with size < size_of::<AllocatorListNode>()
                let allocation_size = size_of::<AllocatorListNode>() / 2;
                let layout = Layout::from_size_align(allocation_size, 0x10).unwrap();
                match fsb.fallback_alloc(layout) {
                    Err(FixedSizeBlockAllocatorError::OutOfMemory(mem_req)) => {
                        assert!(
                            mem_req
                                >= layout.pad_to_align().size()
                                    + Layout::new::<AllocatorListNode>().pad_to_align().size(),
                            "fallback_alloc should request enough memory to fit aligned layout and an aligned AllocatorListNode"
                        );
                    }
                    _ => {
                        panic!(
                            "fallback_alloc with no allocators should return FixedSizeBlockAllocatorError::OutOfMemory"
                        )
                    }
                }
                assert!(fsb.allocators.is_none());

                // Test fallback_alloc with size > size_of::<AllocatorListNode>(), but unaligned to AllocatorListNode
                let allocation_size = size_of::<AllocatorListNode>() + align_of::<AllocatorListNode>() / 2;
                let layout = Layout::from_size_align(allocation_size, 0x10).unwrap();
                match fsb.fallback_alloc(layout) {
                    Err(FixedSizeBlockAllocatorError::OutOfMemory(mem_req)) => {
                        assert!(
                            mem_req
                                >= layout.pad_to_align().size()
                                    + Layout::new::<AllocatorListNode>().pad_to_align().size(),
                            "fallback_alloc should request enough memory to fit aligned layout and an aligned AllocatorListNode"
                        );
                    }
                    _ => {
                        panic!(
                            "fallback_alloc with no allocators should return FixedSizeBlockAllocatorError::OutOfMemory"
                        )
                    }
                }
                assert!(fsb.allocators.is_none());
            });
        });
    }

    #[test]
    fn test_fallback_alloc_accounts_for_metadata_larger_than_one_page() {
        let mut fsb = FixedSizeBlockAllocator::new(efi::BOOT_SERVICES_DATA, DEFAULT_PAGE_ALLOCATION_GRANULARITY);
        let layout = Layout::from_size_align(0x1000000, align_of::<usize>()).unwrap();

        let Err(FixedSizeBlockAllocatorError::OutOfMemory(region_size)) = fsb.fallback_alloc(layout) else {
            panic!("fallback_alloc should request a region large enough for the allocation and its metadata");
        };
        let header = RegionHeader::new(region_size / UEFI_PAGE_SIZE).unwrap();

        assert!(header.size > UEFI_PAGE_SIZE);
        assert!(region_size - header.size >= layout.size());
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
    fn test_allocate_with_metadata_larger_than_one_page() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
                const ALLOCATION_SIZE: usize = 0x1000000;

                init_gcd(&GCD, ALLOCATION_SIZE * 2);
                let fsb = SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    1 as _,
                    efi::RUNTIME_SERVICES_DATA,
                    granularity,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );
                let layout = Layout::from_size_align(ALLOCATION_SIZE, align_of::<usize>()).unwrap();

                let allocation = fsb.allocate(layout).unwrap();
                let claimed_pages = fsb.stats().claimed_pages;

                assert!(RegionHeader::new(claimed_pages).unwrap().size > UEFI_PAGE_SIZE);
                assert_eq!(allocation.len(), ALLOCATION_SIZE);

                // SAFETY: allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation.cast(), layout) };
            });
        });
    }

    #[test]
    fn test_fallback_dealloc() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                // Create a static GCD
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                // Allocate some space on the heap with the global allocator (std) to be used by expand().
                init_gcd(&GCD, 0x400000);

                let mut fsb = FixedSizeBlockAllocator::new(efi::RUNTIME_SERVICES_DATA, granularity);

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();

                // Expand the FSB by the minimum expansion to fit the allocation
                let expansion_size = DEFAULT_PAGE_ALLOCATION_GRANULARITY;
                let expansion_address = GCD
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        UEFI_PAGE_SHIFT,
                        expansion_size,
                        DUMMY_HANDLE,
                        None,
                    )
                    .unwrap();
                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(expansion_address as *mut u8).unwrap(),
                    expansion_size,
                ))
                .unwrap();

                let allocation = fsb.fallback_alloc(layout).unwrap();

                // Finally, we can test fallback_dealloc
                fsb.fallback_dealloc(allocation.cast(), layout);
                let expected_free =
                    expansion_size - RegionHeader::new(uefi_size_to_pages!(expansion_size)).unwrap().size;
                // SAFETY: fsb.allocators points to a valid allocator after expand.
                unsafe {
                    assert_eq!((*fsb.allocators.unwrap()).allocator.free(), expected_free);
                }
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

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.dealloc(allocation, layout) };
                let free_block_ptr =
                    fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap().as_ptr().cast::<u8>();
                assert_eq!(free_block_ptr, allocation);

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                // SAFETY: fsb is initialized and used with a valid layout in tests.
                let allocation = unsafe { fsb.alloc(layout) };

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.dealloc(allocation, layout) };
                let free_block_ptr =
                    fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap().as_ptr().cast::<u8>();
                assert_eq!(free_block_ptr, allocation);
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
                let allocation_ptr = allocation.as_ptr();

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
                let free_block_ptr =
                    fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap().as_ptr().cast::<u8>();
                assert_eq!(free_block_ptr, allocation_ptr);

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                let allocation_ptr = allocation.as_ptr();

                // SAFETY: Allocation was returned by fsb for this layout.
                unsafe { fsb.deallocate(allocation, layout) };
                let free_block_ptr =
                    fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap().as_ptr().cast::<u8>();
                assert_eq!(free_block_ptr, allocation_ptr);
            });
        });
    }

    #[test]
    fn test_contains() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                1 as _,
                efi::BOOT_SERVICES_DATA,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
            assert!(fsb.contains(allocation));
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

                let target_address = (address + 0x400000 - max(8_u64 * ALIGNMENT as u64, granularity as u64))
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

                let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
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

                let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
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
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, 0);

            //test alloc/deallocate and stats
            // SAFETY: fsb is initialized and used with a valid layout for testing.
            let ptr = unsafe {
                fsb.alloc(
                    Layout::from_size_align(TEST_MIN_EXPANSION_SIZE - size_of::<AllocatorListNode>(), 0x8).unwrap(),
                )
            };

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 1);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            let initial_claimed = stats.claimed_pages;

            // SAFETY: Allocation was returned by fsb for this layout.
            unsafe {
                fsb.dealloc(ptr, Layout::from_size_align(0x100, 0x8).unwrap());
            }

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 1);
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
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            let claimed_after_3mb = stats.claimed_pages;

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
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, claimed_after_3mb);

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
            assert_eq!(stats.claimed_pages, claimed_after_3mb + 0x4);

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
            assert_eq!(stats.claimed_pages, claimed_after_3mb);

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
            assert_eq!(stats.claimed_pages, claimed_after_3mb + 0x104);

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
            assert_eq!(stats.claimed_pages, claimed_after_3mb + 0x104 + 0x4);

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
            assert_eq!(stats.claimed_pages, claimed_after_3mb);
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
                                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                                DUMMY_HANDLE,
                                None,
                            )
                            .unwrap() as *mut u8,
                        )
                        .unwrap(),
                        DEFAULT_PAGE_ALLOCATION_GRANULARITY,
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

            // The retired page is out of circulation, so it is not handed back out.
            let next = fsb.allocate(layout).unwrap().cast::<u8>();
            assert_ne!(next.addr().get(), page_base);
        });
    }

    #[test]
    fn test_partially_freed_page_is_not_retired() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            let small = Layout::from_size_align(0x20, 0x20).unwrap();
            let allocation = fsb.allocate(small).unwrap().cast::<u8>();
            let page_base = align_down_size(allocation.addr().get(), UEFI_PAGE_SIZE);
            let retired = fsb.stats().retired_pages;

            // SAFETY: allocation was returned by fsb for this layout.
            unsafe { fsb.deallocate(allocation, small) };

            assert_eq!(fsb.stats().retired_pages, retired);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);
        });
    }

    #[test]
    fn test_update_page_attributes_applies_free_memory_policy() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);
            let allocation = fsb.allocate(Layout::from_size_align(0x20, 0x20).unwrap()).unwrap().cast::<u8>();
            let first_page = align_down_size(allocation.addr().get(), UEFI_PAGE_SIZE);
            let second_page = first_page + UEFI_PAGE_SIZE;

            assert_eq!(
                GCD.set_memory_space_attributes(first_page, UEFI_PAGE_SIZE, efi::MEMORY_WB | efi::MEMORY_XP),
                Err(EfiError::NotReady)
            );
            assert_eq!(
                GCD.set_memory_space_attributes(second_page, UEFI_PAGE_SIZE, efi::MEMORY_WB | efi::MEMORY_RO),
                Err(EfiError::NotReady)
            );

            fsb.update_page_attributes(&(first_page..second_page + UEFI_PAGE_SIZE), true).unwrap();

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

            let result = fsb.update_page_attributes(&(allocation..allocation + uefi_pages_to_size!(pages + 1)), true);

            assert!(result.is_err());
            assert_eq!(page_attributes(&GCD, allocation), allocated_attributes);
        });
    }

    #[test]
    fn test_page_holding_a_large_allocation_is_not_retired() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            // Allocations larger than the largest block size are not tracked for retirement.
            let large = Layout::from_size_align(UEFI_PAGE_SIZE * 2, UEFI_PAGE_SIZE).unwrap();
            let allocation = fsb.allocate(large).unwrap().cast::<u8>();
            let page_base = allocation.addr().get();
            let retired = fsb.stats().retired_pages;

            // SAFETY: allocation was returned by fsb for this layout.
            unsafe { fsb.deallocate(allocation, large) };

            assert_eq!(fsb.stats().retired_pages, retired);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);
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

            // The freed page is the lowest retired page, so it is the next one brought back into service.
            let reclaimed = fsb.stats().reclaimed_pages;
            assert!(fsb.reclaim_pages(1));
            assert_eq!(fsb.stats().reclaimed_pages, reclaimed + 1);
            assert_eq!(page_attributes(&GCD, page_base) & efi::MEMORY_RP, 0);

            // The reclaimed memory is available again.
            assert_eq!(fsb.allocate(layout).unwrap().cast::<u8>().addr().get(), page_base);
        });
    }

    #[test]
    fn test_reclaim_pages_restores_all_requested_pages() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let fsb = fsb_with_gcd(&GCD);

            let layout = Layout::from_size_align(0x20, 0x20).unwrap();
            let _allocation = fsb.allocate(layout).unwrap();
            let retired_pages = fsb.stats().retired_pages;
            assert!(retired_pages > 1);

            let reclaimed = fsb.stats().reclaimed_pages;
            assert!(fsb.reclaim_pages(retired_pages));
            assert_eq!(fsb.stats().reclaimed_pages, reclaimed + retired_pages);
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
            assert_eq!(fsb.find_retired_pages(usize::MAX), Some(pages.clone()));

            // Restoring them puts them back into the backing allocator.
            fsb.restore_pages(pages);
            assert_eq!(fsb.find_retired_pages(usize::MAX), None);
            assert!(fsb.alloc(Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).unwrap()).is_ok());
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
