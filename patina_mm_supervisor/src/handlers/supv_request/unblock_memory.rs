//! Unblocked Memory Region Management
//!
//! This module provides functionality to track and manage memory regions that
//! have been unblocked for access in the MM (Management Mode) environment.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use patina::standard::efi;

use patina::{BinaryGuid, UEFI_PAGE_SIZE};
use patina_paging::{MemoryAttributes, PageTable, PtError};

use patina::management_mode::protocol::mm_supervisor_request::{
    MmSupervisorRequestHeader, MmSupervisorUnblockMemoryParams,
};

use crate::{
    error::{MmSupervisorError, MmSupervisorResult},
    memory::mmram::buffer_overlaps_mmram,
    policy::{self, MemDescriptorV1_0, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_WRITE},
    state::security_state,
};

/// Maximum number of unblocked memory regions that can be tracked.
const MAX_UNBLOCKED_REGIONS: usize = 64;

/// Internal tag distinguishing supervisor-only entries in the unblock tracker.
const SUPERVISOR_TRACKING_ATTRIBUTE: u32 = 1 << 31;

/// Errors that can occur during unblock memory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblockError {
    /// Already initialized (cannot re-initialize).
    AlreadyInitialized,
    /// Too many regions to track (exceeded `MAX_UNBLOCKED_REGIONS`).
    TooManyRegions,
    /// The requested region overlaps with MMRAM.
    OverlapsWithMmram,
    /// The requested region overlaps with an existing unblocked region
    /// but has different attributes.
    ConflictingAttributes,
    /// Invalid parameters (null pointer, zero length, etc.).
    InvalidParameter,
    /// The region's address + size would overflow.
    AddressOverflow,
}

impl core::error::Error for UnblockError {}

impl core::fmt::Display for UnblockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "the unblocked memory tracker is already initialized"),
            Self::TooManyRegions => {
                write!(f, "the request exceeds the {MAX_UNBLOCKED_REGIONS} tracked unblocked regions")
            }
            Self::OverlapsWithMmram => write!(f, "the requested region overlaps MMRAM"),
            Self::ConflictingAttributes => {
                write!(f, "the requested region overlaps an unblocked region with different attributes")
            }
            Self::InvalidParameter => write!(f, "the request has a null pointer or a zero length"),
            Self::AddressOverflow => write!(f, "the requested region's address plus size overflows"),
        }
    }
}

/// A single entry in the unblocked memory region list.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnblockedMemoryEntry {
    /// Base address of the unblocked region (page-aligned).
    pub base_address: u64,
    /// Size of the unblocked region in pages.
    pub num_pages: u32,
    /// Memory attributes (combination of `RESOURCE_ATTR_*`).
    pub attributes: u32,
}

impl UnblockedMemoryEntry {
    /// Creates a new empty entry.
    pub const fn empty() -> Self {
        Self { base_address: 0, num_pages: 0, attributes: 0 }
    }

    /// Creates a new entry from a base address, byte size, and attributes.
    pub const fn new(base_address: u64, size: u64, attributes: u32) -> Self {
        Self { base_address, num_pages: patina::uefi_size_to_pages!(size as usize) as u32, attributes }
    }

    /// Returns the size of this region in bytes.
    pub fn size(&self) -> u64 {
        patina::uefi_pages_to_size!(self.num_pages as usize) as u64
    }

    /// Returns the end address (exclusive) of this region.
    pub fn end_address(&self) -> u64 {
        self.base_address.saturating_add(self.size())
    }

    /// Checks if the given range [base, base + size) is fully contained within this entry.
    pub fn contains(&self, base: u64, size: u64) -> bool {
        if self.num_pages == 0 || size == 0 {
            return false;
        }
        let Some(query_end) = base.checked_add(size) else {
            return false;
        };
        base >= self.base_address && query_end <= self.end_address()
    }

    /// Checks if the given range [base, base + size) overlaps with this entry.
    pub fn overlaps(&self, base: u64, size: u64) -> bool {
        if self.num_pages == 0 || size == 0 {
            return false;
        }
        let Some(query_end) = base.checked_add(size) else {
            return false;
        };
        let entry_end = self.end_address();

        // Two ranges overlap if: start1 < end2 && start2 < end1
        base < entry_end && self.base_address < query_end
    }
}

/// Internal state for the unblocked memory tracker.
struct UnblockedMemoryState {
    /// Array of unblocked memory entries.
    entries: [UnblockedMemoryEntry; MAX_UNBLOCKED_REGIONS],
    /// Number of valid entries in the array.
    count: usize,
}

/// Whether tracking a region added a new entry or found one already recorded.
///
/// The two are kept apart because a repeated unblock request for a region already recorded must
/// succeed without touching the page table again, while a new entry means the mapping still has
/// to be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackOutcome {
    /// The region was not recorded before, so a new entry was taken.
    Added,
    /// The region was already recorded with the same base, size and attributes.
    Existing,
}

impl UnblockedMemoryState {
    /// Creates a new empty state.
    const fn new() -> Self {
        Self { entries: [UnblockedMemoryEntry::empty(); MAX_UNBLOCKED_REGIONS], count: 0 }
    }

    /// Finds an entry that exactly matches the given base and size.
    fn find_exact_match(&self, base: u64, size: u64) -> Option<&UnblockedMemoryEntry> {
        self.entries.iter().take(self.count).find(|e| e.base_address == base && e.size() == size)
    }

    /// Adds a new entry if there's space.
    fn add_entry(&mut self, base: u64, size: u64, attributes: u32) -> MmSupervisorResult<()> {
        let capacity = self.entries.len();
        let slot = self.entries.get_mut(self.count).ok_or_else(|| {
            log::debug!("Cannot track unblocked region 0x{base:016x} (+0x{size:x}): all {capacity} slots are in use");
            UnblockError::TooManyRegions
        })?;
        *slot = UnblockedMemoryEntry::new(base, size, attributes);
        self.count += 1;
        Ok(())
    }
}

/// Global unblocked memory region tracker.
///
/// This struct manages a list of memory regions that have been unblocked for
/// access within the MM environment. It provides thread-safe access to the
/// region list through internal locking.
pub struct UnblockedMemoryTracker {
    /// Whether the tracker has been initialized.
    initialized: AtomicBool,
    /// Flag indicating if core initialization is complete (after which we enforce checks).
    core_init_complete: AtomicBool,
    /// Returns whether a range touches MMRAM at all, including one that only crosses a boundary.
    overlaps_mmram: fn(base: u64, size: u64) -> bool,
    /// Internal state protected by a mutex.
    state: Mutex<UnblockedMemoryState>,
}

impl UnblockedMemoryTracker {
    /// Creates a new unblocked memory tracker that checks ranges against the supervisor's MMRAM.
    pub const fn new() -> Self {
        Self::with_mmram_check(buffer_overlaps_mmram)
    }

    /// Creates a tracker that uses `overlaps_mmram` to decide whether a range touches MMRAM.
    const fn with_mmram_check(overlaps_mmram: fn(base: u64, size: u64) -> bool) -> Self {
        Self {
            initialized: AtomicBool::new(false),
            core_init_complete: AtomicBool::new(false),
            overlaps_mmram,
            state: Mutex::new(UnblockedMemoryState::new()),
        }
    }

    /// Initializes the tracker from an array of memory policy descriptors, which represent the
    /// initial "unblocked" regions.
    ///
    /// This should be called once during BSP initialization after the memory
    /// policy has been generated from the page table walk. Returns an error if the tracker is
    /// already initialized, if there are too many descriptors to track, or if a descriptor is
    /// empty or overlaps MMRAM. The walk never produces either of the last two, so one of them
    /// means the walk is broken, and initialization fails rather than skipping the entry.
    pub fn init_from_descriptors(&self, descriptors: &[MemDescriptorV1_0]) -> MmSupervisorResult<()> {
        // Check if already initialized
        if self.initialized.swap(true, Ordering::SeqCst) {
            log::error!("UnblockedMemoryTracker init rejected: already initialized");
            return Err(UnblockError::AlreadyInitialized.into());
        }

        let mut state = self.state.lock();

        // Add each descriptor as an unblocked region
        for desc in descriptors {
            if desc.size == 0 {
                log::error!(
                    "UnblockedMemoryTracker init rejected: zero-size descriptor at 0x{:016x}",
                    desc.base_address
                );
                return Err(UnblockError::InvalidParameter.into());
            }

            if (self.overlaps_mmram)(desc.base_address, desc.size) {
                log::error!(
                    "UnblockedMemoryTracker init rejected: descriptor 0x{:016x} - 0x{:016x} overlaps MMRAM",
                    desc.base_address,
                    desc.base_address.saturating_add(desc.size)
                );
                return Err(UnblockError::OverlapsWithMmram.into());
            }

            state.add_entry(desc.base_address, desc.size, desc.mem_attributes)?;
        }

        log::debug!("UnblockedMemoryTracker initialized with {} regions", state.count);

        Ok(())
    }

    /// Initializes the tracker from a raw buffer of memory policy descriptors.
    ///
    /// ## Safety
    ///
    /// The caller must ensure:
    /// - `buffer` points to a valid array of `MemDescriptorV1_0` structures
    /// - `count` is the number of valid entries in the buffer
    pub unsafe fn init_from_buffer(&self, buffer: *const MemDescriptorV1_0, count: usize) -> MmSupervisorResult<()> {
        if buffer.is_null() || count == 0 {
            // Empty initialization is valid
            if self.initialized.swap(true, Ordering::SeqCst) {
                return Err(UnblockError::AlreadyInitialized.into());
            }
            log::info!("UnblockedMemoryTracker initialized with 0 regions (empty)");
            return Ok(());
        }

        // SAFETY: Caller guarantees buffer is valid for count entries
        let descriptors = unsafe { core::slice::from_raw_parts(buffer, count) };
        self.init_from_descriptors(descriptors)
    }

    /// Marks core initialization as complete.
    ///
    /// After this is called, memory access checks will be enforced.
    /// Before this, all memory is considered accessible (for bootstrap).
    pub fn set_core_init_complete(&self) {
        self.core_init_complete.store(true, Ordering::Release);
        log::info!("UnblockedMemoryTracker: Core initialization complete, enforcing checks");
    }

    /// Checks if core initialization is complete.
    pub fn is_core_init_complete(&self) -> bool {
        self.core_init_complete.load(Ordering::Acquire)
    }

    /// Unblocks a memory region for access.
    ///
    /// This adds a new region to the unblocked list after validating:
    /// - The region does not overlap with MMRAM
    /// - The region is not already unblocked with different attributes
    /// - Identical unblock requests are allowed (idempotent)
    #[cfg(test)]
    pub fn unblock_memory(&self, base: u64, size: u64, attributes: u32) -> MmSupervisorResult<()> {
        self.track_unblocked_memory(base, size, attributes).map(|_| ())
    }

    /// Records a region as unblocked, reporting whether it was newly added.
    ///
    /// An identical request for a region already recorded reports
    /// [`TrackOutcome::Existing`] rather than failing, so a repeated unblock is harmless. The
    /// same base and size with different attributes is a conflict, not a repeat.
    fn track_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> MmSupervisorResult<TrackOutcome> {
        // Validate parameters. The caller logs the range and the returned variant, so the
        // two rejections below stay quiet.
        if size == 0 {
            return Err(UnblockError::InvalidParameter.into());
        }

        // Check for address overflow
        if base.checked_add(size).is_none() {
            return Err(UnblockError::AddressOverflow.into());
        }

        // Check if the region overlaps with MMRAM
        if (self.overlaps_mmram)(base, size) {
            log::error!(
                "unblock_memory: Region 0x{:016x} - 0x{:016x} overlaps with MMRAM",
                base,
                base.saturating_add(size)
            );
            return Err(UnblockError::OverlapsWithMmram.into());
        }

        let mut state = self.state.lock();

        // Check for existing entries that might conflict
        // First, check for exact match (idempotent unblock)
        if let Some(existing) = state.find_exact_match(base, size) {
            if existing.attributes == attributes {
                // Identical request - this is allowed (idempotent)
                log::debug!(
                    "unblock_memory: Region 0x{:016x} - 0x{:016x} already unblocked with same attributes",
                    base,
                    base.saturating_add(size)
                );
                return Ok(TrackOutcome::Existing);
            }
            // Same base/size but different attributes - conflict
            log::error!(
                "unblock_memory: Region 0x{:016x} - 0x{:016x} already unblocked with different attributes (existing: 0x{:x}, requested: 0x{:x})",
                base,
                base.saturating_add(size),
                existing.attributes,
                attributes
            );
            return Err(UnblockError::ConflictingAttributes.into());
        }

        // Check for partial overlaps (not allowed)
        // We iterate directly without collecting to avoid heap allocation
        let mut has_overlap = false;
        for entry in state.entries.iter().take(state.count) {
            if entry.overlaps(base, size) {
                log::error!(
                    "unblock_memory: Region 0x{:016x} - 0x{:016x} overlaps with existing region 0x{:016x} - 0x{:016x}",
                    base,
                    base.saturating_add(size),
                    entry.base_address,
                    entry.end_address()
                );
                has_overlap = true;
                // Continue to log all overlaps for debugging
            }
        }

        if has_overlap {
            return Err(UnblockError::ConflictingAttributes.into());
        }

        // No conflicts - add the new entry
        state.add_entry(base, size, attributes)?;

        log::debug!(
            "unblock_memory: Unblocked region 0x{:016x} - 0x{:016x} with attributes 0x{:x}",
            base,
            base.saturating_add(size),
            attributes
        );

        Ok(TrackOutcome::Added)
    }

    /// Removes the entry matching `base`, `size` and `attributes` exactly.
    ///
    /// Returns whether an entry was removed. Used to undo a tracked region when the mapping that
    /// should have followed it failed, so all three values must match to avoid dropping an
    /// adjacent entry. The last entry is moved into the freed slot, so the recorded order is
    /// not preserved.
    fn remove_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> bool {
        let mut state = self.state.lock();
        let Some(index) = state
            .entries
            .iter()
            .take(state.count)
            .position(|entry| entry.base_address == base && entry.size() == size && entry.attributes == attributes)
        else {
            return false;
        };
        let Some(last_index) = state.count.checked_sub(1) else {
            return false;
        };
        let Some(last_entry) = state.entries.get(last_index).copied() else {
            return false;
        };
        let Some(removed_entry) = state.entries.get_mut(index) else {
            return false;
        };
        *removed_entry = last_entry;
        let Some(last_entry) = state.entries.get_mut(last_index) else {
            return false;
        };
        *last_entry = UnblockedMemoryEntry::empty();
        state.count = last_index;
        true
    }

    /// Checks if a memory region is blocked (i.e., NOT in the unblocked list).
    ///
    /// This is the inverse of checking if memory is accessible - a blocked region
    /// should not be accessed by MM handlers.
    ///
    /// ## Note
    ///
    /// Before core initialization is complete, this always returns `false`
    /// (everything is accessible during bootstrap).
    pub fn is_memory_blocked(&self, base: u64, size: u64) -> bool {
        // During initialization, everything is accessible
        if !self.core_init_complete.load(Ordering::Acquire) {
            return false;
        }

        // Zero-size queries are invalid
        if size == 0 {
            log::warn!("is_memory_blocked: Zero-size query for address 0x{base:016x}");
            return true; // Invalid query = blocked
        }

        // Check for address overflow
        if base.checked_add(size).is_none() {
            log::warn!("is_memory_blocked: Address overflow for 0x{base:016x} + 0x{size:x}");
            return true; // Invalid query = blocked
        }

        let state = self.state.lock();

        // Check if the queried region is fully contained within any unblocked entry
        for entry in state.entries.iter().take(state.count) {
            if entry.contains(base, size) {
                log::trace!(
                    "is_memory_blocked: Region 0x{:016x} - 0x{:016x} is within unblocked region 0x{:016x} - 0x{:016x}",
                    base,
                    base.saturating_add(size),
                    entry.base_address,
                    entry.end_address()
                );
                return false; // Found within unblocked region
            }
        }

        log::trace!(
            "is_memory_blocked: Region 0x{:016x} - 0x{:016x} is NOT within any unblocked region",
            base,
            base.saturating_add(size)
        );

        true // Not found in any unblocked region = blocked
    }

    /// Checks if a memory region is within unblocked regions (the inverse of `is_memory_blocked`).
    ///
    /// This is a convenience method that returns `true` if the region is accessible.
    pub fn is_within_unblocked_region(&self, base: u64, size: u64) -> bool {
        !self.is_memory_blocked(base, size)
    }

    /// Gets the current count of unblocked regions.
    #[cfg(test)]
    pub fn region_count(&self) -> usize {
        self.state.lock().count
    }

    /// Dumps the unblocked regions for debugging.
    pub fn dump_regions(&self) {
        let state = self.state.lock();

        log::info!("UnblockedMemoryTracker: {} regions", state.count);
        for (i, entry) in state.entries.iter().take(state.count).enumerate() {
            let r = if (entry.attributes & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
            let w = if (entry.attributes & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
            let x = if (entry.attributes & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };
            log::info!("  [{}] 0x{:016x} - 0x{:016x} {}{}{}", i, entry.base_address, entry.end_address(), r, w, x);
        }
    }
}

/// An unblock request whose bounds and placement have already been checked.
///
/// Parsing and validation produce this, so the steps that record the region and change the page
/// table work from checked values rather than re-reading the Ring 3 request.
#[derive(Clone, Copy, Debug)]
struct ValidatedUnblockRequest {
    /// Base address of the region, non-zero, page aligned and outside MMRAM.
    physical_start: u64,
    /// Region length in bytes, known not to overflow.
    region_size: u64,
    /// Whether the region is to be mapped supervisor-only rather than user accessible.
    is_supervisor_page: bool,
    /// Attributes recorded with the tracked entry, used to match a later removal.
    track_attributes: u32,
}

/// Errors produced while updating the page table for an unblock request.
///
/// These describe a single step of one request. The caller sees them widened into
/// [`MmSupervisorError`](crate::error::MmSupervisorError); the request handler converts them to an
/// `efi::Status` for Ring 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageUpdateError {
    /// The page table is not installed yet, so no mapping can be changed.
    PageTableNotReady,
    /// The region is already mapped, so unblocking it again would change attributes in place.
    AlreadyMapped,
    /// The region's current attributes could not be read from the page table.
    QueryFailed,
    /// The region could not be mapped with the requested attributes.
    MapFailed,
}

impl core::error::Error for PageUpdateError {}

impl core::fmt::Display for PageUpdateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PageTableNotReady => write!(f, "the page table is not installed yet"),
            Self::AlreadyMapped => write!(f, "the region is already mapped"),
            Self::QueryFailed => write!(f, "the region's current attributes could not be read"),
            Self::MapFailed => write!(f, "the region could not be mapped with the requested attributes"),
        }
    }
}

/// The state an unblock request reads and the effects it has.
///
/// A request consults the lock, checks the region against MMRAM, records it, and changes the
/// mapping, all of which reach supervisor-global state. Naming them as a trait keeps
/// [`process_unblock_mem`] free of global access, so the rollback ordering, where a failed
/// mapping must remove the entry that was just recorded, can be driven in a test.
trait UnblockMemoryContext {
    /// Returns whether unblocking has been closed off by the Ready-to-Lock transition.
    fn is_locked(&self) -> bool;
    /// Returns whether the region touches MMRAM at all, which no request may unblock.
    fn overlaps_mmram(&self, base: u64, size: u64) -> bool;
    /// Records the region, reporting whether it was newly added or already present.
    fn track_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> MmSupervisorResult<TrackOutcome>;
    /// Removes a recorded region, used to undo a tracked entry when the mapping fails.
    fn remove_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> bool;
    /// Maps the region with the attributes the request asked for.
    fn update_page_table(&self, request: &ValidatedUnblockRequest) -> MmSupervisorResult<()>;
}

/// The live [`UnblockMemoryContext`], reading the supervisor's own global state.
struct SupervisorUnblockMemoryContext<'a> {
    /// The tracker holding the regions unblocked so far, and the lock flag.
    unblocked_tracker: &'a UnblockedMemoryTracker,
}

impl UnblockMemoryContext for SupervisorUnblockMemoryContext<'_> {
    fn is_locked(&self) -> bool {
        self.unblocked_tracker.is_core_init_complete()
    }

    fn overlaps_mmram(&self, base: u64, size: u64) -> bool {
        (self.unblocked_tracker.overlaps_mmram)(base, size)
    }

    fn track_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> MmSupervisorResult<TrackOutcome> {
        self.unblocked_tracker.track_unblocked_memory(base, size, attributes)
    }

    fn remove_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> bool {
        self.unblocked_tracker.remove_unblocked_memory(base, size, attributes)
    }

    fn update_page_table(&self, request: &ValidatedUnblockRequest) -> MmSupervisorResult<()> {
        let mut page_table = security_state().lock_page_table();
        let Some(page_table) = page_table.as_mut() else {
            log::error!("UNBLOCK_MEM: page table not initialized");
            return Err(PageUpdateError::PageTableNotReady.into());
        };

        update_unblocked_page_table(page_table, request)
    }
}

/// Makes the region present with the attributes the request asked for.
///
/// Only a region with no mapping may be unblocked. A region already present is refused rather
/// than having its attributes changed in place, because that would let a second request widen
/// access to memory the first one already published.
fn update_unblocked_page_table<P: PageTable>(
    page_table: &mut P,
    request: &ValidatedUnblockRequest,
) -> MmSupervisorResult<()> {
    match page_table.query_memory_region(request.physical_start, request.region_size) {
        Ok(current_attrs) => {
            log::error!(
                "UNBLOCK_MEM: pages at 0x{:016x} are already present (attrs: {current_attrs:?}). \
                 Only not-present pages may be unblocked.",
                request.physical_start
            );
            return Err(PageUpdateError::AlreadyMapped.into());
        }
        Err(PtError::NoMapping) => {}
        Err(e) => {
            log::error!(
                "UNBLOCK_MEM: failed to query page attributes for 0x{:016x}-0x{:016x}: {e:?}",
                request.physical_start,
                request.physical_start + request.region_size,
            );
            return Err(PageUpdateError::QueryFailed.into());
        }
    }

    let mut new_attrs = MemoryAttributes::ExecuteProtect;
    if request.is_supervisor_page {
        new_attrs |= MemoryAttributes::Supervisor;
    }

    page_table.map_memory_region(request.physical_start, request.region_size, new_attrs).map_err(|e| {
        log::error!(
            "UNBLOCK_MEM: failed to update page table for 0x{:016x}-0x{:016x}: {e:?}",
            request.physical_start,
            request.physical_start + request.region_size,
        );
        PageUpdateError::MapFailed.into()
    })
}

/// Handle an `UNBLOCK_MEM` request.
///
/// Unblocks a memory region so that user-mode MM drivers can access it.
///
/// ## Validation (stricter than the C `ProcessUnblockPages` implementation)
///
/// 1. **Ready-to-lock check** - reject if core init is complete (post-lock state).
/// 2. **Buffer size** - must hold header + [`MmSupervisorUnblockMemoryParams`].
/// 3. **Zero-GUID** - the identifier GUID must be non-zero.
/// 4. **Page alignment** - `PhysicalStart` must be non-zero and 4 KiB aligned.
/// 5. **Non-zero page count** - `NumberOfPages` must be > 0.
/// 6. **Overflow** - `NumberOfPages * UEFI_PAGE_SIZE` and `PhysicalStart + size` must not overflow.
/// 7. **MMRAM overlap** - region must not overlap supervisor RAM.
/// 8. **Duplicate / conflict** - checked by the [`UnblockedMemoryTracker`].
/// 9. **Page attributes** - pages must be not-present (RP set) and not read-only.
/// 10. **Page table update** - make pages present, R/W, NX; optionally supervisor-only (SP).
pub(crate) fn handle_unblock_mem(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    if comm_buffer.is_null() {
        log::error!("UNBLOCK_MEM: communication buffer is null");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: The communication handler contract guarantees that `comm_buffer` is readable for
    // `comm_buffer_size` bytes. The null check above satisfies `from_raw_parts` for all lengths.
    let buffer = unsafe { core::slice::from_raw_parts(comm_buffer, *comm_buffer_size) };
    let context = SupervisorUnblockMemoryContext { unblocked_tracker: security_state().unblocked_tracker() };
    let status = process_unblock_mem(buffer, &context);
    *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
    status
}

/// Serves an `UNBLOCK_MEM` request against `context`.
fn process_unblock_mem<C: UnblockMemoryContext>(comm_buffer: &[u8], context: &C) -> efi::Status {
    // After core initialization is complete, unblock requests are rejected.
    // This mirrors the C `mMmReadyToLockDone` guard.
    if context.is_locked() {
        log::error!("UNBLOCK_MEM: rejected - core initialization already complete (post ready-to-lock)");
        return efi::Status::ACCESS_DENIED;
    }

    let params = match parse_unblock_request(comm_buffer) {
        Ok(params) => params,
        Err(status) => return status,
    };
    let physical_start = params.memory_descriptor.physical_start;
    let number_of_pages = params.memory_descriptor.number_of_pages;
    let attribute = params.memory_descriptor.attribute;
    let identifier_guid = params.identifier_guid;

    log::debug!(
        "UNBLOCK_MEM: request from {} - PhysicalStart=0x{:016x}, Pages={}, Attr=0x{:x}",
        identifier_guid.as_guid(),
        physical_start,
        number_of_pages,
        attribute,
    );

    let request = match validate_unblock_request(&params) {
        Ok(request) => request,
        Err(status) => return status,
    };

    if context.overlaps_mmram(request.physical_start, request.region_size) {
        log::error!(
            "UNBLOCK_MEM: region 0x{:016x}-0x{:016x} overlaps with MMRAM",
            request.physical_start,
            request.physical_start + request.region_size,
        );
        return efi::Status::SECURITY_VIOLATION;
    }

    let track_outcome =
        match context.track_unblocked_memory(request.physical_start, request.region_size, request.track_attributes) {
            Ok(outcome) => outcome,
            Err(MmSupervisorError::Unblock(UnblockError::ConflictingAttributes)) => {
                log::error!(
                    "UNBLOCK_MEM: region 0x{:016x}-0x{:016x} conflicts with existing entry",
                    request.physical_start,
                    request.physical_start + request.region_size,
                );
                return efi::Status::SECURITY_VIOLATION;
            }
            Err(e) => {
                log::error!(
                    "UNBLOCK_MEM: tracker rejected request for 0x{:016x}-0x{:016x}: {e:?}",
                    request.physical_start,
                    request.physical_start + request.region_size,
                );
                return efi::Status::INVALID_PARAMETER;
            }
        };

    if track_outcome == TrackOutcome::Existing {
        log::debug!(
            "UNBLOCK_MEM: region 0x{:016x}-0x{:016x} already unblocked (idempotent)",
            request.physical_start,
            request.physical_start + request.region_size,
        );
        return efi::Status::SUCCESS;
    }

    if let Err(error) = context.update_page_table(&request) {
        if !context.remove_unblocked_memory(request.physical_start, request.region_size, request.track_attributes) {
            log::error!(
                "UNBLOCK_MEM: failed to roll back tracker entry for 0x{:016x}-0x{:016x}",
                request.physical_start,
                request.physical_start + request.region_size,
            );
        }

        return match error {
            MmSupervisorError::PageUpdate(PageUpdateError::PageTableNotReady) => efi::Status::NOT_READY,
            MmSupervisorError::PageUpdate(PageUpdateError::AlreadyMapped) => efi::Status::SECURITY_VIOLATION,
            _ => efi::Status::DEVICE_ERROR,
        };
    }

    log::debug!(
        "UNBLOCK_MEM: SUCCESS - unblocked 0x{:016x}-0x{:016x} ({} pages, {})",
        request.physical_start,
        request.physical_start + request.region_size,
        patina::uefi_size_to_pages!(request.region_size as usize),
        if request.is_supervisor_page { "supervisor-only" } else { "user-accessible" },
    );

    efi::Status::SUCCESS
}

/// Reads the request parameters that follow the request header.
fn parse_unblock_request(comm_buffer: &[u8]) -> Result<MmSupervisorUnblockMemoryParams, efi::Status> {
    let min_size = MmSupervisorRequestHeader::SIZE + MmSupervisorUnblockMemoryParams::SIZE;
    let Some(payload) = comm_buffer.get(MmSupervisorRequestHeader::SIZE..min_size) else {
        log::error!("UNBLOCK_MEM: buffer too small ({} bytes, need at least {min_size})", comm_buffer.len());
        return Err(efi::Status::BUFFER_TOO_SMALL);
    };

    // SAFETY: `payload` contains exactly one complete parameter structure. Its fields are integer
    // and GUID values for which every bit pattern is valid, and `read_unaligned` imposes no
    // alignment requirement on the communication buffer.
    Ok(unsafe { payload.as_ptr().cast::<MmSupervisorUnblockMemoryParams>().read_unaligned() })
}

/// Checks the request parameters and derives the values the later steps work from.
fn validate_unblock_request(params: &MmSupervisorUnblockMemoryParams) -> Result<ValidatedUnblockRequest, efi::Status> {
    let physical_start = params.memory_descriptor.physical_start;
    let number_of_pages = params.memory_descriptor.number_of_pages;

    if params.identifier_guid == BinaryGuid::ZERO {
        log::error!("UNBLOCK_MEM: identifier GUID is zero");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    if physical_start == 0 {
        log::error!("UNBLOCK_MEM: PhysicalStart is 0");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    if !physical_start.is_multiple_of(UEFI_PAGE_SIZE as u64) {
        log::error!("UNBLOCK_MEM: PhysicalStart 0x{physical_start:016x} is not page-aligned");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    if number_of_pages == 0 {
        log::error!("UNBLOCK_MEM: NumberOfPages is 0");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    let region_size = if let Some(s) = number_of_pages.checked_mul(UEFI_PAGE_SIZE as u64) {
        s
    } else {
        log::error!("UNBLOCK_MEM: NumberOfPages ({number_of_pages}) * UEFI_PAGE_SIZE overflows u64");
        return Err(efi::Status::INVALID_PARAMETER);
    };

    if physical_start.checked_add(region_size).is_none() {
        log::error!("UNBLOCK_MEM: address range 0x{physical_start:016x} + 0x{region_size:x} overflows");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    let is_supervisor_page = (params.memory_descriptor.attribute & efi::MEMORY_SP) != 0;
    let mut track_attributes = policy::RESOURCE_ATTR_READ | policy::RESOURCE_ATTR_WRITE;
    if is_supervisor_page {
        track_attributes |= SUPERVISOR_TRACKING_ATTRIBUTE;
    }

    Ok(ValidatedUnblockRequest { physical_start, region_size, is_supervisor_page, track_attributes })
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::cell::Cell;

    use patina_paging::MockPageTable;
    use serial_test::serial;

    use crate::test_support::{self, PageAlignedMemory};

    use super::*;

    const REQUEST_SIZE: usize = MmSupervisorRequestHeader::SIZE + MmSupervisorUnblockMemoryParams::SIZE;
    const MEMORY_DESCRIPTOR_PHYSICAL_START_OFFSET: usize = 8;
    const MEMORY_DESCRIPTOR_NUMBER_OF_PAGES_OFFSET: usize = 24;
    const MEMORY_DESCRIPTOR_ATTRIBUTE_OFFSET: usize = 32;
    const IDENTIFIER_GUID_OFFSET: usize = 40;

    struct TestUnblockMemoryContext {
        locked: bool,
        overlaps_mmram: bool,
        track_result: MmSupervisorResult<TrackOutcome>,
        page_update_result: MmSupervisorResult<()>,
        rollback_result: bool,
        mmram_checks: Cell<usize>,
        track_calls: Cell<usize>,
        page_update_calls: Cell<usize>,
        rollback_calls: Cell<usize>,
        last_request: Cell<Option<ValidatedUnblockRequest>>,
    }

    impl TestUnblockMemoryContext {
        fn new() -> Self {
            Self {
                locked: false,
                overlaps_mmram: false,
                track_result: Ok(TrackOutcome::Added),
                page_update_result: Ok(()),
                rollback_result: true,
                mmram_checks: Cell::new(0),
                track_calls: Cell::new(0),
                page_update_calls: Cell::new(0),
                rollback_calls: Cell::new(0),
                last_request: Cell::new(None),
            }
        }
    }

    impl UnblockMemoryContext for TestUnblockMemoryContext {
        fn is_locked(&self) -> bool {
            self.locked
        }

        fn overlaps_mmram(&self, _base: u64, _size: u64) -> bool {
            self.mmram_checks.set(self.mmram_checks.get() + 1);
            self.overlaps_mmram
        }

        fn track_unblocked_memory(&self, _base: u64, _size: u64, _attributes: u32) -> MmSupervisorResult<TrackOutcome> {
            self.track_calls.set(self.track_calls.get() + 1);
            self.track_result
        }

        fn remove_unblocked_memory(&self, _base: u64, _size: u64, _attributes: u32) -> bool {
            self.rollback_calls.set(self.rollback_calls.get() + 1);
            self.rollback_result
        }

        fn update_page_table(&self, request: &ValidatedUnblockRequest) -> MmSupervisorResult<()> {
            self.page_update_calls.set(self.page_update_calls.get() + 1);
            self.last_request.set(Some(*request));
            self.page_update_result
        }
    }

    /// A tracker whose MMRAM check reports every range as outside MMRAM, so tracker logic can be
    /// tested without the global page allocator.
    fn create_test_tracker() -> UnblockedMemoryTracker {
        test_support::init_test_logger();
        UnblockedMemoryTracker::with_mmram_check(|_, _| false)
    }

    fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
        buffer
            .get_mut(offset..offset + core::mem::size_of::<u64>())
            .expect("test field must fit in the request buffer")
            .copy_from_slice(&value.to_le_bytes());
    }

    fn request_buffer(
        physical_start: u64,
        number_of_pages: u64,
        attribute: u64,
        valid_guid: bool,
    ) -> [u8; REQUEST_SIZE] {
        test_support::init_test_logger();
        let mut buffer = [0u8; REQUEST_SIZE];
        let payload_offset = MmSupervisorRequestHeader::SIZE;
        write_u64(&mut buffer, payload_offset + MEMORY_DESCRIPTOR_PHYSICAL_START_OFFSET, physical_start);
        write_u64(&mut buffer, payload_offset + MEMORY_DESCRIPTOR_NUMBER_OF_PAGES_OFFSET, number_of_pages);
        write_u64(&mut buffer, payload_offset + MEMORY_DESCRIPTOR_ATTRIBUTE_OFFSET, attribute);
        if valid_guid {
            *buffer
                .get_mut(payload_offset + IDENTIFIER_GUID_OFFSET)
                .expect("identifier GUID must fit in the request buffer") = 1;
        }
        buffer
    }

    fn validated_request(is_supervisor_page: bool) -> ValidatedUnblockRequest {
        test_support::init_test_logger();
        ValidatedUnblockRequest {
            physical_start: 0x2000,
            region_size: 2 * UEFI_PAGE_SIZE as u64,
            is_supervisor_page,
            track_attributes: RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE,
        }
    }

    #[test]
    fn handler_rejects_a_null_buffer() {
        let mut size = REQUEST_SIZE;

        assert_eq!(handle_unblock_mem(core::ptr::null_mut(), &mut size), efi::Status::INVALID_PARAMETER);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }

    #[test]
    fn process_rejects_requests_after_ready_to_lock_before_parsing() {
        let mut context = TestUnblockMemoryContext::new();
        context.locked = true;

        assert_eq!(process_unblock_mem(&[], &context), efi::Status::ACCESS_DENIED);
        assert_eq!(context.mmram_checks.get(), 0);
        assert_eq!(context.track_calls.get(), 0);
    }

    #[test]
    fn process_rejects_a_short_request() {
        let context = TestUnblockMemoryContext::new();
        let buffer = [0u8; REQUEST_SIZE - 1];

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::BUFFER_TOO_SMALL);
        assert_eq!(context.mmram_checks.get(), 0);
        assert_eq!(context.track_calls.get(), 0);
    }

    #[test]
    fn process_rejects_a_zero_identifier_guid() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0x1000, 1, 0, false);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.mmram_checks.get(), 0);
    }

    #[test]
    fn process_rejects_a_zero_physical_start() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.mmram_checks.get(), 0);
    }

    #[test]
    fn process_rejects_an_unaligned_physical_start() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0x1001, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.mmram_checks.get(), 0);
    }

    #[test]
    fn process_rejects_zero_pages() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0x1000, 0, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.mmram_checks.get(), 0);
    }

    #[test]
    fn process_rejects_a_page_count_that_overflows_the_region_size() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0x1000, u64::MAX, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.mmram_checks.get(), 0);
    }

    #[test]
    fn process_rejects_an_address_range_that_overflows() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(u64::MAX - (UEFI_PAGE_SIZE as u64 - 1), 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.mmram_checks.get(), 0);
    }

    #[test]
    fn process_rejects_mmram_overlap_before_tracking() {
        let mut context = TestUnblockMemoryContext::new();
        context.overlaps_mmram = true;
        let buffer = request_buffer(0x1000, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SECURITY_VIOLATION);
        assert_eq!(context.mmram_checks.get(), 1);
        assert_eq!(context.track_calls.get(), 0);
    }

    #[test]
    fn process_treats_an_existing_entry_as_an_idempotent_success() {
        let mut context = TestUnblockMemoryContext::new();
        context.track_result = Ok(TrackOutcome::Existing);
        let buffer = request_buffer(0x1000, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SUCCESS);
        assert_eq!(context.track_calls.get(), 1);
        assert_eq!(context.page_update_calls.get(), 0);
        assert_eq!(context.rollback_calls.get(), 0);
    }

    #[test]
    fn process_maps_tracker_conflicts_to_security_violations() {
        let mut context = TestUnblockMemoryContext::new();
        context.track_result = Err(UnblockError::ConflictingAttributes.into());
        let buffer = request_buffer(0x1000, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SECURITY_VIOLATION);
        assert_eq!(context.page_update_calls.get(), 0);
    }

    #[test]
    fn process_maps_other_tracker_failures_to_invalid_parameter() {
        let mut context = TestUnblockMemoryContext::new();
        context.track_result = Err(UnblockError::TooManyRegions.into());
        let buffer = request_buffer(0x1000, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::INVALID_PARAMETER);
        assert_eq!(context.page_update_calls.get(), 0);
    }

    #[test]
    fn process_rolls_back_tracking_after_each_page_table_failure() {
        let cases = [
            (PageUpdateError::PageTableNotReady, efi::Status::NOT_READY),
            (PageUpdateError::AlreadyMapped, efi::Status::SECURITY_VIOLATION),
            (PageUpdateError::QueryFailed, efi::Status::DEVICE_ERROR),
            (PageUpdateError::MapFailed, efi::Status::DEVICE_ERROR),
        ];
        let buffer = request_buffer(0x1000, 1, 0, true);

        for (page_error, expected_status) in cases {
            let mut context = TestUnblockMemoryContext::new();
            context.page_update_result = Err(page_error.into());

            assert_eq!(process_unblock_mem(&buffer, &context), expected_status);
            assert_eq!(context.page_update_calls.get(), 1);
            assert_eq!(context.rollback_calls.get(), 1);
        }
    }

    #[test]
    fn process_returns_success_for_a_user_accessible_mapping() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0x2000, 3, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SUCCESS);
        let request = context.last_request.get().expect("page-table update must receive the validated request");
        assert_eq!(request.physical_start, 0x2000);
        assert_eq!(request.region_size, 3 * UEFI_PAGE_SIZE as u64);
        assert!(!request.is_supervisor_page);
        assert_eq!(request.track_attributes, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE);
        assert_eq!(context.rollback_calls.get(), 0);
    }

    #[test]
    fn process_preserves_the_supervisor_only_attribute() {
        let context = TestUnblockMemoryContext::new();
        let buffer = request_buffer(0x2000, 1, efi::MEMORY_SP, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SUCCESS);
        let request = context.last_request.get().expect("page-table update must receive the validated request");
        assert!(request.is_supervisor_page);
        assert_eq!(request.track_attributes, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE | SUPERVISOR_TRACKING_ATTRIBUTE);
    }

    #[test]
    fn handler_rejects_a_short_non_null_buffer() {
        let mut buffer = [0u8; REQUEST_SIZE - 1];
        let mut size = buffer.len();

        assert_eq!(handle_unblock_mem(buffer.as_mut_ptr(), &mut size), efi::Status::BUFFER_TOO_SMALL);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }

    #[test]
    fn supervisor_unblock_context_delegates_to_its_real_tracker() {
        let tracker = create_test_tracker();
        let context = SupervisorUnblockMemoryContext { unblocked_tracker: &tracker };

        assert!(!context.is_locked());
        assert!(!context.overlaps_mmram(0x1000, 0x1000));
        assert_eq!(context.track_unblocked_memory(0x1000, 0x1000, RESOURCE_ATTR_READ), Ok(TrackOutcome::Added));
        assert_eq!(context.track_unblocked_memory(0x1000, 0x1000, RESOURCE_ATTR_READ), Ok(TrackOutcome::Existing));
        assert!(context.remove_unblocked_memory(0x1000, 0x1000, RESOURCE_ATTR_READ));
        assert_eq!(tracker.region_count(), 0);
    }

    #[test]
    fn supervisor_unblock_context_reports_an_uninitialized_page_table() {
        let tracker = UnblockedMemoryTracker::new();
        let context = SupervisorUnblockMemoryContext { unblocked_tracker: &tracker };
        *security_state().lock_page_table() = None;

        assert_eq!(
            context.update_page_table(&validated_request(false)),
            Err(PageUpdateError::PageTableNotReady.into())
        );
    }

    #[test]
    fn page_table_update_rejects_an_existing_mapping() {
        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().times(1).returning(|_, _| Ok(MemoryAttributes::ReadOnly));
        page_table.expect_map_memory_region().never();

        assert_eq!(
            update_unblocked_page_table(&mut page_table, &validated_request(false)),
            Err(PageUpdateError::AlreadyMapped.into())
        );
    }

    #[test]
    fn page_table_update_maps_user_data_as_writable_and_non_executable() {
        let request = validated_request(false);
        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().times(1).returning(|_, _| Err(PtError::NoMapping));
        page_table
            .expect_map_memory_region()
            .withf(move |base, size, attributes| {
                *base == request.physical_start
                    && *size == request.region_size
                    && *attributes == MemoryAttributes::ExecuteProtect
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        assert_eq!(update_unblocked_page_table(&mut page_table, &request), Ok(()));
    }

    #[test]
    fn page_table_update_preserves_supervisor_only_access() {
        let request = validated_request(true);
        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().times(1).returning(|_, _| Err(PtError::NoMapping));
        page_table
            .expect_map_memory_region()
            .withf(move |base, size, attributes| {
                *base == request.physical_start
                    && *size == request.region_size
                    && *attributes == MemoryAttributes::ExecuteProtect | MemoryAttributes::Supervisor
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        assert_eq!(update_unblocked_page_table(&mut page_table, &request), Ok(()));
    }

    #[test]
    fn page_table_update_reports_query_failure() {
        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().times(1).returning(|_, _| Err(PtError::InvalidParameter));
        page_table.expect_map_memory_region().never();

        assert_eq!(
            update_unblocked_page_table(&mut page_table, &validated_request(false)),
            Err(PageUpdateError::QueryFailed.into())
        );
    }

    #[test]
    fn page_table_update_reports_mapping_failure() {
        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().times(1).returning(|_, _| Err(PtError::NoMapping));
        page_table.expect_map_memory_region().times(1).returning(|_, _, _| Err(PtError::OutOfResources));

        assert_eq!(
            update_unblocked_page_table(&mut page_table, &validated_request(false)),
            Err(PageUpdateError::MapFailed.into())
        );
    }

    #[test]
    fn test_unblock_errors_render_each_variant_distinctly() {
        // An unblock request that fails reaches Ring 3 as a bare EFI status, so this message is
        // the only record of why. The table is exhaustive, so a variant added without a Display
        // arm fails here rather than rendering as another variant's text.
        let all = [
            (UnblockError::AlreadyInitialized, "the unblocked memory tracker is already initialized"),
            (UnblockError::TooManyRegions, "the request exceeds the 64 tracked unblocked regions"),
            (UnblockError::OverlapsWithMmram, "the requested region overlaps MMRAM"),
            (
                UnblockError::ConflictingAttributes,
                "the requested region overlaps an unblocked region with different attributes",
            ),
            (UnblockError::InvalidParameter, "the request has a null pointer or a zero length"),
            (UnblockError::AddressOverflow, "the requested region's address plus size overflows"),
        ];

        for (error, expected) in all {
            assert_eq!(format!("{error}"), expected);
        }

        // The message carries the limit, so it stays correct if the table is resized.
        assert!(format!("{}", UnblockError::TooManyRegions).contains(&MAX_UNBLOCKED_REGIONS.to_string()));
    }

    #[test]
    fn test_page_update_errors_render_each_variant_distinctly() {
        // The four ways a mapping change can fail are reported through one status, so the
        // message is what separates a missing page table from a refused map.
        let all = [
            (PageUpdateError::PageTableNotReady, "the page table is not installed yet"),
            (PageUpdateError::AlreadyMapped, "the region is already mapped"),
            (PageUpdateError::QueryFailed, "the region's current attributes could not be read"),
            (PageUpdateError::MapFailed, "the region could not be mapped with the requested attributes"),
        ];

        for (error, expected) in all {
            assert_eq!(format!("{error}"), expected);
        }

        // A read that failed and a write that failed are different findings.
        assert_ne!(format!("{}", PageUpdateError::QueryFailed), format!("{}", PageUpdateError::MapFailed));
    }

    #[test]
    fn test_empty_entry() {
        let entry = UnblockedMemoryEntry::empty();
        assert_eq!(entry.num_pages, 0);
        assert_eq!(entry.base_address, 0);
        assert_eq!(entry.size(), 0);
    }

    #[test]
    fn test_entry_is_compact() {
        // The tracker is a SEA-audited static, so guard against the per-entry
        // footprint regressing: base (u64) + num_pages (u32) + attributes (u32).
        assert_eq!(core::mem::size_of::<UnblockedMemoryEntry>(), 16);
    }

    #[test]
    fn test_entry_size_round_trips_pages() {
        let entry = UnblockedMemoryEntry::new(0x2000, 4 * UEFI_PAGE_SIZE as u64, RESOURCE_ATTR_READ);
        assert_eq!(entry.num_pages, 4);
        assert_eq!(entry.size(), 4 * UEFI_PAGE_SIZE as u64);
        assert_eq!(entry.end_address(), 0x2000 + 4 * UEFI_PAGE_SIZE as u64);

        // A sub-page size rounds up to a whole page (matching the page-granular mapping).
        let partial = UnblockedMemoryEntry::new(0x1000, 1, RESOURCE_ATTR_READ);
        assert_eq!(partial.num_pages, 1);
        assert_eq!(partial.size(), UEFI_PAGE_SIZE as u64);
    }

    #[test]
    fn test_entry_contains() {
        let entry = UnblockedMemoryEntry::new(0x1000, 0x1000, RESOURCE_ATTR_READ);

        // Fully contained
        assert!(entry.contains(0x1000, 0x1000));
        assert!(entry.contains(0x1000, 0x800));
        assert!(entry.contains(0x1800, 0x800));

        // Partially outside
        assert!(!entry.contains(0x0800, 0x1000)); // Starts before
        assert!(!entry.contains(0x1800, 0x1000)); // Ends after

        // Completely outside
        assert!(!entry.contains(0x3000, 0x1000));
        assert!(!entry.contains(u64::MAX, 1));
        assert!(!entry.contains(0x1000, 0));
    }

    #[test]
    fn test_entry_overlaps() {
        let entry = UnblockedMemoryEntry::new(0x1000, 0x1000, RESOURCE_ATTR_READ);

        // Overlapping cases
        assert!(entry.overlaps(0x1000, 0x1000)); // Exact match
        assert!(entry.overlaps(0x0800, 0x1000)); // Starts before, ends inside
        assert!(entry.overlaps(0x1800, 0x1000)); // Starts inside, ends after
        assert!(entry.overlaps(0x0800, 0x2000)); // Completely contains entry

        // Non-overlapping
        assert!(!entry.overlaps(0x2000, 0x1000)); // Immediately after
        assert!(!entry.overlaps(0x0000, 0x1000)); // Immediately before
        assert!(!entry.overlaps(0x3000, 0x1000)); // Far after
        assert!(!entry.overlaps(u64::MAX, 1));
        assert!(!entry.overlaps(0x1000, 0));
    }

    #[test]
    fn test_tracker_before_init_complete() {
        let tracker = create_test_tracker();

        // Before core init complete, nothing is blocked
        assert!(!tracker.is_memory_blocked(0x1000, 0x1000));
        assert!(!tracker.is_memory_blocked(0x0, 0x100000));
        assert!(!tracker.is_core_init_complete());
    }

    #[test]
    fn test_tracker_after_init_complete_empty() {
        let tracker = create_test_tracker();
        tracker.set_core_init_complete();
        assert!(tracker.is_core_init_complete());

        // After init complete with no regions, everything is blocked
        assert!(tracker.is_memory_blocked(0x1000, 0x1000));
        assert!(tracker.is_memory_blocked(0x1000, 0));
        assert!(tracker.is_memory_blocked(u64::MAX, 1));
        assert!(!tracker.is_within_unblocked_region(0x1000, 0x1000));
    }

    #[test]
    fn test_unblock_memory() {
        let tracker = create_test_tracker();

        // Unblock a region
        assert!(tracker.unblock_memory(0x1000, 0x1000, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE).is_ok());

        tracker.set_core_init_complete();

        // Region should be accessible
        assert!(!tracker.is_memory_blocked(0x1000, 0x1000));
        assert!(!tracker.is_memory_blocked(0x1000, 0x800));

        // Outside region should be blocked
        assert!(tracker.is_memory_blocked(0x3000, 0x1000));
    }

    #[test]
    fn test_idempotent_unblock() {
        let tracker = create_test_tracker();

        // First unblock
        assert!(tracker.unblock_memory(0x1000, 0x1000, RESOURCE_ATTR_READ).is_ok());

        // Identical unblock should succeed
        assert!(tracker.unblock_memory(0x1000, 0x1000, RESOURCE_ATTR_READ).is_ok());

        // Same region with different attributes should fail
        assert_eq!(
            tracker.unblock_memory(0x1000, 0x1000, RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE),
            Err(UnblockError::ConflictingAttributes.into())
        );
    }

    #[test]
    fn test_overlapping_unblock_fails() {
        let tracker = create_test_tracker();

        // First unblock
        assert!(tracker.unblock_memory(0x1000, 0x1000, RESOURCE_ATTR_READ).is_ok());

        // Overlapping unblock should fail
        assert_eq!(
            tracker.unblock_memory(0x1800, 0x1000, RESOURCE_ATTR_READ),
            Err(UnblockError::ConflictingAttributes.into())
        );
    }

    #[test]
    fn test_invalid_parameters() {
        let tracker = create_test_tracker();
        tracker.set_core_init_complete();

        // Zero size
        assert_eq!(tracker.unblock_memory(0x1000, 0, RESOURCE_ATTR_READ), Err(UnblockError::InvalidParameter.into()));

        // Overflow
        assert_eq!(
            tracker.unblock_memory(u64::MAX, 0x1000, RESOURCE_ATTR_READ),
            Err(UnblockError::AddressOverflow.into())
        );
    }

    #[test]
    fn test_region_count() {
        let tracker = create_test_tracker();

        assert_eq!(tracker.region_count(), 0);

        tracker.unblock_memory(0x1000, 0x1000, RESOURCE_ATTR_READ).unwrap();
        assert_eq!(tracker.region_count(), 1);

        tracker.unblock_memory(0x3000, 0x1000, RESOURCE_ATTR_WRITE).unwrap();
        assert_eq!(tracker.region_count(), 2);
    }

    #[test]
    fn test_tracker_reports_capacity_exhaustion() {
        let tracker = create_test_tracker();

        for index in 0..MAX_UNBLOCKED_REGIONS {
            let base = (index as u64 + 1) * 0x2000;
            assert_eq!(tracker.track_unblocked_memory(base, 0x1000, RESOURCE_ATTR_READ), Ok(TrackOutcome::Added));
        }

        assert_eq!(
            tracker.track_unblocked_memory(0x10_0000, 0x1000, RESOURCE_ATTR_READ),
            Err(UnblockError::TooManyRegions.into())
        );
    }

    #[test]
    fn test_tracker_removes_only_the_matching_entry() {
        let tracker = create_test_tracker();
        assert_eq!(tracker.track_unblocked_memory(0x1000, 0x1000, RESOURCE_ATTR_READ), Ok(TrackOutcome::Added));
        assert_eq!(tracker.track_unblocked_memory(0x3000, 0x1000, RESOURCE_ATTR_WRITE), Ok(TrackOutcome::Added));

        assert!(!tracker.remove_unblocked_memory(0x1000, 0x1000, RESOURCE_ATTR_WRITE));
        assert!(tracker.remove_unblocked_memory(0x1000, 0x1000, RESOURCE_ATTR_READ));
        assert_eq!(tracker.region_count(), 1);

        tracker.set_core_init_complete();
        assert!(tracker.is_memory_blocked(0x1000, 0x1000));
        assert!(!tracker.is_memory_blocked(0x3000, 0x1000));
    }

    #[test]
    fn test_tracker_initializes_from_descriptors_once() {
        let tracker = create_test_tracker();
        let descriptors = [
            MemDescriptorV1_0 { base_address: 0x1000, size: 0x1000, mem_attributes: RESOURCE_ATTR_READ, reserved: 0 },
            MemDescriptorV1_0 { base_address: 0x3000, size: 0x2000, mem_attributes: RESOURCE_ATTR_WRITE, reserved: 0 },
        ];

        assert_eq!(tracker.init_from_descriptors(&descriptors), Ok(()));
        assert_eq!(tracker.region_count(), 2);
        assert_eq!(tracker.init_from_descriptors(&[]), Err(UnblockError::AlreadyInitialized.into()));
        tracker.dump_regions();
    }

    #[test]
    fn test_tracker_init_rejects_a_zero_size_descriptor() {
        let tracker = create_test_tracker();
        let descriptors = [
            MemDescriptorV1_0 { base_address: 0x1000, size: 0x1000, mem_attributes: RESOURCE_ATTR_READ, reserved: 0 },
            MemDescriptorV1_0 { base_address: 0x3000, size: 0, mem_attributes: RESOURCE_ATTR_READ, reserved: 0 },
        ];

        assert_eq!(tracker.init_from_descriptors(&descriptors), Err(UnblockError::InvalidParameter.into()));
    }

    #[test]
    fn test_tracker_rejects_a_region_that_overlaps_mmram() {
        test_support::init_test_logger();
        let tracker = UnblockedMemoryTracker::with_mmram_check(|_, _| true);

        assert_eq!(
            tracker.track_unblocked_memory(0x1000, 0x2000, RESOURCE_ATTR_READ),
            Err(UnblockError::OverlapsWithMmram.into())
        );
        assert_eq!(tracker.region_count(), 0);
    }

    #[test]
    #[serial]
    fn test_tracker_init_rejects_a_descriptor_that_overlaps_mmram() {
        let mmram = PageAlignedMemory::new(crate::memory::DEFAULT_PAGING_POOL_PAGES + 8);
        test_support::init_global_state_over(&mmram);
        let page = UEFI_PAGE_SIZE as u64;
        let mmram_end = mmram.base() + mmram.size();

        // Fully inside, crossing the end, and crossing the start of MMRAM.
        for (base, size) in [(mmram.base(), page), (mmram_end - page, 2 * page), (mmram.base() - page, 2 * page)] {
            let tracker = UnblockedMemoryTracker::new();
            let descriptors =
                [MemDescriptorV1_0 { base_address: base, size, mem_attributes: RESOURCE_ATTR_READ, reserved: 0 }];

            assert_eq!(tracker.init_from_descriptors(&descriptors), Err(UnblockError::OverlapsWithMmram.into()));
            assert_eq!(tracker.region_count(), 0);
        }
    }

    #[test]
    #[serial]
    fn test_unblock_request_that_crosses_an_mmram_boundary_is_rejected() {
        let mmram = PageAlignedMemory::new(crate::memory::DEFAULT_PAGING_POOL_PAGES + 8);
        test_support::init_global_state_over(&mmram);
        let page = UEFI_PAGE_SIZE as u64;
        let mmram_end = mmram.base() + mmram.size();
        let tracker = UnblockedMemoryTracker::new();
        let context = SupervisorUnblockMemoryContext { unblocked_tracker: &tracker };

        // The test page table maps nothing, which is how free MMRAM pages look, so only the
        // MMRAM check stands between these requests and a Ring 3 mapping of MMRAM.
        for (base, pages) in [(mmram_end - page, 1), (mmram_end - page, 2), (mmram.base() - page, 2)] {
            let buffer = request_buffer(base, pages, 0, true);

            assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SECURITY_VIOLATION);
        }

        let page_table = security_state().lock_page_table();
        let page_table = page_table.as_ref().expect("a page table is installed");
        assert_eq!(page_table.query_memory_region(mmram.base(), page), Err(PtError::NoMapping));
        assert_eq!(page_table.query_memory_region(mmram_end - page, page), Err(PtError::NoMapping));
        assert_eq!(tracker.region_count(), 0);
    }

    #[test]
    fn test_tracker_reports_descriptor_initialization_capacity_exhaustion() {
        let tracker = create_test_tracker();
        let descriptors: Vec<_> = (0..=MAX_UNBLOCKED_REGIONS)
            .map(|index| MemDescriptorV1_0 {
                base_address: (index as u64 + 1) * 0x2000,
                size: 0x1000,
                mem_attributes: RESOURCE_ATTR_READ,
                reserved: 0,
            })
            .collect();

        assert_eq!(tracker.init_from_descriptors(&descriptors), Err(UnblockError::TooManyRegions.into()));
        assert_eq!(tracker.region_count(), MAX_UNBLOCKED_REGIONS);
    }

    #[test]
    fn test_tracker_initializes_from_empty_raw_buffer_once() {
        let tracker = create_test_tracker();

        // SAFETY: A null buffer with zero entries is explicitly supported as empty initialization.
        assert_eq!(unsafe { tracker.init_from_buffer(core::ptr::null(), 0) }, Ok(()));
        assert_eq!(
            // SAFETY: A null buffer with zero entries remains a valid request and reaches the
            // already-initialized check.
            unsafe { tracker.init_from_buffer(core::ptr::null(), 0) },
            Err(UnblockError::AlreadyInitialized.into())
        );
    }

    #[test]
    fn test_tracker_initializes_from_a_raw_descriptor_buffer() {
        let tracker = create_test_tracker();
        let descriptors =
            [MemDescriptorV1_0 { base_address: 0x1000, size: 0x1000, mem_attributes: RESOURCE_ATTR_READ, reserved: 0 }];

        // SAFETY: `descriptors` is a live array containing exactly one valid descriptor.
        assert_eq!(unsafe { tracker.init_from_buffer(descriptors.as_ptr(), descriptors.len()) }, Ok(()));
        assert_eq!(tracker.region_count(), 1);
    }
}
