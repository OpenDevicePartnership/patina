//! Locked View Over the Page Allocator State
//!
//! [`LockedState`] holds the page allocator's mutex and exposes the bookkeeping it guards: the
//! region table and the allocation and type bitmaps.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{mem::size_of, ops::Range, slice};

use patina::{MemoryRange, UEFI_PAGE_SIZE, uefi_pages_to_size};
use spin::{MutexGuard, relax::Spin};

use super::mmram::MmramRegion;
use crate::error::MmSupervisorResult;
use crate::memory::AllocError;
use crate::memory::mmram::MmramPlacement;
use crate::memory::page_allocator::{AllocationType, RegionInfo};

/// Internal state for the page allocator, stored in bookkeeping pages.
#[repr(C)]
pub(crate) struct AllocatorState {
    /// Number of regions.
    pub(crate) region_count: usize,
    /// Total number of pages across all regions.
    pub(crate) total_pages: usize,
    // Followed by:
    // - RegionInfo array (region_count entries)
    // - Allocation bitmap (total_pages bits, rounded up to bytes)
    // - Type bitmap (total_pages bits, rounded up to bytes)
}

/// Raw pointer to the allocator state living in SMRAM.
pub(crate) struct StatePtr(pub(crate) *mut AllocatorState);

// SAFETY: the state lives in SMRAM for the program's lifetime and is only dereferenced while the
// enclosing `Mutex<StatePtr>` is held.
unsafe impl Send for StatePtr {}

/// A view over the allocator state that owns the state mutex guard.
///
/// The region and bitmap slices borrow from `self`, so the borrow checker keeps mutable and shared
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
        // immediately follows the header within the bookkeeping allocation. The lock is held, so
        // no other reference is live.
        unsafe { slice::from_raw_parts(regions_ptr, self.region_count) }
    }

    /// Mutable region metadata array.
    pub(crate) fn regions_mut(&mut self) -> &mut [RegionInfo] {
        if self.state.is_null() {
            return &mut [];
        }

        let regions_ptr = self.state.cast::<u8>().wrapping_add(size_of::<AllocatorState>()).cast::<RegionInfo>();
        // SAFETY: as `regions`, and `&mut self` plus the lock make this the only live reference
        // to the array.
        unsafe { slice::from_raw_parts_mut(regions_ptr, self.region_count) }
    }

    /// Byte offset of the allocation bitmap and its length in bytes.
    pub(crate) fn bitmap_offset_len(&self) -> (usize, usize) {
        let offset = size_of::<AllocatorState>() + self.region_count * size_of::<RegionInfo>();
        let bitmap_bytes = self.total_pages.div_ceil(u8::BITS as usize);
        (offset, bitmap_bytes)
    }

    /// Allocation bitmap, one bit per page. A set bit means allocated.
    pub(crate) fn alloc_bitmap(&self) -> &[u8] {
        if self.state.is_null() {
            return &[];
        }
        let (offset, bitmap_bytes) = self.bitmap_offset_len();
        let ptr = (self.state as *const u8).wrapping_add(offset);
        // SAFETY: the allocation bitmap occupies `bitmap_bytes` bytes at `offset` within the
        // bookkeeping allocation, and the lock is held.
        unsafe { slice::from_raw_parts(ptr, bitmap_bytes) }
    }

    /// Type bitmap, one bit per page. A set bit means user, a clear bit supervisor.
    pub(crate) fn type_bitmap(&self) -> &[u8] {
        if self.state.is_null() {
            return &[];
        }
        let (offset, bitmap_bytes) = self.bitmap_offset_len();
        let ptr = (self.state as *const u8).wrapping_add(offset + bitmap_bytes);
        // SAFETY: the type bitmap occupies `bitmap_bytes` bytes immediately after the allocation
        // bitmap, and the lock is held.
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
        // within the bookkeeping allocation, taken here as one slice. `&mut self` and the lock
        // mean no other reference exists.
        let both = unsafe { slice::from_raw_parts_mut(ptr, bitmap_bytes * 2) };
        both.split_at_mut(bitmap_bytes)
    }

    /// Returns whether the page at `bit_index` is allocated.
    ///
    /// ## Panics
    ///
    /// If `bit_index` is past the end of the bitmap.
    pub(crate) fn is_bit_allocated(&self, bit_index: usize) -> bool {
        let bitmap = self.alloc_bitmap();
        let byte_index = bit_index / (u8::BITS as usize);
        let bit_offset = bit_index % (u8::BITS as usize);
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
    ///
    /// ## Panics
    ///
    /// If `bit_index` is past the end of the bitmap.
    pub(crate) fn bit_type(&self, bit_index: usize) -> AllocationType {
        let bitmap = self.type_bitmap();
        let byte_index = bit_index / (u8::BITS as usize);
        let bit_offset = bit_index % (u8::BITS as usize);
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

    /// Marks `bit_index` as allocated with the given type. Out of range indices are ignored.
    pub(crate) fn set_bit_allocated(&mut self, bit_index: usize, alloc_type: AllocationType) {
        let byte_index = bit_index / (u8::BITS as usize);
        let bit_offset = bit_index % (u8::BITS as usize);
        let (alloc_bitmap, type_bitmap) = self.bitmaps_mut();
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
        let byte_index = bit_index / (u8::BITS as usize);
        let bit_offset = bit_index % (u8::BITS as usize);
        let (alloc_bitmap, type_bitmap) = self.bitmaps_mut();
        if let (Some(alloc_byte), Some(type_byte)) = (alloc_bitmap.get_mut(byte_index), type_bitmap.get_mut(byte_index))
        {
            *alloc_byte &= !(1 << bit_offset);
            *type_byte &= !(1 << bit_offset);
        }
    }

    /// Finds which region contains `addr`, returning `(region_index, page_in_region)`.
    pub(crate) fn find_region_for_address(&self, addr: u64) -> Option<(usize, usize)> {
        for (index, region) in self.regions().iter().enumerate() {
            let range = region.range()?;
            if range.contains(addr) {
                let page_in_region = usize::try_from((addr - range.base()) / UEFI_PAGE_SIZE as u64).ok()?;
                return Some((index, page_in_region));
            }
        }
        None
    }

    /// Converts a region index and page-in-region to a global bit index, or `None` if there is no
    /// region at `region_index`.
    pub(crate) fn region_page_to_bit(&self, region_index: usize, page_in_region: usize) -> Option<usize> {
        self.regions().get(region_index).map(|region| region.bitmap_start_bit + page_in_region)
    }

    /// Finds the first run of `num_pages` free pages within one region, marks it allocated with
    /// `alloc_type` and returns its base address.
    pub(crate) fn allocate(&mut self, num_pages: usize, alloc_type: AllocationType) -> Option<u64> {
        let mut found: Option<(u64, usize)> = None; // (addr, first global bit)
        'outer: for region_index in 0..self.region_count() {
            // `RegionInfo` is `Copy`, so this releases the `regions()` borrow immediately.
            let Some(region) = self.regions().get(region_index).copied() else {
                continue;
            };

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
        for page in 0..num_pages {
            self.set_bit_allocated(first_bit + page, alloc_type);
        }
        log::trace!("Allocated {num_pages} {alloc_type:?} page(s) at 0x{addr:016x}");
        Some(addr)
    }

    /// Returns the bit range covering `[addr, addr + num_pages)`.
    ///
    /// Checks:
    /// - The range is valid for the allocator (see [`allocation_bit_range`](Self::allocation_bit_range)).
    /// - Every page in it is allocated.
    ///
    /// Separate from [`mark_free`](Self::mark_free) so a caller can keep the range allocated while
    /// it scrubs and unmaps, and abandon the free if either step fails.
    pub(crate) fn verify_allocated(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<Range<usize>> {
        let bit_range = self.allocation_bit_range(addr, num_pages)?;

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

    /// Returns the bit range covering `[addr, addr + num_pages)`.
    ///
    /// Checks:
    /// - The range is valid for the allocator (see [`allocation_bit_range`](Self::allocation_bit_range)).
    /// - Every page in it is allocated.
    /// - Every page in it has `expected_type`.
    ///
    /// Split from [`mark_free`](Self::mark_free) for the same reason as
    /// [`verify_allocated`](Self::verify_allocated).
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
                return Err(AllocError::TypeMismatch.into());
            }
        }

        Ok(bit_range)
    }

    /// Returns the bitmap range for `num_pages` pages at `addr`.
    ///
    /// Checks:
    /// - `num_pages` is not zero.
    /// - The whole range lies in one MMRAM region and inside the bitmap.
    pub(crate) fn allocation_bit_range(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<Range<usize>> {
        if num_pages == 0 {
            log::error!("0x{addr:016x}: page count is 0");
            return Err(AllocError::InvalidSize.into());
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
        let bit = self.region_page_to_bit(region_index, page_in_region)?;
        if self.is_bit_allocated(bit) { Some(self.bit_type(bit)) } else { None }
    }

    /// Classifies `range` against the committed MMRAM regions.
    pub(crate) fn classify_mmram(&self, range: &MemoryRange) -> MmramPlacement {
        range.coverage(self.regions().iter().filter_map(RegionInfo::range)).into()
    }

    /// Fills zeroed bookkeeping with the region metadata and marks the pre-allocated regions and
    /// the bookkeeping pages as supervisor pages.
    ///
    /// The `AllocatorState` header must already be written so that
    /// [`regions_mut`](Self::regions_mut) exposes the full array.
    pub(crate) fn initialize(
        &mut self,
        scanned: &[MmramRegion],
        bookkeeping_base: u64,
        bookkeeping_pages: usize,
    ) -> MmSupervisorResult<()> {
        let mut bitmap_start_bit = 0usize;
        for (region, scanned_region) in self.regions_mut().iter_mut().zip(scanned.iter()) {
            let pages = usize::try_from(scanned_region.range.pages()).map_err(|_| {
                log::error!("Allocator init failed: region {} does not fit a page count", scanned_region.range);
                AllocError::OutOfMemory
            })?;
            region.base = scanned_region.range.base();
            region.total_pages = pages;
            region.bitmap_start_bit = bitmap_start_bit;
            bitmap_start_bit = bitmap_start_bit.checked_add(pages).ok_or_else(|| {
                log::error!("Allocator init failed: bitmap start bit overflows at {}", scanned_region.range);
                AllocError::OutOfMemory
            })?;
        }

        for (index, scanned_region) in scanned.iter().enumerate() {
            let pages = usize::try_from(scanned_region.range.pages()).map_err(|_| {
                log::error!("Allocator init failed: region {} does not fit a page count", scanned_region.range);
                AllocError::OutOfMemory
            })?;
            let Some(start_bit) = self.regions().get(index).map(|region| region.bitmap_start_bit) else {
                continue;
            };

            if scanned_region.pre_allocated {
                for page in 0..pages {
                    self.set_bit_allocated(start_bit + page, AllocationType::Supervisor);
                }
            } else if scanned_region.range.base() == bookkeeping_base {
                // The bookkeeping pages sit at the start of this region.
                for page in 0..bookkeeping_pages {
                    self.set_bit_allocated(start_bit + page, AllocationType::Supervisor);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use spin::Mutex;

    const PAGE: u64 = UEFI_PAGE_SIZE as u64;

    /// Zeroed bookkeeping for `region_count` regions and `total_pages` pages, laid out as the page
    /// allocator lays it out. No memory backs the regions it describes, so only the bookkeeping is
    /// ever written.
    struct Bookkeeping {
        words: Vec<u64>,
        region_count: usize,
        total_pages: usize,
    }

    impl Bookkeeping {
        fn new(region_count: usize, total_pages: usize) -> Self {
            let bytes = size_of::<AllocatorState>()
                + region_count * size_of::<RegionInfo>()
                + 2 * total_pages.div_ceil(u8::BITS as usize);
            Self { words: vec![0; bytes.div_ceil(size_of::<u64>())], region_count, total_pages }
        }

        /// Runs `check` on a locked view over the bookkeeping.
        fn with_locked<R>(&mut self, check: impl FnOnce(&mut LockedState<'_>) -> R) -> R {
            let state = self.words.as_mut_ptr().cast::<AllocatorState>();
            let mutex = Mutex::new(StatePtr(state));
            let mut locked = LockedState {
                state,
                region_count: self.region_count,
                total_pages: self.total_pages,
                guard: mutex.lock(),
            };
            check(&mut locked)
        }
    }

    fn region(base: u64, pages: u64, pre_allocated: bool) -> MmramRegion {
        MmramRegion::new(MemoryRange::from_pages(base, pages).unwrap(), pre_allocated)
    }

    #[test]
    fn test_locked_state_rejects_a_bit_range_past_the_bitmap() {
        // `initialize` keeps the region table and the header consistent. These checks keep a region
        // table that disagrees with the header from indexing outside the bitmap.
        let mut bookkeeping = Bookkeeping::new(1, 4);
        bookkeeping.with_locked(|state| {
            let mut check = |bitmap_start_bit: usize, addr: u64, num_pages: usize| {
                state.regions_mut()[0] = RegionInfo { base: 0x10_0000, total_pages: 4, bitmap_start_bit };
                state.allocation_bit_range(addr, num_pages)
            };

            // The first bit, the end bit, or the end of the bitmap overflows or runs past it.
            assert_eq!(check(usize::MAX, 0x10_0000 + PAGE, 1), Err(AllocError::InvalidAddress.into()));
            assert_eq!(check(usize::MAX - 1, 0x10_0000, 2), Err(AllocError::InvalidAddress.into()));
            assert_eq!(check(2, 0x10_0000, 4), Err(AllocError::InvalidAddress.into()));

            // The same region fits when the range ends exactly at the end of the bitmap.
            assert_eq!(check(2, 0x10_0000, 2), Ok(2..4));
        });
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn test_locked_state_allocation_bit_past_the_bitmap_panics() {
        let mut bookkeeping = Bookkeeping::new(1, 4);
        // The bitmap holds one byte, so bit 8 is the first one past it.
        bookkeeping.with_locked(|state| state.is_bit_allocated(8));
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn test_locked_state_type_bit_past_the_bitmap_panics() {
        let mut bookkeeping = Bookkeeping::new(1, 4);
        bookkeeping.with_locked(|state| state.bit_type(8));
    }

    #[test]
    fn test_locked_state_set_and_clear_ignore_bits_past_the_bitmap() {
        let mut bookkeeping = Bookkeeping::new(1, 8);
        bookkeeping.with_locked(|state| {
            // Bit 8 of the allocation bitmap would be the first bit of the type bitmap.
            state.set_bit_allocated(8, AllocationType::User);
            assert_eq!((state.alloc_bitmap(), state.type_bitmap()), (&[0u8][..], &[0u8][..]));

            state.set_bit_allocated(7, AllocationType::User);
            assert_eq!((state.alloc_bitmap(), state.type_bitmap()), (&[0x80u8][..], &[0x80u8][..]));
            state.set_bit_free(8);
            assert_eq!((state.alloc_bitmap(), state.type_bitmap()), (&[0x80u8][..], &[0x80u8][..]));
            state.set_bit_free(7);
            assert_eq!((state.alloc_bitmap(), state.type_bitmap()), (&[0u8][..], &[0u8][..]));
        });
    }

    #[test]
    fn test_locked_state_initialize_rejects_a_page_total_that_overflows() {
        // Each region claims the whole address space, and enough of them are present that the
        // running bitmap start bit wraps.
        let pages_per_region = MemoryRange::new(0, u64::MAX).unwrap().pages() as usize;
        let regions_needed = usize::MAX / pages_per_region + 1;
        let scanned = vec![MmramRegion::new(MemoryRange::new(0, u64::MAX).unwrap(), false); regions_needed];
        let mut bookkeeping = Bookkeeping::new(regions_needed, 0);

        let result = bookkeeping.with_locked(|state| state.initialize(&scanned, 0, 1));

        assert_eq!(result, Err(AllocError::OutOfMemory.into()));
    }

    #[test]
    fn test_locked_state_initialize_reserves_only_the_bookkeeping_in_free_regions() {
        let mut bookkeeping = Bookkeeping::new(2, 8);
        let scanned = [region(0x10_0000, 4, false), region(0x20_0000, 4, false)];

        bookkeeping.with_locked(|state| {
            assert_eq!(state.initialize(&scanned, 0x10_0000, 2), Ok(()));

            assert_eq!(state.regions()[1].bitmap_start_bit, 4);
            assert_eq!(state.allocated_page_count(AllocationType::Supervisor), 2);
            assert_eq!(state.allocation_type(0x10_0000 + PAGE), Some(AllocationType::Supervisor));
            assert_eq!(state.allocation_type(0x20_0000), None);
            // Four free pages in a row exist only in the second region.
            assert_eq!(state.allocate(4, AllocationType::User), Some(0x20_0000));
            assert_eq!(state.free_page_count(), 2);
        });
    }

    #[test]
    fn test_locked_state_initialize_ignores_regions_past_the_table() {
        // The table has room for one region, so the pre-allocated second one has no bits to mark.
        let mut bookkeeping = Bookkeeping::new(1, 4);
        let scanned = [region(0x10_0000, 4, false), region(0x20_0000, 4, true)];

        bookkeeping.with_locked(|state| {
            assert_eq!(state.initialize(&scanned, 0x10_0000, 1), Ok(()));

            assert_eq!(state.regions().len(), 1);
            assert_eq!(state.allocated_page_count(AllocationType::Supervisor), 1);
            assert_eq!(state.free_page_count(), 3);
            assert_eq!(state.allocation_type(0x20_0000), None);
        });
    }
}
