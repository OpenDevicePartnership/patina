//! Locked View Over the Page Allocator State
//!
//! The page allocator keeps its bookkeeping in SMRAM rather than in statics, reached through a
//! raw pointer behind a mutex. This module owns that representation: the header layout
//! ([`AllocatorState`]), the pointer wrapper that makes it `Send` ([`StatePtr`]), and the guard
//! ([`LockedState`]) that hands out the region and bitmap slices.
//!
//! Keeping it separate from the allocator means the unsafe slice construction lives in one place,
//! and the borrow checker enforces that shared and mutable views never overlap.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{mem::size_of, ops::Range, slice};

use patina::{UEFI_PAGE_SIZE, uefi_pages_to_size};
use spin::{MutexGuard, relax::Spin};

use super::mmram::MmramRegion;
use crate::error::MmSupervisorResult;
use crate::memory::AllocError;
use crate::memory::mmram::{MmramPlacement, classify_coverage};
use crate::memory::page_allocator::{AllocationType, RegionInfo};

/// Bits per byte.
pub(crate) const BITS_PER_BYTE: usize = 8;

/// Converts a byte size to a page count without truncation or overflow.
pub(crate) fn page_count(size: u64) -> Option<usize> {
    usize::try_from(size).ok().map(|size| size.div_ceil(UEFI_PAGE_SIZE))
}

/// Internal state for the page allocator, stored in bookkeeping pages.
#[repr(C)]
pub(crate) struct AllocatorState {
    /// Number of regions.
    pub(crate) region_count: usize,
    /// Total number of pages across all regions.
    pub(crate) total_pages: usize,
    /// Number of pages used for bookkeeping.
    pub(crate) bookkeeping_pages: usize,
    /// Base address of bookkeeping memory.
    pub(crate) bookkeeping_base: u64,
    // Followed by:
    // - RegionInfo array (region_count entries)
    // - Allocation bitmap (total_pages bits, rounded up to bytes)
    // - Type bitmap (total_pages bits, rounded up to bytes)
}

/// Raw pointer to the allocator state living in SMRAM.
pub(crate) struct StatePtr(pub(crate) *mut AllocatorState);

// SAFETY: The content lives in SMRAM for the lifetime of the program and is only
// ever dereferenced while the enclosing `Mutex<StatePtr>` is held, which
// serializes all access to it.
unsafe impl Send for StatePtr {}

/// A lock-held view over the allocator state.
///
/// Obtaining a `LockedState` requires holding the state mutex (it owns the
/// guard), so every method is guaranteed exclusive access to the SMRAM
/// bookkeeping. Because the region/bitmap slices borrow from `&self` / `&mut
/// self`, the borrow checker - not convention - prevents mutable and shared
/// views from overlapping.
pub(crate) struct LockedState<'a> {
    /// State pointer copied out of the guard. Null until the allocator is initialized.
    pub(crate) state: *mut AllocatorState,
    /// Number of SMRAM regions, cached from the header at construction (0 if uninitialized).
    pub(crate) region_count: usize,
    /// Total pages across all regions, cached from the header at construction (0 if uninitialized).
    pub(crate) total_pages: usize,
    /// Held for the lifetime of this view to keep the lock acquired.
    pub(crate) guard: MutexGuard<'a, StatePtr, Spin>,
}

impl LockedState<'_> {
    /// Number of SMRAM regions, or 0 if the allocator is uninitialized.
    pub(crate) fn region_count(&self) -> usize {
        self.region_count
    }

    /// Total number of pages across all regions, or 0 if uninitialized.
    pub(crate) fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Region metadata array.
    pub(crate) fn regions(&self) -> &[RegionInfo] {
        if self.state.is_null() {
            return &[];
        }

        let regions_ptr = (self.state as *const u8).wrapping_add(size_of::<AllocatorState>()).cast::<RegionInfo>();
        // SAFETY: `regions_ptr` points to the `RegionInfo` array of `region_count` entries that
        // immediately follows the header within the bookkeeping allocation; we hold the lock so
        // no other reference is live.
        unsafe { slice::from_raw_parts(regions_ptr, self.region_count) }
    }

    /// Mutable region metadata array.
    pub(crate) fn regions_mut(&mut self) -> &mut [RegionInfo] {
        if self.state.is_null() {
            return &mut [];
        }

        let regions_ptr = self.state.cast::<u8>().wrapping_add(size_of::<AllocatorState>()).cast::<RegionInfo>();
        // SAFETY: as `regions`, plus we hold `&mut self` and the lock, so this is the only live
        // reference to the array.
        unsafe { slice::from_raw_parts_mut(regions_ptr, self.region_count) }
    }

    /// Byte offset of the allocation bitmap and its length in bytes.
    pub(crate) fn bitmap_offset_len(&self) -> (usize, usize) {
        let offset = size_of::<AllocatorState>() + self.region_count * size_of::<RegionInfo>();
        let bitmap_bytes = self.total_pages.div_ceil(BITS_PER_BYTE);
        (offset, bitmap_bytes)
    }

    /// Allocation bitmap (1 bit per page; set == allocated).
    pub(crate) fn alloc_bitmap(&self) -> &[u8] {
        if self.state.is_null() {
            return &[];
        }
        let (offset, bitmap_bytes) = self.bitmap_offset_len();
        let ptr = (self.state as *const u8).wrapping_add(offset);
        // SAFETY: the allocation bitmap occupies `bitmap_bytes` bytes at `offset` within the
        // bookkeeping allocation; we hold the lock.
        unsafe { slice::from_raw_parts(ptr, bitmap_bytes) }
    }

    /// Type bitmap (1 bit per page; set == user, clear == supervisor).
    pub(crate) fn type_bitmap(&self) -> &[u8] {
        if self.state.is_null() {
            return &[];
        }
        let (offset, bitmap_bytes) = self.bitmap_offset_len();
        let ptr = (self.state as *const u8).wrapping_add(offset + bitmap_bytes);
        // SAFETY: the type bitmap occupies `bitmap_bytes` bytes immediately after the allocation
        // bitmap; we hold the lock.
        unsafe { slice::from_raw_parts(ptr, bitmap_bytes) }
    }

    /// Mutable access to both bitmaps at once, returned as `(allocation, type)`.
    pub(crate) fn bitmaps_mut(&mut self) -> (&mut [u8], &mut [u8]) {
        if self.state.is_null() {
            return (&mut [], &mut []);
        }
        let (offset, bitmap_bytes) = self.bitmap_offset_len();
        let ptr = self.state.cast::<u8>().wrapping_add(offset);
        // SAFETY: the allocation and type bitmaps are two contiguous `bitmap_bytes`-sized ranges
        // within the bookkeeping allocation. Materialize both as one slice; we hold `&mut self`
        // and the lock, so no other reference exists.
        let both = unsafe { slice::from_raw_parts_mut(ptr, bitmap_bytes * 2) };
        // Safe split into the two disjoint halves.
        both.split_at_mut(bitmap_bytes)
    }

    /// Returns whether the page at `bit_index` is allocated.
    pub(crate) fn is_bit_allocated(&self, bit_index: usize) -> bool {
        let bitmap = self.alloc_bitmap();
        let byte_index = bit_index / BITS_PER_BYTE;
        let bit_offset = bit_index % BITS_PER_BYTE;
        // Out of bounds is treated as allocated.
        match bitmap.get(byte_index) {
            Some(byte) => (byte & (1 << bit_offset)) != 0,
            None => panic!(
                "{}: bit_index {} out of bounds (total_pages = {})",
                patina::function!(),
                bit_index,
                self.total_pages
            ),
        }
    }

    /// Returns the allocation type recorded for `bit_index`.
    pub(crate) fn bit_type(&self, bit_index: usize) -> AllocationType {
        let bitmap = self.type_bitmap();
        let byte_index = bit_index / BITS_PER_BYTE;
        let bit_offset = bit_index % BITS_PER_BYTE;
        // Out of bounds (or a clear bit) is treated as supervisor-owned.
        match bitmap.get(byte_index) {
            None => panic!(
                "{}: bit_index {} out of bounds (total_pages = {})",
                patina::function!(),
                bit_index,
                self.total_pages
            ),
            Some(byte) if (byte & (1 << bit_offset)) != 0 => AllocationType::User,
            _ => AllocationType::Supervisor,
        }
    }

    /// Marks `bit_index` as allocated with the given type.
    pub(crate) fn set_bit_allocated(&mut self, bit_index: usize, alloc_type: AllocationType) {
        let byte_index = bit_index / BITS_PER_BYTE;
        let bit_offset = bit_index % BITS_PER_BYTE;
        let (alloc_bitmap, type_bitmap) = self.bitmaps_mut();
        // The two bitmaps are the same length, so a hit in one is a hit in the other.
        if let (Some(alloc_byte), Some(type_byte)) = (alloc_bitmap.get_mut(byte_index), type_bitmap.get_mut(byte_index))
        {
            *alloc_byte |= 1 << bit_offset;
            match alloc_type {
                AllocationType::User => *type_byte |= 1 << bit_offset,
                AllocationType::Supervisor => *type_byte &= !(1 << bit_offset),
            }
        }
    }

    /// Marks `bit_index` as free.
    pub(crate) fn set_bit_free(&mut self, bit_index: usize) {
        let byte_index = bit_index / BITS_PER_BYTE;
        let bit_offset = bit_index % BITS_PER_BYTE;
        let (alloc_bitmap, type_bitmap) = self.bitmaps_mut();
        if let (Some(alloc_byte), Some(type_byte)) = (alloc_bitmap.get_mut(byte_index), type_bitmap.get_mut(byte_index))
        {
            *alloc_byte &= !(1 << bit_offset);
            *type_byte &= !(1 << bit_offset);
        }
    }

    /// Finds which region contains `addr`, returning `(region_index, page_in_region)`.
    pub(crate) fn find_region_for_address(&self, addr: u64) -> Option<(usize, usize)> {
        for (i, region) in self.regions().iter().enumerate() {
            let region_size = u64::try_from(region.total_pages.checked_mul(UEFI_PAGE_SIZE)?).ok()?;
            let region_end = region.base.checked_add(region_size)?;
            if addr >= region.base && addr < region_end {
                let page_in_region = usize::try_from((addr - region.base) / UEFI_PAGE_SIZE as u64).ok()?;
                return Some((i, page_in_region));
            }
        }
        None
    }

    /// Converts a region index and page-in-region to a global bit index.
    pub(crate) fn region_page_to_bit(&self, region_index: usize, page_in_region: usize) -> usize {
        self.regions().get(region_index).map_or(0, |region| region.bitmap_start_bit + page_in_region)
    }

    /// First-fit search for `num_pages` contiguous free pages, marking them
    /// allocated with `alloc_type`. Returns the base address on success.
    pub(crate) fn allocate(&mut self, num_pages: usize, alloc_type: AllocationType) -> Option<u64> {
        let mut found: Option<(u64, usize)> = None; // (addr, first global bit)
        'outer: for region_index in 0..self.region_count() {
            // `RegionInfo` is `Copy`, so this releases the `regions()` borrow immediately.
            let Some(region) = self.regions().get(region_index).copied() else {
                continue;
            };

            // First-fit search for contiguous pages within this region.
            let mut run_start = 0usize;
            let mut run_length = 0usize;
            for page_in_region in 0..region.total_pages {
                if self.is_bit_allocated(region.bitmap_start_bit + page_in_region) {
                    run_start = page_in_region + 1;
                    run_length = 0;
                } else {
                    run_length += 1;
                    if run_length == num_pages {
                        let addr = region.base + uefi_pages_to_size!(run_start) as u64;
                        found = Some((addr, region.bitmap_start_bit + run_start));
                        break 'outer;
                    }
                }
            }
        }

        let (addr, first_bit) = found?;
        for p in 0..num_pages {
            self.set_bit_allocated(first_bit + p, alloc_type);
        }
        log::trace!("Allocated {num_pages} {alloc_type:?} page(s) at 0x{addr:016x}");
        Some(addr)
    }

    /// Returns the bit range covering `[addr, addr + num_pages)`, requiring every page in it to be
    /// allocated.
    ///
    /// Separate from [`mark_free`](Self::mark_free) so a caller can keep the range allocated while
    /// it scrubs and unmaps, and abandon the free if either step fails.
    pub(crate) fn verify_allocated(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<Range<usize>> {
        let bit_range = self.allocation_bit_range(addr, num_pages)?;

        // Verify all pages are allocated
        for (page_offset, bit) in bit_range.clone().enumerate() {
            if !self.is_bit_allocated(bit) {
                log::error!(
                    "Cannot free 0x{:016x}: page is not allocated",
                    addr + uefi_pages_to_size!(page_offset) as u64
                );
                return Err(AllocError::NotAllocated.into());
            }
        }

        Ok(bit_range)
    }

    /// Publishes `bit_range` as free and therefore reusable.
    pub(crate) fn mark_free(&mut self, bit_range: Range<usize>) {
        for bit in bit_range {
            self.set_bit_free(bit);
        }
    }

    /// Returns the bit range covering `[addr, addr + num_pages)`, requiring every page in it to be
    /// allocated with `expected_type`.
    ///
    /// The type-checked counterpart to [`verify_allocated`](Self::verify_allocated), and split
    /// from [`mark_free`](Self::mark_free) for the same reason.
    pub(crate) fn verify_allocated_with_type(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: AllocationType,
    ) -> MmSupervisorResult<Range<usize>> {
        let bit_range = self.allocation_bit_range(addr, num_pages)?;

        for (page_offset, bit) in bit_range.clone().enumerate() {
            if !self.is_bit_allocated(bit) {
                log::error!(
                    "Cannot free 0x{:016x}: page is not allocated",
                    addr + uefi_pages_to_size!(page_offset) as u64
                );
                return Err(AllocError::NotAllocated.into());
            }
            if self.bit_type(bit) != expected_type {
                log::error!(
                    "Cannot free 0x{:016x}: expected {:?}, got {:?}",
                    addr + uefi_pages_to_size!(page_offset) as u64,
                    expected_type,
                    self.bit_type(bit)
                );
                return Err(AllocError::InvalidAddress.into());
            }
        }

        Ok(bit_range)
    }

    /// Returns the global bitmap range for a page-aligned allocation range.
    pub(crate) fn allocation_bit_range(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<Range<usize>> {
        if num_pages == 0 {
            log::error!("0x{addr:016x}: page count is 0");
            return Err(AllocError::InvalidAddress.into());
        }

        let Some((region_index, page_in_region)) = self.find_region_for_address(addr) else {
            log::error!("0x{addr:016x} is not inside any known MMRAM region");
            return Err(AllocError::InvalidAddress.into());
        };
        let Some(region) = self.regions().get(region_index) else {
            log::error!("0x{addr:016x} resolved to region {region_index}, which is out of range");
            return Err(AllocError::InvalidAddress.into());
        };
        let Some(end_page) = page_in_region.checked_add(num_pages) else {
            log::error!("0x{addr:016x} plus {num_pages} page(s) overflows the region page index");
            return Err(AllocError::InvalidAddress.into());
        };
        if end_page > region.total_pages {
            log::error!(
                "0x{addr:016x} plus {num_pages} page(s) runs past the end of region {region_index} ({} pages)",
                region.total_pages
            );
            return Err(AllocError::InvalidAddress.into());
        }

        let Some(first_bit) = region.bitmap_start_bit.checked_add(page_in_region) else {
            log::error!("0x{addr:016x} overflows the allocation bitmap start bit");
            return Err(AllocError::InvalidAddress.into());
        };
        let Some(end_bit) = first_bit.checked_add(num_pages) else {
            log::error!("0x{addr:016x} plus {num_pages} page(s) overflows the allocation bitmap");
            return Err(AllocError::InvalidAddress.into());
        };
        if end_bit > self.total_pages {
            log::error!(
                "0x{addr:016x} plus {num_pages} page(s) runs past the end of the allocation bitmap ({} pages)",
                self.total_pages
            );
            return Err(AllocError::InvalidAddress.into());
        }

        Ok(first_bit..end_bit)
    }

    /// Counts free pages across all regions.
    pub(crate) fn free_page_count(&self) -> usize {
        (0..self.total_pages()).filter(|&bit| !self.is_bit_allocated(bit)).count()
    }

    /// Counts pages allocated with the given type.
    pub(crate) fn allocated_page_count(&self, alloc_type: AllocationType) -> usize {
        (0..self.total_pages()).filter(|&bit| self.is_bit_allocated(bit) && self.bit_type(bit) == alloc_type).count()
    }

    /// Returns the allocation type for `addr`, or `None` if not allocated.
    pub(crate) fn allocation_type(&self, addr: u64) -> Option<AllocationType> {
        let (region_index, page_in_region) = self.find_region_for_address(addr)?;
        let bit = self.region_page_to_bit(region_index, page_in_region);
        if self.is_bit_allocated(bit) { Some(self.bit_type(bit)) } else { None }
    }

    /// Returns whether `[addr, addr + size)` lies entirely within a single region.
    pub(crate) fn is_region_inside_mmram(&self, addr: u64, size: u64) -> bool {
        let Some(request_end) = addr.checked_add(size) else {
            return false;
        };

        self.regions().iter().any(|region| {
            let Some(region_size) =
                region.total_pages.checked_mul(UEFI_PAGE_SIZE).and_then(|size| u64::try_from(size).ok())
            else {
                return false;
            };
            let Some(region_end) = region.base.checked_add(region_size) else {
                return false;
            };
            addr >= region.base && request_end <= region_end
        })
    }

    /// Classifies `[addr, addr + size)` against the committed MMRAM regions.
    pub(crate) fn classify_mmram(&self, addr: u64, size: u64) -> MmramPlacement {
        classify_coverage(
            addr,
            size,
            self.regions().iter().filter_map(|region| {
                let size = u64::try_from(region.total_pages.checked_mul(UEFI_PAGE_SIZE)?).ok()?;
                Some((region.base, region.base.checked_add(size)?))
            }),
        )
    }

    /// Populates freshly-zeroed bookkeeping with per-region metadata and marks
    /// the pre-allocated regions and the bookkeeping pages themselves as
    /// supervisor-allocated.
    ///
    /// The `AllocatorState` header (including `region_count`) must already be
    /// written so that [`regions_mut`](Self::regions_mut) exposes the full array.
    pub(crate) fn initialize(
        &mut self,
        scanned: &[MmramRegion],
        bookkeeping_base: u64,
        bookkeeping_pages: usize,
    ) -> MmSupervisorResult<()> {
        // Fill in per-region metadata and assign each region its bitmap range.
        let mut bitmap_start_bit = 0usize;
        for (region, scanned_region) in self.regions_mut().iter_mut().zip(scanned.iter()) {
            let pages = page_count(scanned_region.size).ok_or_else(|| {
                log::error!(
                    "Allocator init failed: region 0x{:016x} size 0x{:x} does not fit a page count",
                    scanned_region.base,
                    scanned_region.size
                );
                AllocError::OutOfMemory
            })?;
            region.base = scanned_region.base;
            region.total_pages = pages;
            region.bitmap_start_bit = bitmap_start_bit;
            bitmap_start_bit = bitmap_start_bit.checked_add(pages).ok_or_else(|| {
                log::error!("Allocator init failed: bitmap start bit overflows at 0x{:016x}", scanned_region.base);
                AllocError::OutOfMemory
            })?;
        }

        // Mark pre-allocated regions and the bookkeeping pages as allocated (supervisor).
        for (i, scanned_region) in scanned.iter().enumerate() {
            let pages = page_count(scanned_region.size).ok_or_else(|| {
                log::error!(
                    "Allocator init failed: region 0x{:016x} size 0x{:x} does not fit a page count",
                    scanned_region.base,
                    scanned_region.size
                );
                AllocError::OutOfMemory
            })?;
            let Some(start_bit) = self.regions().get(i).map(|region| region.bitmap_start_bit) else {
                continue;
            };

            if scanned_region.pre_allocated {
                // Mark the entire region as allocated.
                for p in 0..pages {
                    self.set_bit_allocated(start_bit + p, AllocationType::Supervisor);
                }
            } else if scanned_region.base == bookkeeping_base {
                // Mark just the bookkeeping pages at the start of this region.
                for p in 0..bookkeeping_pages {
                    self.set_bit_allocated(start_bit + p, AllocationType::Supervisor);
                }
            }
        }

        Ok(())
    }
}
