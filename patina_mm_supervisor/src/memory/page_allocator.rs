//! MM Supervisor Core Page Allocator
//!
//! A page-granularity allocator over the MMRAM regions the HOB list describes. Each page is
//! tracked as allocated or free and as supervisor- or user-owned, in bookkeeping pages taken from
//! the first free region.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    mem::size_of,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use patina::{
    MemoryRange, UEFI_PAGE_SIZE,
    pi::hob::{Hob, PhaseHandoffInformationTable},
    pi::mmram::{
        EFI_ALLOCATED, MM_PEI_MMRAM_MEMORY_RESERVE_GUID, SMM_SMRAM_MEMORY_GUID, SmramDescriptor, SmramReserveHobData,
    },
    uefi_pages_to_size, uefi_size_to_pages,
};
use patina_paging::{MemoryAttributes, PageTable};
use spin::Mutex;
use zerocopy::FromBytes;

use super::mmram::MmramRegion;
use crate::memory::AllocError;
use crate::memory::locked_state::{AllocatorState, LockedState, StatePtr};
use crate::memory::mmram::MmramPlacement;
use crate::{error::MmSupervisorResult, user_access_guard::with_user_access};

/// Maximum number of SMRAM regions collected on the stack while scanning the HOB list.
pub(crate) const MAX_TEMP_REGIONS: usize = 256;

/// Which ring owns an allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AllocationType {
    /// Supervisor-owned pages, mapped supervisor-only (for example, core data structures).
    Supervisor = 0,
    /// User-owned pages that Ring 3 can access (for example, MM driver allocations).
    User = 1,
}

/// Metadata for one SMRAM region, stored in the bookkeeping pages.
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

impl RegionInfo {
    /// Returns the region's address range, or `None` if its page count does not describe one.
    pub(crate) fn range(&self) -> Option<MemoryRange> {
        MemoryRange::from_pages(self.base, u64::try_from(self.total_pages).ok()?).ok()
    }
}

/// The SMRAM regions collected from the HOB list, held in a fixed array on the stack.
///
/// Only the collected regions are visible, through [`as_slice`](Self::as_slice).
pub(crate) struct ScannedRegions {
    regions: [MmramRegion; MAX_TEMP_REGIONS],
    count: usize,
}

impl ScannedRegions {
    /// Fills the slots that hold no collected region.
    const UNUSED: MmramRegion = match MemoryRange::new(0, 1) {
        Ok(range) => MmramRegion::new(range, false),
        Err(_) => panic!("a one-byte range at address 0 is valid"),
    };

    /// Creates an empty collection.
    const fn new() -> Self {
        Self { regions: [Self::UNUSED; MAX_TEMP_REGIONS], count: 0 }
    }

    /// Appends `region`, or fails when all [`MAX_TEMP_REGIONS`] slots are in use.
    fn push(&mut self, region: MmramRegion) -> MmSupervisorResult<()> {
        let slot = self.regions.get_mut(self.count).ok_or_else(|| {
            log::error!(
                "Too many SMRAM regions for temp storage (MAX_TEMP_REGIONS = {MAX_TEMP_REGIONS}), increase MAX_TEMP_REGIONS"
            );
            AllocError::OutOfMemory
        })?;
        *slot = region;
        self.count += 1;
        Ok(())
    }

    /// Returns the collected regions.
    pub(crate) fn as_slice(&self) -> &[MmramRegion] {
        self.regions.get(..self.count).unwrap_or_default()
    }
}

/// Page-granularity allocator for SMRAM memory.
pub struct PageAllocator {
    /// Pointer to the bookkeeping in SMRAM. Every access to the bookkeeping holds this lock.
    state: Mutex<StatePtr>,
    /// Whether the allocator has been initialized.
    initialized: AtomicBool,
}

impl PageAllocator {
    /// Creates a new uninitialized page allocator.
    pub const fn new() -> Self {
        Self { state: Mutex::new(StatePtr(ptr::null_mut())), initialized: AtomicBool::new(false) }
    }

    /// Returns `(bookkeeping_base, bookkeeping_pages)`: the bookkeeping is placed at the start of
    /// the first region that is not pre-allocated, which must be large enough to hold it.
    fn calculate_bookkeeping(regions: &[MmramRegion]) -> MmSupervisorResult<(u64, usize)> {
        let total_pages = regions.iter().try_fold(0usize, |total, region| {
            let pages = usize::try_from(region.range.pages()).map_err(|_| {
                log::error!("Bookkeeping sizing failed: region {} does not fit a page count", region.range);
                AllocError::OutOfMemory
            })?;
            total.checked_add(pages).ok_or_else(|| {
                log::error!("Bookkeeping sizing failed: total page count overflows at {}", region.range);
                AllocError::OutOfMemory
            })
        })?;

        let header_size = size_of::<AllocatorState>();
        let regions_size = regions.len().checked_mul(size_of::<RegionInfo>()).ok_or_else(|| {
            log::error!("Bookkeeping sizing failed: {} regions overflow the region table", regions.len());
            AllocError::OutOfMemory
        })?;
        let bitmap_bytes = total_pages.div_ceil(u8::BITS as usize);
        let bitmaps_size = bitmap_bytes.checked_mul(2).ok_or_else(|| {
            log::error!("Bookkeeping sizing failed: {bitmap_bytes} bitmap bytes overflow when doubled");
            AllocError::OutOfMemory
        })?;
        let total_bytes =
            header_size.checked_add(regions_size).and_then(|size| size.checked_add(bitmaps_size)).ok_or_else(|| {
                log::error!("Bookkeeping sizing failed: header, regions and bitmaps overflow a usize");
                AllocError::OutOfMemory
            })?;
        let bookkeeping_pages = uefi_size_to_pages!(total_bytes);

        log::info!(
            "Allocator needs {} pages for bookkeeping ({} regions, {} total pages)",
            bookkeeping_pages,
            regions.len(),
            total_pages
        );

        let first_free =
            regions.iter().find(|region| !region.pre_allocated).map(|region| region.range).ok_or_else(|| {
                log::error!("No free SMRAM region available for bookkeeping");
                AllocError::OutOfMemory
            })?;
        let bookkeeping_base = first_free.base();
        if bookkeeping_base == 0 || !bookkeeping_base.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("Bookkeeping region base is null or not page-aligned");
            return Err(AllocError::InvalidAlignment.into());
        }

        let bookkeeping_size = bookkeeping_pages.checked_mul(UEFI_PAGE_SIZE).ok_or(AllocError::OutOfMemory)?;
        if u64::try_from(bookkeeping_size).map_or(true, |size| size > first_free.size()) {
            log::error!("First free region too small for bookkeeping");
            return Err(AllocError::OutOfMemory.into());
        }

        Ok((bookkeeping_base, bookkeeping_pages))
    }

    /// Appends each `SmramDescriptor` in one SMRAM reserve HOB payload to `scanned`.
    ///
    /// Checks:
    /// - The payload holds its header and every descriptor it declares.
    /// - Each descriptor is a valid, non-empty [`MemoryRange`].
    fn collect_smram_regions(data: &[u8], scanned: &mut ScannedRegions) -> MmSupervisorResult<()> {
        let (header, descriptor_bytes) = SmramReserveHobData::read_from_prefix(data).map_err(|_| {
            log::error!("SMRAM HOB payload of {} bytes is too short for its header", data.len());
            AllocError::InvalidAddress
        })?;

        let max_fit = descriptor_bytes.len() / size_of::<SmramDescriptor>();
        let declared = header.number_of_smram_regions as usize;
        if declared > max_fit {
            log::error!("SMRAM HOB declares {declared} descriptors but only {max_fit} fit in the payload");
            return Err(AllocError::InvalidAddress.into());
        }

        let (descriptor_chunks, _) = descriptor_bytes.as_chunks::<{ size_of::<SmramDescriptor>() }>();
        for descriptor_bytes in descriptor_chunks.iter().take(declared) {
            let (descriptor, _) =
                SmramDescriptor::read_from_prefix(descriptor_bytes).map_err(|_| AllocError::InvalidAddress)?;

            let pre_allocated = (descriptor.region_state & EFI_ALLOCATED) != 0;
            let range = MemoryRange::new(descriptor.physical_start, descriptor.physical_size).map_err(|err| {
                log::error!(
                    "SMRAM descriptor [0x{:016x}, +0x{:x}) is not a valid range: {err}",
                    descriptor.physical_start,
                    descriptor.physical_size
                );
                AllocError::InvalidAddress
            })?;

            log::info!(
                "SMRAM Region {}: {range}, pages={}, state=0x{:x}, allocated={}",
                scanned.as_slice().len(),
                range.pages(),
                descriptor.region_state,
                pre_allocated
            );

            scanned.push(MmramRegion::new(range, pre_allocated))?;
        }

        Ok(())
    }

    /// Acquires the state lock and returns a [`LockedState`] view over the bookkeeping.
    fn lock_state(&self) -> LockedState<'_> {
        let guard = self.state.lock();
        let state = guard.0;

        // Cache the header sizes so the view's length accessors stay safe.
        let (region_count, total_pages) = if state.is_null() {
            (0, 0)
        } else {
            // SAFETY: when non-null, `state` is a valid initialized header, and the lock is held.
            unsafe { ((*state).region_count, (*state).total_pages) }
        };
        LockedState { state, region_count, total_pages, guard }
    }

    /// Collects the regions from every SMRAM and MMRAM reserve HOB into a stack array.
    ///
    /// Kept out of line so the copies of the `ScannedRegions` array in its frame do not stay on
    /// the small MM stack for the whole of BSP initialization.
    ///
    /// ## Safety
    ///
    /// `handoff` must start a HOB list that passed
    /// [`validate_hob_list_structure`](crate::hob::validation::validate_hob_list_structure).
    #[inline(never)]
    pub(crate) unsafe fn scan_hob_list_for_smram_regions(
        handoff: &PhaseHandoffInformationTable,
    ) -> MmSupervisorResult<ScannedRegions> {
        let mut scanned = ScannedRegions::new();
        for current_hob in &Hob::Handoff(handoff) {
            if let Hob::GuidHob(guid_hob, data) = current_hob
                && (guid_hob.name == SMM_SMRAM_MEMORY_GUID || guid_hob.name == MM_PEI_MMRAM_MEMORY_RESERVE_GUID)
            {
                log::info!("Found SMRAM memory HOB with GUID {}", guid_hob.name.as_guid());
                Self::collect_smram_regions(data, &mut scanned)?;
            }
        }

        if scanned.as_slice().is_empty() {
            log::error!("No SMRAM regions found in HOB list");
            return Err(AllocError::NotInitialized.into());
        }

        Ok(scanned)
    }

    /// Initializes the bookkeeping from validated SMRAM regions and returns the pages it occupies.
    ///
    /// Checks:
    /// - `scanned` is not empty.
    /// - Every region is page aligned and no two overlap.
    /// - The allocator is not already initialized.
    ///
    /// ## Safety
    ///
    /// Every region must be valid memory for its full size, and regions that are not
    /// pre-allocated must be exclusively owned.
    pub(crate) unsafe fn init_from_regions(&self, scanned: &[MmramRegion]) -> MmSupervisorResult<MemoryRange> {
        if scanned.is_empty() {
            log::error!("Page allocator init failed: no SMRAM regions were supplied");
            return Err(AllocError::NotInitialized.into());
        }

        for (index, region) in scanned.iter().enumerate() {
            let Ok(true) = region.range.is_aligned(UEFI_PAGE_SIZE as u64) else {
                log::error!("SMRAM region {index} {} is not page aligned", region.range);
                return Err(AllocError::InvalidAlignment.into());
            };
        }
        if let Some((first, second)) = MemoryRange::find_overlap(scanned.iter().map(|region| region.range)) {
            log::error!("SMRAM regions {first} and {second} overlap");
            return Err(AllocError::InvalidAddress.into());
        }

        let guard = self.state.lock();
        if self.initialized.load(Ordering::Acquire) {
            log::error!("Page allocator init failed: the allocator is already initialized");
            return Err(AllocError::AlreadyInitialized.into());
        }

        let total_pages = scanned.iter().try_fold(0usize, |total, region| {
            let pages = usize::try_from(region.range.pages()).map_err(|_| {
                log::error!("SMRAM region {} does not fit a page count", region.range);
                AllocError::OutOfMemory
            })?;
            total.checked_add(pages).ok_or_else(|| {
                log::error!("Total SMRAM page count overflows at region {}", region.range);
                AllocError::OutOfMemory
            })
        })?;

        let (bookkeeping_base, bookkeeping_pages) = Self::calculate_bookkeeping(scanned)?;
        // `calculate_bookkeeping` placed these pages inside the first free region.
        let bookkeeping = MemoryRange::from_pages(bookkeeping_base, bookkeeping_pages as u64)
            .map_err(|_| AllocError::InvalidAddress)?;

        log::info!("Using {bookkeeping} for bookkeeping ({bookkeeping_pages} pages)");

        let state_ptr = bookkeeping_base as *mut AllocatorState;

        // SAFETY: the bookkeeping pages are page aligned, reserved for this use, and the lock is
        // held. Zeroing makes the header and bitmaps valid, and this is the only reference.
        let state = unsafe {
            super::write_zeros_volatile(bookkeeping_base as *mut u8, uefi_pages_to_size!(bookkeeping_pages));
            &mut *state_ptr
        };
        *state = AllocatorState { region_count: scanned.len(), total_pages };

        // Release the lock before the logging below takes it again.
        let mut locked = LockedState { state: state_ptr, region_count: scanned.len(), total_pages, guard };
        locked.initialize(scanned, bookkeeping_base, bookkeeping_pages)?;
        locked.guard.0 = state_ptr;
        drop(locked);

        self.initialized.store(true, Ordering::Release);

        log::info!(
            "Page allocator initialized: {} region(s), {} total pages, {} free pages, {} allocated supervisor pages, {} allocated user pages",
            self.region_count(),
            self.total_page_count(),
            self.free_page_count(),
            self.allocated_page_count(AllocationType::Supervisor),
            self.allocated_page_count(AllocationType::User)
        );

        Ok(bookkeeping)
    }

    /// Allocates contiguous pages from SMRAM for supervisor use.
    pub fn allocate_pages(&self, num_pages: usize) -> MmSupervisorResult<u64> {
        self.allocate_pages_with_type(num_pages, AllocationType::Supervisor)
    }

    /// Allocates contiguous pages from SMRAM, mapped as R/W, non-executable data that only the
    /// supervisor or also Ring 3 can reach, per `alloc_type`. User pages are returned zeroed.
    pub fn allocate_pages_with_type(&self, num_pages: usize, alloc_type: AllocationType) -> MmSupervisorResult<u64> {
        if !self.is_initialized() {
            log::error!("Page allocation of {num_pages} page(s) rejected: allocator is not initialized");
            return Err(AllocError::NotInitialized.into());
        }

        if num_pages == 0 {
            log::error!("Page allocation rejected: page count is 0");
            return Err(AllocError::InvalidSize.into());
        }

        // The state lock is released before the page table lock is taken.
        let addr = self.lock_state().allocate(num_pages, alloc_type).ok_or_else(|| {
            log::error!("Page allocation of {num_pages} {alloc_type:?} page(s) failed: no free run that large");
            AllocError::OutOfMemory
        })?;

        if let Err(err) = self.apply_data_page_attributes(addr, num_pages, alloc_type) {
            // Hand the reservation back so a failed mapping does not leak the pages.
            let mut state = self.lock_state();
            if let Ok(bit_range) = state.verify_allocated(addr, num_pages) {
                state.mark_free(bit_range);
            }
            return Err(err);
        }

        // Pages handed out for the first time still hold what MMRAM held at startup.
        if alloc_type == AllocationType::User {
            Self::zero_pages(addr, num_pages);
        }

        Ok(addr)
    }

    /// Maps a range as R/W, NX data, supervisor-only for [`AllocationType::Supervisor`].
    ///
    /// Does nothing if the page table is not installed yet.
    ///
    /// ## Errors
    ///
    /// Returns `MapFailed` if the page table rejects the mapping.
    fn apply_data_page_attributes(
        &self,
        addr: u64,
        num_pages: usize,
        alloc_type: AllocationType,
    ) -> MmSupervisorResult<()> {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        if let Some(ref mut pt) = *pt_guard {
            // R/W (no ReadOnly) and NX (ExecuteProtect).
            let mut attributes = MemoryAttributes::ExecuteProtect;

            if alloc_type == AllocationType::Supervisor {
                attributes |= MemoryAttributes::Supervisor;
            }

            if let Err(err) = pt.map_memory_region(addr, size, attributes) {
                log::error!(
                    "Failed to set {alloc_type:?} page attributes for 0x{addr:016x} ({num_pages} pages): {err:?}"
                );
                return Err(AllocError::MapFailed.into());
            }
            log::trace!("Marked 0x{addr:016x} ({num_pages} pages) as {alloc_type:?} R/W+NX");
        } else {
            log::warn!("Page table not initialized, skipping attribute update for 0x{addr:016x}");
        }
        Ok(())
    }

    /// Zeros a mapped page range, so a later `User` allocation cannot read old contents. On a free
    /// this must run before [`Self::apply_freed_page_attributes`] unmaps the range.
    fn zero_pages(addr: u64, num_pages: usize) {
        // SAFETY: the caller holds the range as a live allocation in one SMRAM region and has
        // mapped it R/W, so the write stays inside it. SMAP is lifted because the range may be
        // user-owned.
        unsafe {
            with_user_access(|| {
                super::write_zeros_volatile(addr as *mut u8, uefi_pages_to_size!(num_pages));
            });
        }
    }

    /// Unmaps a freed range so any later access to it faults.
    ///
    /// Reports success, with a warning, when the page table is not installed yet, which only
    /// happens before [`init_page_table`](crate::MmSupervisorCore::init_page_table).
    fn apply_freed_page_attributes(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<()> {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        let Some(pt) = pt_guard.as_mut() else {
            log::warn!("Page table not initialized, skipping freed page attribute update for 0x{addr:016x}");
            return Ok(());
        };

        if let Err(err) = pt.unmap_memory_region(addr, size) {
            log::error!("Failed to set freed page attributes for 0x{addr:016x} ({num_pages} pages): {err:?}");
            return Err(AllocError::UnmapFailed.into());
        }

        log::trace!("Marked 0x{addr:016x} ({num_pages} pages) as inaccessible (unmapped)");
        Ok(())
    }

    /// Frees previously allocated pages: zeros them, unmaps them, then returns them to the pool.
    pub fn free_pages(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<()> {
        self.release_pages(addr, num_pages, None)
    }

    /// As [`free_pages`](Self::free_pages), but the pages must have `expected_type`.
    pub fn free_pages_checked(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: AllocationType,
    ) -> MmSupervisorResult<()> {
        self.release_pages(addr, num_pages, Some(expected_type))
    }

    /// Scrubs `[addr, addr + num_pages)`, makes it inaccessible, and only then returns it to the
    /// pool.
    ///
    /// The range stays allocated, under the state lock, until both steps succeed, so no other core
    /// can take it while it is scrubbed or still mapped. The page table lock is only taken after
    /// the state lock.
    fn release_pages(
        &self,
        addr: u64,
        num_pages: usize,
        expected_type: Option<AllocationType>,
    ) -> MmSupervisorResult<()> {
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
        restrict: impl FnOnce(u64, usize) -> MmSupervisorResult<()>,
    ) -> MmSupervisorResult<()> {
        if !self.is_initialized() {
            log::error!("Free of 0x{addr:016x} ({num_pages} pages) rejected: allocator is not initialized");
            return Err(AllocError::NotInitialized.into());
        }

        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("Free of 0x{addr:016x} ({num_pages} pages) rejected: address is not page aligned");
            return Err(AllocError::InvalidAlignment.into());
        }

        let mut state = self.lock_state();
        // Both checks log the reason when they reject the request.
        let bit_range = match expected_type {
            Some(expected_type) => state.verify_allocated_with_type(addr, num_pages, expected_type)?,
            None => state.verify_allocated(addr, num_pages)?,
        };

        // A pre-allocated range keeps the inherited mapping, which may be read-only (the HOB list
        // is), so map it R/W before the scrub. On failure the range stays allocated.
        self.apply_data_page_attributes(addr, num_pages, state.bit_type(bit_range.start))?;
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

    /// Classifies `range` against the MMRAM regions, or `None` if the regions are not known yet.
    pub fn classify_mmram(&self, range: &MemoryRange) -> Option<MmramPlacement> {
        if !self.is_initialized() {
            return None;
        }
        Some(self.lock_state().classify_mmram(range))
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support;

    /// Returns `[base, base + size)` as a range.
    fn range(base: u64, size: u64) -> MemoryRange {
        MemoryRange::new(base, size).unwrap()
    }

    const TEST_REGION_PAGES: usize = 16;
    const TEST_REGION_BYTES: usize = uefi_pages_to_size!(TEST_REGION_PAGES);

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
            let regions = [MmramRegion::new(range(base, TEST_REGION_BYTES as u64), false)];

            // SAFETY: `memory` is page-aligned, exclusively owned by the fixture,
            // remains live with the allocator, and covers the declared region.
            unsafe {
                allocator.init_from_regions(&regions).unwrap();
            }

            Self { allocator, base, _memory: memory }
        }
    }

    #[test]
    fn test_scan_hob_list_rejects_a_list_that_describes_no_smram() {
        // A producer that names no SMRAM leaves nothing to allocate from. Reporting that as an
        // error keeps the caller from committing to an allocator with zero regions, which would
        // then classify every address as outside MMRAM.
        let hob_list = test_support::RawHobList::new().finish();

        // SAFETY: `hob_list` is a valid contiguous HOB list that outlives the call.
        let scanned = unsafe { PageAllocator::scan_hob_list_for_smram_regions(hob_list.handoff()) };

        assert_eq!(scanned.map(|scanned| scanned.as_slice().len()), Err(AllocError::NotInitialized.into()));
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
        assert_eq!(allocator.allocated_page_count(AllocationType::Supervisor), 0);
        assert_eq!(allocator.allocate_pages(1), Err(AllocError::NotInitialized.into()));
        assert_eq!(allocator.free_pages(UEFI_PAGE_SIZE as u64, 1), Err(AllocError::NotInitialized.into()));
        assert_eq!(allocator.get_allocation_type(UEFI_PAGE_SIZE as u64), None);
        assert_eq!(allocator.classify_mmram(&range(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64)), None);

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
        test_support::init_test_logger();
        let fixture = AllocatorFixture::new();

        assert!(fixture.allocator.is_initialized());
        assert_eq!(fixture.allocator.region_count(), 1);
        assert_eq!(fixture.allocator.total_page_count(), TEST_REGION_PAGES);
        assert_eq!(fixture.allocator.free_page_count(), TEST_REGION_PAGES - 1);
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::Supervisor), 1);
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::User), 0);
        assert_eq!(fixture.allocator.get_allocation_type(fixture.base), Some(AllocationType::Supervisor));
        assert_eq!(fixture.allocator.allocate_pages(0), Err(AllocError::InvalidSize.into()));
        assert_eq!(fixture.allocator.free_pages(fixture.base + 1, 1), Err(AllocError::InvalidAlignment.into()));
        assert_eq!(fixture.allocator.free_pages(fixture.base, 0), Err(AllocError::InvalidSize.into()));
        assert_eq!(
            fixture.allocator.free_pages(fixture.base + TEST_REGION_BYTES as u64, 1),
            Err(AllocError::InvalidAddress.into())
        );
    }

    #[test]
    fn test_page_allocator_rejects_double_initialization() {
        let fixture = AllocatorFixture::new();
        let regions = [MmramRegion::new(range(fixture.base, TEST_REGION_BYTES as u64), false)];

        // SAFETY: the fixture owns the page-aligned region for its full declared size.
        unsafe {
            assert_eq!(fixture.allocator.init_from_regions(&regions), Err(AllocError::AlreadyInitialized.into()));
        }
        assert_eq!(fixture.allocator.free_page_count(), TEST_REGION_PAGES - 1);
    }

    #[test]
    fn test_page_allocator_validates_regions_before_writing_bookkeeping() {
        let allocator = PageAllocator::new();
        let empty: [MmramRegion; 0] = [];
        let unaligned = [MmramRegion::new(range(0x1001, UEFI_PAGE_SIZE as u64), false)];
        let partial_page = [MmramRegion::new(range(0x1000, UEFI_PAGE_SIZE as u64 - 1), false)];
        let overlapping = [
            MmramRegion::new(range(0x1000, 2 * UEFI_PAGE_SIZE as u64), false),
            MmramRegion::new(range(0x2000, UEFI_PAGE_SIZE as u64), false),
        ];

        // SAFETY: these invalid descriptors are rejected before any address is dereferenced.
        unsafe {
            assert_eq!(allocator.init_from_regions(&empty), Err(AllocError::NotInitialized.into()));
            assert_eq!(allocator.init_from_regions(&unaligned), Err(AllocError::InvalidAlignment.into()));
            assert_eq!(allocator.init_from_regions(&partial_page), Err(AllocError::InvalidAlignment.into()));
            assert_eq!(allocator.init_from_regions(&overlapping), Err(AllocError::InvalidAddress.into()));
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
        assert_eq!(
            fixture.allocator.classify_mmram(&range(user, 3 * UEFI_PAGE_SIZE as u64)),
            Some(MmramPlacement::Inside)
        );

        assert_eq!(
            fixture.allocator.free_pages_checked(user, 3, AllocationType::Supervisor),
            Err(AllocError::TypeMismatch.into())
        );
        assert_eq!(fixture.allocator.free_pages_checked(user, 3, AllocationType::User), Ok(()));
        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Ok(()));
        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Err(AllocError::NotAllocated.into()));
        assert_eq!(fixture.allocator.free_page_count(), TEST_REGION_PAGES - 1);
    }

    #[test]
    fn test_page_allocator_zeroes_fresh_user_pages() {
        let fixture = AllocatorFixture::new();

        // The fixture memory starts as 0xA5, as MMRAM the producer used before the allocator took
        // it over might, and these pages were never freed, so no scrub has touched them.
        let user = fixture.allocator.allocate_pages_with_type(2, AllocationType::User).unwrap();

        // SAFETY: the fixture owns this range for the allocation's full size.
        let fresh = unsafe { core::slice::from_raw_parts(user as *const u8, 2 * UEFI_PAGE_SIZE) };
        assert!(fresh.iter().all(|&byte| byte == 0), "a fresh user allocation exposed old MMRAM contents");
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
        assert!(reused.iter().all(|&byte| byte == 0), "freed supervisor pages leaked into a user allocation");
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
        assert!(scrubbed.iter().all(|&byte| byte == 0), "checked free left page contents behind");
    }

    #[test]
    fn test_page_allocator_classifies_ranges_containment_misses() {
        let fixture = AllocatorFixture::new();
        let base = fixture.base;
        let end = base + TEST_REGION_BYTES as u64;
        let classify = |addr, size| fixture.allocator.classify_mmram(&range(addr, size));

        // Wholly inside.
        assert_eq!(classify(base, 0x1000), Some(MmramPlacement::Inside));
        assert_eq!(classify(base, TEST_REGION_BYTES as u64), Some(MmramPlacement::Inside));

        // Crossing either boundary. Containment alone reports these as not inside MMRAM, which is
        // why an overlap check must use the partly-inside result.
        assert_eq!(classify(base - 0x1000, 0x2000), Some(MmramPlacement::PartlyInside));
        assert_eq!(classify(end - 0x1000, 0x2000), Some(MmramPlacement::PartlyInside));

        // A range that swallows the whole region is also not "inside" it.
        assert_eq!(classify(base - 0x1000, TEST_REGION_BYTES as u64 + 0x2000), Some(MmramPlacement::PartlyInside));

        // Clear of the region on either side, and touching only its exclusive bounds.
        assert_eq!(classify(base - 0x1000, 0x1000), Some(MmramPlacement::Outside));
        assert_eq!(classify(end, 0x1000), Some(MmramPlacement::Outside));
    }

    #[test]
    fn test_page_allocator_reports_overlap_before_it_knows_any_regions() {
        // Nothing can be shown to lie either inside or outside MMRAM before the regions are
        // scanned, so the layout is reported as unknown rather than guessed at.
        let allocator = PageAllocator::new();

        assert!(!allocator.is_initialized());
        assert_eq!(allocator.classify_mmram(&range(0x1000, 0x1000)), None);
    }

    #[test]
    fn test_page_allocator_checked_free_is_atomic_on_type_mismatch() {
        test_support::init_test_logger();
        let fixture = AllocatorFixture::new();
        let mut state = fixture.allocator.lock_state();
        let user = state.allocate(2, AllocationType::User).unwrap();

        assert_eq!(
            state.verify_allocated_with_type(user, 2, AllocationType::Supervisor),
            Err(AllocError::TypeMismatch.into())
        );
        assert_eq!(state.allocation_type(user), Some(AllocationType::User));
        assert_eq!(state.allocation_type(user + UEFI_PAGE_SIZE as u64), Some(AllocationType::User));

        let bit_range =
            state.verify_allocated_with_type(user, 2, AllocationType::User).expect("the range is user-allocated");
        state.mark_free(bit_range);
        assert_eq!(state.verify_allocated(user, 2), Err(AllocError::NotAllocated.into()));
        assert_eq!(state.verify_allocated(user, 0), Err(AllocError::InvalidSize.into()));
    }

    #[test]
    fn test_page_allocator_rejects_a_free_whose_page_count_overflows() {
        let fixture = AllocatorFixture::new();
        let state = fixture.allocator.lock_state();

        // Starting one page into the region makes the page index plus the count overflow.
        let second_page = fixture.base + UEFI_PAGE_SIZE as u64;
        assert_eq!(state.verify_allocated(second_page, usize::MAX), Err(AllocError::InvalidAddress.into()));
    }

    #[test]
    fn test_page_allocator_rejects_a_free_outside_every_region() {
        let fixture = AllocatorFixture::new();
        let state = fixture.allocator.lock_state();

        // An address below the region start belongs to no region at all.
        assert_eq!(state.verify_allocated(UEFI_PAGE_SIZE as u64, 1), Err(AllocError::InvalidAddress.into()));
    }

    #[test]
    fn test_page_allocator_public_free_rejects_unaligned_addresses() {
        let fixture = AllocatorFixture::new();
        let unaligned = fixture.base + 1;

        assert_eq!(fixture.allocator.free_pages(unaligned, 1), Err(AllocError::InvalidAlignment.into()));
        assert_eq!(
            fixture.allocator.free_pages_checked(unaligned, 1, AllocationType::User),
            Err(AllocError::InvalidAlignment.into())
        );
    }

    #[test]
    fn test_page_allocator_public_free_reports_pages_that_are_not_allocated() {
        test_support::init_test_logger();
        let fixture = AllocatorFixture::new();
        let last_page = fixture.base + (TEST_REGION_PAGES - 1) as u64 * UEFI_PAGE_SIZE as u64;

        assert_eq!(fixture.allocator.free_pages(last_page, 1), Err(AllocError::NotAllocated.into()));
        assert_eq!(
            fixture.allocator.free_pages_checked(last_page, 1, AllocationType::User),
            Err(AllocError::NotAllocated.into())
        );
    }

    #[test]
    fn test_page_allocator_marks_pre_allocated_regions_as_used() {
        test_support::init_test_logger();
        let mut memory = Box::new(AlignedRegion([0u8; TEST_REGION_BYTES]));
        let base = memory.0.as_mut_ptr() as u64;
        let half = (TEST_REGION_BYTES / 2) as u64;
        let allocator = PageAllocator::new();
        // The second half is handed over pre-allocated, so none of it is available.
        let regions = [MmramRegion::new(range(base, half), false), MmramRegion::new(range(base + half, half), true)];

        // SAFETY: `memory` is page-aligned, owned by this test, and covers both regions.
        let bookkeeping = unsafe { allocator.init_from_regions(&regions).unwrap() };

        // The bookkeeping takes one page at the start of the first free region.
        assert_eq!(bookkeeping, range(base, UEFI_PAGE_SIZE as u64));
        assert_eq!(allocator.region_count(), 2);
        assert_eq!(allocator.total_page_count(), TEST_REGION_PAGES);
        // Every pre-allocated page plus the bookkeeping page is charged to the supervisor.
        assert_eq!(allocator.allocated_page_count(AllocationType::Supervisor), TEST_REGION_PAGES / 2 + 1);
        assert_eq!(allocator.free_page_count(), TEST_REGION_PAGES / 2 - 1);
    }

    #[test]
    fn test_page_allocator_rejects_an_empty_allocation_request() {
        let fixture = AllocatorFixture::new();

        assert_eq!(
            fixture.allocator.allocate_pages_with_type(0, AllocationType::User),
            Err(AllocError::InvalidSize.into())
        );
    }

    #[test]
    fn test_page_allocator_reports_an_allocation_larger_than_the_region() {
        let fixture = AllocatorFixture::new();

        assert_eq!(
            fixture.allocator.allocate_pages_with_type(TEST_REGION_PAGES + 1, AllocationType::User),
            Err(AllocError::OutOfMemory.into())
        );
    }

    #[test]
    fn test_page_allocator_rejects_an_empty_region_list() {
        let allocator = PageAllocator::new();

        // SAFETY: an empty list is rejected before any region is read.
        let result = unsafe { allocator.init_from_regions(&[]) };
        assert_eq!(result, Err(AllocError::NotInitialized.into()));
    }

    #[test]
    fn test_page_allocator_rejects_free_crossing_region_end() {
        test_support::init_test_logger();
        let fixture = AllocatorFixture::new();
        let mut state = fixture.allocator.lock_state();
        let allocation = state.allocate(TEST_REGION_PAGES - 1, AllocationType::User).unwrap();
        let last_page = fixture.base + (TEST_REGION_PAGES - 1) as u64 * UEFI_PAGE_SIZE as u64;

        assert_eq!(state.verify_allocated(last_page, 2), Err(AllocError::InvalidAddress.into()));
        assert_eq!(state.verify_allocated(allocation, TEST_REGION_PAGES), Err(AllocError::InvalidAddress.into()));
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
            .release_pages_with(allocation, 2, Some(AllocationType::User), |_, _| Err(AllocError::UnmapFailed.into()));

        assert_eq!(result, Err(AllocError::UnmapFailed.into()));
        assert_eq!(fixture.allocator.free_page_count(), free_before, "the failed range was returned to the pool");
        assert_eq!(fixture.allocator.get_allocation_type(allocation), Some(AllocationType::User));
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::User), 2);
    }

    /// Installs a global page table whose paging pool holds `pool_pages` pages, the first of which
    /// the root table takes.
    fn install_page_table(pool_pages: usize) {
        use crate::memory::{PagingPoolAllocator, SharedPagingAllocator};
        use patina_paging::{PagingType, x64::X64PageTable};

        let memory = Box::leak(Box::new(test_support::PageAlignedMemory::new(pool_pages)));
        let pool = Box::leak(Box::new(PagingPoolAllocator::new()));
        // SAFETY: `memory` is page aligned, leaked for the rest of the process, and used by
        // nothing else.
        unsafe { pool.init(memory.base(), pool_pages).unwrap() };
        let root = pool.allocate_page_internal(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, true).unwrap();
        // SAFETY: `root` is a page from the pool, which `init` zeroed, so the table is empty.
        let page_table =
            unsafe { X64PageTable::from_existing(root, SharedPagingAllocator::new(pool), PagingType::Paging4Level) }
                .unwrap();
        *crate::state::security_state().lock_page_table() = Some(page_table);
    }

    #[test]
    #[serial_test::serial]
    fn test_page_allocator_returns_pages_it_cannot_map() {
        let fixture = AllocatorFixture::new();
        let free_before = fixture.allocator.free_page_count();
        // Only the root table fits in the pool, so mapping any page needs a table it cannot get.
        install_page_table(1);

        let result = fixture.allocator.allocate_pages_with_type(2, AllocationType::User);
        *crate::state::security_state().lock_page_table() = None;

        assert_eq!(result, Err(AllocError::MapFailed.into()));
        assert_eq!(fixture.allocator.free_page_count(), free_before, "the unmapped range was leaked");
        assert_eq!(fixture.allocator.allocated_page_count(AllocationType::User), 0);
        // The range the failed request reserved is the one the next request receives.
        assert_eq!(
            fixture.allocator.allocate_pages_with_type(2, AllocationType::User),
            Ok(fixture.base + UEFI_PAGE_SIZE as u64)
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_page_allocator_keeps_pages_it_cannot_remap_for_the_scrub() {
        *crate::state::security_state().lock_page_table() = None;
        let fixture = AllocatorFixture::new();
        let allocation = fixture.allocator.allocate_pages(1).unwrap();
        // SAFETY: the fixture owns this range for the allocation's full size.
        unsafe { core::slice::from_raw_parts_mut(allocation as *mut u8, UEFI_PAGE_SIZE) }.fill(0x5A);
        install_page_table(1);

        let result = fixture.allocator.free_pages(allocation, 1);
        *crate::state::security_state().lock_page_table() = None;

        // The range stays allocated, and nothing was written to it.
        assert_eq!(result, Err(AllocError::MapFailed.into()));
        assert_eq!(fixture.allocator.get_allocation_type(allocation), Some(AllocationType::Supervisor));
        // SAFETY: as above; the range is still owned by the fixture.
        let contents = unsafe { core::slice::from_raw_parts(allocation as *const u8, UEFI_PAGE_SIZE) };
        assert!(contents.iter().all(|&byte| byte == 0x5A), "a failed free scrubbed the range");
    }

    #[test]
    #[serial_test::serial]
    fn test_page_allocator_reports_a_freed_range_it_cannot_unmap() {
        const LARGE_PAGE: u64 = 0x20_0000;
        let base = 0x4020_0000;
        // The root, a PDPT and a PD use up the pool, so the 2 MiB page below is mapped whole and
        // there is no page left for the table that splitting it needs.
        install_page_table(3);
        crate::state::security_state()
            .lock_page_table()
            .as_mut()
            .unwrap()
            .map_memory_region(base, LARGE_PAGE, MemoryAttributes::ExecuteProtect | MemoryAttributes::Supervisor)
            .unwrap();
        let allocator = PageAllocator::new();

        let partial = allocator.apply_freed_page_attributes(base, 1);
        let whole = allocator.apply_freed_page_attributes(base, (LARGE_PAGE / UEFI_PAGE_SIZE as u64) as usize);
        *crate::state::security_state().lock_page_table() = None;

        assert_eq!(partial, Err(AllocError::UnmapFailed.into()));
        // Unmapping the whole large page needs no split, so it succeeds.
        assert_eq!(whole, Ok(()));
    }

    #[test]
    fn test_page_allocator_mmram_range_checks_do_not_overflow() {
        let fixture = AllocatorFixture::new();
        let state = fixture.allocator.lock_state();

        assert_eq!(state.classify_mmram(&range(fixture.base, TEST_REGION_BYTES as u64)), MmramPlacement::Inside);
        assert_eq!(
            state.classify_mmram(&range(
                fixture.base + TEST_REGION_BYTES as u64 - UEFI_PAGE_SIZE as u64,
                2 * UEFI_PAGE_SIZE as u64
            )),
            MmramPlacement::PartlyInside
        );
        // A range ending at the top of the address space is classified without overflowing.
        assert_eq!(state.classify_mmram(&range(u64::MAX - 2, 2)), MmramPlacement::Outside);
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
        let mut scanned = ScannedRegions::new();

        PageAllocator::collect_smram_regions(&unaligned[1..], &mut scanned).unwrap();

        assert_eq!(
            scanned.as_slice(),
            [
                MmramRegion::new(range(descriptors[0].physical_start, descriptors[0].physical_size), false),
                MmramRegion::new(range(descriptors[1].physical_start, descriptors[1].physical_size), true),
            ]
        );
    }

    #[test]
    fn test_page_allocator_collect_rejects_descriptors_that_are_not_valid_ranges() {
        // An empty descriptor, or one that runs past the end of the address space, never becomes a
        // region, so nothing later has to check for either case.
        for (physical_start, physical_size) in [(0x8000_0000, 0), (u64::MAX - 0xfff, 2 * UEFI_PAGE_SIZE as u64)] {
            let descriptor =
                SmramDescriptor { physical_start, cpu_start: physical_start, physical_size, region_state: 0 };
            let payload = smram_hob_payload(1, &[descriptor]);
            let mut scanned = ScannedRegions::new();

            assert_eq!(
                PageAllocator::collect_smram_regions(&payload, &mut scanned),
                Err(AllocError::InvalidAddress.into())
            );
            assert!(scanned.as_slice().is_empty());
        }
    }

    #[test]
    fn test_page_allocator_collect_rejects_a_truncated_descriptor_list() {
        let descriptor = SmramDescriptor {
            physical_start: 0x8000_0000,
            cpu_start: 0x8000_0000,
            physical_size: UEFI_PAGE_SIZE as u64,
            region_state: 0,
        };
        // The header declares two descriptors, but the payload holds one.
        let payload = smram_hob_payload(2, &[descriptor]);
        let mut scanned = ScannedRegions::new();

        assert_eq!(
            PageAllocator::collect_smram_regions(&payload, &mut scanned),
            Err(AllocError::InvalidAddress.into())
        );
        assert!(scanned.as_slice().is_empty());
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
        let mut scanned = ScannedRegions::new();

        assert_eq!(
            PageAllocator::collect_smram_regions(&payload[..size_of::<SmramReserveHobData>() - 1], &mut scanned),
            Err(AllocError::InvalidAddress.into())
        );
        assert!(scanned.as_slice().is_empty());

        scanned.count = MAX_TEMP_REGIONS;
        assert_eq!(PageAllocator::collect_smram_regions(&payload, &mut scanned), Err(AllocError::OutOfMemory.into()));
    }

    #[test]
    fn test_page_allocator_calculates_bookkeeping_requirements() {
        let valid = [MmramRegion::new(range(0x8000_0000, TEST_REGION_BYTES as u64), false)];
        let allocated = [MmramRegion::new(range(0x8000_0000, TEST_REGION_BYTES as u64), true)];
        let too_small = [MmramRegion::new(range(0x8000_0000, 1), false)];
        let unaligned = [MmramRegion::new(range(0x8000_0001, TEST_REGION_BYTES as u64), false)];

        assert_eq!(PageAllocator::calculate_bookkeeping(&valid), Ok((valid[0].range.base(), 1)));
        assert_eq!(PageAllocator::calculate_bookkeeping(&allocated), Err(AllocError::OutOfMemory.into()));
        assert_eq!(PageAllocator::calculate_bookkeeping(&too_small), Err(AllocError::OutOfMemory.into()));
        assert_eq!(PageAllocator::calculate_bookkeeping(&unaligned), Err(AllocError::InvalidAlignment.into()));
    }

    #[test]
    fn test_page_allocator_bookkeeping_rejects_a_page_total_that_overflows() {
        // The descriptors are produced outside MMRAM, so the sizing has to survive a list that
        // claims more pages than a usize can hold. Each region here claims the whole address
        // space, and enough of them are present that the running total wraps. The sizing must
        // report that rather than allocating a bookkeeping area too small for the bitmaps.
        let pages_per_region = u64::MAX as usize / UEFI_PAGE_SIZE;
        let regions_needed = usize::MAX / pages_per_region + 1;
        let regions = vec![MmramRegion::new(range(0, u64::MAX), false); regions_needed];

        assert_eq!(PageAllocator::calculate_bookkeeping(&regions), Err(AllocError::OutOfMemory.into()));
    }

    #[test]
    fn test_page_allocator_span_classification_matches_the_committed_classifier() {
        // The pre-paging HOB checks classify against the contiguous MMRAM span before the
        // allocator commits, and later code uses the allocator. Both decide the same security
        // questions, so a range must not be judged differently depending on which one is used.
        let fixture = AllocatorFixture::new();
        let base = fixture.base;
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
        ] {
            assert_eq!(
                MmramPlacement::from(range(addr, size).coverage([range(base, span)])),
                fixture.allocator.classify_mmram(&range(addr, size)).expect("fixture allocator is initialized"),
                "classification diverged for 0x{addr:016x} size 0x{size:x}"
            );
        }
    }
}
