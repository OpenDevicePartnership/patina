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
