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
use super::{AllocationStrategy, DEFAULT_ALLOCATION_STRATEGY};

use crate::{gcd::SpinLockedGcd, tpl_mutex};

use alloc::vec::Vec;
use core::{
    alloc::{AllocError, Allocator, GlobalAlloc, Layout},
    cmp::max,
    debug_assert,
    fmt::{self, Display},
    mem::{align_of, size_of},
    ops::Range,
    ptr::{NonNull, slice_from_raw_parts_mut},
    result::Result,
};
use linked_list_allocator::{align_down_size, align_up_size};
use patina::{
    base::{UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, align_up},
    error::EfiError,
    pi::{dxe_services::GcdMemoryType, hob::EFiMemoryTypeInformation},
    uefi_pages_to_size, uefi_size_to_pages,
};
use r_efi::efi;

/// Type for describing errors that this implementation can produce.
#[derive(Debug, PartialEq)]
pub enum FixedSizeBlockAllocatorError {
    /// Could not satisfy allocation request, and expansion failed.
    ///
    /// Specifies how much additional memory is required to be added to the allocator through
    /// [FixedSizeBlockAllocator::expand()] in order to fulfill the attempted allocation.
    OutOfMemory(usize),
    /// The provided layout was invalid.
    InvalidLayout,
    /// The memory region provided to extend the allocator was invalid.
    InvalidExpansion,
}

/// Minimum expansion size - allocator will request at least this much memory
/// from the underlying GCD instance expansion is needed.
pub const MIN_EXPANSION: usize = 0x100000;
const ALIGNMENT: usize = 0x1000;

const BLOCK_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

// Compile-time check to ensure the MIN_EXPANSION is a multiple of RUNTIME_PAGE_ALLOCATION_GRANULARITY.
const _: () = assert!(MIN_EXPANSION.is_multiple_of(super::RUNTIME_PAGE_ALLOCATION_GRANULARITY));

// Returns the index in the block list for the minimum size block that will
// satisfy allocation for the given layout
fn list_index(layout: &Layout) -> Option<usize> {
    let required_block_size = layout.size().max(layout.align());
    BLOCK_SIZES.iter().position(|&s| s >= required_block_size)
}

/// Converts the given alignment to a shift value.
const fn page_shift_from_alignment(alignment: usize) -> Result<usize, EfiError> {
    let shift = alignment.trailing_zeros() as usize;
    if !alignment.is_power_of_two() || shift < UEFI_PAGE_SHIFT {
        return Err(EfiError::InvalidParameter);
    }

    Ok(shift)
}

struct BlockListNode {
    next: Option<&'static mut BlockListNode>,
}

struct AllocatorListNode {
    next: Option<*mut AllocatorListNode>,
    allocator: linked_list_allocator::Heap,
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
            self.current = unsafe { (*current).next };
            Some(current)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AllocationStatistics {
    /// The number of calls to `alloc()`.
    ///
    /// Note: [SpinLockedFixedSizeBlockAllocator::alloc()] and [SpinLockedFixedSizeBlockAllocator::allocate()] will call alloc() twice when
    /// additional memory is required.
    pub pool_allocation_calls: usize,

    /// The number of calls to `dealloc()`.
    pub pool_free_calls: usize,

    /// The number of calls to allocate pages.
    pub page_allocation_calls: usize,

    /// The number of calls to free pages.
    pub page_free_calls: usize,

    /// The amount of memory set aside in the backing allocator for use by this allocator.
    pub reserved_size: usize,

    /// The amount of the memory used in the pool of memory set aside in the backing allocator for use by this allocator.
    pub reserved_used: usize,

    /// The number of pages claimed for use by this allocator.
    pub claimed_pages: usize,
}

impl AllocationStatistics {
    const fn new() -> Self {
        Self {
            pool_allocation_calls: 0,
            pool_free_calls: 0,
            page_allocation_calls: 0,
            page_free_calls: 0,
            reserved_size: 0,
            reserved_used: 0,
            claimed_pages: 0,
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
    /// The memory type this allocator is managing and number of pages allocated for this memory type. This is used
    /// to bucketize memory for the EFI_MEMORY_MAP and handle any special cases for memory types.
    memory_type_info: NonNull<EFiMemoryTypeInformation>,

    /// The heads of the linked lists for each fixed-size block. Each index corresponds to a block size in
    /// `BLOCK_SIZES`.
    list_heads: [Option<&'static mut BlockListNode>; BLOCK_SIZES.len()],

    /// The linked-list of allocators that this allocator uses to back allocations that are larger than the fixed-size
    /// blocks or if the required fixed-size block list is empty.
    allocators: Option<*mut AllocatorListNode>,

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
    /// Creates a new empty FixedSizeBlockAllocator
    pub const fn new(memory_type_info: NonNull<EFiMemoryTypeInformation>, page_allocation_granularity: usize) -> Self {
        const EMPTY: Option<&'static mut BlockListNode> = None;
        FixedSizeBlockAllocator {
            memory_type_info,
            list_heads: [EMPTY; BLOCK_SIZES.len()],
            allocators: None,
            reserved_range: None,
            stats: AllocationStatistics::new(),
            page_allocation_granularity,
        }
    }

    // This routine resets some aspects of allocator state for testing purposes.
    // Note: this does not change the page_change_callback.
    #[cfg(test)]
    pub fn reset(&mut self) {
        const EMPTY: Option<&'static mut BlockListNode> = None;
        self.list_heads = [EMPTY; BLOCK_SIZES.len()];
        self.allocators = None;
        self.reserved_range = None;
        self.memory_type_info_mut().number_of_pages = 0;
        self.stats = AllocationStatistics::new();
    }

    /// Expand the memory available to this allocator with a new contiguous region of memory, setting up a new allocator
    /// node to manage this range. `new_region.len() - size_of::<AllocatorListNode>()` additional memory will be available
    /// to the allocator.
    ///
    /// ## Errors
    ///
    /// Returns [`FixedSizeBlockAllocatorError::InvalidExpansion`] if the new region is not larger than and aligned to
    /// AllocatorListNode.
    pub fn expand(&mut self, new_region: NonNull<[u8]>) -> core::result::Result<(), FixedSizeBlockAllocatorError> {
        // Ensure we're expanding enough to fit a new allocator list node
        if new_region.len() <= size_of::<AllocatorListNode>() {
            debug_assert!(false, "FSB expanded with insufficiently sized memory region.");
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        // Interpret the first part of the provided region as an AllocatorListNode
        let alloc_node_ptr = new_region.as_ptr() as *mut AllocatorListNode;

        if !alloc_node_ptr.is_aligned() {
            debug_assert!(false, "FSB expanded with memory region unaligned to AllocatorListNode.");
            return Err(FixedSizeBlockAllocatorError::InvalidExpansion);
        }

        let heap_region: NonNull<[u8]> = NonNull::slice_from_raw_parts(
            NonNull::new(unsafe { alloc_node_ptr.add(1) }).unwrap().cast(),
            new_region.len() - size_of::<AllocatorListNode>(),
        );

        //write the allocator node structure into the start of the range, initialize its heap with the remainder of
        //the range, and add the new allocator to the front of the allocator list.
        let node = AllocatorListNode { next: None, allocator: linked_list_allocator::Heap::empty() };
        unsafe {
            alloc_node_ptr.write(node);
            (*alloc_node_ptr).allocator.init(heap_region.cast::<u8>().as_ptr(), heap_region.len());
            (*alloc_node_ptr).next = self.allocators;
        }

        self.allocators = Some(alloc_node_ptr);

        if self.in_reserved_range(alloc_node_ptr.addr() as efi::PhysicalAddress) {
            self.stats.reserved_used += new_region.len();
        } else {
            self.stats.claimed_pages += uefi_size_to_pages!(new_region.len());
        }

        // if we managed to allocate pages, call into the page change callback to update stats
        self.update_memory_type_info();

        Ok(())
    }

    // allocates from the linked-list backing allocator if a free block of the
    // appropriate size is not available.
    fn fallback_alloc(&mut self, layout: Layout) -> Result<NonNull<[u8]>, FixedSizeBlockAllocatorError> {
        for node in AllocatorIterator::new(self.allocators) {
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
        let additional_mem_required = layout.pad_to_align().size()
            + Layout::new::<AllocatorListNode>().pad_to_align().size()
            + 3 * size_of::<usize>();
        let additional_mem_required = align_up_size(additional_mem_required, align_of::<AllocatorListNode>());

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
    pub fn alloc(&mut self, layout: Layout) -> Result<NonNull<[u8]>, FixedSizeBlockAllocatorError> {
        self.stats.pool_allocation_calls += 1;

        match list_index(&layout) {
            Some(index) => {
                match self.list_heads[index].take() {
                    Some(node) => {
                        self.list_heads[index] = node.next.take();
                        let ptr: NonNull<u8> = NonNull::from(node).cast();
                        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
                    }
                    None => {
                        // no block exists in list => allocate new block
                        let block_size = BLOCK_SIZES[index];
                        // only works if all block sizes are a power of 2
                        let block_align = block_size;
                        let layout = match Layout::from_size_align(block_size, block_align) {
                            Ok(layout) => layout,
                            Err(_) => return Err(FixedSizeBlockAllocatorError::InvalidLayout),
                        };
                        self.fallback_alloc(layout)
                    }
                }
            }
            None => self.fallback_alloc(layout),
        }
    }

    // deallocates back to the linked-list backing allocator if the size of
    // layout being freed is too big to be tracked as a fixed-size free block.
    fn fallback_dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        for node in AllocatorIterator::new(self.allocators) {
            let allocator = unsafe { &mut (*node).allocator };
            if (allocator.bottom() <= ptr.as_ptr()) && (ptr.as_ptr() < allocator.top()) {
                unsafe { allocator.deallocate(ptr, layout) };
            }
        }
    }

    /// Deallocates a buffer allocated by [`Self::alloc`].
    ///
    /// ## Safety
    ///
    /// Caller must ensure that `ptr` was created by a call to [`Self::alloc`] with the same `layout`.
    pub unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        self.stats.pool_free_calls += 1;
        match list_index(&layout) {
            Some(index) => {
                let new_node = BlockListNode { next: self.list_heads[index].take() };
                // verify that block has size and alignment required for storing node
                assert!(size_of::<BlockListNode>() <= BLOCK_SIZES[index]);
                assert!(align_of::<BlockListNode>() <= BLOCK_SIZES[index]);
                let new_node_ptr = ptr.as_ptr() as *mut BlockListNode;
                unsafe {
                    new_node_ptr.write(new_node);
                    self.list_heads[index] = Some(&mut *new_node_ptr);
                }
            }
            None => {
                self.fallback_dealloc(ptr, layout);
            }
        }
    }

    /// Returns whether the provided address is in the FSB's reserved range
    pub fn in_reserved_range(&self, address: efi::PhysicalAddress) -> bool {
        match &self.reserved_range {
            Some(reserved_range) => reserved_range.contains(&address),
            _ => false,
        }
    }

    /// Informs the allocator of it's reserved memory range.
    ///
    /// This function is intended to be called on a region of memory that has been marked with a backing memory allocator
    /// as reserved for this allocator. Calling this funcion does not itself reserve the region of memory.
    ///
    /// ## Safety
    ///
    /// The range must not overlap with any existing allocations.
    pub fn set_reserved_range(&mut self, range: NonNull<[u8]>) -> Result<(), EfiError> {
        if self.reserved_range.is_some() {
            Err(EfiError::AlreadyStarted)?;
        }

        self.reserved_range = Some(
            range.addr().get() as efi::PhysicalAddress
                ..range.addr().get() as efi::PhysicalAddress + range.len() as efi::PhysicalAddress,
        );

        self.stats.reserved_size = range.len();
        self.stats.reserved_used = 0;
        self.stats.claimed_pages += uefi_size_to_pages!(range.len());

        // call into the page change callback to keep track of the updated reserved stats and
        // any memory map changes made when reserving the range.
        self.update_memory_type_info();

        Ok(())
    }

    /// Indicates whether the given pointer falls within a memory region managed by this allocator.
    ///
    /// Note: `true` does not indicate that the pointer corresponds to an active allocation - it may be in either
    /// allocated or freed memory. `true` just means that the pointer falls within a memory region that this allocator
    /// manages.
    pub fn contains(&self, ptr: *mut u8) -> bool {
        AllocatorIterator::new(self.allocators).any(|node| {
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

        // if we managed to allocate pages, call into the page change callback to update stats
        self.update_memory_type_info();
    }

    /// Tracks page freeing for record keeping
    pub fn notify_pages_freed(&mut self, address: efi::PhysicalAddress, pages: usize) {
        if self.in_reserved_range(address) {
            self.stats.reserved_used = self.stats.reserved_used.saturating_sub(pages * ALIGNMENT);
        } else {
            self.stats.claimed_pages = self.stats.claimed_pages.saturating_sub(pages);
        }

        // call into the page change callback to update stats
        self.update_memory_type_info();
    }

    /// Get the ranges of the memory owned by this allocator
    ///
    /// Returns an iterator of ranges of the memory owned by this allocator.
    /// If the allocator does not own any memory, it will return an empty iterator.
    pub(crate) fn get_memory_ranges(&self) -> impl Iterator<Item = Range<usize>> {
        AllocatorIterator::new(self.allocators).map(|node| {
            // This is safe because the node is a valid pointer to an AllocatorListNode
            let allocator = unsafe { &(*node).allocator };
            allocator.bottom() as usize..allocator.top() as usize
        })
    }

    #[inline(always)]
    fn memory_type_info(&self) -> &EFiMemoryTypeInformation {
        // SAFETY: memory_type_info is a pointer to a leaked MemoryTypeInfo structure and there have been no type casts
        unsafe { self.memory_type_info.as_ref() }
    }

    #[inline(always)]
    fn memory_type_info_mut(&mut self) -> &mut EFiMemoryTypeInformation {
        // SAFETY: memory_type_info is a pointer to a leaked MemoryTypeInfo structure and there have been no type casts
        unsafe { self.memory_type_info.as_mut() }
    }

    /// Returns the memory type for this allocator
    #[inline(always)]
    pub fn memory_type(&self) -> efi::MemoryType {
        self.memory_type_info().memory_type
    }

    /// Returns a reference to the allocation stats for this allocator.
    pub fn stats(&self) -> &AllocationStatistics {
        &self.stats
    }

    /// Re-calculates the number of pages allocated for this memory type and updates the memory type info.
    fn update_memory_type_info(&mut self) {
        let stats = self.stats();
        let reserved_free = uefi_size_to_pages!(stats.reserved_size - stats.reserved_used);
        let page_count = (stats.claimed_pages - reserved_free) as u32;
        self.memory_type_info_mut().number_of_pages = page_count;
    }
}

impl Display for FixedSizeBlockAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Memory Type: {:x?}", self.memory_type())?;
        writeln!(f, "Allocation Ranges:")?;
        for node in AllocatorIterator::new(self.allocators) {
            let allocator = unsafe { &mut (*node).allocator };
            writeln!(
                f,
                "  PhysRange: {:#x}-{:#x}, Size: {:#x}, Used: {:#x} Free: {:#x}",
                align_down_size(allocator.bottom() as usize, 0x1000), //account for AllocatorListNode
                allocator.top() as usize,
                align_up_size(allocator.size(), 0x1000), //account for AllocatorListNode
                allocator.used(),
                allocator.free(),
            )?;
        }
        writeln!(f, "Bucket Range: {:x?}", self.reserved_range)?;
        writeln!(f, "Allocation Stats:")?;
        writeln!(f, "  pool_allocation_calls: {}", self.stats.pool_allocation_calls)?;
        writeln!(f, "  pool_free_calls: {}", self.stats.pool_free_calls)?;
        writeln!(f, "  page_allocation_calls: {}", self.stats.page_allocation_calls)?;
        writeln!(f, "  page_free_calls: {}", self.stats.page_free_calls)?;
        writeln!(f, "  reserved_size: {}", self.stats.reserved_size)?;
        writeln!(f, "  reserved_used: {}", self.stats.reserved_used)?;
        writeln!(f, "  claimed_pages: {}", self.stats.claimed_pages)?;
        Ok(())
    }
}

/// Spin Locked Fixed Size Block Allocator
///
/// A wrapper for [`FixedSizeBlockAllocator`] that allocates additional memory as needed from a GCD
/// and provides Sync/Send via means of a spin mutex.
pub struct SpinLockedFixedSizeBlockAllocator {
    /// The GCD instance that this allocator uses to allocate additional memory as needed.
    gcd: &'static SpinLockedGcd,

    /// The handle associated with this allocator. It is used to track ownership of the memory allocated
    /// by this allocator in the GCD.
    handle: efi::Handle,

    /// The inner allocator that is protected by a TPL mutex.
    inner: tpl_mutex::TplMutex<FixedSizeBlockAllocator>,
}

impl SpinLockedFixedSizeBlockAllocator {
    /// Creates a new empty FixedSizeBlockAllocator that will request memory from `gcd` as needed to satisfy
    /// requests.
    pub const fn new(
        gcd: &'static SpinLockedGcd,
        allocator_handle: efi::Handle,
        memory_type_info: NonNull<EFiMemoryTypeInformation>,
        page_allocation_granularity: usize,
    ) -> Self {
        SpinLockedFixedSizeBlockAllocator {
            gcd,
            handle: allocator_handle,
            inner: tpl_mutex::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                FixedSizeBlockAllocator::new(memory_type_info, page_allocation_granularity),
                "FsbLock",
            ),
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
            .gcd
            .allocate_memory_space(
                allocation_strategy,
                GcdMemoryType::SystemMemory,
                align_shift,
                uefi_pages_to_size!(required_pages),
                self.handle,
                None,
            )
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
    /// ## Safety
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [Self::allocate_pages]
    pub unsafe fn free_pages(&self, address: usize, pages: usize) -> Result<(), EfiError> {
        self.lock().stats.page_free_calls += 1;

        let granularity = self.lock().page_allocation_granularity;

        // Ensure that the requested number of pages is a multiple of the granularity
        let required_pages = align_up(pages, uefi_size_to_pages!(granularity))?;

        if !address.is_multiple_of(granularity) {
            return Err(EfiError::InvalidParameter);
        }

        let descriptor =
            self.gcd.get_memory_descriptor_for_address(address as efi::PhysicalAddress).map_err(|err| match err {
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

    /// Reserves a range of memory to be used by this allocator of the given size in pages.
    ///
    /// The caller specifies a maximum number of pages this allocator is expected to require, and as long as the number
    /// of pages actually used by the allocator is less than that amount, then all the allocations for this allocator
    /// will be in a single contiguous block. This capability can be used to ensure that the memory map presented to the
    /// OS is stable from boot-to-boot despite small boot-to-boot variations in actual page usage.
    ///
    /// For best memory stability, this routine should be called only during the initialization of the memory subsystem;
    /// calling it after other allocations/frees have occurred will not cause allocation errors, but may cause the
    /// memory map to vary from boot-to-boot.
    ///
    /// This routine will return Err(efi::Status::ALREADY_STARTED) if it is called more than once.
    ///
    pub fn reserve_memory_pages(&self, pages: usize) -> Result<(), EfiError> {
        if self.lock().reserved_range.is_some() {
            Err(EfiError::AlreadyStarted)?;
        }

        // Even though the platform is telling us what the memory buckets are, we have to take into account
        // architecture-specific requirements for runtime page allocation granularity.
        let granularity = self.lock().page_allocation_granularity;

        // Ensure that the requested number of pages is a multiple of the granularity
        let required_pages = align_up(pages, uefi_size_to_pages!(granularity))?;

        let reserved_block_len = uefi_pages_to_size!(required_pages);

        // Allocate then free a block of the requested length in the GCD while preserving ownership.
        // This, in effect, reserves this region in the GCD for use by this allocator.
        let reserved_block_addr = self.gcd.allocate_memory_space(
            DEFAULT_ALLOCATION_STRATEGY,
            GcdMemoryType::SystemMemory,
            page_shift_from_alignment(granularity)?,
            reserved_block_len,
            self.handle,
            None,
        )?;
        self.gcd.free_memory_space_preserving_ownership(reserved_block_addr, reserved_block_len)?;

        self.lock().set_reserved_range(NonNull::slice_from_raw_parts(
            NonNull::new(reserved_block_addr as *mut u8).unwrap(),
            reserved_block_len,
        ))
    }

    /// Returns an iterator of the ranges of memory owned by this allocator
    /// Returns an empty iterator if the allocator does not own any memory.
    pub fn get_memory_ranges(&self) -> alloc::vec::IntoIter<Range<usize>> {
        let ranges: Vec<_> = self.lock().get_memory_ranges().collect();
        ranges.into_iter()
    }

    /// Returns the allocator handle associated with this allocator.
    pub fn handle(&self) -> efi::Handle {
        self.handle
    }

    /// Returns the reserved memory range, if any.
    pub fn reserved_range(&self) -> Option<Range<efi::PhysicalAddress>> {
        self.inner.lock().reserved_range.clone()
    }

    /// Returns the memory type for this allocator.
    #[allow(dead_code)]
    pub fn memory_type(&self) -> efi::MemoryType {
        self.inner.lock().memory_type()
    }

    /// Returns allocation statistics for this allocator.
    #[allow(dead_code)]
    pub fn stats(&self) -> AllocationStatistics {
        *self.inner.lock().stats()
    }
}

unsafe impl GlobalAlloc for SpinLockedFixedSizeBlockAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match self.allocate(layout) {
            Ok(alloc) => alloc.as_ptr() as *mut u8,
            Err(_) => core::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(ptr) = NonNull::new(ptr) {
            unsafe { self.deallocate(ptr, layout) }
        }
    }
}

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

                // As a matter of policy, allocate at least `MIN_EXPANSION` memory and ensure the size is
                // aligned to `ALIGNMENT`.
                let mut allocation_size = max(additional_mem_required, MIN_EXPANSION);
                let required_alignment = self.lock().page_allocation_granularity;

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
                let start_address: usize = self
                    .gcd
                    .allocate_memory_space(
                        DEFAULT_ALLOCATION_STRATEGY,
                        GcdMemoryType::SystemMemory,
                        page_shift_from_alignment(required_alignment).map_err(|_| {
                            debug_assert!(false);
                            AllocError
                        })?,
                        allocation_size,
                        self.handle,
                        None,
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
                if self.lock().expand(NonNull::slice_from_raw_parts(allocated_ptr, allocation_size)).is_err() {
                    debug_assert!(false);
                    return Err(AllocError);
                }

                // Try the allocation one more time
                match self.lock().alloc(layout) {
                    Ok(alloc) => Ok(alloc),
                    Err(_) => {
                        debug_assert!(false);
                        Err(AllocError)
                    }
                }
            }
            Err(_) => {
                debug_assert!(false);
                Err(AllocError)
            }
        }
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { self.lock().dealloc(ptr, layout) }
    }
}

impl Display for SpinLockedFixedSizeBlockAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.lock().fmt(f)
    }
}

unsafe impl Sync for SpinLockedFixedSizeBlockAllocator {}
unsafe impl Send for SpinLockedFixedSizeBlockAllocator {}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use crate::{
        allocator::{DEFAULT_ALLOCATION_STRATEGY, DEFAULT_PAGE_ALLOCATION_GRANULARITY},
        gcd, test_support,
    };
    use core::{alloc::GlobalAlloc, ffi::c_void, panic};
    use std::alloc::System;

    use patina::{
        base::{SIZE_64KB, UEFI_PAGE_SIZE},
        uefi_pages_to_size,
    };

    use super::*;

    fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
        unsafe { gcd.reset() };

        gcd.init(48, 16);
        let layout = Layout::from_size_align(size, UEFI_PAGE_SIZE).unwrap();
        let base = unsafe { System.alloc(layout) as u64 };
        unsafe {
            gcd.init_memory_blocks(GcdMemoryType::SystemMemory, base as usize, size, efi::MEMORY_WB).unwrap();
        }
        base
    }

    // Test function to create a memory type info structure.
    fn memory_type_info(memory_type: efi::MemoryType) -> NonNull<EFiMemoryTypeInformation> {
        let memory_type_info = Box::new(EFiMemoryTypeInformation { memory_type, number_of_pages: 0 });
        NonNull::new(Box::leak(memory_type_info)).unwrap()
    }

    // this runs each test twice, once with 4KB page allocation granularity and once with 64KB page allocation
    // granularity. This is to ensure that the allocator works correctly with both page allocation granularities.
    fn with_granularity_modulation<F: Fn(usize) + std::panic::RefUnwindSafe>(f: F) {
        f(DEFAULT_PAGE_ALLOCATION_GRANULARITY);
        f(SIZE_64KB);
    }

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            f();
        })
        .unwrap();
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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();

                unsafe { fsb.deallocate(allocation, layout) };

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();

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
            let fsb = FixedSizeBlockAllocator::new(
                memory_type_info(efi::BOOT_SERVICES_DATA),
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            assert!(fsb.list_heads.iter().all(|x| x.is_none()));
            assert!(fsb.allocators.is_none());
        });
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
                let mut fsb = FixedSizeBlockAllocator::new(memory_type_info(efi::RUNTIME_SERVICES_DATA), granularity);
                assert!(fsb.allocators.is_none());

                let allocation_size = MIN_EXPANSION;

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

                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(allocated_address as *mut u8).unwrap(),
                    allocation_size,
                ))
                .unwrap();

                assert!(fsb.allocators.is_some());
                unsafe {
                    assert!((*fsb.allocators.unwrap()).next.is_none());
                    assert!((*fsb.allocators.unwrap()).allocator.bottom() as usize > base as usize);
                    assert_eq!(
                        (*fsb.allocators.unwrap()).allocator.free(),
                        allocation_size - size_of::<AllocatorListNode>()
                    );
                }

                //expand by larger than MIN_EXPANSION.
                let allocation_size = MIN_EXPANSION + 0x1000;

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

                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(allocated_address as *mut u8).unwrap(),
                    allocation_size,
                ))
                .unwrap();
                assert!(fsb.allocators.is_some());
                unsafe {
                    assert!((*fsb.allocators.unwrap()).next.is_some());
                    assert!((*(*fsb.allocators.unwrap()).next.unwrap()).next.is_none());
                    assert!((*fsb.allocators.unwrap()).allocator.bottom() as usize > base as usize);
                    assert_eq!(
                        (*fsb.allocators.unwrap()).allocator.free(),
                        //expected free: size + a page to hold allocator node - size of allocator node.
                        allocation_size - size_of::<AllocatorListNode>()
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

            let mut fsb = FixedSizeBlockAllocator::new(
                memory_type_info(efi::BOOT_SERVICES_DATA),
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );

            const NUM_ALLOCATIONS: usize = 5;

            let allocation_size = MIN_EXPANSION;
            for _ in 0..NUM_ALLOCATIONS {
                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(
                        GCD.allocate_memory_space(
                            DEFAULT_ALLOCATION_STRATEGY,
                            GcdMemoryType::SystemMemory,
                            UEFI_PAGE_SHIFT,
                            allocation_size,
                            DUMMY_HANDLE,
                            None,
                        )
                        .unwrap() as *mut u8,
                    )
                    .unwrap(),
                    allocation_size,
                ))
                .unwrap();
            }

            assert_eq!(NUM_ALLOCATIONS, AllocatorIterator::new(fsb.allocators).count());
            assert!(
                AllocatorIterator::new(fsb.allocators)
                    .all(|node| unsafe { (*node).allocator.free() == MIN_EXPANSION - size_of::<AllocatorListNode>() })
            );
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

                let mut fsb = FixedSizeBlockAllocator::new(memory_type_info(efi::RUNTIME_SERVICES_DATA), granularity);

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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
                let allocation = fsb.allocate(layout).unwrap().as_ptr() as *mut u8;
                assert!(fsb.lock().allocators.is_some());
                assert!((allocation as u64) > base);
                assert!((allocation as u64) < base + 0x400000);
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

                let mut fsb = FixedSizeBlockAllocator::new(memory_type_info(efi::RUNTIME_SERVICES_DATA), granularity);

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();

                // Expand the FSB by `MIN_EXPANSION` to fit the allocation
                fsb.expand(NonNull::slice_from_raw_parts(
                    NonNull::new(
                        GCD.allocate_memory_space(
                            DEFAULT_ALLOCATION_STRATEGY,
                            GcdMemoryType::SystemMemory,
                            UEFI_PAGE_SHIFT,
                            MIN_EXPANSION,
                            DUMMY_HANDLE,
                            None,
                        )
                        .unwrap() as *mut u8,
                    )
                    .unwrap(),
                    MIN_EXPANSION,
                ))
                .unwrap();

                let allocation = fsb.fallback_alloc(layout).unwrap();

                // Finally, we can test fallback_dealloc
                fsb.fallback_dealloc(allocation.cast(), layout);
                unsafe {
                    assert_eq!(
                        (*fsb.allocators.unwrap()).allocator.free(),
                        MIN_EXPANSION - size_of::<AllocatorListNode>()
                    );
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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                let allocation = unsafe { fsb.alloc(layout) };

                unsafe { fsb.dealloc(allocation, layout) };
                let free_block_ptr = fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap()
                    as *mut BlockListNode as *mut u8;
                assert_eq!(free_block_ptr, allocation);

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                let allocation = unsafe { fsb.alloc(layout) };

                unsafe { fsb.dealloc(allocation, layout) };
                let free_block_ptr = fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap()
                    as *mut BlockListNode as *mut u8;
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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                let allocation_ptr = allocation.as_ptr();

                unsafe { fsb.deallocate(allocation, layout) };
                let free_block_ptr = fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap()
                    as *mut BlockListNode as *mut u8;
                assert_eq!(free_block_ptr, allocation_ptr);

                let layout = Layout::from_size_align(0x20, 0x20).unwrap();
                let allocation = fsb.allocate(layout).unwrap().cast::<u8>();
                let allocation_ptr = allocation.as_ptr();

                unsafe { fsb.deallocate(allocation, layout) };
                let free_block_ptr = fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap()
                    as *mut BlockListNode as *mut u8;
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
                memory_type_info(efi::BOOT_SERVICES_DATA),
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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let pages = 4;

                let allocation =
                    fsb.allocate_pages(gcd::AllocateType::BottomUp(None), pages, UEFI_PAGE_SIZE).unwrap().cast::<u8>();

                assert!(allocation.as_ptr() as u64 >= address);
                assert!((allocation.as_ptr() as u64) < address + 0x1000000);

                unsafe {
                    match fsb.free_pages(0, pages) {
                        Err(EfiError::NotFound) => {}
                        _ => panic!("Expected NOT_FOUND"),
                    };
                };

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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let target_address = (address + 0x400000 - max(8_u64 * ALIGNMENT as u64, granularity as u64))
                    & (!(granularity as u64 - 1_u64));
                let pages = 4;

                let allocation = fsb
                    .allocate_pages(gcd::AllocateType::Address(target_address as usize), pages, UEFI_PAGE_SIZE)
                    .unwrap()
                    .cast::<u8>();

                assert_eq!(allocation.as_ptr() as u64, target_address);

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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
                let pages = 4;

                let allocation = fsb
                    .allocate_pages(gcd::AllocateType::BottomUp(Some(target_address as usize)), pages, UEFI_PAGE_SIZE)
                    .unwrap()
                    .cast::<u8>();
                assert!((allocation.as_ptr() as u64) < target_address);

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
                    memory_type_info(efi::RUNTIME_SERVICES_DATA),
                    granularity,
                );

                let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
                let pages = 4;

                let allocation = fsb
                    .allocate_pages(gcd::AllocateType::TopDown(Some(target_address as usize)), pages, UEFI_PAGE_SIZE)
                    .unwrap()
                    .cast::<u8>();
                assert!((allocation.as_ptr() as usize + uefi_pages_to_size!(pages)) <= target_address as usize);

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
                memory_type_info(efi::BOOT_SERVICES_DATA),
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            match fsb.allocate_pages(AllocationStrategy::Address(0x1000), 5, UEFI_PAGE_SIZE) {
                Err(EfiError::InvalidParameter) => {}
                _ => panic!("Expected INVALID_PARAMETER"),
            }

            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                1 as _,
                memory_type_info(efi::BOOT_SERVICES_DATA),
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
                memory_type_info(efi::BOOT_SERVICES_DATA),
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

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x1000000);

            // Make a fixed-sized-block allocator
            let fsb = SpinLockedFixedSizeBlockAllocator::new(
                &GCD,
                1 as _,
                memory_type_info(efi::BOOT_SERVICES_DATA),
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 0);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, 0);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, 0);

            //reserve some space and check the stats.
            fsb.reserve_memory_pages(uefi_size_to_pages!(MIN_EXPANSION * 2)).unwrap();

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 0);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 2));

            //test alloc/deallocate and stats within the bucket
            let ptr = unsafe {
                fsb.alloc(Layout::from_size_align(MIN_EXPANSION - size_of::<AllocatorListNode>(), 0x8).unwrap())
            };

            let stats = fsb.stats();
            //an additional allocation call will be made as the first one will fail due to a lack of memory
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 2));

            unsafe {
                fsb.dealloc(ptr, Layout::from_size_align(0x100, 0x8).unwrap());
            }

            let stats = fsb.stats();
            //an additional allocation call will be made as the first one will fail due to a lack of memory
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 2));

            //test alloc/deallocate and stats blowing the bucket
            let ptr = unsafe { fsb.alloc(Layout::from_size_align(MIN_EXPANSION * 3, 0x8).unwrap()) };

            //after this allocate, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node.

            let stats = fsb.stats();
            //an additional allocation call will be made as the first one will fail due to a lack of memory
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1);

            unsafe {
                fsb.dealloc(ptr, Layout::from_size_align(MIN_EXPANSION * 3, 0x8).unwrap());
            }

            //after this free, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 0);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1);

            // test that a small page allocation fits in the 1MB free reserved region.
            let ptr = fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 0x4, UEFI_PAGE_SIZE).unwrap().as_ptr();

            //after this allocate_pages, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //16K allocated.
            //1MB-16k free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 0);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(5));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1);

            unsafe {
                fsb.free_pages(ptr as *mut u8 as usize, 0x4).unwrap();
            }

            //after this free, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1);

            //test that a lage page allocation results in more claimed pages.
            let ptr = fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 0x104, UEFI_PAGE_SIZE).unwrap().as_ptr();

            //after this allocate_pages, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.
            //104 pages (1MB+16K) page as a result of allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 2);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1 + 0x104);

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
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 3);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(5));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1 + 0x104);

            unsafe {
                fsb.free_pages(ptr1 as *mut u8 as usize, 0x4).unwrap();
            }
            unsafe {
                fsb.free_pages(ptr as *mut u8 as usize, 0x104).unwrap();
            }

            //after this free, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node - available for pool allocation.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 4);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 3);
            assert_eq!(stats.page_free_calls, 3);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(1));
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1);
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

                let mut fsb = FixedSizeBlockAllocator::new(memory_type_info(efi::RUNTIME_SERVICES_DATA), granularity);

                const NUM_ALLOCATIONS: usize = 3;

                // Expand the FSB multiple times
                for _ in 0..NUM_ALLOCATIONS {
                    fsb.expand(NonNull::slice_from_raw_parts(
                        NonNull::new(
                            GCD.allocate_memory_space(
                                DEFAULT_ALLOCATION_STRATEGY,
                                GcdMemoryType::SystemMemory,
                                UEFI_PAGE_SHIFT,
                                MIN_EXPANSION,
                                DUMMY_HANDLE,
                                None,
                            )
                            .unwrap() as *mut u8,
                        )
                        .unwrap(),
                        MIN_EXPANSION,
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
}
