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
    mem::size_of,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use patina::standard::efi;
use patina::{
    UEFI_PAGE_SIZE,
    pi::hob::{Hob, PhaseHandoffInformationTable},
    uefi_pages_to_size,
};
use patina_paging::{MemoryAttributes, PageTable};
use spin::Mutex;
use zerocopy::FromBytes;

use super::mmram::MmramRegion;
use crate::memory::AllocError;
use crate::memory::locked_state::{AllocatorState, BITS_PER_BYTE, LockedState, StatePtr, page_count};
use crate::memory::mmram::MmramPlacement;
use crate::{error::MmSupervisorResult, user_access_guard::with_user_access};

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

/// Maximum number of SMRAM regions collected on the stack while scanning the
/// HOB list. The authoritative region metadata is stored in SMRAM afterwards.
pub(crate) const MAX_TEMP_REGIONS: usize = 256;

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
            let pages = page_count(region.size).ok_or_else(|| {
                log::error!("Bookkeeping sizing failed: region size 0x{:x} does not fit a page count", region.size);
                AllocError::OutOfMemory
            })?;
            total.checked_add(pages).ok_or_else(|| {
                log::error!("Bookkeeping sizing failed: total page count overflows at 0x{:016x}", region.base);
                AllocError::OutOfMemory
            })
        })?;

        let header_size = size_of::<AllocatorState>();
        let regions_size = regions.len().checked_mul(size_of::<RegionInfo>()).ok_or_else(|| {
            log::error!("Bookkeeping sizing failed: {} regions overflow the region table", regions.len());
            AllocError::OutOfMemory
        })?;
        let bitmap_bytes = total_pages.div_ceil(BITS_PER_BYTE);
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
        let first_free = regions.iter().find(|region| !region.pre_allocated);
        let bookkeeping_base = first_free.map(|region| region.base).ok_or_else(|| {
            log::error!("No free SMRAM region available for bookkeeping");
            AllocError::OutOfMemory
        })?;
        let first_free_size = first_free.map_or(0, |region| region.size);
        if bookkeeping_base == 0 || !bookkeeping_base.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("Bookkeeping region base is null or not page-aligned");
            return Err(AllocError::InvalidAlignment.into());
        }

        let bookkeeping_size = bookkeeping_pages.checked_mul(UEFI_PAGE_SIZE).ok_or(AllocError::OutOfMemory)?;
        if u64::try_from(bookkeeping_size).map_or(true, |size| size > first_free_size) {
            log::error!("First free region too small for bookkeeping");
            return Err(AllocError::OutOfMemory.into());
        }

        Ok((bookkeeping_base, bookkeeping_pages))
    }

    /// Scans the entire HOB list and collects every SMRAM/MMRAM region reported
    /// under the supported GUIDs into `regions`, updating `count`.
    fn scan_smram_regions(
        handoff: &PhaseHandoffInformationTable,
        regions: &mut [MmramRegion; MAX_TEMP_REGIONS],
        region_count: &mut usize,
    ) -> MmSupervisorResult<()> {
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
        regions: &mut [MmramRegion; MAX_TEMP_REGIONS],
        region_count: &mut usize,
    ) -> MmSupervisorResult<()> {
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

        for descriptor_bytes in descriptor_bytes.chunks_exact(size_of::<SmramDescriptor>()).take(count) {
            let (descriptor, _) =
                SmramDescriptor::read_from_prefix(descriptor_bytes).map_err(|_| AllocError::InvalidAddress)?;
            if *region_count >= MAX_TEMP_REGIONS {
                log::error!(
                    "Too many SMRAM regions for temp storage (MAX_TEMP_REGIONS = {MAX_TEMP_REGIONS}), increase MAX_TEMP_REGIONS"
                );
                return Err(AllocError::OutOfMemory.into());
            }

            let pre_allocated = (descriptor.region_state & EFI_ALLOCATED) != 0;
            let pages = page_count(descriptor.physical_size).ok_or(AllocError::OutOfMemory)?;

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
                return Err(AllocError::OutOfMemory.into());
            };
            *slot = MmramRegion::new(descriptor.physical_start, descriptor.physical_size, pre_allocated);
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
        handoff: &PhaseHandoffInformationTable,
    ) -> MmSupervisorResult<([MmramRegion; MAX_TEMP_REGIONS], usize)> {
        let mut regions = [MmramRegion::default(); MAX_TEMP_REGIONS];
        let mut count = 0usize;
        Self::scan_smram_regions(handoff, &mut regions, &mut count)?;

        if count == 0 {
            log::error!("No SMRAM regions found in HOB list");
            return Err(AllocError::NotInitialized.into());
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
    pub(crate) unsafe fn init_from_regions(&self, scanned: &[MmramRegion]) -> MmSupervisorResult<()> {
        if scanned.is_empty() {
            log::error!("Page allocator init failed: no SMRAM regions were supplied");
            return Err(AllocError::NotInitialized.into());
        }

        for (index, region) in scanned.iter().enumerate() {
            let is_page_aligned =
                region.base.is_multiple_of(UEFI_PAGE_SIZE as u64) && region.size.is_multiple_of(UEFI_PAGE_SIZE as u64);
            if !is_page_aligned {
                log::error!("SMRAM region {index} [0x{:016x}, +0x{:x}) is not page aligned", region.base, region.size);
                return Err(AllocError::InvalidAlignment.into());
            }
            let Some(region_end) = region.base.checked_add(region.size).filter(|_| region.size != 0) else {
                log::error!(
                    "SMRAM region {index} [0x{:016x}, +0x{:x}) is empty or overflows",
                    region.base,
                    region.size
                );
                return Err(AllocError::InvalidAddress.into());
            };
            let Some(previous_regions) = scanned.get(..index) else {
                log::error!("SMRAM region {index} is out of range while checking for overlap");
                return Err(AllocError::InvalidAddress.into());
            };
            let overlaps_earlier_region = previous_regions.iter().any(|previous| {
                previous
                    .base
                    .checked_add(previous.size)
                    .is_some_and(|previous_end| region.base < previous_end && previous.base < region_end)
            });
            if overlaps_earlier_region {
                log::error!(
                    "SMRAM region {index} [0x{:016x}, 0x{region_end:016x}) overlaps an earlier region",
                    region.base
                );
                return Err(AllocError::InvalidAddress.into());
            }
        }

        let guard = self.state.lock();
        if self.initialized.load(Ordering::Acquire) {
            log::error!("Page allocator init failed: the allocator is already initialized");
            return Err(AllocError::AlreadyInitialized.into());
        }

        let total_pages = scanned.iter().try_fold(0usize, |total, region| {
            let pages = page_count(region.size).ok_or_else(|| {
                log::error!("SMRAM region size 0x{:x} does not fit a page count", region.size);
                AllocError::OutOfMemory
            })?;
            total.checked_add(pages).ok_or_else(|| {
                log::error!("Total SMRAM page count overflows at region base 0x{:016x}", region.base);
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
        if let Err(e) = pt.unmap_memory_region(addr, size) {
            log::error!("Failed to set freed page attributes for 0x{addr:016x} ({num_pages} pages): {e:?}");
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

    /// Returns whether `[addr, addr + size)` lies entirely inside MMRAM.
    ///
    /// Reports `false` before the allocator is initialized, since nothing can be shown to be
    /// inside MMRAM before the regions are known. A caller that must tell "outside" apart from
    /// "not known yet" should use [`PageAllocator::classify_mmram`] instead.
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
    use crate::{memory::mmram::classify_mmram_in_regions, test_support};

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
            let regions = [MmramRegion::new(base, TEST_REGION_BYTES as u64, false)];

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
        let scanned = unsafe { PageAllocator::scan_hob_list(hob_list.handoff()) };

        assert_eq!(scanned.map(|(_, count)| count), Err(AllocError::NotInitialized.into()));
    }

    /// Builds a `MmramRegion` list from `(base, size, pre_allocated)` tuples.
    fn regions_from(entries: &[(u64, u64, bool)]) -> Vec<MmramRegion> {
        entries.iter().map(|&(base, size, pre_allocated)| MmramRegion::new(base, size, pre_allocated)).collect()
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
        assert_eq!(allocator.allocate_pages(1), Err(AllocError::NotInitialized.into()));
        assert_eq!(allocator.free_pages(UEFI_PAGE_SIZE as u64, 1), Err(AllocError::NotInitialized.into()));
        assert_eq!(allocator.get_allocation_type(UEFI_PAGE_SIZE as u64), None);
        assert!(!allocator.is_region_inside_mmram(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64));

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
        assert_eq!(fixture.allocator.allocate_pages(0), Err(AllocError::OutOfMemory.into()));
        assert_eq!(fixture.allocator.free_pages(fixture.base + 1, 1), Err(AllocError::InvalidAlignment.into()));
        assert_eq!(fixture.allocator.free_pages(fixture.base, 0), Err(AllocError::InvalidAddress.into()));
        assert_eq!(
            fixture.allocator.free_pages(fixture.base + TEST_REGION_BYTES as u64, 1),
            Err(AllocError::InvalidAddress.into())
        );
    }

    #[test]
    fn test_page_allocator_rejects_double_initialization() {
        let fixture = AllocatorFixture::new();
        let regions = [MmramRegion::new(fixture.base, TEST_REGION_BYTES as u64, false)];

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
        let unaligned = [MmramRegion::new(0x1001, UEFI_PAGE_SIZE as u64, false)];
        let partial_page = [MmramRegion::new(0x1000, UEFI_PAGE_SIZE as u64 - 1, false)];
        let overflowing = [MmramRegion::new(u64::MAX - UEFI_PAGE_SIZE as u64 + 1, UEFI_PAGE_SIZE as u64, false)];
        let overlapping = [
            MmramRegion::new(0x1000, 2 * UEFI_PAGE_SIZE as u64, false),
            MmramRegion::new(0x2000, UEFI_PAGE_SIZE as u64, false),
        ];

        // SAFETY: these invalid descriptors are rejected before any address is dereferenced.
        unsafe {
            assert_eq!(allocator.init_from_regions(&empty), Err(AllocError::NotInitialized.into()));
            assert_eq!(allocator.init_from_regions(&unaligned), Err(AllocError::InvalidAlignment.into()));
            assert_eq!(allocator.init_from_regions(&partial_page), Err(AllocError::InvalidAlignment.into()));
            assert_eq!(allocator.init_from_regions(&overflowing), Err(AllocError::InvalidAddress.into()));
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
        assert!(fixture.allocator.is_region_inside_mmram(user, 3 * UEFI_PAGE_SIZE as u64));

        assert_eq!(
            fixture.allocator.free_pages_checked(user, 3, AllocationType::Supervisor),
            Err(AllocError::InvalidAddress.into())
        );
        assert_eq!(fixture.allocator.free_pages_checked(user, 3, AllocationType::User), Ok(()));
        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Ok(()));
        assert_eq!(fixture.allocator.free_pages(supervisor, 2), Err(AllocError::NotAllocated.into()));
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
        test_support::init_test_logger();
        let fixture = AllocatorFixture::new();
        let mut state = fixture.allocator.lock_state();
        let user = state.allocate(2, AllocationType::User).unwrap();

        assert_eq!(
            state.verify_allocated_with_type(user, 2, AllocationType::Supervisor),
            Err(AllocError::InvalidAddress.into())
        );
        assert_eq!(state.allocation_type(user), Some(AllocationType::User));
        assert_eq!(state.allocation_type(user + UEFI_PAGE_SIZE as u64), Some(AllocationType::User));

        let bit_range =
            state.verify_allocated_with_type(user, 2, AllocationType::User).expect("the range is user-allocated");
        state.mark_free(bit_range);
        assert_eq!(state.verify_allocated(user, 2), Err(AllocError::NotAllocated.into()));
        assert_eq!(state.verify_allocated(user, 0), Err(AllocError::InvalidAddress.into()));
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
        let regions = [MmramRegion::new(base, half, false), MmramRegion::new(base + half, half, true)];

        // SAFETY: `memory` is page-aligned, owned by this test, and covers both regions.
        unsafe { allocator.init_from_regions(&regions).unwrap() };

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
            Err(AllocError::OutOfMemory.into())
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
        let mut regions = [MmramRegion::default(); MAX_TEMP_REGIONS];
        let mut count = 0;

        PageAllocator::collect_smram_regions(&unaligned[1..], &mut regions, &mut count).unwrap();

        assert_eq!(count, 2);
        assert_eq!(regions[0], MmramRegion::new(descriptors[0].physical_start, descriptors[0].physical_size, false));
        assert_eq!(regions[1], MmramRegion::new(descriptors[1].physical_start, descriptors[1].physical_size, true));
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
        let mut regions = [MmramRegion::default(); MAX_TEMP_REGIONS];
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
        let mut regions = [MmramRegion::default(); MAX_TEMP_REGIONS];
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
            Err(AllocError::OutOfMemory.into())
        );
    }

    #[test]
    fn test_page_allocator_calculates_bookkeeping_requirements() {
        let valid = [MmramRegion::new(0x8000_0000, TEST_REGION_BYTES as u64, false)];
        let allocated = [MmramRegion::new(0x8000_0000, TEST_REGION_BYTES as u64, true)];
        let too_small = [MmramRegion::new(0x8000_0000, 1, false)];
        let unaligned = [MmramRegion::new(0x8000_0001, TEST_REGION_BYTES as u64, false)];

        assert_eq!(PageAllocator::calculate_bookkeeping(&valid), Ok((valid[0].base, 1)));
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
        let regions = vec![MmramRegion::new(0x8000_0000, u64::MAX, false); regions_needed];

        assert_eq!(PageAllocator::calculate_bookkeeping(&regions), Err(AllocError::OutOfMemory.into()));
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
}
