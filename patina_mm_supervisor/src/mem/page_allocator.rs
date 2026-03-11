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

use patina::{
    base::UEFI_PAGE_SIZE,
    pi::hob::{Hob, PhaseHandoffInformationTable},
    uefi_pages_to_size, uefi_size_to_pages,
};
use patina_paging::{MemoryAttributes, PageTable};
use r_efi::efi;
use spin::{Mutex, MutexGuard, relax::Spin};

/// Bits per byte.
const BITS_PER_BYTE: usize = 8;

/// EFI_ALLOCATED bit in RegionState.
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
    /// No free pages available to satisfy the request.
    OutOfMemory,
    /// The requested address is not aligned to page boundary.
    NotAligned,
    /// The address is not within any known SMRAM region.
    InvalidAddress,
    /// The address was not previously allocated.
    NotAllocated,
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

/// SMRAM descriptor structure matching EFI_SMRAM_DESCRIPTOR.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SmramDescriptor {
    /// Physical start address of the SMRAM region.
    pub physical_start: efi::PhysicalAddress,
    /// CPU start address (may differ from physical for remapping).
    pub cpu_start: efi::PhysicalAddress,
    /// Size of the SMRAM region in bytes.
    pub physical_size: u64,
    /// Region state flags (EFI_ALLOCATED, etc.).
    pub region_state: u64,
}

/// SMRAM reserve descriptor count structure.
/// This is the data that immediately follows a GuidHob with SMM_SMRAM_MEMORY_GUID
/// or MM_PEI_MMRAM_MEMORY_RESERVE_GUID.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
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
    _guard: MutexGuard<'a, StatePtr, Spin>,
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
            let region_end = region.base + uefi_pages_to_size!(region.total_pages) as u64;
            if addr >= region.base && addr < region_end {
                let page_in_region = uefi_size_to_pages!((addr - region.base) as usize);
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
        log::trace!("Allocated {} {:?} page(s) at 0x{:016x}", num_pages, alloc_type, addr);
        Some(addr)
    }

    /// Frees `num_pages` starting at `addr`, verifying the pages are allocated.
    fn free(&mut self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
        let (region_index, page_in_region) =
            self.find_region_for_address(addr).ok_or(PageAllocError::InvalidAddress)?;
        let base_bit =
            self.regions().get(region_index).ok_or(PageAllocError::InvalidAddress)?.bitmap_start_bit + page_in_region;

        // Verify all pages are allocated
        for p in 0..num_pages {
            if !self.is_bit_allocated(base_bit + p) {
                return Err(PageAllocError::NotAllocated);
            }
        }
        // Free the pages
        for p in 0..num_pages {
            self.set_bit_free(base_bit + p);
        }
        log::trace!("Freed {} page(s) at 0x{:016x}", num_pages, addr);
        Ok(())
    }

    /// Frees `num_pages` starting at `addr`, verifying the allocation type matches.
    fn free_checked(
        &mut self,
        addr: u64,
        num_pages: usize,
        expected_type: AllocationType,
    ) -> Result<(), PageAllocError> {
        let (region_index, page_in_region) =
            self.find_region_for_address(addr).ok_or(PageAllocError::InvalidAddress)?;
        let base_bit =
            self.regions().get(region_index).ok_or(PageAllocError::InvalidAddress)?.bitmap_start_bit + page_in_region;

        // Verify all pages are allocated with the expected type
        for p in 0..num_pages {
            let bit = base_bit + p;
            if !self.is_bit_allocated(bit) {
                return Err(PageAllocError::NotAllocated);
            }
            if self.bit_type(bit) != expected_type {
                log::warn!(
                    "Type mismatch at 0x{:016x}: expected {:?}, got {:?}",
                    addr + uefi_pages_to_size!(p) as u64,
                    expected_type,
                    self.bit_type(bit)
                );
                return Err(PageAllocError::InvalidAddress);
            }
        }
        // Free the pages
        for p in 0..num_pages {
            self.set_bit_free(base_bit + p);
        }
        log::trace!("Freed {} {:?} page(s) at 0x{:016x}", num_pages, expected_type, addr);
        Ok(())
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
        self.regions().iter().any(|region| {
            let region_end = region.base + uefi_pages_to_size!(region.total_pages) as u64;
            addr >= region.base && (addr + size) <= region_end
        })
    }

    /// Populates freshly-zeroed bookkeeping with per-region metadata and marks
    /// the pre-allocated regions and the bookkeeping pages themselves as
    /// supervisor-allocated.
    ///
    /// The `AllocatorState` header (including `region_count`) must already be
    /// written so that [`regions_mut`](Self::regions_mut) exposes the full array.
    fn initialize(&mut self, scanned: &ScannedRegions, bookkeeping_base: u64, bookkeeping_pages: usize) {
        // Fill in per-region metadata and assign each region its bitmap range.
        let mut bitmap_start_bit = 0usize;
        for (region, &(base, size, _)) in self.regions_mut().iter_mut().zip(scanned.regions.iter()) {
            let pages = uefi_size_to_pages!(size as usize);
            region.base = base;
            region.total_pages = pages;
            region.bitmap_start_bit = bitmap_start_bit;
            bitmap_start_bit += pages;
        }

        // Mark pre-allocated regions and the bookkeeping pages as allocated (supervisor).
        for i in 0..scanned.count {
            let Some(&(base, size, pre_allocated)) = scanned.regions.get(i) else {
                continue;
            };
            let pages = uefi_size_to_pages!(size as usize);
            let Some(start_bit) = self.regions().get(i).map(|region| region.bitmap_start_bit) else {
                continue;
            };

            if pre_allocated {
                // Mark the entire region as allocated.
                for p in 0..pages {
                    self.set_bit_allocated(start_bit + p, AllocationType::Supervisor);
                }
            } else if base == bookkeeping_base {
                // Mark just the bookkeeping pages at the start of this region.
                for p in 0..bookkeeping_pages {
                    self.set_bit_allocated(start_bit + p, AllocationType::Supervisor);
                }
            }
        }
    }
}

/// Maximum number of SMRAM regions collected on the stack while scanning the
/// HOB list. The authoritative region metadata is stored in SMRAM afterwards.
const MAX_TEMP_REGIONS: usize = 256;

/// SMRAM regions discovered while scanning the HOB list, before they are copied
/// into the SMRAM bookkeeping structures.
struct ScannedRegions {
    /// Discovered regions as `(base, size, pre_allocated)`.
    regions: [(u64, u64, bool); MAX_TEMP_REGIONS],
    /// Number of valid entries in `regions`.
    count: usize,
    /// Total pages across all discovered regions.
    total_pages: usize,
    /// Base of the first non-pre-allocated region (used to host bookkeeping).
    first_free_base: Option<u64>,
    /// Size in bytes of the first non-pre-allocated region.
    first_free_size: u64,
}

impl ScannedRegions {
    /// Creates an empty scan result.
    fn new() -> Self {
        Self {
            regions: [(0, 0, false); MAX_TEMP_REGIONS],
            count: 0,
            total_pages: 0,
            first_free_base: None,
            first_free_size: 0,
        }
    }

    /// Whether the temporary storage is full.
    fn is_full(&self) -> bool {
        self.count >= MAX_TEMP_REGIONS
    }

    /// Records one region. Precondition: `!self.is_full()`.
    fn push(&mut self, base: u64, size: u64, pre_allocated: bool) {
        if let Some(slot) = self.regions.get_mut(self.count) {
            *slot = (base, size, pre_allocated);
        }
        if self.first_free_base.is_none() && !pre_allocated {
            self.first_free_base = Some(base);
            self.first_free_size = size;
        }
        self.total_pages += uefi_size_to_pages!(size as usize);
        self.count += 1;
    }
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

    /// Calculates the number of pages needed for bookkeeping.
    fn calculate_bookkeeping_pages(region_count: usize, total_pages: usize) -> usize {
        let header_size = size_of::<AllocatorState>();
        let regions_size = region_count * size_of::<RegionInfo>();
        let bitmap_bytes = total_pages.div_ceil(BITS_PER_BYTE);
        let total_bytes = header_size + regions_size + bitmap_bytes * 2; // alloc + type bitmaps
        uefi_size_to_pages!(total_bytes)
    }

    /// Scans the entire HOB list and collects every SMRAM/MMRAM region reported
    /// under the supported GUIDs into `scanned`.
    fn scan_smram_regions(handoff: &PhaseHandoffInformationTable, scanned: &mut ScannedRegions) {
        let hob = Hob::Handoff(handoff);
        for current_hob in &hob {
            if let Hob::GuidHob(guid_hob, data) = current_hob
                && (guid_hob.name == SMM_SMRAM_MEMORY_GUID || guid_hob.name == MM_PEI_MMRAM_MEMORY_RESERVE_GUID)
            {
                log::info!("Found SMRAM memory HOB with GUID {}", guid_hob.name.as_guid());
                Self::collect_smram_regions(data, scanned);
            }
        }
    }

    /// Parses the `SmramReserveHobData` header and trailing `SmramDescriptor`
    /// array from a single matching GUID HOB payload, appending each region to
    /// `scanned`.
    fn collect_smram_regions(data: &[u8], scanned: &mut ScannedRegions) {
        let header_size = size_of::<SmramReserveHobData>();
        if data.len() < header_size {
            return;
        }

        // SAFETY: `data` is at least `header_size` bytes (checked above) and, per the
        // `init_from_hob_list` contract, begins with a suitably aligned `SmramReserveHobData`
        // header. Materialize it once; reading its fields below is then ordinary safe access.
        let header = unsafe { &*(data.as_ptr() as *const SmramReserveHobData) };

        // Clamp the declared count to what the payload can actually hold, so the descriptor
        // bytes below are guaranteed in-bounds even if the HOB is malformed.
        let max_fit = (data.len() - header_size) / size_of::<SmramDescriptor>();
        let declared = header.number_of_smram_regions as usize;
        if declared > max_fit {
            log::warn!("SMRAM HOB declares {} descriptors but only {} fit in the payload", declared, max_fit);
        }
        let count = declared.min(max_fit);

        // Take the descriptor region as a safe, bounds-checked sub-slice (this is where an
        // out-of-range offset would be caught).
        let end = header_size + count * size_of::<SmramDescriptor>();
        let Some(descriptor_bytes) = data.get(header_size..end) else {
            return;
        };

        // SAFETY: `descriptor_bytes` is exactly `count` `SmramDescriptor`s' worth of in-bounds
        // bytes, and per the `init_from_hob_list` contract the payload is suitably aligned for
        // `SmramDescriptor`. The loop below is then fully safe slice iteration.
        let descriptors = unsafe { slice::from_raw_parts(descriptor_bytes.as_ptr() as *const SmramDescriptor, count) };

        for descriptor in descriptors {
            if scanned.is_full() {
                log::warn!("Too many SMRAM regions for temp storage, increase MAX_TEMP_REGIONS");
                break;
            }

            let pre_allocated = (descriptor.region_state & EFI_ALLOCATED) != 0;
            let pages = uefi_size_to_pages!(descriptor.physical_size as usize);

            log::info!(
                "SMRAM Region {}: base=0x{:016x}, size=0x{:x}, pages={}, state=0x{:x}, allocated={}",
                scanned.count,
                descriptor.physical_start,
                descriptor.physical_size,
                pages,
                descriptor.region_state,
                pre_allocated
            );

            scanned.push(descriptor.physical_start, descriptor.physical_size, pre_allocated);
        }
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
        LockedState { state, region_count, total_pages, _guard: guard }
    }

    /// Initializes the page allocator from the HOB list.
    ///
    /// This function:
    /// 1. Scans HOBs to count regions and total pages
    /// 2. Finds the first non-allocated region for bookkeeping
    /// 3. Reserves pages for bookkeeping structures
    /// 4. Initializes the bitmaps
    ///
    /// ## Safety
    ///
    /// The caller must ensure that `hob_list` points to a valid HOB list. Only null pointer will be rejected.
    pub unsafe fn init_from_hob_list(&self, hob_list: *const c_void) -> Result<(), PageAllocError> {
        if hob_list.is_null() {
            return Err(PageAllocError::NotInitialized);
        }

        let mut guard = self.state.lock();

        // SAFETY: per this function's contract, a non-null `hob_list` points to a valid HOB
        // list whose first entry is the Phase Handoff Information Table.
        let hob_list_info = unsafe {
            (hob_list as *const PhaseHandoffInformationTable).as_ref().ok_or(PageAllocError::NotInitialized)?
        };

        // First pass: scan the HOB list for every SMRAM region.
        let mut scanned = ScannedRegions::new();
        Self::scan_smram_regions(hob_list_info, &mut scanned);

        if scanned.count == 0 {
            log::error!("No SMRAM regions found in HOB list");
            return Err(PageAllocError::NotInitialized);
        }

        // Reserve bookkeeping space in the first free region.
        let bookkeeping_pages = Self::calculate_bookkeeping_pages(scanned.count, scanned.total_pages);

        log::info!(
            "Allocator needs {} pages for bookkeeping ({} regions, {} total pages)",
            bookkeeping_pages,
            scanned.count,
            scanned.total_pages
        );

        let bookkeeping_base = scanned.first_free_base.ok_or_else(|| {
            log::error!("No free SMRAM region available for bookkeeping");
            PageAllocError::OutOfMemory
        })?;

        if uefi_pages_to_size!(bookkeeping_pages) as u64 > scanned.first_free_size {
            log::error!("First free region too small for bookkeeping");
            return Err(PageAllocError::OutOfMemory);
        }

        log::info!("Using 0x{:016x} for bookkeeping ({} pages)", bookkeeping_base, bookkeeping_pages);

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
        *state = AllocatorState {
            region_count: scanned.count,
            total_pages: scanned.total_pages,
            bookkeeping_pages,
            bookkeeping_base,
        };

        *guard = StatePtr(state_ptr);

        // Populate region metadata and the allocation bitmaps under the held lock, then
        // release the lock before the stats logging below re-acquires it.
        let mut locked = LockedState {
            state: state_ptr,
            region_count: scanned.count,
            total_pages: scanned.total_pages,
            _guard: guard,
        };
        locked.initialize(&scanned, bookkeeping_base, bookkeeping_pages);
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
    fn apply_data_page_attributes(&self, addr: u64, num_pages: usize, _alloc_type: AllocationType) {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        if let Some(ref mut pt) = *pt_guard {
            // Data pages: R/W (no ReadOnly) + NX (ExecuteProtect)
            let mut attributes = MemoryAttributes::ExecuteProtect;

            if _alloc_type == AllocationType::Supervisor {
                // For Supervisor allocations, we additionally want the U/S bit cleared (Supervisor-only).
                attributes |= MemoryAttributes::Supervisor; // Ensure not writable by user code
            }

            if let Err(e) = pt.map_memory_region(addr, size, attributes) {
                log::error!(
                    "Failed to set supervisor page attributes for 0x{:016x} ({} pages): {:?}",
                    addr,
                    num_pages,
                    e
                );
            } else {
                log::trace!("Marked 0x{:016x} ({} pages) as supervisor R/W+NX", addr, num_pages,);
            }
        } else {
            log::warn!("Page table not initialized, skipping attribute update for 0x{:016x}", addr);
        }
    }

    /// Applies restrictive page table attributes to freed pages.
    ///
    /// Marks pages as completely inaccessible: Supervisor + ReadProtect + ExecuteProtect (NX).
    /// This prevents any read, write, or execute access to freed memory, mitigating
    /// use-after-free vulnerabilities.
    ///
    /// If the global page table is not yet initialized (e.g., during early boot),
    /// this is a no-op with a warning.
    fn apply_freed_page_attributes(&self, addr: u64, num_pages: usize) {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        if let Some(ref mut pt) = *pt_guard {
            // Freed pages: ReadProtect (not present) + NX (no execute) + ReadOnly (no write)
            // This makes the pages completely inaccessible.
            if let Err(e) = pt.unmap_memory_region(addr, size) {
                log::error!("Failed to set freed page attributes for 0x{:016x} ({} pages): {:?}", addr, num_pages, e);
            } else {
                log::trace!("Marked 0x{:016x} ({} pages) as inaccessible (RP+NX+RO+S)", addr, num_pages,);
            }
        } else {
            log::warn!("Page table not initialized, skipping freed page attribute update for 0x{:016x}", addr);
        }
    }

    /// Frees previously allocated pages.
    ///
    /// After freeing, the pages are marked as inaccessible in the page table
    /// (Supervisor + ReadProtect + ExecuteProtect) to prevent use-after-free.
    pub fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
        if !self.is_initialized() {
            return Err(PageAllocError::NotInitialized);
        }

        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            return Err(PageAllocError::NotAligned);
        }

        self.lock_state().free(addr, num_pages)?;

        // Mark freed pages as inaccessible in the page table.
        self.apply_freed_page_attributes(addr, num_pages);

        Ok(())
    }

    /// Frees previously allocated pages, verifying the allocation type matches.
    ///
    /// After freeing, the pages are marked as inaccessible in the page table
    /// (Supervisor + ReadProtect + ExecuteProtect) to prevent use-after-free.
    pub fn free_pages_checked(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: AllocationType,
    ) -> Result<(), PageAllocError> {
        if !self.is_initialized() {
            return Err(PageAllocError::NotInitialized);
        }

        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            return Err(PageAllocError::NotAligned);
        }

        self.lock_state().free_checked(addr, num_pages, expected_type)?;

        // Mark freed pages as inaccessible in the page table.
        self.apply_freed_page_attributes(addr, num_pages);

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
}
