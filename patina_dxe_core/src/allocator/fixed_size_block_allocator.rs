//! Fixed-sized block allocator.
//!
//! Implements a fixed-sized block allocator backed by a linked list allocator. Based on the example fixed-sized block
//! allocator presented here: <https://os.phil-opp.com/allocator-designs/#fixed-size-block-allocator>.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

extern crate alloc;
use super::{AllocationStrategy, DEFAULT_ALLOCATION_STRATEGY};

use crate::{gcd::SpinLockedGcd, tpl_lock};
use patina_sdk::{base::UEFI_PAGE_SIZE, error::EfiError};

use core::{
    alloc::{AllocError, Allocator, GlobalAlloc, Layout},
    cmp::max,
    fmt::{self, Display},
    mem::{align_of, size_of},
    ops::Range,
    ptr::{self, slice_from_raw_parts_mut, NonNull},
};
use linked_list_allocator::{align_down_size, align_up_size};
use mu_pi::dxe_services::GcdMemoryType;
use patina_sdk::{base::UEFI_PAGE_SHIFT, uefi_size_to_pages};
use r_efi::efi;

/// Type for describing errors that this implementation can produce.
#[derive(Debug, PartialEq)]
pub enum FixedSizeBlockAllocatorError {
    /// Could not satisfy allocation request, and expansion failed.
    OutOfMemory,
}

/// Minimum expansion size - allocator will request at least this much memory
/// from the underlying GCD instance expansion is needed.
pub const MIN_EXPANSION: usize = 0x100000;
const ALIGNMENT: usize = 0x1000;

const BLOCK_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

// Returns the index in the block list for the minimum size block that will
// satisfy allocation for the given layout
fn list_index(layout: &Layout) -> Option<usize> {
    let required_block_size = layout.size().max(layout.align());
    BLOCK_SIZES.iter().position(|&s| s >= required_block_size)
}

/// Converts the given alignment to a shift value.
fn page_shift_from_alignment(alignment: usize) -> Result<usize, EfiError> {
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
    pub pool_allocation_calls: usize,
    pub pool_free_calls: usize,
    pub page_allocation_calls: usize,
    pub page_free_calls: usize,
    pub reserved_size: usize,
    pub reserved_used: usize,
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

/// PageChangeCallback is invoked whenever the allocator performs an operation that would potentially allocate or free
/// pages from the GCD and thus change the memory map. It receives a mutable reference to the allocator that is
/// performing the operation.
///
/// ## Safety
/// This callback has several constraints and cautions on its usage:
/// 1. The callback is invoked while the allocator in question is locked. This means that to avoid a re-entrant lock
///    on the allocator, any operations required from the allocator must be invoked via the given reference, and not
///    via other means (such as global allocation routines that target this same allocator).
/// 2. The allocator could potentially be the "global" allocator (i.e. EFI_BOOT_SERVICES_DATA). Extra care should be
///    taken to avoid implicit heap usage (e.g. `Box::new()`) if that's the case.
///
/// Generally - be very cautious about any allocations performed with this callback. There be dragons.
///
pub type PageChangeCallback = fn(&mut FixedSizeBlockAllocator);

/// Fixed Size Block Allocator
///
/// Implements an expandable memory allocator using fixed-sized blocks for speed backed by a linked-list allocator
/// implementation when an appropriate sized free block is not available. If more memory is required than can be
/// satisfied by either the block list or the linked-list, more memory is requested from the GCD supplied at
/// instantiation and a new backing linked-list is created.
///
pub struct FixedSizeBlockAllocator {
    gcd: &'static SpinLockedGcd,
    handle: efi::Handle,
    memory_type: efi::MemoryType,
    list_heads: [Option<&'static mut BlockListNode>; BLOCK_SIZES.len()],
    allocators: Option<*mut AllocatorListNode>,
    pub(crate) preferred_range: Option<Range<efi::PhysicalAddress>>,
    stats: AllocationStatistics,
    page_change_callback: PageChangeCallback,
}

impl FixedSizeBlockAllocator {
    /// Creates a new empty FixedSizeBlockAllocator that will request memory from `gcd` as needed to satisfy
    /// requests.
    pub const fn new(
        gcd: &'static SpinLockedGcd,
        allocator_handle: efi::Handle,
        memory_type: efi::MemoryType,
        page_change_callback: PageChangeCallback,
    ) -> Self {
        const EMPTY: Option<&'static mut BlockListNode> = None;
        FixedSizeBlockAllocator {
            gcd,
            handle: allocator_handle,
            memory_type,
            list_heads: [EMPTY; BLOCK_SIZES.len()],
            allocators: None,
            preferred_range: None,
            stats: AllocationStatistics::new(),
            page_change_callback,
        }
    }

    // This routine resets some aspects of allocator state for testing purposes.
    // Note: this does not reset the GCD nor change the page_change_callback.
    #[cfg(test)]
    pub fn reset(&mut self) {
        const EMPTY: Option<&'static mut BlockListNode> = None;
        self.list_heads = [EMPTY; BLOCK_SIZES.len()];
        self.allocators = None;
        self.preferred_range = None;
        self.stats = AllocationStatistics::new();
    }

    // Expand the memory available to this allocator by requesting a new contiguous region of memory from the gcd setting
    // up a new allocator node to manage this range
    fn expand(&mut self, layout: Layout) -> Result<(), FixedSizeBlockAllocatorError> {
        let size = layout.pad_to_align().size() + Layout::new::<AllocatorListNode>().pad_to_align().size();
        let size = max(size, MIN_EXPANSION);
        //ensure size is a multiple of alignment to avoid fragmentation.
        let size = align_up_size(size, ALIGNMENT);
        //Allocate memory from the gcd.
        let start_address = self
            .gcd
            .allocate_memory_space(
                DEFAULT_ALLOCATION_STRATEGY,
                GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                size,
                self.handle,
                None,
            )
            .map_err(|_| FixedSizeBlockAllocatorError::OutOfMemory)?;

        //set up the new allocator, reserving space at the beginning of the range for the AllocatorListNode structure.

        let heap_bottom = start_address + size_of::<AllocatorListNode>();
        let heap_size = size - (heap_bottom - start_address);

        let alloc_node_ptr = start_address as *mut AllocatorListNode;
        let node = AllocatorListNode { next: None, allocator: linked_list_allocator::Heap::empty() };

        //write the allocator node structure into the start of the range, initialize its heap with the remainder of
        //the range, and add the new allocator to the front of the allocator list.
        unsafe {
            alloc_node_ptr.write(node);
            (*alloc_node_ptr).allocator.init(heap_bottom as *mut u8, heap_size);
            (*alloc_node_ptr).next = self.allocators;
        }

        self.allocators = Some(alloc_node_ptr);

        if self.preferred_range.as_ref().is_some_and(|range| range.contains(&(start_address as efi::PhysicalAddress))) {
            self.stats.reserved_used += size;
        } else {
            self.stats.claimed_pages += uefi_size_to_pages!(size);
        }

        // if we managed to allocate pages, call into the page change callback to update stats
        (self.page_change_callback)(self);

        Ok(())
    }

    // allocates from the linked-list backing allocator if a free block of the
    // appropriate size is not available.
    fn fallback_alloc(&mut self, layout: Layout) -> *mut u8 {
        for node in AllocatorIterator::new(self.allocators) {
            let allocator = unsafe { &mut (*node).allocator };
            if let Ok(ptr) = allocator.allocate_first_fit(layout) {
                return ptr.as_ptr();
            }
        }
        //if we get here, then allocation failed in all current allocation ranges.
        //attempt to expand and then allocate again
        if self.expand(layout).is_err() {
            return ptr::null_mut();
        }
        self.fallback_alloc(layout)
    }

    /// Allocates and returns a pointer to a memory buffer for the given layout.
    ///
    /// This routine is designed to satisfy the [`GlobalAlloc`] trait, except that it requires a mutable self.
    /// [`SpinLockedFixedSizeBlockAllocator`] provides a [`GlobalAlloc`] trait impl by wrapping this routine.
    ///
    /// Memory allocated by this routine should be deallocated with
    /// [`Self::dealloc`]
    ///
    /// ## Errors
    ///
    /// Returns [`core::ptr::null_mut()`] on failure to allocate.
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        self.stats.pool_allocation_calls += 1;
        match list_index(&layout) {
            Some(index) => {
                match self.list_heads[index].take() {
                    Some(node) => {
                        self.list_heads[index] = node.next.take();
                        node as *mut BlockListNode as *mut u8
                    }
                    None => {
                        // no block exists in list => allocate new block
                        let block_size = BLOCK_SIZES[index];
                        // only works if all block sizes are a power of 2
                        let block_align = block_size;
                        let layout = match Layout::from_size_align(block_size, block_align) {
                            Ok(layout) => layout,
                            Err(_) => return core::ptr::null_mut(),
                        };
                        self.fallback_alloc(layout)
                    }
                }
            }
            None => self.fallback_alloc(layout),
        }
    }

    /// Allocates and returns a NonNull byte slice for the given layout.
    ///
    /// This routine is designed to satisfy the [`Allocator`] trait, except that it  requires a mutable self.
    /// [`SpinLockedFixedSizeBlockAllocator`] provides an [`Allocator`] trait impl by wrapping this routine.
    ///
    /// Memory allocated by this routine should be deallocated with
    /// [`Self::deallocate`]
    ///
    /// ## Errors
    ///
    /// returns AllocError on failure to allocate.
    pub fn allocate(&mut self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let allocation = self.alloc(layout);
        let allocation = slice_from_raw_parts_mut(allocation, layout.size());
        let allocation = NonNull::new(allocation).ok_or(AllocError)?;
        Ok(allocation)
    }

    // deallocates back to the linked-list backing allocator if the size of
    // layout being freed is too big to be tracked as a fixed-size free block.
    fn fallback_dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        if let Some(ptr) = NonNull::new(ptr) {
            for node in AllocatorIterator::new(self.allocators) {
                let allocator = unsafe { &mut (*node).allocator };
                if (allocator.bottom() <= ptr.as_ptr()) && (ptr.as_ptr() < allocator.top()) {
                    unsafe { allocator.deallocate(ptr, layout) };
                }
            }
        }
    }

    /// Deallocates a buffer allocated by [`Self::alloc`].
    ///
    /// This routine is designed to satisfy the [`GlobalAlloc`] trait, except  that it requires a mutable self.
    /// [`SpinLockedFixedSizeBlockAllocator`] provides a [`GlobalAlloc`] trait impl by wrapping this routine.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that `ptr` was created by a call to [`Self::alloc`] with the same `layout`.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        self.stats.pool_free_calls += 1;
        match list_index(&layout) {
            Some(index) => {
                let new_node = BlockListNode { next: self.list_heads[index].take() };
                // verify that block has size and alignment required for storing node
                assert!(size_of::<BlockListNode>() <= BLOCK_SIZES[index]);
                assert!(align_of::<BlockListNode>() <= BLOCK_SIZES[index]);
                let new_node_ptr = ptr as *mut BlockListNode;
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

    /// Deallocates a buffer allocated by [`Self::allocate`] .
    ///
    /// This routine is designed to satisfy the [`Allocator`] trait, except that it requires a mutable self.
    /// [`SpinLockedFixedSizeBlockAllocator`] provides an [`Allocator`] trait impl by wrapping this routine.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that `ptr` was created by a call to [`Self::allocate`] with the same `layout`.
    pub unsafe fn deallocate(&mut self, ptr: NonNull<u8>, layout: Layout) {
        self.dealloc(ptr.as_ptr(), layout)
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

    /// Attempts to allocate the given number of pages according to the given allocation strategy.
    /// Valid allocation strategies are:
    /// - BottomUp(None): Allocate the block of pages from the lowest available free memory.
    /// - BottomUp(Some(address)): Allocate the block of pages from the lowest available free memory. Fail if memory
    ///     cannot be found below `address`.
    /// - TopDown(None): Allocate the block of pages from the highest available free memory.
    /// - TopDown(Some(address)): Allocate the block of pages from the highest available free memory. Fail if memory
    ///      cannot be found above `address`.
    /// - Address(address): Allocate the block of pages at exactly the given address (or fail).
    ///
    /// If an address is specified as part of a strategy, it must be page-aligned.
    pub fn allocate_pages(
        &mut self,
        allocation_strategy: AllocationStrategy,
        pages: usize,
        alignment: usize,
    ) -> Result<core::ptr::NonNull<[u8]>, EfiError> {
        self.stats.page_allocation_calls += 1;

        let align_shift = page_shift_from_alignment(alignment)?;

        if let AllocationStrategy::Address(address) = allocation_strategy {
            // validate allocation strategy addresses for direct address allocation is properly aligned.
            // for BottomUp and TopDown strategies, the address parameter doesn't have to be page-aligned, but
            // the resulting allocation will be page-aligned.
            if address % alignment != 0 {
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
                pages << UEFI_PAGE_SHIFT,
                self.handle,
                None,
            )
            .map_err(|err| match err {
                EfiError::InvalidParameter | EfiError::NotFound => err,
                _ => EfiError::OutOfResources,
            })?;

        let allocation = slice_from_raw_parts_mut(start_address as *mut u8, pages * ALIGNMENT);
        let allocation = NonNull::new(allocation).ok_or(EfiError::OutOfResources)?;

        if self.preferred_range.as_ref().is_some_and(|range| range.contains(&(start_address as efi::PhysicalAddress))) {
            self.stats.reserved_used += pages * ALIGNMENT;
        } else {
            self.stats.claimed_pages += pages;
        }

        // if we managed to allocate pages, call into the page change callback to update stats
        (self.page_change_callback)(self);

        Ok(allocation)
    }

    /// Frees the block of pages at the given address of the given size.
    /// ## Safety
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [Self::allocate_pages]
    pub unsafe fn free_pages(&mut self, address: usize, pages: usize) -> Result<(), EfiError> {
        self.stats.page_free_calls += 1;
        if address % ALIGNMENT != 0 {
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

        if self.preferred_range.as_ref().is_some_and(|range| range.contains(&(address as efi::PhysicalAddress))) {
            self.gcd.free_memory_space_preserving_ownership(address, pages * ALIGNMENT).map_err(|err| match err {
                EfiError::NotFound => err,
                _ => EfiError::InvalidParameter,
            })?;
            self.stats.reserved_used -= pages * ALIGNMENT;
            // don't update claimed_pages stats here, because they are never actually "released".
        } else {
            self.gcd.free_memory_space(address, pages * ALIGNMENT).map_err(|err| match err {
                EfiError::NotFound => err,
                _ => EfiError::InvalidParameter,
            })?;
            self.stats.claimed_pages -= pages;
        }

        // if we managed to allocate pages, call into the page change callback to update stats
        (self.page_change_callback)(self);

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
    pub fn reserve_memory_pages(&mut self, pages: usize) -> Result<(), EfiError> {
        if self.preferred_range.is_some() {
            Err(EfiError::AlreadyStarted)?;
        }

        // Set up the preferred range of memory for this allocator by allocating a block of the given size, and then
        // freeing them back with preserved ownership to the GCD.
        //
        // Note: using this for memory map stability is predicated on the assumption that the GCD returns allocations
        // in a consistent order such that memory that is allocated and freed preserving ownership will be encountered
        // before "non-owned" free memory. If memory is allocated before this call and then later freed back to the GCD
        // without ownership, then this assumption may not hold, and memory may be allocated outside the preferred range
        // even if there is space in the preferred range. This will not break memory allocation, but may result in
        // an unstable memory map. To avoid this, memory ranges should be reserved during memory subsystem init before
        // any general allocations are serviced; that way all "owned" memory is in prime position before any "unowned"
        // memory.
        //
        let preferred_block = self.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, pages, UEFI_PAGE_SIZE)?;
        let preferred_block_address = preferred_block.as_ptr() as *mut u8 as efi::PhysicalAddress;

        // this will fail if called more than once, but check at start of function should guarantee that doesn't happen.
        self.preferred_range =
            Some(preferred_block_address..preferred_block_address + (pages * ALIGNMENT) as efi::PhysicalAddress);

        // update reserved stat here, since allocate_pages was not yet aware of preferred range to properly track.
        self.stats.reserved_size = pages * ALIGNMENT;
        self.stats.reserved_used += pages * ALIGNMENT;
        unsafe {
            self.free_pages(preferred_block_address as usize, pages).unwrap();
        };

        Ok(())
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

    /// Returns the memory type for this allocator
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
        writeln!(f, "Memory Type: {:x?}", self.memory_type)?;
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
        writeln!(f, "Bucket Range: {:x?}", self.preferred_range)?;
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
/// A wrapper for [`FixedSizeBlockAllocator`] that provides Sync/Send via means of a spin mutex.
pub struct SpinLockedFixedSizeBlockAllocator {
    inner: tpl_lock::TplMutex<FixedSizeBlockAllocator>,
}

impl SpinLockedFixedSizeBlockAllocator {
    /// Creates a new empty FixedSizeBlockAllocator that will request memory from `gcd` as needed to satisfy
    /// requests.
    pub const fn new(
        gcd: &'static SpinLockedGcd,
        allocator_handle: efi::Handle,
        memory_type: efi::MemoryType,
        callback: fn(allocator: &mut FixedSizeBlockAllocator),
    ) -> Self {
        SpinLockedFixedSizeBlockAllocator {
            inner: tpl_lock::TplMutex::new(
                efi::TPL_HIGH_LEVEL,
                FixedSizeBlockAllocator::new(gcd, allocator_handle, memory_type, callback),
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
    pub fn lock(&self) -> tpl_lock::TplGuard<FixedSizeBlockAllocator> {
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
    ///     cannot be found below `address`.
    /// - TopDown(None): Allocate the block of pages from the highest available free memory.
    /// - TopDown(Some(address)): Allocate the block of pages from the highest available free memory. Fail if memory
    ///      cannot be found above `address`.
    /// - Address(address): Allocate the block of pages at exactly the given address (or fail).
    ///
    /// If an address is specified as part of a strategy, it must be page-aligned.
    pub fn allocate_pages(
        &self,
        allocation_strategy: AllocationStrategy,
        pages: usize,
        alignment: usize,
    ) -> Result<core::ptr::NonNull<[u8]>, EfiError> {
        self.lock().allocate_pages(allocation_strategy, pages, alignment)
    }

    /// Frees the block of pages at the given address of the given size.
    /// ## Safety
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [Self::allocate_pages]
    pub unsafe fn free_pages(&self, address: usize, pages: usize) -> Result<(), EfiError> {
        self.lock().free_pages(address, pages)
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
        self.lock().reserve_memory_pages(pages)
    }

    /// Returns an iterator of the ranges of memory owned by this allocator
    /// Returns an empty iterator if the allocator does not own any memory.
    pub fn get_memory_ranges(&self) -> impl Iterator<Item = Range<usize>> {
        self.lock().get_memory_ranges()
    }

    /// Returns the allocator handle associated with this allocator.
    pub fn handle(&self) -> efi::Handle {
        self.inner.lock().handle
    }

    /// Returns the preferred memory range, if any.
    pub fn preferred_range(&self) -> Option<Range<efi::PhysicalAddress>> {
        self.inner.lock().preferred_range.clone()
    }

    /// Returns the memory type for this allocator.
    #[allow(dead_code)]
    pub fn memory_type(&self) -> efi::MemoryType {
        self.inner.lock().memory_type
    }

    /// Returns allocation statistics for this allocator.
    #[allow(dead_code)]
    pub fn stats(&self) -> AllocationStatistics {
        *self.inner.lock().stats()
    }
}

unsafe impl GlobalAlloc for SpinLockedFixedSizeBlockAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock().alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.lock().dealloc(ptr, layout)
    }
}

unsafe impl Allocator for SpinLockedFixedSizeBlockAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.lock().allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.lock().deallocate(ptr, layout)
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
mod tests {
    extern crate std;
    use crate::{gcd, test_support};
    use core::alloc::GlobalAlloc;
    use std::alloc::System;

    use patina_sdk::{base::UEFI_PAGE_SIZE, uefi_pages_to_size};

    use super::*;

    fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
        let layout = Layout::from_size_align(size, UEFI_PAGE_SIZE).unwrap();
        let base = unsafe { System.alloc(layout) as u64 };
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, base as usize, size, efi::MEMORY_WB).unwrap();
        }
        base
    }

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            f();
        })
        .unwrap();
    }

    fn page_change_callback(_allocator: &mut FixedSizeBlockAllocator) {}

    #[test]
    fn allocate_deallocate_test() {
        with_locked_state(|| {
            // Create a static GCD for test.
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();

            unsafe { fsb.deallocate(allocation, layout) };

            let layout = Layout::from_size_align(0x20, 0x20).unwrap();
            let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();

            unsafe { fsb.deallocate(allocation, layout) };
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
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);
            let fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);
            assert!(core::ptr::eq(fsb.gcd, &GCD));
            assert!(fsb.list_heads.iter().all(|x| x.is_none()));
            assert!(fsb.allocators.is_none());
        });
    }

    #[test]
    fn test_expand() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let base = init_gcd(&GCD, 0x400000);

            //verify no allocators exist before expand.
            let mut fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);
            assert!(fsb.allocators.is_none());

            //expand by a page. This will round up to MIN_EXPANSION.
            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            fsb.expand(layout).unwrap();
            assert!(fsb.allocators.is_some());
            unsafe {
                assert!((*fsb.allocators.unwrap()).next.is_none());
                assert!((*fsb.allocators.unwrap()).allocator.bottom() as usize > base as usize);
                assert_eq!((*fsb.allocators.unwrap()).allocator.free(), MIN_EXPANSION - size_of::<AllocatorListNode>());
            }
            //expand by larger than MIN_EXPANSION.
            let layout = Layout::from_size_align(MIN_EXPANSION + 0x1000, 0x10).unwrap();
            fsb.expand(layout).unwrap();
            assert!(fsb.allocators.is_some());
            unsafe {
                assert!((*fsb.allocators.unwrap()).next.is_some());
                assert!((*(*fsb.allocators.unwrap()).next.unwrap()).next.is_none());
                assert!((*fsb.allocators.unwrap()).allocator.bottom() as usize > base as usize);
                assert_eq!(
                    (*fsb.allocators.unwrap()).allocator.free(),
                    //expected free: size + a page to hold allocator node - size of allocator node.
                    layout.pad_to_align().size() + 0x1000 - size_of::<AllocatorListNode>()
                );
            }
        });
    }

    #[test]
    fn test_allocation_iterator() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x800000);

            let mut fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);
            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            fsb.expand(layout).unwrap();
            fsb.expand(layout).unwrap();
            fsb.expand(layout).unwrap();
            fsb.expand(layout).unwrap();
            fsb.expand(layout).unwrap();

            assert_eq!(5, AllocatorIterator::new(fsb.allocators).count());
            assert!(AllocatorIterator::new(fsb.allocators)
                .all(|node| unsafe { (*node).allocator.free() == MIN_EXPANSION - size_of::<AllocatorListNode>() }));
        });
    }

    #[test]
    fn test_fallback_alloc() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let base = init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            let allocation = fsb.fallback_alloc(layout);
            assert!(fsb.allocators.is_some());
            assert!((allocation as u64) > base);
            assert!((allocation as u64) < base + 0x400000);
        });
    }

    #[test]
    fn test_alloc() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let base = init_gcd(&GCD, 0x400000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            let allocation = unsafe { fsb.alloc(layout) };
            assert!(fsb.lock().allocators.is_some());
            assert!((allocation as u64) > base);
            assert!((allocation as u64) < base + 0x400000);
        });
    }

    #[test]
    fn test_allocate() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let base = init_gcd(&GCD, 0x400000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            let allocation = fsb.allocate(layout).unwrap().as_ptr() as *mut u8;
            assert!(fsb.lock().allocators.is_some());
            assert!((allocation as u64) > base);
            assert!((allocation as u64) < base + 0x400000);
        });
    }

    #[test]
    fn test_fallback_dealloc() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = fsb.fallback_alloc(layout);

            fsb.fallback_dealloc(allocation, layout);
            unsafe {
                assert_eq!((*fsb.allocators.unwrap()).allocator.free(), MIN_EXPANSION - size_of::<AllocatorListNode>());
            }
        });
    }

    #[test]
    fn test_dealloc() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = unsafe { fsb.alloc(layout) };

            unsafe { fsb.dealloc(allocation, layout) };
            let free_block_ptr =
                fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap() as *mut BlockListNode as *mut u8;
            assert_eq!(free_block_ptr, allocation);

            let layout = Layout::from_size_align(0x20, 0x20).unwrap();
            let allocation = unsafe { fsb.alloc(layout) };

            unsafe { fsb.dealloc(allocation, layout) };
            let free_block_ptr =
                fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap() as *mut BlockListNode as *mut u8;
            assert_eq!(free_block_ptr, allocation);
        });
    }

    #[test]
    fn test_deallocate() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();
            let allocation_ptr = allocation.as_ptr();

            unsafe { fsb.deallocate(allocation, layout) };
            let free_block_ptr =
                fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap() as *mut BlockListNode as *mut u8;
            assert_eq!(free_block_ptr, allocation_ptr);

            let layout = Layout::from_size_align(0x20, 0x20).unwrap();
            let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();
            let allocation_ptr = allocation.as_ptr();

            unsafe { fsb.deallocate(allocation, layout) };
            let free_block_ptr =
                fsb.lock().list_heads[list_index(&layout).unwrap()].take().unwrap() as *mut BlockListNode as *mut u8;
            assert_eq!(free_block_ptr, allocation_ptr);
        });
    }

    #[test]
    fn test_contains() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();
            assert!(fsb.contains(allocation));
        });
    }

    #[test]
    fn test_allocate_pages() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to back the test GCD.
            let address = init_gcd(&GCD, 0x1000000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let pages = 4;

            let allocation =
                fsb.allocate_pages(gcd::AllocateType::BottomUp(None), pages, UEFI_PAGE_SIZE).unwrap().as_non_null_ptr();

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
    }

    #[test]
    fn test_allocate_at_address() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to back the test GCD.
            let address = init_gcd(&GCD, 0x1000000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
            let pages = 4;

            let allocation = fsb
                .allocate_pages(gcd::AllocateType::Address(target_address as usize), pages, UEFI_PAGE_SIZE)
                .unwrap()
                .as_non_null_ptr();

            assert_eq!(allocation.as_ptr() as u64, target_address);

            unsafe {
                fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
            };
        });
    }

    #[test]
    fn test_allocate_below_address() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be back the test GCD.
            let address = init_gcd(&GCD, 0x1000000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
            let pages = 4;

            let allocation = fsb
                .allocate_pages(gcd::AllocateType::BottomUp(Some(target_address as usize)), pages, UEFI_PAGE_SIZE)
                .unwrap()
                .as_non_null_ptr();
            assert!((allocation.as_ptr() as u64) < target_address);

            unsafe {
                fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
            };
        });
    }

    #[test]
    fn test_allocate_above_address() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to back the test GCD.
            let address = init_gcd(&GCD, 0x1000000);

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            let target_address = address + 0x400000 - 8 * (ALIGNMENT as u64);
            let pages = 4;

            let allocation = fsb
                .allocate_pages(gcd::AllocateType::TopDown(Some(target_address as usize)), pages, UEFI_PAGE_SIZE)
                .unwrap()
                .as_non_null_ptr();
            assert!((allocation.as_ptr() as u64) > target_address);

            unsafe {
                fsb.free_pages(allocation.as_ptr() as usize, pages).unwrap();
            };
        });
    }

    #[test]
    fn test_allocator_commands_with_invalid_parameters() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x400000);

            // Test commands with bad handle.
            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 0 as _, efi::BOOT_SERVICES_DATA, page_change_callback);
            match fsb.allocate_pages(AllocationStrategy::Address(0x1000), 5, UEFI_PAGE_SIZE) {
                Err(EfiError::InvalidParameter) => {}
                _ => panic!("Expected INVALID_PARAMETER"),
            }

            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

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
    fn validate_display_impl_does_not_panic() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);
            fsb.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 5, UEFI_PAGE_SIZE).unwrap();

            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            fsb.expand(layout).unwrap();

            let _ = std::format!("{}", fsb);
        });
    }

    #[test]
    fn test_allocation_stats() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let _ = init_gcd(&GCD, 0x1000000);

            // Make a fixed-sized-block allocator
            let fsb =
                SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

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
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, 0);
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 2));

            //test alloc/deallocate and stats within the bucket
            let ptr = unsafe { fsb.alloc(Layout::from_size_align(0x100, 0x8).unwrap()) };

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 1);
            assert_eq!(stats.pool_free_calls, 0);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 2));

            unsafe {
                fsb.dealloc(ptr, Layout::from_size_align(0x100, 0x8).unwrap());
            }

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 1);
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 2));

            //test alloc/deallocate and stats blowing the bucket
            let ptr = unsafe { fsb.alloc(Layout::from_size_align(MIN_EXPANSION * 3, 0x8).unwrap()) };

            //after this allocate, the basic memory map of the FSB should look like:
            //1MB range as a result of previous pool allocation expand - available for pool allocation.
            //    Claims first 1MB of 2MB reserved region.
            //1MB free but owned by the allocator (not pool) as a result of 2MB reservation.
            //3MB+1 page range as a result of 3MB allocation + 1 page to hold allocator node.

            let stats = fsb.stats();
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 1);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
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
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 1);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
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
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 2);
            assert_eq!(stats.page_free_calls, 1);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(4));
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
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 2);
            assert_eq!(stats.page_free_calls, 2);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
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
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 3);
            assert_eq!(stats.page_free_calls, 2);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
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
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 4);
            assert_eq!(stats.page_free_calls, 2);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION + uefi_pages_to_size!(4));
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
            assert_eq!(stats.pool_allocation_calls, 2);
            assert_eq!(stats.pool_free_calls, 2);
            assert_eq!(stats.page_allocation_calls, 4);
            assert_eq!(stats.page_free_calls, 4);
            assert_eq!(stats.reserved_size, MIN_EXPANSION * 2);
            assert_eq!(stats.reserved_used, MIN_EXPANSION);
            assert_eq!(stats.claimed_pages, uefi_size_to_pages!(MIN_EXPANSION * 5) + 1);
        });
    }

    #[test]
    fn test_get_memory_ranges() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            GCD.init(48, 16);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            let base = init_gcd(&GCD, 0x400000);

            let mut fsb = FixedSizeBlockAllocator::new(&GCD, 1 as _, efi::BOOT_SERVICES_DATA, page_change_callback);

            // Expand the allocator multiple times to add memory ranges
            let layout = Layout::from_size_align(0x1000, 0x10).unwrap();
            fsb.expand(layout).unwrap();
            fsb.expand(layout).unwrap();
            fsb.expand(layout).unwrap();

            // Collect the memory ranges reported by the allocator
            let memory_ranges: Vec<_> = fsb.get_memory_ranges().collect();

            // Verify that the reported ranges match the expected ranges
            assert_eq!(memory_ranges.len(), 3);
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
    }

    #[test]
    fn test_alignment_page_to_shift() {
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
            assert_eq!(result, config.expected, "Test config: {:?}", config);
        }
    }
}
