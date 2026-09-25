//! MM Supervisor Core Page and Pool Allocators
//!
//! Provides a page-granularity memory allocator and a pool allocator for the MM Supervisor Core.
//!
//! ## Page Allocator
//!
//! When the one-time initialization routine is called, it will mark the blocks reported under
//! `gEfiSmmSmramMemoryGuid` or `gEfiMmPeiMmramMemoryReserveGuid` in the HOB list accordingly.
//! Blocks that have the `EFI_ALLOCATED` bit set in the `RegionState` field will be marked as allocated,
//! indicating they are in use. All other blocks will be marked as free.
//!
//! The page allocator is fully dynamic:
//! - No fixed limit on number of SMRAM regions
//! - No fixed limit on pages per region (supports up to 4GB per region)
//! - Bookkeeping is stored in SMRAM itself
//!
//! The page allocator provides:
//! - `allocate_pages(num_pages)` - Allocate contiguous pages
//! - `free_pages(addr, num_pages)` - Free previously allocated pages
//!
//! ## Pool Allocator
//!
//! Built on top of the page allocator, the pool allocator provides smaller-granularity allocations.
//! It allocates pages from the page allocator and subdivides them for pool allocations.
//! When a pool page is exhausted, more pages are allocated as needed.
//!
//! The pool allocator implements the `GlobalAlloc` trait for use as a global allocator.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ffi::c_void,
    mem::size_of,
    ptr, slice,
    sync::atomic::{AtomicBool, Ordering},
};

use patina::standard::efi;
use patina::{
    SIZE_256KB, UEFI_PAGE_SIZE,
    pi::hob::{Hob, PhaseHandoffInformationTable},
    uefi_pages_to_size,
};
use patina_paging::{MemoryAttributes, PageTable};
use spin::{Mutex, MutexGuard, relax::Spin};
use zerocopy::FromBytes;

use crate::smrr::{SmramRegion, verify_smrr_base_size};

/// Bits per byte.
const BITS_PER_BYTE: usize = 8;

/// `EFI_ALLOCATED` bit in `RegionState`.
pub const EFI_ALLOCATED: u64 = 0x0000000000000010;

// GUID for gEfiSmmSmramMemoryGuid
// { 0x6dadf1d1, 0xd4cc, 0x4910, { 0xbb, 0x6e, 0x82, 0xb1, 0xfd, 0x80, 0xff, 0x3d }}
pub const SMM_SMRAM_MEMORY_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("6dadf1d1-d4cc-4910-bb6e-82b1fd80ff3d");

// GUID for gEfiMmPeiMmramMemoryReserveGuid
// { 0x0703f912, 0xbf8d, 0x4e2a, { 0xbe, 0x07, 0xab, 0x27, 0x25, 0x25, 0xc5, 0x92 }}
pub const MM_PEI_MMRAM_MEMORY_RESERVE_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("0703f912-bf8d-4e2a-be07-ab272525c592");

/// Errors that can occur during page allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAllocError {
    /// The allocator has not been initialized.
    NotInitialized,
    /// The allocator has already been initialized.
    AlreadyInitialized,
    /// No free pages available to satisfy the request.
    OutOfMemory,
    /// The requested address is not aligned to page boundary.
    NotAligned,
    /// The address is not within any known SMRAM region.
    InvalidAddress,
    /// The address was not previously allocated.
    NotAllocated,
    /// The freed range could not be made inaccessible in the page table, so it was left allocated.
    UnmapFailed,
}

/// Type of memory allocation - distinguishes supervisor-internal vs user/driver allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AllocationType {
    /// Supervisor-internal allocation (e.g., for core data structures).
    /// These are typically never freed and may have stricter protections.
    Supervisor = 0,
    /// User/driver allocation (e.g., for MM driver requests).
    /// These can be allocated and freed by external code.
    User = 1,
}

/// SMRAM descriptor structure matching `EFI_SMRAM_DESCRIPTOR`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, zerocopy_derive::FromBytes, zerocopy_derive::Immutable)]
pub struct SmramDescriptor {
    /// Physical start address of the SMRAM region.
    pub physical_start: efi::PhysicalAddress,
    /// CPU start address (may differ from physical for remapping).
    pub cpu_start: efi::PhysicalAddress,
    /// Size of the SMRAM region in bytes.
    pub physical_size: u64,
    /// Region state flags (`EFI_ALLOCATED`, etc.).
    pub region_state: u64,
}

/// SMRAM reserve descriptor count structure.
/// This is the data that immediately follows a `GuidHob` with `SMM_SMRAM_MEMORY_GUID`
/// or `MM_PEI_MMRAM_MEMORY_RESERVE_GUID`.
#[repr(C)]
#[derive(Clone, Copy, Debug, zerocopy_derive::FromBytes, zerocopy_derive::Immutable)]
pub struct SmramReserveHobData {
    /// Number of SMRAM descriptors that follow.
    pub number_of_smram_regions: u32,
    /// Reserved for alignment.
    pub reserved: u32,
    // SmramDescriptor array follows immediately after
}

/// Metadata for a single SMRAM region.
/// This struct is stored in the bookkeeping pages, not statically.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RegionInfo {
    /// Base physical address of the region.
    pub base: u64,
    /// Total number of pages in this region.
    pub total_pages: usize,
    /// Starting bit index in the global allocation bitmap.
    pub bitmap_start_bit: usize,
}

/// Where an address range sits relative to MMRAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmramPlacement {
    /// Every byte of the range is MMRAM.
    Inside,
    /// No byte of the range is MMRAM.
    Outside,
    /// The range is partly MMRAM and partly not.
    PartlyInside,
}

impl MmramPlacement {
    /// Resolves this placement to whether `[base, base + size)` may be treated as MM memory.
    ///
    /// ## Panics
    ///
    /// Panics on [`MmramPlacement::PartlyInside`]. Callers are deciding whether a
    /// firmware-described buffer is MM memory, and a range crossing the boundary is neither:
    /// trusting it exposes the MMRAM half, rejecting it silently leaves a producer describing a
    /// region it does not own. There is no correct reading, so it is the configuration error it
    /// looks like.
    pub fn is_inside(self, base: u64, size: u64) -> bool {
        match self {
            Self::Inside => true,
            Self::Outside => false,
            Self::PartlyInside => {
                let end = base.saturating_add(size);
                panic!("Buffer 0x{base:016x}-0x{end:016x} crosses an MMRAM boundary");
            }
        }
    }
}

/// Classifies `[addr, addr + size)` against regions given as `(base, exclusive end)` pairs.
///
/// Coverage is measured over the union of the regions, so a range spanning two adjacent regions
/// is [`MmramPlacement::Inside`] rather than partly inside. An empty range is
/// [`MmramPlacement::Outside`]; a range whose end overflows is reported as partly inside so a
/// malformed descriptor fails closed.
fn classify_coverage(addr: u64, size: u64, mut regions: impl Iterator<Item = (u64, u64)> + Clone) -> MmramPlacement {
    if size == 0 {
        return MmramPlacement::Outside;
    }
    let Some(end) = addr.checked_add(size) else {
        return MmramPlacement::PartlyInside;
    };

    // Extend the covered prefix one region at a time until a gap appears.
    let mut cursor = addr;
    while let Some(region_end) =
        regions.clone().find_map(|(base, region_end)| (cursor >= base && cursor < region_end).then_some(region_end))
    {
        cursor = region_end;
        if cursor >= end {
            return MmramPlacement::Inside;
        }
    }

    if cursor > addr || regions.any(|(base, region_end)| cursor < region_end && base < end) {
        MmramPlacement::PartlyInside
    } else {
        MmramPlacement::Outside
    }
}

/// Classifies `[addr, addr + size)` against SMRAM descriptors that have been scanned but not yet
/// committed to the allocator's bookkeeping.
pub(crate) fn classify_mmram_in_regions(regions: &[SmramRegion], addr: u64, size: u64) -> MmramPlacement {
    classify_coverage(
        addr,
        size,
        regions.iter().filter_map(|region| Some((region.base, region.base.checked_add(region.size)?))),
    )
}

/// Returns whether any region in `regions` contains `address`.
pub(crate) fn regions_contain(regions: &[SmramRegion], address: u64) -> bool {
    regions.iter().any(|region| {
        region.base <= address && region.base.checked_add(region.size).is_some_and(|region_end| address < region_end)
    })
}

/// Internal state for the page allocator, stored in bookkeeping pages.
#[repr(C)]
struct AllocatorState {
    /// Number of regions.
    region_count: usize,
    /// Total number of pages across all regions.
    total_pages: usize,
    /// Number of pages used for bookkeeping.
    bookkeeping_pages: usize,
    /// Base address of bookkeeping memory.
    bookkeeping_base: u64,
    // Followed by:
    // - RegionInfo array (region_count entries)
    // - Allocation bitmap (total_pages bits, rounded up to bytes)
    // - Type bitmap (total_pages bits, rounded up to bytes)
}

/// Raw pointer to the allocator state living in SMRAM.
struct StatePtr(*mut AllocatorState);

// SAFETY: The content lives in SMRAM for the lifetime of the program and is only
// ever dereferenced while the enclosing `Mutex<StatePtr>` is held, which
// serializes all access to it.
unsafe impl Send for StatePtr {}

/// A lock-held view over the allocator state.
///
/// Obtaining a `LockedState` requires holding the state mutex (it owns the
/// guard), so every method is guaranteed exclusive access to the SMRAM
/// bookkeeping. Because the region/bitmap slices borrow from `&self` / `&mut
/// self`, the borrow checker — not convention — prevents mutable and shared
/// views from overlapping.
struct LockedState<'a> {
    /// State pointer copied out of the guard. Null until the allocator is initialized.
    state: *mut AllocatorState,
    /// Number of SMRAM regions, cached from the header at construction (0 if uninitialized).
    region_count: usize,
    /// Total pages across all regions, cached from the header at construction (0 if uninitialized).
    total_pages: usize,
    /// Held for the lifetime of this view to keep the lock acquired.
    guard: MutexGuard<'a, StatePtr, Spin>,
}

impl LockedState<'_> {
    /// Number of SMRAM regions, or 0 if the allocator is uninitialized.
    fn region_count(&self) -> usize {
        self.region_count
    }

    /// Total number of pages across all regions, or 0 if uninitialized.
    fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Region metadata array.
    fn regions(&self) -> &[RegionInfo] {
        if self.state.is_null() {
            return &[];
        }

        let regions_ptr = (self.state as *const u8).wrapping_add(size_of::<AllocatorState>()) as *const RegionInfo;
        // SAFETY: `regions_ptr` points to the `RegionInfo` array of `region_count` entries that
        // immediately follows the header within the bookkeeping allocation; we hold the lock so
        // no other reference is live.
        unsafe { slice::from_raw_parts(regions_ptr, self.region_count) }
    }

    /// Mutable region metadata array.
    fn regions_mut(&mut self) -> &mut [RegionInfo] {
        if self.state.is_null() {
            return &mut [];
        }

        let regions_ptr = (self.state as *mut u8).wrapping_add(size_of::<AllocatorState>()) as *mut RegionInfo;
        // SAFETY: as `regions`, plus we hold `&mut self` and the lock, so this is the only live
        // reference to the array.
        unsafe { slice::from_raw_parts_mut(regions_ptr, self.region_count) }
    }

    /// Byte offset of the allocation bitmap and its length in bytes.
    fn bitmap_offset_len(&self) -> (usize, usize) {
        let offset = size_of::<AllocatorState>() + self.region_count * size_of::<RegionInfo>();
        let bitmap_bytes = self.total_pages.div_ceil(BITS_PER_BYTE);
        (offset, bitmap_bytes)
    }

    /// Allocation bitmap (1 bit per page; set == allocated).
    fn alloc_bitmap(&self) -> &[u8] {
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
    fn type_bitmap(&self) -> &[u8] {
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
    fn bitmaps_mut(&mut self) -> (&mut [u8], &mut [u8]) {
        if self.state.is_null() {
            return (&mut [], &mut []);
        }
        let (offset, bitmap_bytes) = self.bitmap_offset_len();
        let ptr = (self.state as *mut u8).wrapping_add(offset);
        // SAFETY: the allocation and type bitmaps are two contiguous `bitmap_bytes`-sized ranges
        // within the bookkeeping allocation. Materialize both as one slice; we hold `&mut self`
        // and the lock, so no other reference exists.
        let both = unsafe { slice::from_raw_parts_mut(ptr, bitmap_bytes * 2) };
        // Safe split into the two disjoint halves.
        both.split_at_mut(bitmap_bytes)
    }

    /// Returns whether the page at `bit_index` is allocated.
    fn is_bit_allocated(&self, bit_index: usize) -> bool {
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
    fn bit_type(&self, bit_index: usize) -> AllocationType {
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
    fn set_bit_allocated(&mut self, bit_index: usize, alloc_type: AllocationType) {
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
    fn set_bit_free(&mut self, bit_index: usize) {
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
    fn find_region_for_address(&self, addr: u64) -> Option<(usize, usize)> {
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
    fn region_page_to_bit(&self, region_index: usize, page_in_region: usize) -> usize {
        self.regions().get(region_index).map_or(0, |region| region.bitmap_start_bit + page_in_region)
    }

    /// First-fit search for `num_pages` contiguous free pages, marking them
    /// allocated with `alloc_type`. Returns the base address on success.
    fn allocate(&mut self, num_pages: usize, alloc_type: AllocationType) -> Option<u64> {
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
    fn verify_allocated(&self, addr: u64, num_pages: usize) -> Result<core::ops::Range<usize>, PageAllocError> {
        let bit_range = self.allocation_bit_range(addr, num_pages)?;

        for bit in bit_range.clone() {
            if !self.is_bit_allocated(bit) {
                return Err(PageAllocError::NotAllocated);
            }
        }

        Ok(bit_range)
    }

    /// Publishes `bit_range` as free and therefore reusable.
    fn mark_free(&mut self, bit_range: core::ops::Range<usize>) {
        for bit in bit_range {
            self.set_bit_free(bit);
        }
    }

    /// Returns the bit range covering `[addr, addr + num_pages)`, requiring every page in it to be
    /// allocated with `expected_type`.
    ///
    /// The type-checked counterpart to [`verify_allocated`](Self::verify_allocated), and split
    /// from [`mark_free`](Self::mark_free) for the same reason.
    fn verify_allocated_with_type(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: AllocationType,
    ) -> Result<core::ops::Range<usize>, PageAllocError> {
        let bit_range = self.allocation_bit_range(addr, num_pages)?;

        for (page_offset, bit) in bit_range.clone().enumerate() {
            if !self.is_bit_allocated(bit) {
                return Err(PageAllocError::NotAllocated);
            }
            if self.bit_type(bit) != expected_type {
                log::warn!(
                    "Type mismatch at 0x{:016x}: expected {:?}, got {:?}",
                    addr + uefi_pages_to_size!(page_offset) as u64,
                    expected_type,
                    self.bit_type(bit)
                );
                return Err(PageAllocError::InvalidAddress);
            }
        }

        Ok(bit_range)
    }

    /// Returns the global bitmap range for a page-aligned allocation range.
    fn allocation_bit_range(&self, addr: u64, num_pages: usize) -> Result<core::ops::Range<usize>, PageAllocError> {
        if num_pages == 0 {
            return Err(PageAllocError::InvalidAddress);
        }

        let (region_index, page_in_region) =
            self.find_region_for_address(addr).ok_or(PageAllocError::InvalidAddress)?;
        let region = self.regions().get(region_index).ok_or(PageAllocError::InvalidAddress)?;
        let end_page = page_in_region.checked_add(num_pages).ok_or(PageAllocError::InvalidAddress)?;
        if end_page > region.total_pages {
            return Err(PageAllocError::InvalidAddress);
        }

        let first_bit = region.bitmap_start_bit.checked_add(page_in_region).ok_or(PageAllocError::InvalidAddress)?;
        let end_bit = first_bit.checked_add(num_pages).ok_or(PageAllocError::InvalidAddress)?;
        if end_bit > self.total_pages {
            return Err(PageAllocError::InvalidAddress);
        }

        Ok(first_bit..end_bit)
    }

    /// Counts free pages across all regions.
    fn free_page_count(&self) -> usize {
        (0..self.total_pages()).filter(|&bit| !self.is_bit_allocated(bit)).count()
    }

    /// Counts pages allocated with the given type.
    fn allocated_page_count(&self, alloc_type: AllocationType) -> usize {
        (0..self.total_pages()).filter(|&bit| self.is_bit_allocated(bit) && self.bit_type(bit) == alloc_type).count()
    }

    /// Returns the allocation type for `addr`, or `None` if not allocated.
    fn allocation_type(&self, addr: u64) -> Option<AllocationType> {
        let (region_index, page_in_region) = self.find_region_for_address(addr)?;
        let bit = self.region_page_to_bit(region_index, page_in_region);
        if self.is_bit_allocated(bit) { Some(self.bit_type(bit)) } else { None }
    }

    /// Returns whether `[addr, addr + size)` lies entirely within a single region.
    fn is_region_inside_mmram(&self, addr: u64, size: u64) -> bool {
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
    fn classify_mmram(&self, addr: u64, size: u64) -> MmramPlacement {
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
    fn initialize(
        &mut self,
        scanned: &[SmramRegion],
        bookkeeping_base: u64,
        bookkeeping_pages: usize,
    ) -> Result<(), PageAllocError> {
        // Fill in per-region metadata and assign each region its bitmap range.
        let mut bitmap_start_bit = 0usize;
        for (region, scanned_region) in self.regions_mut().iter_mut().zip(scanned.iter()) {
            let pages = PageAllocator::page_count(scanned_region.size).ok_or(PageAllocError::OutOfMemory)?;
            region.base = scanned_region.base;
            region.total_pages = pages;
            region.bitmap_start_bit = bitmap_start_bit;
            bitmap_start_bit = bitmap_start_bit.checked_add(pages).ok_or(PageAllocError::OutOfMemory)?;
        }

        // Mark pre-allocated regions and the bookkeeping pages as allocated (supervisor).
        for (i, scanned_region) in scanned.iter().enumerate() {
            let pages = PageAllocator::page_count(scanned_region.size).ok_or(PageAllocError::OutOfMemory)?;
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

/// Maximum number of SMRAM regions collected on the stack while scanning the
/// HOB list. The authoritative region metadata is stored in SMRAM afterwards.
pub(crate) const MAX_TEMP_REGIONS: usize = 256;

/// Selects the primary SMRR range from the scanned SMRAM regions and coalesces
/// any physically adjacent regions into it.
///
/// It picks the largest non pre-allocated region in `[1 MiB, 4 GiB]` that is at
/// least `256 KiB - 4 KiB`, then extends it downward and upward across every
/// region that is physically contiguous with it (regardless of allocation
/// state), scanning repeatedly until no further adjacent region is found.
///
/// Returns the coalesced [`SmramRegion`] on success (with `pre_allocated` set to
/// `false`, as it describes the SMRR programming range rather than a discovered
/// region), or `None` if no scanned region meets the SMRR base/size
/// requirements.
pub(crate) fn coalesced_smrr_range(regions: &[SmramRegion]) -> Option<SmramRegion> {
    /// Lowest CPU start address a candidate SMRR range may have.
    const BASE_1MB: u64 = 0x0010_0000;
    /// Highest address the primary SMRR can cover (4 GiB).
    const SMRR_MAX_ADDRESS: u64 = 0x1_0000_0000;

    // Find the largest usable (non-pre-allocated) range in [1 MiB, 4 GiB] that is at least
    // 256 KiB - 4 KiB.
    let mut max_size = SIZE_256KB as u64 - UEFI_PAGE_SIZE as u64;
    let mut current: Option<(u64, u64)> = None;
    for region in regions {
        if region.pre_allocated {
            continue;
        }
        let Some(region_end) = region.base.checked_add(region.size) else {
            continue;
        };
        if region.base >= BASE_1MB && region_end <= SMRR_MAX_ADDRESS && region.size >= max_size {
            max_size = region.size;
            current = Some((region.base, region.size));
        }
    }

    let (mut smrr_base, mut smrr_size) = current?;

    // Coalesce any physically adjacent ranges into the selected range. This
    // scans the (unsorted) region array repeatedly until no further
    // adjacent range is found, so ordering does not matter. Adjacency is
    // considered regardless of allocation state, because the SMRR must
    // cover a single contiguous physical range.
    loop {
        let mut found = false;
        for region in regions {
            let region_base = region.base;
            let region_size = region.size;
            let region_end = region_base.checked_add(region_size);
            let smrr_end = smrr_base.checked_add(smrr_size)?;
            if region_base < smrr_base && Some(smrr_base) == region_end {
                // Region sits immediately before the current range: extend downward.
                smrr_base = region_base;
                smrr_size = smrr_size.checked_add(region_size)?;
                found = true;
            } else if smrr_end == region_base && region_size > 0 {
                // Region sits immediately after the current range: extend upward.
                smrr_size = smrr_size.checked_add(region_size)?;
                found = true;
            }
        }
        if !found {
            break;
        }
    }

    let smrr_base = u32::try_from(smrr_base).ok()?;
    let smrr_size = u32::try_from(smrr_size).ok()?;

    if !verify_smrr_base_size(smrr_base, smrr_size) {
        log::warn!(
            "Coalesced SMRR range base=0x{smrr_base:x} size=0x{smrr_size:x} does not meet SMRR alignment/size requirements"
        );
        return None;
    }

    log::info!("SMRR Base: 0x{smrr_base:x}, SMRR Size: 0x{smrr_size:x}");
    Some(SmramRegion::new(u64::from(smrr_base), u64::from(smrr_size), false))
}

/// Page-granularity allocator for SMRAM memory.
pub struct PageAllocator {
    /// Allocator state pointer (into SMRAM), guarded by the lock.
    ///
    /// Holding the mutex is the single synchronization point for all access to
    /// the SMRAM bookkeeping.
    state: Mutex<StatePtr>,
    /// Whether the allocator has been initialized.
    initialized: AtomicBool,
}

impl PageAllocator {
    /// Creates a new uninitialized page allocator.
    pub const fn new() -> Self {
        Self { state: Mutex::new(StatePtr(ptr::null_mut())), initialized: AtomicBool::new(false) }
    }

    /// Converts a byte size to a page count without truncation or overflow.
    fn page_count(size: u64) -> Option<usize> {
        usize::try_from(size).ok().map(|size| size.div_ceil(UEFI_PAGE_SIZE))
    }

    /// Determines where the bookkeeping structures live and how large they are.
    ///
    /// Sums the pages across `regions`, selects the first non-pre-allocated
    /// region to host the bookkeeping, and verifies that region is large enough
    /// to hold it. Returns `(bookkeeping_base, bookkeeping_pages)`.
    fn calculate_bookkeeping(regions: &[SmramRegion]) -> Result<(u64, usize), PageAllocError> {
        let total_pages = regions.iter().try_fold(0usize, |total, region| {
            let pages = Self::page_count(region.size).ok_or(PageAllocError::OutOfMemory)?;
            total.checked_add(pages).ok_or(PageAllocError::OutOfMemory)
        })?;

        let header_size = size_of::<AllocatorState>();
        let regions_size = regions.len().checked_mul(size_of::<RegionInfo>()).ok_or(PageAllocError::OutOfMemory)?;
        let bitmap_bytes = total_pages.div_ceil(BITS_PER_BYTE);
        let bitmaps_size = bitmap_bytes.checked_mul(2).ok_or(PageAllocError::OutOfMemory)?;
        let total_bytes = header_size
            .checked_add(regions_size)
            .and_then(|size| size.checked_add(bitmaps_size))
            .ok_or(PageAllocError::OutOfMemory)?;
        let bookkeeping_pages = total_bytes.div_ceil(UEFI_PAGE_SIZE);

        log::info!(
            "Allocator needs {} pages for bookkeeping ({} regions, {} total pages)",
            bookkeeping_pages,
            regions.len(),
            total_pages
        );

        // Reserve bookkeeping space in the first free (non-pre-allocated) region.
        let first_free = regions.iter().find(|region| !region.pre_allocated);
        let bookkeeping_base = first_free.map(|region| region.base).ok_or_else(|| {
            log::error!("No free SMRAM region available for bookkeeping");
            PageAllocError::OutOfMemory
        })?;
        let first_free_size = first_free.map_or(0, |region| region.size);
        if bookkeeping_base == 0 || !bookkeeping_base.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("Bookkeeping region base is null or not page-aligned");
            return Err(PageAllocError::NotAligned);
        }

        let bookkeeping_size = bookkeeping_pages.checked_mul(UEFI_PAGE_SIZE).ok_or(PageAllocError::OutOfMemory)?;
        if u64::try_from(bookkeeping_size).map_or(true, |size| size > first_free_size) {
            log::error!("First free region too small for bookkeeping");
            return Err(PageAllocError::OutOfMemory);
        }

        Ok((bookkeeping_base, bookkeeping_pages))
    }

    /// Scans the entire HOB list and collects every SMRAM/MMRAM region reported
    /// under the supported GUIDs into `regions`, updating `count`.
    fn scan_smram_regions(
        handoff: &PhaseHandoffInformationTable,
        regions: &mut [SmramRegion; MAX_TEMP_REGIONS],
        region_count: &mut usize,
    ) -> Result<(), PageAllocError> {
        let hob = Hob::Handoff(handoff);
        for current_hob in &hob {
            if let Hob::GuidHob(guid_hob, data) = current_hob
                && (guid_hob.name == SMM_SMRAM_MEMORY_GUID || guid_hob.name == MM_PEI_MMRAM_MEMORY_RESERVE_GUID)
            {
                log::info!("Found SMRAM memory HOB with GUID {}", guid_hob.name.as_guid());
                Self::collect_smram_regions(data, regions, region_count)?;
            }
        }
        Ok(())
    }

    /// Parses the `SmramReserveHobData` header and trailing `SmramDescriptor`
    /// array from a single matching GUID HOB payload, appending each region to
    /// `regions` and updating `region_count`.
    fn collect_smram_regions(
        data: &[u8],
        regions: &mut [SmramRegion; MAX_TEMP_REGIONS],
        region_count: &mut usize,
    ) -> Result<(), PageAllocError> {
        let header_size = size_of::<SmramReserveHobData>();
        if data.len() < header_size {
            return Ok(());
        }

        let (header, descriptor_bytes) =
            SmramReserveHobData::read_from_prefix(data).map_err(|_| PageAllocError::InvalidAddress)?;

        // Clamp the declared count to what the payload can actually hold, so the descriptor
        // bytes below are guaranteed in-bounds even if the HOB is malformed.
        let max_fit = descriptor_bytes.len() / size_of::<SmramDescriptor>();
        let declared = header.number_of_smram_regions as usize;
        if declared > max_fit {
            log::warn!("SMRAM HOB declares {declared} descriptors but only {max_fit} fit in the payload");
        }
        let count = declared.min(max_fit);

        for descriptor_bytes in descriptor_bytes.chunks_exact(size_of::<SmramDescriptor>()).take(count) {
            let (descriptor, _) =
                SmramDescriptor::read_from_prefix(descriptor_bytes).map_err(|_| PageAllocError::InvalidAddress)?;
            if *region_count >= MAX_TEMP_REGIONS {
                log::error!(
                    "Too many SMRAM regions for temp storage (MAX_TEMP_REGIONS = {MAX_TEMP_REGIONS}), increase MAX_TEMP_REGIONS"
                );
                return Err(PageAllocError::OutOfMemory);
            }

            let pre_allocated = (descriptor.region_state & EFI_ALLOCATED) != 0;
            let pages = Self::page_count(descriptor.physical_size).ok_or(PageAllocError::OutOfMemory)?;

            log::info!(
                "SMRAM Region {}: base=0x{:016x}, size=0x{:x}, pages={}, state=0x{:x}, allocated={}",
                *region_count,
                descriptor.physical_start,
                descriptor.physical_size,
                pages,
                descriptor.region_state,
                pre_allocated
            );

            let Some(slot) = regions.get_mut(*region_count) else {
                log::error!(
                    "Too many SMRAM regions for temp storage (MAX_TEMP_REGIONS = {MAX_TEMP_REGIONS}), increase MAX_TEMP_REGIONS"
                );
                return Err(PageAllocError::OutOfMemory);
            };
            *slot = SmramRegion::new(descriptor.physical_start, descriptor.physical_size, pre_allocated);
            *region_count += 1;
        }

        Ok(())
    }

    /// Acquires the state lock and returns a [`LockedState`] view over the SMRAM
    /// bookkeeping. All access to the bookkeeping goes through this so the lock
    /// is held for the full duration of the access.
    fn lock_state(&self) -> LockedState<'_> {
        let guard = self.state.lock();
        let state = guard.0;

        // Cache the header-derived sizes once so the view's length accessors stay safe.
        let (region_count, total_pages) = if state.is_null() {
            (0, 0)
        } else {
            // SAFETY: when non-null, `state` is a valid initialized header and we hold the lock.
            unsafe { ((*state).region_count, (*state).total_pages) }
        };
        LockedState { state, region_count, total_pages, guard }
    }

    /// Collects every SMRAM region the HOB list describes into a stack array.
    ///
    /// Nothing in MMRAM is written and no allocator state is published, so the descriptors can
    /// be validated against a trusted bound before [`init_from_regions`](Self::init_from_regions)
    /// commits to them. Splitting the two is what keeps a forged descriptor from steering a write
    /// before it has been rejected.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list.
    pub(crate) unsafe fn scan_hob_list(
        hob_list: *const c_void,
    ) -> Result<([SmramRegion; MAX_TEMP_REGIONS], usize), PageAllocError> {
        if hob_list.is_null() {
            return Err(PageAllocError::NotInitialized);
        }

        // SAFETY: per this function's contract, a non-null `hob_list` points to a valid HOB
        // list whose first entry is the Phase Handoff Information Table.
        let hob_list_info = unsafe {
            (hob_list as *const PhaseHandoffInformationTable).as_ref().ok_or(PageAllocError::NotInitialized)?
        };

        let mut regions = [SmramRegion::default(); MAX_TEMP_REGIONS];
        let mut count = 0usize;
        Self::scan_smram_regions(hob_list_info, &mut regions, &mut count)?;

        if count == 0 {
            log::error!("No SMRAM regions found in HOB list");
            return Err(PageAllocError::NotInitialized);
        }

        Ok((regions, count))
    }

    /// Initializes allocator bookkeeping from already parsed SMRAM regions.
    ///
    /// This is the first write into memory the producer named, so the caller is expected to have
    /// validated `scanned` beforehand.
    ///
    /// ## Safety
    ///
    /// Any region that passes the descriptor validation below must describe
    /// valid memory for its full declared size. Non-pre-allocated regions must
    /// additionally be exclusively owned.
    pub(crate) unsafe fn init_from_regions(&self, scanned: &[SmramRegion]) -> Result<(), PageAllocError> {
        if scanned.is_empty() {
            return Err(PageAllocError::NotInitialized);
        }

        for (index, region) in scanned.iter().enumerate() {
            if !region.base.is_multiple_of(UEFI_PAGE_SIZE as u64) || !region.size.is_multiple_of(UEFI_PAGE_SIZE as u64)
            {
                return Err(PageAllocError::NotAligned);
            }
            let region_end = region
                .base
                .checked_add(region.size)
                .filter(|_| region.size != 0)
                .ok_or(PageAllocError::InvalidAddress)?;
            let previous_regions = scanned.get(..index).ok_or(PageAllocError::InvalidAddress)?;
            if previous_regions.iter().any(|previous| {
                previous
                    .base
                    .checked_add(previous.size)
                    .is_some_and(|previous_end| region.base < previous_end && previous.base < region_end)
            }) {
                return Err(PageAllocError::InvalidAddress);
            }
        }

        let guard = self.state.lock();
        if self.initialized.load(Ordering::Acquire) {
            return Err(PageAllocError::AlreadyInitialized);
        }

        let total_pages = scanned.iter().try_fold(0usize, |total, region| {
            let pages = Self::page_count(region.size).ok_or(PageAllocError::OutOfMemory)?;
            total.checked_add(pages).ok_or(PageAllocError::OutOfMemory)
        })?;

        // Determine where the bookkeeping structures live and how large they are.
        let (bookkeeping_base, bookkeeping_pages) = Self::calculate_bookkeeping(scanned)?;

        log::info!("Using 0x{bookkeeping_base:016x} for bookkeeping ({bookkeeping_pages} pages)");

        // Zero the bookkeeping region, write the state header, and publish the pointer.
        let state_ptr = bookkeeping_base as *mut AllocatorState;

        // SAFETY: `bookkeeping_base` is a page-aligned region of `bookkeeping_pages` pages that
        // we exclusively reserved for bookkeeping and hold the lock for. Zeroing it makes both
        // the all-zero `AllocatorState` header and the trailing bitmaps valid, and we take the
        // sole reference to the header at its start.
        let state = unsafe {
            ptr::write_bytes(bookkeeping_base as *mut u8, 0, uefi_pages_to_size!(bookkeeping_pages));
            &mut *state_ptr
        };
        *state = AllocatorState { region_count: scanned.len(), total_pages, bookkeeping_pages, bookkeeping_base };

        // Populate region metadata and the allocation bitmaps under the held lock, then
        // release the lock before the stats logging below re-acquires it.
        let mut locked = LockedState { state: state_ptr, region_count: scanned.len(), total_pages, guard };
        locked.initialize(scanned, bookkeeping_base, bookkeeping_pages)?;
        locked.guard.0 = state_ptr;
        drop(locked);

        self.initialized.store(true, Ordering::Release);

        // print page allocator statistics after init
        log::info!(
            "Page allocator initialized: {} region(s), {} total pages, {} free pages, {} allocated supervisor pages, {} allocated user pages",
            self.region_count(),
            self.total_page_count(),
            self.free_page_count(),
            self.allocated_page_count(AllocationType::Supervisor),
            self.allocated_page_count(AllocationType::User)
        );

        Ok(())
    }

    /// Allocates contiguous pages from SMRAM for supervisor use.
    pub fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError> {
        self.allocate_pages_with_type(num_pages, AllocationType::Supervisor)
    }

    /// Allocates contiguous pages from SMRAM with the specified allocation type.
    ///
    /// For `Supervisor` allocations, the allocated region is marked as supervisor-owned
    /// data pages (R/W, non-executable) in the page table.
    pub fn allocate_pages_with_type(
        &self,
        num_pages: usize,
        alloc_type: AllocationType,
    ) -> Result<u64, PageAllocError> {
        if !self.is_initialized() {
            return Err(PageAllocError::NotInitialized);
        }

        if num_pages == 0 {
            return Err(PageAllocError::OutOfMemory);
        }

        // Reserve pages under the state lock, then release it before touching the
        // page table (which takes its own lock).
        let addr = self.lock_state().allocate(num_pages, alloc_type).ok_or(PageAllocError::OutOfMemory)?;

        // For supervisor allocations, update page table attributes to mark as
        // supervisor-owned data pages (R/W/NX/S), otherwise they would
        // default to user data (R/W/NX/U).
        self.apply_data_page_attributes(addr, num_pages, alloc_type);

        Ok(addr)
    }

    /// Applies supervisor page table attributes to a newly allocated region.
    ///
    /// Marks pages as supervisor-owned data pages: Read/Write + Non-Executable (NX).
    /// This ensures supervisor data cannot be executed, providing W^X enforcement.
    ///
    /// If the global page table is not yet initialized (e.g., during early boot),
    /// this is a no-op with a warning.
    fn apply_data_page_attributes(&self, addr: u64, num_pages: usize, alloc_type: AllocationType) {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        if let Some(ref mut pt) = *pt_guard {
            // Data pages: R/W (no ReadOnly) + NX (ExecuteProtect)
            let mut attributes = MemoryAttributes::ExecuteProtect;

            if alloc_type == AllocationType::Supervisor {
                // For Supervisor allocations, we additionally want the U/S bit cleared (Supervisor-only).
                attributes |= MemoryAttributes::Supervisor; // Ensure not writable by user code
            }

            if let Err(e) = pt.map_memory_region(addr, size, attributes) {
                log::error!("Failed to set supervisor page attributes for 0x{addr:016x} ({num_pages} pages): {e:?}");
            } else {
                log::trace!("Marked 0x{addr:016x} ({num_pages} pages) as supervisor R/W+NX");
            }
        } else {
            log::warn!("Page table not initialized, skipping attribute update for 0x{addr:016x}");
        }
    }

    /// Overwrites a still-mapped page range with zeros.
    ///
    /// Freed pages go back into the same pool that later serves `User` (Ring 3) allocations, so a
    /// supervisor allocation that is released without scrubbing would disclose its contents to
    /// Ring 3 on the next reuse. This must run before [`Self::apply_freed_page_attributes`] unmaps
    /// the range.
    fn zero_pages(addr: u64, num_pages: usize) {
        // SAFETY: the caller verified under the state lock that `[addr, addr + num_pages)` is a
        // live allocation inside a single SMRAM region and has just mapped it R/W, so the only
        // access made while SMAP is lifted stays inside that range. SMAP has to come down
        // because the range may be user-owned (U/S = 1).
        unsafe {
            crate::runtime::with_user_access(|| {
                core::ptr::write_bytes(addr as *mut u8, 0, uefi_pages_to_size!(num_pages));
            });
        }
    }

    /// Applies restrictive page table attributes to freed pages.
    ///
    /// Marks pages as completely inaccessible: Supervisor + `ReadProtect` + `ExecuteProtect` (NX).
    /// This prevents any read, write, or execute access to freed memory, mitigating
    /// use-after-free vulnerabilities.
    ///
    /// A failure is reported so the caller can leave the range allocated rather than hand a still
    /// reachable range back to the pool.
    ///
    /// Reports success when the global page table is not yet initialized, which only happens
    /// before [`init_page_table`](crate::MmSupervisorCore::init_page_table). No free path runs
    /// that early, so this is a diagnostic for a caller that starts to.
    fn apply_freed_page_attributes(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        let Some(pt) = pt_guard.as_mut() else {
            log::warn!("Page table not initialized, skipping freed page attribute update for 0x{addr:016x}");
            return Ok(());
        };

        // Freed pages: ReadProtect (not present) + NX (no execute) + ReadOnly (no write)
        // This makes the pages completely inaccessible.
        if let Err(e) = pt.unmap_memory_region(addr, size) {
            log::error!("Failed to set freed page attributes for 0x{addr:016x} ({num_pages} pages): {e:?}");
            return Err(PageAllocError::UnmapFailed);
        }

        log::trace!("Marked 0x{addr:016x} ({num_pages} pages) as inaccessible (RP+NX+RO+S)");
        Ok(())
    }

    /// Frees previously allocated pages.
    ///
    /// The pages are zeroed so their contents cannot be recovered through a later allocation, then
    /// marked as inaccessible in the page table (Supervisor + `ReadProtect` + `ExecuteProtect`) to
    /// prevent use-after-free.
    pub fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
        self.release_pages(addr, num_pages, None)
    }

    /// Frees previously allocated pages, verifying the allocation type matches.
    ///
    /// The pages are zeroed so their contents cannot be recovered through a later allocation, then
    /// marked as inaccessible in the page table (Supervisor + `ReadProtect` + `ExecuteProtect`) to
    /// prevent use-after-free.
    pub fn free_pages_checked(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: AllocationType,
    ) -> Result<(), PageAllocError> {
        self.release_pages(addr, num_pages, Some(expected_type))
    }

    /// Scrubs `[addr, addr + num_pages)`, makes it inaccessible, and only then returns it to the
    /// pool.
    ///
    /// The range stays marked allocated for the whole sequence. Publishing it first would let
    /// another core take it while this one is still scrubbing, and would leave it advertised as
    /// reusable if the page table transition failed, handing a caller memory that a stale mapping
    /// still reaches. The state lock is held throughout for the same reason; the page table lock
    /// is only ever taken after it, never before, so the order cannot invert.
    fn release_pages(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: Option<AllocationType>,
    ) -> Result<(), PageAllocError> {
        self.release_pages_with(addr, num_pages, expected_type, |addr, num_pages| {
            self.apply_freed_page_attributes(addr, num_pages)
        })
    }

    /// Runs the [`release_pages`](Self::release_pages) sequence with `restrict` standing in for the
    /// page table transition.
    fn release_pages_with(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: Option<AllocationType>,
        restrict: impl FnOnce(u64, usize) -> Result<(), PageAllocError>,
    ) -> Result<(), PageAllocError> {
        if !self.is_initialized() {
            return Err(PageAllocError::NotInitialized);
        }

        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            return Err(PageAllocError::NotAligned);
        }

        let mut state = self.lock_state();
        let bit_range = match expected_type {
            Some(expected_type) => state.verify_allocated_with_type(addr, num_pages, expected_type)?,
            None => state.verify_allocated(addr, num_pages)?,
        };

        // A range this allocator handed out is already mapped R/W, but one marked allocated from a
        // producer's `EFI_ALLOCATED` descriptor keeps whatever the inherited page table gave it,
        // and the MM IPL maps its HOB list read-only. Restore the attributes the allocator would
        // have set so the scrub cannot fault on a mapping it never chose.
        self.apply_data_page_attributes(addr, num_pages, state.bit_type(bit_range.start));
        Self::zero_pages(addr, num_pages);
        restrict(addr, num_pages)?;
        state.mark_free(bit_range);

        log::trace!("Freed {num_pages} page(s) at 0x{addr:016x}");
        Ok(())
    }

    /// Returns the total number of free pages across all regions.
    pub fn free_page_count(&self) -> usize {
        if !self.is_initialized() {
            return 0;
        }
        self.lock_state().free_page_count()
    }

    /// Returns the number of pages allocated for a specific type.
    pub fn allocated_page_count(&self, alloc_type: AllocationType) -> usize {
        if !self.is_initialized() {
            return 0;
        }
        self.lock_state().allocated_page_count(alloc_type)
    }

    /// Returns the allocation type for a given address.
    pub fn get_allocation_type(&self, addr: u64) -> Option<AllocationType> {
        if !self.is_initialized() {
            return None;
        }
        self.lock_state().allocation_type(addr)
    }

    /// Returns whether the allocator has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Returns the total number of pages across all regions.
    pub fn total_page_count(&self) -> usize {
        if !self.is_initialized() {
            return 0;
        }
        self.lock_state().total_pages()
    }

    /// Returns the number of regions.
    pub fn region_count(&self) -> usize {
        if !self.is_initialized() {
            return 0;
        }
        self.lock_state().region_count()
    }

    pub fn is_region_inside_mmram(&self, addr: u64, size: u64) -> bool {
        if !self.is_initialized() {
            return false;
        }
        self.lock_state().is_region_inside_mmram(addr, size)
    }

    /// Classifies `[addr, addr + size)` against the MMRAM regions, or `None` if the regions are
    /// not known yet.
    ///
    /// Callers decide what an unknown layout means for them; nothing can be proven to be either
    /// inside or outside MMRAM before the allocator is initialized.
    pub fn classify_mmram(&self, addr: u64, size: u64) -> Option<MmramPlacement> {
        if !self.is_initialized() {
            return None;
        }
        Some(self.lock_state().classify_mmram(addr, size))
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    /// Smallest region size `coalesced_smrr_range` will accept (256 KiB - 4 KiB).
    const MIN_SMRR_SIZE: u64 = SIZE_256KB as u64 - UEFI_PAGE_SIZE as u64;
    const TEST_REGION_PAGES: usize = 16;
    const TEST_REGION_BYTES: usize = TEST_REGION_PAGES * UEFI_PAGE_SIZE;

    #[repr(align(4096))]
    struct AlignedRegion([u8; TEST_REGION_BYTES]);

    struct AllocatorFixture {
        allocator: PageAllocator,
        base: u64,
        _memory: Box<AlignedRegion>,
    }

    impl AllocatorFixture {
        fn new() -> Self {
            let mut memory = Box::new(AlignedRegion([0xA5; TEST_REGION_BYTES]));
            let base = memory.0.as_mut_ptr() as u64;
            let allocator = PageAllocator::new();
            let regions = [SmramRegion::new(base, TEST_REGION_BYTES as u64, false)];

            // SAFETY: `memory` is page-aligned, exclusively owned by the fixture,
            // remains live with the allocator, and covers the declared region.
            unsafe {
                allocator.init_from_regions(&regions).unwrap();
            }

            Self { allocator, base, _memory: memory }
        }
    }

    /// Builds a `SmramRegion` list from `(base, size, pre_allocated)` tuples.
    fn regions_from(entries: &[(u64, u64, bool)]) -> Vec<SmramRegion> {
        entries.iter().map(|&(base, size, pre_allocated)| SmramRegion::new(base, size, pre_allocated)).collect()
    }

    fn smram_hob_payload(declared_count: u32, descriptors: &[SmramDescriptor]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(size_of::<SmramReserveHobData>() + size_of_val(descriptors));
        payload.extend_from_slice(&declared_count.to_ne_bytes());
        payload.extend_from_slice(&0u32.to_ne_bytes());
        for descriptor in descriptors {
            payload.extend_from_slice(&descriptor.physical_start.to_ne_bytes());
            payload.extend_from_slice(&descriptor.cpu_start.to_ne_bytes());
            payload.extend_from_slice(&descriptor.physical_size.to_ne_bytes());
            payload.extend_from_slice(&descriptor.region_state.to_ne_bytes());
        }
        payload
    }

    #[test]
    fn test_page_allocator_uninitialized_operations() {
        let allocator = PageAllocator::new();

        assert!(!allocator.is_initialized());
        assert_eq!(allocator.region_count(), 0);
        assert_eq!(allocator.total_page_count(), 0);
        assert_eq!(allocator.free_page_count(), 0);
        assert_eq!(allocator.allocate_pages(1), Err(PageAllocError::NotInitialized));
        assert_eq!(allocator.free_pages(UEFI_PAGE_SIZE as u64, 1), Err(PageAllocError::NotInitialized));
        assert_eq!(allocator.get_allocation_type(UEFI_PAGE_SIZE as u64), None);
        assert!(!allocator.is_region_inside_mmram(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64));

        // SAFETY: a null HOB pointer is explicitly rejected before dereference.
        unsafe {
            assert!(matches!(PageAllocator::scan_hob_list(ptr::null()), Err(PageAllocError::NotInitialized)));
        }

        let mut state = allocator.lock_state();
        assert!(state.regions().is_empty());
        assert!(state.regions_mut().is_empty());
        assert!(state.alloc_bitmap().is_empty());
        assert!(state.type_bitmap().is_empty());
        let (alloc_bitmap, type_bitmap) = state.bitmaps_mut();
        assert!(alloc_bitmap.is_empty());
        assert!(type_bitmap.is_empty());
    }

    #[test]
    fn test_page_allocator_initialization_reserves_bookkeeping() {
        let fixture = AllocatorFixture::new();

        assert!(fixture.allocator.is_initialized());
        assert_eq!(fixture.allocator.region_count(), 1);
        assert_eq!(fixture.allocator.total_page_count(), TEST_REGION_PAGES);
        assert_eq!(fixture.allocator.free_page_count(), TEST_REGION_PAGES - 1);
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::Supervisor), 1);
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::User), 0);
        assert_eq!(fixture.allocator.get_allocation_type(fixture.base), Some(AllocationType::Supervisor));
        assert_eq!(fixture.allocator.allocate_pages(0), Err(PageAllocError::OutOfMemory));
        assert_eq!(fixture.allocator.free_pages(fixture.base + 1, 1), Err(PageAllocError::NotAligned));
        assert_eq!(fixture.allocator.free_pages(fixture.base, 0), Err(PageAllocError::InvalidAddress));
        assert_eq!(
            fixture.allocator.free_pages(fixture.base + TEST_REGION_BYTES as u64, 1),
            Err(PageAllocError::InvalidAddress)
        );
    }

    #[test]
    fn test_page_allocator_rejects_double_initialization() {
        let fixture = AllocatorFixture::new();
        let regions = [SmramRegion::new(fixture.base, TEST_REGION_BYTES as u64, false)];

        // SAFETY: the fixture owns the page-aligned region for its full declared size.
        unsafe {
            assert_eq!(fixture.allocator.init_from_regions(&regions), Err(PageAllocError::AlreadyInitialized));
        }
        assert_eq!(fixture.allocator.free_page_count(), TEST_REGION_PAGES - 1);
    }

    #[test]
    fn test_page_allocator_validates_regions_before_writing_bookkeeping() {
        let allocator = PageAllocator::new();
        let empty: [SmramRegion; 0] = [];
        let unaligned = [SmramRegion::new(0x1001, UEFI_PAGE_SIZE as u64, false)];
        let partial_page = [SmramRegion::new(0x1000, UEFI_PAGE_SIZE as u64 - 1, false)];
        let overflowing = [SmramRegion::new(u64::MAX - UEFI_PAGE_SIZE as u64 + 1, UEFI_PAGE_SIZE as u64, false)];
        let overlapping = [
            SmramRegion::new(0x1000, 2 * UEFI_PAGE_SIZE as u64, false),
            SmramRegion::new(0x2000, UEFI_PAGE_SIZE as u64, false),
        ];

        // SAFETY: these invalid descriptors are rejected before any address is dereferenced.
        unsafe {
            assert_eq!(allocator.init_from_regions(&empty), Err(PageAllocError::NotInitialized));
            assert_eq!(allocator.init_from_regions(&unaligned), Err(PageAllocError::NotAligned));
            assert_eq!(allocator.init_from_regions(&partial_page), Err(PageAllocError::NotAligned));
            assert_eq!(allocator.init_from_regions(&overflowing), Err(PageAllocError::InvalidAddress));
            assert_eq!(allocator.init_from_regions(&overlapping), Err(PageAllocError::InvalidAddress));
        }
        assert!(!allocator.is_initialized());
    }

    #[test]
    fn test_page_allocator_tracks_types_and_uses_first_fit() {
        let fixture = AllocatorFixture::new();
        let mut state = fixture.allocator.lock_state();

        let supervisor = state.allocate(2, AllocationType::Supervisor).unwrap();
        let user = state.allocate(3, AllocationType::User).unwrap();

        assert_eq!(supervisor, fixture.base + UEFI_PAGE_SIZE as u64);
        assert_eq!(user, fixture.base + 3 * UEFI_PAGE_SIZE as u64);
        assert_eq!(state.allocation_type(supervisor), Some(AllocationType::Supervisor));
        assert_eq!(state.allocation_type(user), Some(AllocationType::User));
        assert_eq!(state.allocation_type(user + 1), Some(AllocationType::User));
        assert_eq!(state.allocated_page_count(AllocationType::Supervisor), 3);
        assert_eq!(state.allocated_page_count(AllocationType::User), 3);
        assert_eq!(state.free_page_count(), TEST_REGION_PAGES - 6);
    }

    #[test]
    fn test_page_allocator_public_allocate_and_free_api() {
        let fixture = AllocatorFixture::new();

        let supervisor = fixture.allocator.allocate_pages(2).unwrap();
        let user = fixture.allocator.allocate_pages_with_type(3, AllocationType::User).unwrap();

        assert_eq!(fixture.allocator.get_allocation_type(supervisor), Some(AllocationType::Supervisor));
        assert_eq!(fixture.allocator.get_allocation_type(user), Some(AllocationType::User));
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::Supervisor), 3);
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::User), 3);
        assert!(fixture.allocator.is_region_inside_mmram(user, 3 * UEFI_PAGE_SIZE as u64));

        assert_eq!(
            fixture.allocator.free_pages_checked(user, 3, AllocationType::Supervisor),
            Err(PageAllocError::InvalidAddress)
        );
        assert_eq!(fixture.allocator.free_pages_checked(user, 3, AllocationType::User), Ok(()));
        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Ok(()));
        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Err(PageAllocError::NotAllocated));
        assert_eq!(fixture.allocator.free_page_count(), TEST_REGION_PAGES - 1);
    }

    #[test]
    fn test_page_allocator_scrubs_pages_on_free() {
        let fixture = AllocatorFixture::new();

        // A supervisor allocation holding secrets is released back into the shared pool.
        let supervisor = fixture.allocator.allocate_pages(2).unwrap();
        let secret_len = 2 * UEFI_PAGE_SIZE;
        // SAFETY: the fixture owns this range for the allocation's full size.
        let secret = unsafe { core::slice::from_raw_parts_mut(supervisor as *mut u8, secret_len) };
        secret.fill(0x5A);

        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Ok(()));

        // Nothing may survive the free, so the next allocation cannot observe it.
        let user = fixture.allocator.allocate_pages_with_type(2, AllocationType::User).unwrap();
        assert_eq!(user, supervisor, "first-fit must hand back the just-freed range");
        // SAFETY: as above, for the range the allocator just handed out.
        let reused = unsafe { core::slice::from_raw_parts(user as *const u8, secret_len) };
        assert!(reused.iter().all(|&b| b == 0), "freed supervisor pages leaked into a user allocation");
    }

    #[test]
    fn test_page_allocator_checked_free_scrubs_pages() {
        let fixture = AllocatorFixture::new();

        let user = fixture.allocator.allocate_pages_with_type(1, AllocationType::User).unwrap();
        // SAFETY: the fixture owns this range for the allocation's full size.
        unsafe { core::slice::from_raw_parts_mut(user as *mut u8, UEFI_PAGE_SIZE) }.fill(0xC3);

        assert_eq!(fixture.allocator.free_pages_checked(user, 1, AllocationType::User), Ok(()));

        // SAFETY: as above; the range is still owned by the fixture after the free.
        let scrubbed = unsafe { core::slice::from_raw_parts(user as *const u8, UEFI_PAGE_SIZE) };
        assert!(scrubbed.iter().all(|&b| b == 0), "checked free left page contents behind");
    }

    #[test]
    fn test_page_allocator_classifies_ranges_containment_misses() {
        let fixture = AllocatorFixture::new();
        let base = fixture.base;
        let end = base + TEST_REGION_BYTES as u64;
        let classify = |addr, size| fixture.allocator.classify_mmram(addr, size);

        // Wholly inside.
        assert_eq!(classify(base, 0x1000), Some(MmramPlacement::Inside));
        assert_eq!(classify(base, TEST_REGION_BYTES as u64), Some(MmramPlacement::Inside));

        // Crossing either boundary. These are the ranges `is_region_inside_mmram` reports as
        // not inside MMRAM, which is why containment alone cannot keep a buffer out of it.
        assert!(!fixture.allocator.is_region_inside_mmram(base - 0x1000, 0x2000));
        assert_eq!(classify(base - 0x1000, 0x2000), Some(MmramPlacement::PartlyInside));
        assert!(!fixture.allocator.is_region_inside_mmram(end - 0x1000, 0x2000));
        assert_eq!(classify(end - 0x1000, 0x2000), Some(MmramPlacement::PartlyInside));

        // A range that swallows the whole region is also not "inside" it.
        assert!(!fixture.allocator.is_region_inside_mmram(base - 0x1000, TEST_REGION_BYTES as u64 + 0x2000));
        assert_eq!(classify(base - 0x1000, TEST_REGION_BYTES as u64 + 0x2000), Some(MmramPlacement::PartlyInside));

        // Clear of the region on either side, and touching only its exclusive bounds.
        assert_eq!(classify(base - 0x1000, 0x1000), Some(MmramPlacement::Outside));
        assert_eq!(classify(end, 0x1000), Some(MmramPlacement::Outside));

        // An empty range touches nothing; an overflowing one fails closed.
        assert_eq!(classify(base, 0), Some(MmramPlacement::Outside));
        assert_eq!(classify(u64::MAX, 2), Some(MmramPlacement::PartlyInside));
    }

    #[test]
    fn test_page_allocator_reports_overlap_before_it_knows_any_regions() {
        // Nothing can be shown to lie either inside or outside MMRAM before the regions are
        // scanned, so the layout is reported as unknown rather than guessed at.
        let allocator = PageAllocator::new();

        assert!(!allocator.is_initialized());
        assert_eq!(allocator.classify_mmram(0x1000, 0x1000), None);
    }

    #[test]
    fn test_page_allocator_checked_free_is_atomic_on_type_mismatch() {
        let fixture = AllocatorFixture::new();
        let mut state = fixture.allocator.lock_state();
        let user = state.allocate(2, AllocationType::User).unwrap();

        assert_eq!(
            state.verify_allocated_with_type(user, 2, AllocationType::Supervisor),
            Err(PageAllocError::InvalidAddress)
        );
        assert_eq!(state.allocation_type(user), Some(AllocationType::User));
        assert_eq!(state.allocation_type(user + UEFI_PAGE_SIZE as u64), Some(AllocationType::User));

        let bit_range =
            state.verify_allocated_with_type(user, 2, AllocationType::User).expect("the range is user-allocated");
        state.mark_free(bit_range);
        assert_eq!(state.verify_allocated(user, 2), Err(PageAllocError::NotAllocated));
        assert_eq!(state.verify_allocated(user, 0), Err(PageAllocError::InvalidAddress));
    }

    #[test]
    fn test_page_allocator_rejects_free_crossing_region_end() {
        let fixture = AllocatorFixture::new();
        let mut state = fixture.allocator.lock_state();
        let allocation = state.allocate(TEST_REGION_PAGES - 1, AllocationType::User).unwrap();
        let last_page = fixture.base + (TEST_REGION_PAGES - 1) as u64 * UEFI_PAGE_SIZE as u64;

        assert_eq!(state.verify_allocated(last_page, 2), Err(PageAllocError::InvalidAddress));
        assert_eq!(state.verify_allocated(allocation, TEST_REGION_PAGES), Err(PageAllocError::InvalidAddress));
        assert_eq!(state.allocated_page_count(AllocationType::User), TEST_REGION_PAGES - 1);
    }

    #[test]
    fn test_page_allocator_keeps_pages_allocated_when_they_cannot_be_unmapped() {
        // A range that could not be made inaccessible must not go back into the pool: a later
        // allocation would hand out memory that a stale mapping still reaches.
        let fixture = AllocatorFixture::new();
        let allocation = fixture.allocator.allocate_pages_with_type(2, AllocationType::User).unwrap();
        let free_before = fixture.allocator.free_page_count();

        let result = fixture
            .allocator
            .release_pages_with(allocation, 2, Some(AllocationType::User), |_, _| Err(PageAllocError::UnmapFailed));

        assert_eq!(result, Err(PageAllocError::UnmapFailed));
        assert_eq!(fixture.allocator.free_page_count(), free_before, "the failed range was returned to the pool");
        assert_eq!(fixture.allocator.get_allocation_type(allocation), Some(AllocationType::User));
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::User), 2);
    }

    #[test]
    fn test_page_allocator_mmram_range_checks_do_not_overflow() {
        let fixture = AllocatorFixture::new();
        let state = fixture.allocator.lock_state();

        assert!(state.is_region_inside_mmram(fixture.base, TEST_REGION_BYTES as u64));
        assert!(state.is_region_inside_mmram(fixture.base + TEST_REGION_BYTES as u64, 0));
        assert!(!state.is_region_inside_mmram(
            fixture.base + TEST_REGION_BYTES as u64 - UEFI_PAGE_SIZE as u64,
            2 * UEFI_PAGE_SIZE as u64
        ));
        assert!(!state.is_region_inside_mmram(u64::MAX - 1, 2));
    }

    #[test]
    fn test_page_allocator_collects_unaligned_hob_payload_safely() {
        let descriptors = [
            SmramDescriptor {
                physical_start: 0x8000_0000,
                cpu_start: 0x8000_0000,
                physical_size: 2 * UEFI_PAGE_SIZE as u64,
                region_state: 0,
            },
            SmramDescriptor {
                physical_start: 0x9000_0000,
                cpu_start: 0x9000_0000,
                physical_size: UEFI_PAGE_SIZE as u64,
                region_state: EFI_ALLOCATED,
            },
        ];
        let payload = smram_hob_payload(descriptors.len() as u32, &descriptors);
        let mut unaligned = vec![0xFF];
        unaligned.extend_from_slice(&payload);
        let mut regions = [SmramRegion::default(); MAX_TEMP_REGIONS];
        let mut count = 0;

        PageAllocator::collect_smram_regions(&unaligned[1..], &mut regions, &mut count).unwrap();

        assert_eq!(count, 2);
        assert_eq!(regions[0], SmramRegion::new(descriptors[0].physical_start, descriptors[0].physical_size, false));
        assert_eq!(regions[1], SmramRegion::new(descriptors[1].physical_start, descriptors[1].physical_size, true));
    }

    #[test]
    fn test_page_allocator_collect_clamps_truncated_descriptor_list() {
        let descriptor = SmramDescriptor {
            physical_start: 0x8000_0000,
            cpu_start: 0x8000_0000,
            physical_size: UEFI_PAGE_SIZE as u64,
            region_state: 0,
        };
        let payload = smram_hob_payload(2, &[descriptor]);
        let mut regions = [SmramRegion::default(); MAX_TEMP_REGIONS];
        let mut count = 0;

        PageAllocator::collect_smram_regions(&payload, &mut regions, &mut count).unwrap();

        assert_eq!(count, 1);
        assert_eq!(regions[0].base, descriptor.physical_start);
    }

    #[test]
    fn test_page_allocator_collect_handles_short_payload_and_full_output() {
        let descriptor = SmramDescriptor {
            physical_start: 0x8000_0000,
            cpu_start: 0x8000_0000,
            physical_size: UEFI_PAGE_SIZE as u64,
            region_state: 0,
        };
        let payload = smram_hob_payload(1, &[descriptor]);
        let mut regions = [SmramRegion::default(); MAX_TEMP_REGIONS];
        let mut count = 0;

        PageAllocator::collect_smram_regions(
            &payload[..size_of::<SmramReserveHobData>() - 1],
            &mut regions,
            &mut count,
        )
        .unwrap();
        assert_eq!(count, 0);

        count = MAX_TEMP_REGIONS;
        assert_eq!(
            PageAllocator::collect_smram_regions(&payload, &mut regions, &mut count),
            Err(PageAllocError::OutOfMemory)
        );
    }

    #[test]
    fn test_page_allocator_calculates_bookkeeping_requirements() {
        let valid = [SmramRegion::new(0x8000_0000, TEST_REGION_BYTES as u64, false)];
        let allocated = [SmramRegion::new(0x8000_0000, TEST_REGION_BYTES as u64, true)];
        let too_small = [SmramRegion::new(0x8000_0000, 1, false)];
        let unaligned = [SmramRegion::new(0x8000_0001, TEST_REGION_BYTES as u64, false)];

        assert_eq!(PageAllocator::calculate_bookkeeping(&valid), Ok((valid[0].base, 1)));
        assert_eq!(PageAllocator::calculate_bookkeeping(&allocated), Err(PageAllocError::OutOfMemory));
        assert_eq!(PageAllocator::calculate_bookkeeping(&too_small), Err(PageAllocError::OutOfMemory));
        assert_eq!(PageAllocator::calculate_bookkeeping(&unaligned), Err(PageAllocError::NotAligned));
    }

    #[test]
    fn test_page_allocator_classify_mmram_in_regions_matches_the_committed_classifier() {
        // The pre-commit and post-commit classifiers decide the same security questions, so a
        // range must not be judged differently depending on which one a caller reaches for.
        let fixture = AllocatorFixture::new();
        let base = fixture.base;
        let regions = regions_from(&[(base, TEST_REGION_BYTES as u64, false)]);
        let page = UEFI_PAGE_SIZE as u64;
        let span = TEST_REGION_BYTES as u64;

        for (addr, size) in [
            (base, page),
            (base, span),
            (base + span - page, page),
            (base + span, page),
            (base - page, page),
            (base - page, span),
            (base + span - page, span),
            (base, 0),
            (u64::MAX, page),
        ] {
            assert_eq!(
                classify_mmram_in_regions(&regions, addr, size),
                fixture.allocator.classify_mmram(addr, size).expect("fixture allocator is initialized"),
                "classification diverged for 0x{addr:016x} size 0x{size:x}"
            );
        }
    }

    #[test]
    fn test_page_allocator_regions_contain_address() {
        let regions = regions_from(&[(0x1000, 0x2000, false), (0x8000, 0x1000, true)]);

        assert!(regions_contain(&regions, 0x1000));
        assert!(regions_contain(&regions, 0x2fff));
        // Pre-allocated regions still describe MMRAM, so they anchor just as well.
        assert!(regions_contain(&regions, 0x8000));
        assert!(!regions_contain(&regions, 0x0fff));
        assert!(!regions_contain(&regions, 0x3000));
        assert!(!regions_contain(&regions, 0x9000));
        assert!(!regions_contain(&[], 0x1000));
    }

    #[test]
    fn test_page_allocator_regions_contain_address_does_not_overflow() {
        let regions = regions_from(&[(u64::MAX - 0xfff, 0x2000, false)]);

        // The declared extent wraps, so nothing can be shown to be inside it.
        assert!(!regions_contain(&regions, u64::MAX));
    }

    #[test]
    fn test_page_allocator_classify_mmram_in_regions() {
        let regions = regions_from(&[(0x1000, 0x1000, false), (0x2000, 0x1000, true)]);

        assert_eq!(classify_mmram_in_regions(&regions, 0x1000, 0x1000), MmramPlacement::Inside);
        // Adjacent regions cover the range jointly, so this is wholly inside.
        assert_eq!(classify_mmram_in_regions(&regions, 0x1000, 0x2000), MmramPlacement::Inside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x4000, 0x1000), MmramPlacement::Outside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x2800, 0x1000), MmramPlacement::PartlyInside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x0800, 0x1000), MmramPlacement::PartlyInside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x1000, 0), MmramPlacement::Outside);
        // A range whose end overflows fails closed.
        assert_eq!(classify_mmram_in_regions(&regions, u64::MAX, 0x1000), MmramPlacement::PartlyInside);
        assert_eq!(classify_mmram_in_regions(&[], 0x1000, 0x1000), MmramPlacement::Outside);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_empty_returns_none() {
        assert_eq!(coalesced_smrr_range(&[]), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_region_too_small_returns_none() {
        let regions = regions_from(&[(0x0010_0000, MIN_SMRR_SIZE - UEFI_PAGE_SIZE as u64, false)]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_below_1mb_returns_none() {
        // Base below 1 MiB is rejected even when the region is large enough.
        let regions = regions_from(&[(0x0008_0000, SIZE_256KB as u64, false)]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_above_4gb_returns_none() {
        // A range whose end exceeds 4 GiB is rejected.
        let regions = regions_from(&[(0xFFFF_F000, SIZE_256KB as u64, false)]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_overflow_returns_none() {
        let regions = regions_from(&[(u64::MAX - MIN_SMRR_SIZE + 1, MIN_SMRR_SIZE, false)]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_single_valid_region() {
        let base = 0x8000_0000u64;
        let size = SIZE_256KB as u64;
        let regions = regions_from(&[(base, size, false)]);
        assert_eq!(coalesced_smrr_range(&regions), Some(SmramRegion::new(base, size, false)));
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_rejects_non_power_of_two_size() {
        // A region large enough to be selected but whose size is not a power of
        // two fails SMRR verification and is rejected.
        let base = 0x8000_0000u64;
        let regions = regions_from(&[(base, MIN_SMRR_SIZE, false)]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_selects_largest_region() {
        let small = (0x8000_0000u64, SIZE_256KB as u64, false);
        let large = (0x9000_0000u64, SIZE_256KB as u64 * 4, false);
        let regions = regions_from(&[small, large]);
        assert_eq!(coalesced_smrr_range(&regions), Some(SmramRegion::new(large.0, large.1, false)));
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_ignores_pre_allocated_for_selection() {
        // A pre-allocated region cannot be selected as the primary range, so with
        // no other usable region the result is None.
        let regions = regions_from(&[(0x8000_0000, SIZE_256KB as u64, true)]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_coalesces_adjacent_upward() {
        // Selected (larger) region extends upward into the adjacent region; the
        // coalesced size is a power of two with a naturally aligned base.
        let base = 0x8000_0000u64;
        let size = SIZE_256KB as u64 * 6; // selected as the largest region
        let above_size = SIZE_256KB as u64 * 2;
        let above = (base + size, above_size, false);
        let regions = regions_from(&[(base, size, false), above]);
        assert_eq!(coalesced_smrr_range(&regions), Some(SmramRegion::new(base, size + above_size, false)));
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_coalesces_adjacent_downward() {
        // Selected (larger) region extends downward into the adjacent region;
        // the coalesced size is a power of two with a naturally aligned base.
        let low_base = 0x8000_0000u64;
        let below_size = SIZE_256KB as u64 * 2;
        let base = low_base + below_size;
        let size = SIZE_256KB as u64 * 6; // selected as the largest region
        let below = (low_base, below_size, false);
        let regions = regions_from(&[below, (base, size, false)]);
        assert_eq!(coalesced_smrr_range(&regions), Some(SmramRegion::new(low_base, size + below_size, false)));
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_rejects_non_power_of_two_coalesced_size() {
        // Coalescing yields 0xC0000 bytes, which is not a power of two, so the
        // range is rejected rather than causing a later panic in smrr_initialize.
        let base = 0x8000_0000u64;
        let size = SIZE_256KB as u64 * 2; // 0x80000, selected
        let above = (base + size, SIZE_256KB as u64, false); // + 0x40000 => 0xC0000
        let regions = regions_from(&[(base, size, false), above]);
        assert_eq!(coalesced_smrr_range(&regions), None);
    }

    #[test]
    fn test_page_allocator_coalesced_smrr_range_coalesces_pre_allocated_adjacent() {
        let base = 0x8000_0000u64;
        let size = SIZE_256KB as u64;
        // A physically adjacent pre-allocated region is still coalesced, since the
        // SMRR must cover a single contiguous physical range.
        let above = (base + size, size, true);
        let regions = regions_from(&[(base, size, false), above]);
        assert_eq!(coalesced_smrr_range(&regions), Some(SmramRegion::new(base, size * 2, false)));
    }
}
