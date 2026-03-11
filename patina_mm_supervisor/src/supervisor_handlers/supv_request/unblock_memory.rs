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

use r_efi::efi;

use patina::base::UEFI_PAGE_SIZE;
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

/// Errors that can occur during unblock memory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblockError {
    /// Already initialized (cannot re-initialize).
    AlreadyInitialized,
    /// Too many regions to track (exceeded MAX_UNBLOCKED_REGIONS).
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
        self.num_pages as u64 * UEFI_PAGE_SIZE as u64
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
        let query_end = base.saturating_add(size);
        base >= self.base_address && query_end <= self.end_address()
    }

    /// Checks if the given range [base, base + size) overlaps with this entry.
    pub fn overlaps(&self, base: u64, size: u64) -> bool {
        if self.num_pages == 0 || size == 0 {
            return false;
        }
        let query_end = base.saturating_add(size);
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
    pub fn unblock_memory(&self, base: u64, size: u64, attributes: u32) -> Result<(), UnblockError> {
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
                return Ok(());
            } else {
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

        Ok(())
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
            log::warn!("is_memory_blocked: Zero-size query for address 0x{:016x}", base);
            return true; // Invalid query = blocked
        }

        // Check for address overflow
        if base.checked_add(size).is_none() {
            log::warn!("is_memory_blocked: Address overflow for 0x{:016x} + 0x{:x}", base, size);
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

/// Handle an UNBLOCK_MEM request.
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
/// 8. **Duplicate / conflict** - checked by the [`UNBLOCKED_MEMORY_TRACKER`].
/// 9. **Page attributes** - pages must be not-present (RP set) and not read-only.
/// 10. **Page table update** - make pages present, R/W, NX; optionally supervisor-only (SP).
pub(crate) fn handle_unblock_mem(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("UNBLOCK_MEM request");

    // 1. Ready-to-lock check
    // After core initialization is complete, unblock requests are rejected.
    // This mirrors the C `mMmReadyToLockDone` guard.
    if crate::state::security_state().unblocked_tracker().is_core_init_complete() {
        log::error!("UNBLOCK_MEM: rejected - core initialization already complete (post ready-to-lock)");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::ACCESS_DENIED;
    }

    // 2. Buffer size check
    let min_size = MmSupervisorRequestHeader::SIZE + MmSupervisorUnblockMemoryParams::SIZE;
    if *comm_buffer_size < min_size {
        log::error!("UNBLOCK_MEM: buffer too small ({} bytes, need at least {})", *comm_buffer_size, min_size,);
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::BUFFER_TOO_SMALL;
    }

    // 3. Parse the payload
    // SAFETY: We verified the buffer is large enough for header + params.
    let params =
        unsafe { &*(comm_buffer.add(MmSupervisorRequestHeader::SIZE) as *const MmSupervisorUnblockMemoryParams) };

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

    // 4. Zero-GUID check
    if *identifier_guid.as_bytes() == [0u8; 16] {
        log::error!("UNBLOCK_MEM: identifier GUID is zero");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // 5. Page alignment check (stricter than C)
    if !physical_start.is_multiple_of(UEFI_PAGE_SIZE as u64) {
        log::error!("UNBLOCK_MEM: PhysicalStart 0x{:016x} is not page-aligned", physical_start,);
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // 6. Non-zero page count
    if number_of_pages == 0 {
        log::error!("UNBLOCK_MEM: NumberOfPages is 0");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // 7. Overflow checks
    let region_size = match number_of_pages.checked_mul(UEFI_PAGE_SIZE as u64) {
        Some(s) => s,
        None => {
            log::error!("UNBLOCK_MEM: NumberOfPages ({}) * UEFI_PAGE_SIZE overflows u64", number_of_pages,);
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::INVALID_PARAMETER;
        }
    };

    if physical_start.checked_add(region_size).is_none() {
        log::error!("UNBLOCK_MEM: address range 0x{:016x} + 0x{:x} overflows", physical_start, region_size,);
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // 8. MMRAM overlap check
    if security_state().page_allocator().is_region_inside_mmram(physical_start, region_size) {
        log::error!(
            "UNBLOCK_MEM: region 0x{:016x}-0x{:016x} overlaps with MMRAM",
            physical_start,
            physical_start + region_size,
        );
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::SECURITY_VIOLATION;
    }

    // 9. Duplicate / conflict check via tracker
    // We use the tracker's region count to distinguish newly-added vs idempotent.
    // For newly-added regions we must additionally verify page attributes and
    // apply page table changes. For idempotent (exact duplicate) requests we
    // can short-circuit with SUCCESS.
    let is_supervisor_page = (attribute & efi::MEMORY_SP) != 0;
    let track_attributes: u32 = if is_supervisor_page {
        mm_policy::RESOURCE_ATTR_READ | mm_policy::RESOURCE_ATTR_WRITE | 0x80000000 // high bit tag for supervisor-only tracking
    } else {
        mm_policy::RESOURCE_ATTR_READ | mm_policy::RESOURCE_ATTR_WRITE
    };

    let count_before = security_state().unblocked_tracker().region_count();
    match security_state().unblocked_tracker().unblock_memory(physical_start, region_size, track_attributes) {
        Ok(()) => {
            let count_after = security_state().unblocked_tracker().region_count();
            if count_after == count_before {
                // Idempotent - already tracked with same attributes, nothing more to do.
                log::info!(
                    "UNBLOCK_MEM: region 0x{:016x}-0x{:016x} already unblocked (idempotent)",
                    physical_start,
                    physical_start + region_size,
                );
                *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
                return efi::Status::SUCCESS;
            }
            // Newly added - continue to verify page attributes and update page table.
        }
        Err(UnblockError::ConflictingAttributes) => {
            log::error!(
                "UNBLOCK_MEM: region 0x{:016x}-0x{:016x} conflicts with existing entry",
                physical_start,
                physical_start + region_size,
            );
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::SECURITY_VIOLATION;
        }
        Err(e) => {
            log::error!(
                "UNBLOCK_MEM: tracker rejected request for 0x{:016x}-0x{:016x}: {:?}",
                physical_start,
                physical_start + region_size,
                e,
            );
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::INVALID_PARAMETER;
        }
    }

    // 10. Verify current page attributes and apply page table changes.
    //
    // Acquire the page table lock once and hold it across both the read
    // (verification) and the write (update). The page table mutex is not
    // reentrant, so acquiring it a second time while the first guard is still
    // alive would deadlock.
    let mut pt_guard = security_state().lock_page_table();
    let Some(pt) = pt_guard.as_mut() else {
        log::error!("UNBLOCK_MEM: page table not initialized");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::NOT_READY;
    };

    // 10. Verify current page attributes
    // Pages must be not-present (ReadProtect) and NOT read-only. This ensures
    // we only unblock pages that were explicitly guarded, matching the C
    // `VerifyUnblockRequest` logic with an additional RO check.
    match pt.query_memory_region(physical_start, region_size) {
        Ok(current_attrs) => {
            log::error!(
                "UNBLOCK_MEM: pages at 0x{:016x} are already present (attrs: {:?}). \
                    Only not-present pages may be unblocked.",
                physical_start,
                current_attrs,
            );
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::SECURITY_VIOLATION;
        }
        Err(PtError::NoMapping) => {
            // Expected case - pages are currently not present, so we can unblock them.
        }
        Err(e) => {
            log::error!(
                "UNBLOCK_MEM: failed to query page attributes for 0x{:016x}-0x{:016x}: {:?}",
                physical_start,
                physical_start + region_size,
                e,
            );
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::DEVICE_ERROR;
        }
    }

    // 11. Apply page table changes
    // Make the region:
    //   - Present (clear ReadProtect)
    //   - Read/Write (clear ReadOnly)
    //   - Non-executable (set ExecuteProtect) - data pages must be W^X
    //   - Optionally Supervisor-only (set Supervisor) if EFI_MEMORY_SP requested
    let mut new_attrs = MemoryAttributes::ExecuteProtect; // NX - data pages are non-executable
    if is_supervisor_page {
        new_attrs |= MemoryAttributes::Supervisor; // Supervisor-only (U/S=0)
    }

    if let Err(e) = pt.map_memory_region(physical_start, region_size, new_attrs) {
        log::error!(
            "UNBLOCK_MEM: failed to update page table for 0x{:016x}-0x{:016x}: {:?}",
            physical_start,
            physical_start + region_size,
            e,
        );
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::DEVICE_ERROR;
    }

    log::info!(
        "UNBLOCK_MEM: SUCCESS - unblocked 0x{:016x}-0x{:016x} ({} pages, {})",
        physical_start,
        physical_start + region_size,
        number_of_pages,
        if is_supervisor_page { "supervisor-only" } else { "user-accessible" },
    );

    *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
    efi::Status::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tracker() -> UnblockedMemoryTracker {
        UnblockedMemoryTracker::new()
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
    }

    #[test]
    fn test_tracker_before_init_complete() {
        let tracker = create_test_tracker();

        // Before core init complete, nothing is blocked
        assert!(!tracker.is_memory_blocked(0x1000, 0x1000));
        assert!(!tracker.is_memory_blocked(0x0, 0x100000));
    }

    #[test]
    fn test_tracker_after_init_complete_empty() {
        let tracker = create_test_tracker();
        tracker.set_core_init_complete();

        // After init complete with no regions, everything is blocked
        assert!(tracker.is_memory_blocked(0x1000, 0x1000));
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
}
