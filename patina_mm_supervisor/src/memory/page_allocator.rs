//! MM Supervisor Core Page and Pool Allocators
//!
//! Provides a page-granularity memory allocator and a pool allocator for the MM
//! Supervisor Core.
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
    uefi_pages_to_size,
};
use patina_paging::{MemoryAttributes, PageTable};
use spin::Mutex;
use zerocopy::FromBytes;

use super::mmram::MmramRegion;
use crate::memory::AllocError;
use crate::memory::locked_state::{AllocatorState, LockedState, StatePtr};
use crate::memory::mmram::MmramPlacement;
use crate::{error::MmSupervisorResult, user_access_guard::with_user_access};

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

impl RegionInfo {
    /// Returns the region's address range, or `None` if its page count does not describe one.
    pub(crate) fn range(&self) -> Option<MemoryRange> {
        MemoryRange::from_pages(self.base, u64::try_from(self.total_pages).ok()?).ok()
    }
}

/// Maximum number of SMRAM regions collected on the stack while scanning the
/// HOB list. The authoritative region metadata is stored in SMRAM afterwards.
pub(crate) const MAX_TEMP_REGIONS: usize = 256;

/// The SMRAM regions collected from the HOB list, held in a fixed array on the stack.
///
/// Only the collected regions are visible, through [`as_slice`](Self::as_slice). The rest of the
/// array holds a placeholder that is never read.
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

    /// Determines where the bookkeeping structures live and how large they are.
    ///
    /// Sums the pages across `regions`, selects the first non-pre-allocated
    /// region to host the bookkeeping, and verifies that region is large enough
    /// to hold it. Returns `(bookkeeping_base, bookkeeping_pages)`.
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
        let bookkeeping_pages = total_bytes.div_ceil(UEFI_PAGE_SIZE);

        log::info!(
            "Allocator needs {} pages for bookkeeping ({} regions, {} total pages)",
            bookkeeping_pages,
            regions.len(),
            total_pages
        );

        // Reserve bookkeeping space in the first free (non-pre-allocated) region.
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

    /// Parses the `SmramReserveHobData` header and trailing `SmramDescriptor`
    /// array from a single matching GUID HOB payload, appending each region to
    /// `scanned`.
    ///
    /// A descriptor with a size of zero, or one that runs past the end of the address space, is
    /// rejected here, so every collected region is a valid [`MemoryRange`].
    fn collect_smram_regions(data: &[u8], scanned: &mut ScannedRegions) -> MmSupervisorResult<()> {
        let header_size = size_of::<SmramReserveHobData>();
        if data.len() < header_size {
            return Ok(());
        }

        let (header, descriptor_bytes) = SmramReserveHobData::read_from_prefix(data).map_err(|_| {
            log::error!("SMRAM HOB payload of {} bytes could not be parsed", data.len());
            AllocError::InvalidAddress
        })?;

        // Clamp the declared count to what the payload can actually hold, so the descriptor
        // bytes below are guaranteed in-bounds even if the HOB is malformed.
        let max_fit = descriptor_bytes.len() / size_of::<SmramDescriptor>();
        let declared = header.number_of_smram_regions as usize;
        if declared > max_fit {
            log::warn!("SMRAM HOB declares {declared} descriptors but only {max_fit} fit in the payload");
        }
        let count = declared.min(max_fit);

        let (descriptor_chunks, _) = descriptor_bytes.as_chunks::<{ size_of::<SmramDescriptor>() }>();
        for descriptor_bytes in descriptor_chunks.iter().take(count) {
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

    /// Collects the regions from every SMRAM and MMRAM reserve HOB into a stack array.
    ///
    /// Only reads the HOB list. The regions are validated before
    /// [`init_from_regions`](Self::init_from_regions) writes to them.
    ///
    /// ## Safety
    ///
    /// `handoff` must start a HOB list that passed
    /// [`validate_hob_list_structure`](crate::hob::validation::validate_hob_list_structure).
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
    pub(crate) unsafe fn init_from_regions(&self, scanned: &[MmramRegion]) -> MmSupervisorResult<()> {
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
        *state = AllocatorState { region_count: scanned.len(), total_pages };

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
    pub fn allocate_pages(&self, num_pages: usize) -> MmSupervisorResult<u64> {
        self.allocate_pages_with_type(num_pages, AllocationType::Supervisor)
    }

    /// Allocates contiguous pages from SMRAM with the specified allocation type.
    ///
    /// For `Supervisor` allocations, the allocated region is marked as supervisor-owned
    /// data pages (R/W, non-executable) in the page table.
    pub fn allocate_pages_with_type(&self, num_pages: usize, alloc_type: AllocationType) -> MmSupervisorResult<u64> {
        if !self.is_initialized() {
            log::error!("Page allocation of {num_pages} page(s) rejected: allocator is not initialized");
            return Err(AllocError::NotInitialized.into());
        }

        if num_pages == 0 {
            log::error!("Page allocation rejected: page count is 0");
            return Err(AllocError::OutOfMemory.into());
        }

        // Reserve pages under the state lock, then release it before touching the
        // page table (which takes its own lock).
        let addr = self.lock_state().allocate(num_pages, alloc_type).ok_or_else(|| {
            log::error!("Page allocation of {num_pages} {alloc_type:?} page(s) failed: no free run that large");
            AllocError::OutOfMemory
        })?;

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

            if let Err(err) = pt.map_memory_region(addr, size, attributes) {
                log::error!("Failed to set supervisor page attributes for 0x{addr:016x} ({num_pages} pages): {err:?}");
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
            with_user_access(|| {
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
    fn apply_freed_page_attributes(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<()> {
        let size = uefi_pages_to_size!(num_pages) as u64;
        let mut pt_guard = crate::state::security_state().lock_page_table();
        let Some(pt) = pt_guard.as_mut() else {
            log::warn!("Page table not initialized, skipping freed page attribute update for 0x{addr:016x}");
            return Ok(());
        };

        // Freed pages: ReadProtect (not present) + NX (no execute) + ReadOnly (no write)
        // This makes the pages completely inaccessible.
        if let Err(err) = pt.unmap_memory_region(addr, size) {
            log::error!("Failed to set freed page attributes for 0x{addr:016x} ({num_pages} pages): {err:?}");
            return Err(AllocError::UnmapFailed.into());
        }

        log::trace!("Marked 0x{addr:016x} ({num_pages} pages) as inaccessible (RP+NX+RO+S)");
        Ok(())
    }

    /// Frees previously allocated pages.
    ///
    /// The pages are zeroed so their contents cannot be recovered through a later allocation, then
    /// marked as inaccessible in the page table (Supervisor + `ReadProtect` + `ExecuteProtect`) to
    /// prevent use-after-free.
    pub fn free_pages(&self, addr: u64, num_pages: usize) -> MmSupervisorResult<()> {
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
    ) -> MmSupervisorResult<()> {
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
        // `verify_allocated` and `verify_allocated_with_type` log the address and the reason when
        // they reject the request.
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

    /// Classifies `range` against the MMRAM regions, or `None` if the regions are not known yet.
    ///
    /// Callers decide what an unknown layout means for them; nothing can be proven to be either
    /// inside or outside MMRAM before the allocator is initialized.
    pub fn classify_mmram(&self, range: &MemoryRange) -> Option<MmramPlacement> {
        if !self.is_initialized() {
            return None;
        }
        Some(self.lock_state().classify_mmram(range))
    }
}
