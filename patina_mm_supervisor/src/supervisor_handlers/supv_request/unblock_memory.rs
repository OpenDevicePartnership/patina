//! Unblocked Memory Region Management
//!
//! This module provides functionality to track and manage memory regions that have been
//! unblocked for access in the MM (Management Mode) environment, similar to `UnblockMemory.c`.
//!
//! ## Overview
//!
//! The MM Supervisor maintains a list of memory regions that have been explicitly unblocked
//! for access. By default, all memory outside MMRAM is blocked. Drivers and handlers can
//! request specific regions to be unblocked via the `unblock_memory` interface.
//!
//! ## Design
//!
//! - The unblocked region tracker is initialized from memory policy descriptors
//! - Regions can be dynamically added via `unblock_memory()`
//! - Access checks use `is_memory_blocked()` to validate memory access requests
//! - Duplicate unblock requests with identical attributes are allowed (idempotent)
//! - Overlapping requests with different attributes are rejected
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

use patina::UEFI_PAGE_SIZE;
use patina_paging::{MemoryAttributes, PageTable, PtError};

use patina::management_mode::protocol::mm_supervisor_request::{
    MmSupervisorRequestHeader, MmSupervisorUnblockMemoryParams,
};

use crate::{
    mm_policy,
    mm_policy::{MemDescriptorV1_0, RESOURCE_ATTR_EXECUTE, RESOURCE_ATTR_READ, RESOURCE_ATTR_WRITE},
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
        u64::from(self.num_pages) * UEFI_PAGE_SIZE as u64
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackOutcome {
    Added,
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
    fn add_entry(&mut self, base: u64, size: u64, attributes: u32) -> Result<(), UnblockError> {
        let slot = self.entries.get_mut(self.count).ok_or(UnblockError::TooManyRegions)?;
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
    /// Internal state protected by a mutex.
    state: Mutex<UnblockedMemoryState>,
}

impl UnblockedMemoryTracker {
    /// Creates a new unblocked memory tracker.
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            core_init_complete: AtomicBool::new(false),
            state: Mutex::new(UnblockedMemoryState::new()),
        }
    }

    /// Initializes the tracker from an array of memory policy descriptors, which represent the
    /// initial "unblocked" regions.
    ///
    /// This should be called once during BSP initialization after the memory
    /// policy has been generated from the page table walk. Returns an error if the tracker is
    /// already initialized or if there are too many descriptors to track.
    pub fn init_from_descriptors(&self, descriptors: &[MemDescriptorV1_0]) -> Result<(), UnblockError> {
        // Check if already initialized
        if self.initialized.swap(true, Ordering::SeqCst) {
            return Err(UnblockError::AlreadyInitialized);
        }

        let mut state = self.state.lock();

        // Add each descriptor as an unblocked region
        for desc in descriptors {
            if desc.size == 0 {
                continue; // Skip zero-size entries
            }

            // Skip regions inside MMRAM - those are supervisor-controlled, not "unblocked"
            if security_state().page_allocator().is_region_inside_mmram(desc.base_address, desc.size) {
                log::trace!(
                    "Skipping MMRAM region during unblock init: 0x{:016x} - 0x{:016x}",
                    desc.base_address,
                    desc.base_address.saturating_add(desc.size)
                );
                continue;
            }

            state.add_entry(desc.base_address, desc.size, desc.mem_attributes)?;
        }

        log::info!("UnblockedMemoryTracker initialized with {} regions", state.count);

        Ok(())
    }

    /// Initializes the tracker from a raw buffer of memory policy descriptors.
    ///
    /// ## Safety
    ///
    /// The caller must ensure:
    /// - `buffer` points to a valid array of `MemDescriptorV1_0` structures
    /// - `count` is the number of valid entries in the buffer
    pub unsafe fn init_from_buffer(&self, buffer: *const MemDescriptorV1_0, count: usize) -> Result<(), UnblockError> {
        if buffer.is_null() || count == 0 {
            // Empty initialization is valid
            if self.initialized.swap(true, Ordering::SeqCst) {
                return Err(UnblockError::AlreadyInitialized);
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn unblock_memory(&self, base: u64, size: u64, attributes: u32) -> Result<(), UnblockError> {
        self.track_unblocked_memory(base, size, attributes).map(|_| ())
    }

    fn track_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> Result<TrackOutcome, UnblockError> {
        // Validate parameters
        if size == 0 {
            return Err(UnblockError::InvalidParameter);
        }

        // Check for address overflow
        if base.checked_add(size).is_none() {
            return Err(UnblockError::AddressOverflow);
        }

        // Check if the region overlaps with MMRAM
        if security_state().page_allocator().is_region_inside_mmram(base, size) {
            log::error!(
                "unblock_memory: Region 0x{:016x} - 0x{:016x} overlaps with MMRAM",
                base,
                base.saturating_add(size)
            );
            return Err(UnblockError::OverlapsWithMmram);
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
            return Err(UnblockError::ConflictingAttributes);
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
            return Err(UnblockError::ConflictingAttributes);
        }

        // No conflicts - add the new entry
        state.add_entry(base, size, attributes)?;

        log::info!(
            "unblock_memory: Unblocked region 0x{:016x} - 0x{:016x} with attributes 0x{:x}",
            base,
            base.saturating_add(size),
            attributes
        );

        Ok(TrackOutcome::Added)
    }

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
    #[inline]
    pub fn is_within_unblocked_region(&self, base: u64, size: u64) -> bool {
        !self.is_memory_blocked(base, size)
    }

    /// Gets the current count of unblocked regions.
    #[cfg_attr(not(test), allow(dead_code))]
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

#[derive(Clone, Copy, Debug)]
struct ValidatedUnblockRequest {
    physical_start: u64,
    number_of_pages: u64,
    region_size: u64,
    is_supervisor_page: bool,
    track_attributes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageUpdateError {
    PageTableNotReady,
    AlreadyMapped,
    QueryFailed,
    MapFailed,
}

trait UnblockMemoryContext {
    fn is_locked(&self) -> bool;
    fn is_inside_mmram(&self, base: u64, size: u64) -> bool;
    fn track_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> Result<TrackOutcome, UnblockError>;
    fn remove_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> bool;
    fn update_page_table(&self, request: &ValidatedUnblockRequest) -> Result<(), PageUpdateError>;
}

trait UnblockPageTable {
    fn query_region(&self, base: u64, size: u64) -> Result<MemoryAttributes, PtError>;
    fn map_region(&mut self, base: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError>;
}

impl<T: PageTable> UnblockPageTable for T {
    fn query_region(&self, base: u64, size: u64) -> Result<MemoryAttributes, PtError> {
        self.query_memory_region(base, size)
    }

    fn map_region(&mut self, base: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
        self.map_memory_region(base, size, attributes)
    }
}

struct SupervisorUnblockMemoryContext<'a> {
    unblocked_tracker: &'a UnblockedMemoryTracker,
}

impl UnblockMemoryContext for SupervisorUnblockMemoryContext<'_> {
    fn is_locked(&self) -> bool {
        self.unblocked_tracker.is_core_init_complete()
    }

    fn is_inside_mmram(&self, base: u64, size: u64) -> bool {
        security_state().page_allocator().is_region_inside_mmram(base, size)
    }

    fn track_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> Result<TrackOutcome, UnblockError> {
        self.unblocked_tracker.track_unblocked_memory(base, size, attributes)
    }

    fn remove_unblocked_memory(&self, base: u64, size: u64, attributes: u32) -> bool {
        self.unblocked_tracker.remove_unblocked_memory(base, size, attributes)
    }

    fn update_page_table(&self, request: &ValidatedUnblockRequest) -> Result<(), PageUpdateError> {
        let mut page_table = security_state().lock_page_table();
        let Some(page_table) = page_table.as_mut() else {
            log::error!("UNBLOCK_MEM: page table not initialized");
            return Err(PageUpdateError::PageTableNotReady);
        };

        update_unblocked_page_table(page_table, request)
    }
}

fn update_unblocked_page_table<P: UnblockPageTable>(
    page_table: &mut P,
    request: &ValidatedUnblockRequest,
) -> Result<(), PageUpdateError> {
    match page_table.query_region(request.physical_start, request.region_size) {
        Ok(current_attrs) => {
            log::error!(
                "UNBLOCK_MEM: pages at 0x{:016x} are already present (attrs: {current_attrs:?}). \
                 Only not-present pages may be unblocked.",
                request.physical_start
            );
            return Err(PageUpdateError::AlreadyMapped);
        }
        Err(PtError::NoMapping) => {}
        Err(e) => {
            log::error!(
                "UNBLOCK_MEM: failed to query page attributes for 0x{:016x}-0x{:016x}: {e:?}",
                request.physical_start,
                request.physical_start + request.region_size,
            );
            return Err(PageUpdateError::QueryFailed);
        }
    }

    let mut new_attrs = MemoryAttributes::ExecuteProtect;
    if request.is_supervisor_page {
        new_attrs |= MemoryAttributes::Supervisor;
    }

    page_table.map_region(request.physical_start, request.region_size, new_attrs).map_err(|e| {
        log::error!(
            "UNBLOCK_MEM: failed to update page table for 0x{:016x}-0x{:016x}: {e:?}",
            request.physical_start,
            request.physical_start + request.region_size,
        );
        PageUpdateError::MapFailed
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
/// 4. **Page alignment** - `PhysicalStart` must be 4 KiB aligned.
/// 5. **Non-zero page count** - `NumberOfPages` must be > 0.
/// 6. **Overflow** - `NumberOfPages * UEFI_PAGE_SIZE` and `PhysicalStart + size` must not overflow.
/// 7. **MMRAM overlap** - region must not overlap supervisor RAM.
/// 8. **Duplicate / conflict** - checked by the [`UnblockedMemoryTracker`].
/// 9. **Page attributes** - pages must be not-present (RP set) and not read-only.
/// 10. **Page table update** - make pages present, R/W, NX; optionally supervisor-only (SP).
pub(crate) fn handle_unblock_mem(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("UNBLOCK_MEM request");

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

    log::info!(
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

    if context.is_inside_mmram(request.physical_start, request.region_size) {
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
            Err(UnblockError::ConflictingAttributes) => {
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
        log::info!(
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
            PageUpdateError::PageTableNotReady => efi::Status::NOT_READY,
            PageUpdateError::AlreadyMapped => efi::Status::SECURITY_VIOLATION,
            PageUpdateError::QueryFailed | PageUpdateError::MapFailed => efi::Status::DEVICE_ERROR,
        };
    }

    log::info!(
        "UNBLOCK_MEM: SUCCESS - unblocked 0x{:016x}-0x{:016x} ({} pages, {})",
        request.physical_start,
        request.physical_start + request.region_size,
        request.number_of_pages,
        if request.is_supervisor_page { "supervisor-only" } else { "user-accessible" },
    );

    efi::Status::SUCCESS
}

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

fn validate_unblock_request(params: &MmSupervisorUnblockMemoryParams) -> Result<ValidatedUnblockRequest, efi::Status> {
    let physical_start = params.memory_descriptor.physical_start;
    let number_of_pages = params.memory_descriptor.number_of_pages;

    if *params.identifier_guid.as_bytes() == [0u8; 16] {
        log::error!("UNBLOCK_MEM: identifier GUID is zero");
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
    let mut track_attributes = mm_policy::RESOURCE_ATTR_READ | mm_policy::RESOURCE_ATTR_WRITE;
    if is_supervisor_page {
        track_attributes |= SUPERVISOR_TRACKING_ATTRIBUTE;
    }

    Ok(ValidatedUnblockRequest { physical_start, number_of_pages, region_size, is_supervisor_page, track_attributes })
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::cell::Cell;

    use super::*;

    const REQUEST_SIZE: usize = MmSupervisorRequestHeader::SIZE + MmSupervisorUnblockMemoryParams::SIZE;
    const MEMORY_DESCRIPTOR_PHYSICAL_START_OFFSET: usize = 8;
    const MEMORY_DESCRIPTOR_NUMBER_OF_PAGES_OFFSET: usize = 24;
    const MEMORY_DESCRIPTOR_ATTRIBUTE_OFFSET: usize = 32;
    const IDENTIFIER_GUID_OFFSET: usize = 40;

    struct SilentLogger;

    impl log::Log for SilentLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, _record: &log::Record<'_>) {}

        fn flush(&self) {}
    }

    static SILENT_LOGGER: SilentLogger = SilentLogger;

    fn enable_test_logging() {
        let _ = log::set_logger(&SILENT_LOGGER);
        log::set_max_level(log::LevelFilter::Trace);
    }

    #[derive(Clone, Copy)]
    enum TestQueryResult {
        Mapped(MemoryAttributes),
        NoMapping,
        Failed,
    }

    struct TestPageTable {
        query_result: TestQueryResult,
        map_fails: bool,
        mapped: Option<(u64, u64, MemoryAttributes)>,
    }

    impl TestPageTable {
        fn new(query_result: TestQueryResult) -> Self {
            Self { query_result, map_fails: false, mapped: None }
        }
    }

    impl UnblockPageTable for TestPageTable {
        fn query_region(&self, _base: u64, _size: u64) -> Result<MemoryAttributes, PtError> {
            match self.query_result {
                TestQueryResult::Mapped(attributes) => Ok(attributes),
                TestQueryResult::NoMapping => Err(PtError::NoMapping),
                TestQueryResult::Failed => Err(PtError::InvalidParameter),
            }
        }

        fn map_region(&mut self, base: u64, size: u64, attributes: MemoryAttributes) -> Result<(), PtError> {
            if self.map_fails {
                Err(PtError::OutOfResources)
            } else {
                self.mapped = Some((base, size, attributes));
                Ok(())
            }
        }
    }

    struct TestUnblockMemoryContext {
        locked: bool,
        inside_mmram: bool,
        track_result: Result<TrackOutcome, UnblockError>,
        page_update_result: Result<(), PageUpdateError>,
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
                inside_mmram: false,
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

        fn is_inside_mmram(&self, _base: u64, _size: u64) -> bool {
            self.mmram_checks.set(self.mmram_checks.get() + 1);
            self.inside_mmram
        }

        fn track_unblocked_memory(
            &self,
            _base: u64,
            _size: u64,
            _attributes: u32,
        ) -> Result<TrackOutcome, UnblockError> {
            self.track_calls.set(self.track_calls.get() + 1);
            self.track_result
        }

        fn remove_unblocked_memory(&self, _base: u64, _size: u64, _attributes: u32) -> bool {
            self.rollback_calls.set(self.rollback_calls.get() + 1);
            self.rollback_result
        }

        fn update_page_table(&self, request: &ValidatedUnblockRequest) -> Result<(), PageUpdateError> {
            self.page_update_calls.set(self.page_update_calls.get() + 1);
            self.last_request.set(Some(*request));
            self.page_update_result
        }
    }

    fn create_test_tracker() -> UnblockedMemoryTracker {
        enable_test_logging();
        UnblockedMemoryTracker::new()
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
        enable_test_logging();
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
        enable_test_logging();
        ValidatedUnblockRequest {
            physical_start: 0x2000,
            number_of_pages: 2,
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
        context.inside_mmram = true;
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
        context.track_result = Err(UnblockError::ConflictingAttributes);
        let buffer = request_buffer(0x1000, 1, 0, true);

        assert_eq!(process_unblock_mem(&buffer, &context), efi::Status::SECURITY_VIOLATION);
        assert_eq!(context.page_update_calls.get(), 0);
    }

    #[test]
    fn process_maps_other_tracker_failures_to_invalid_parameter() {
        let mut context = TestUnblockMemoryContext::new();
        context.track_result = Err(UnblockError::TooManyRegions);
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
            context.page_update_result = Err(page_error);

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
        assert_eq!(request.number_of_pages, 3);
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
        let tracker = UnblockedMemoryTracker::new();
        let context = SupervisorUnblockMemoryContext { unblocked_tracker: &tracker };

        assert!(!context.is_locked());
        assert!(!context.is_inside_mmram(0x1000, 0x1000));
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

        assert_eq!(context.update_page_table(&validated_request(false)), Err(PageUpdateError::PageTableNotReady));
    }

    #[test]
    fn page_table_update_rejects_an_existing_mapping() {
        let mut page_table = TestPageTable::new(TestQueryResult::Mapped(MemoryAttributes::ReadOnly));

        assert_eq!(
            update_unblocked_page_table(&mut page_table, &validated_request(false)),
            Err(PageUpdateError::AlreadyMapped)
        );
        assert!(page_table.mapped.is_none());
    }

    #[test]
    fn page_table_update_maps_user_data_as_writable_and_non_executable() {
        let mut page_table = TestPageTable::new(TestQueryResult::NoMapping);
        let request = validated_request(false);

        assert_eq!(update_unblocked_page_table(&mut page_table, &request), Ok(()));
        assert_eq!(
            page_table.mapped,
            Some((request.physical_start, request.region_size, MemoryAttributes::ExecuteProtect))
        );
    }

    #[test]
    fn page_table_update_preserves_supervisor_only_access() {
        let mut page_table = TestPageTable::new(TestQueryResult::NoMapping);
        let request = validated_request(true);

        assert_eq!(update_unblocked_page_table(&mut page_table, &request), Ok(()));
        assert_eq!(
            page_table.mapped,
            Some((
                request.physical_start,
                request.region_size,
                MemoryAttributes::ExecuteProtect | MemoryAttributes::Supervisor
            ))
        );
    }

    #[test]
    fn page_table_update_reports_query_failure() {
        let mut page_table = TestPageTable::new(TestQueryResult::Failed);

        assert_eq!(
            update_unblocked_page_table(&mut page_table, &validated_request(false)),
            Err(PageUpdateError::QueryFailed)
        );
        assert!(page_table.mapped.is_none());
    }

    #[test]
    fn page_table_update_reports_mapping_failure() {
        let mut page_table = TestPageTable::new(TestQueryResult::NoMapping);
        page_table.map_fails = true;

        assert_eq!(
            update_unblocked_page_table(&mut page_table, &validated_request(false)),
            Err(PageUpdateError::MapFailed)
        );
        assert!(page_table.mapped.is_none());
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
            Err(UnblockError::ConflictingAttributes)
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
            Err(UnblockError::ConflictingAttributes)
        );
    }

    #[test]
    fn test_invalid_parameters() {
        let tracker = create_test_tracker();
        tracker.set_core_init_complete();

        // Zero size
        assert_eq!(tracker.unblock_memory(0x1000, 0, RESOURCE_ATTR_READ), Err(UnblockError::InvalidParameter));

        // Overflow
        assert_eq!(tracker.unblock_memory(u64::MAX, 0x1000, RESOURCE_ATTR_READ), Err(UnblockError::AddressOverflow));
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
            Err(UnblockError::TooManyRegions)
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
            MemDescriptorV1_0::default(),
            MemDescriptorV1_0 { base_address: 0x1000, size: 0x1000, mem_attributes: RESOURCE_ATTR_READ, reserved: 0 },
            MemDescriptorV1_0 { base_address: 0x3000, size: 0x2000, mem_attributes: RESOURCE_ATTR_WRITE, reserved: 0 },
        ];

        assert_eq!(tracker.init_from_descriptors(&descriptors), Ok(()));
        assert_eq!(tracker.region_count(), 2);
        assert_eq!(tracker.init_from_descriptors(&[]), Err(UnblockError::AlreadyInitialized));
        tracker.dump_regions();
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

        assert_eq!(tracker.init_from_descriptors(&descriptors), Err(UnblockError::TooManyRegions));
        assert_eq!(tracker.region_count(), MAX_UNBLOCKED_REGIONS);
    }

    #[test]
    fn test_tracker_initializes_from_empty_raw_buffer_once() {
        let tracker = create_test_tracker();

        // SAFETY: A null buffer with zero entries is explicitly supported as empty initialization.
        assert_eq!(unsafe { tracker.init_from_buffer(core::ptr::null(), 0) }, Ok(()));
        // SAFETY: A null buffer with zero entries remains a valid request and reaches the
        // already-initialized check.
        assert_eq!(unsafe { tracker.init_from_buffer(core::ptr::null(), 0) }, Err(UnblockError::AlreadyInitialized));
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
