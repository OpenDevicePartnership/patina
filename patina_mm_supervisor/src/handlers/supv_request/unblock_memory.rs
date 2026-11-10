//! Unblocked memory region management.
//!
//! Tracks the memory regions unblocked for MM access and serves the `UNBLOCK_MEM` request.
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

use patina::{BinaryGuid, MemoryRange, MemoryRangeError, UEFI_PAGE_SIZE};
use patina_paging::{MemoryAttributes, PageTable, PtError};

use patina::management_mode::protocol::mm_supervisor_request::{
    MmSupervisorRequestHeader, MmSupervisorUnblockMemoryParams,
};

use crate::{
    error::{MmSupervisorError, MmSupervisorResult},
    memory::mmram::range_overlaps_mmram,
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
    /// The requested region overlaps an existing unblocked region without matching it exactly.
    ConflictingAttributes,
    /// Invalid parameter, such as a zero length or too many pages for one entry.
    InvalidParameter,
    /// The region's address plus size would overflow.
    AddressOverflow,
}

impl core::error::Error for UnblockError {}

impl From<MemoryRangeError> for UnblockError {
    /// Maps an empty range to `InvalidParameter` and anything else to `AddressOverflow`.
    fn from(err: MemoryRangeError) -> Self {
        match err {
            MemoryRangeError::Empty => Self::InvalidParameter,
            _ => Self::AddressOverflow,
        }
    }
}

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
    /// Creates an empty entry.
    pub const fn empty() -> Self {
        Self { base_address: 0, num_pages: 0, attributes: 0 }
    }

    /// Creates an entry covering `range`, rounded up to whole pages, with `attributes`.
    ///
    /// Returns `None` if the page count does not fit the `u32` the entry records.
    pub const fn new(range: MemoryRange, attributes: u32) -> Option<Self> {
        let num_pages = patina::uefi_size_to_pages!(range.size() as usize);
        if num_pages > u32::MAX as usize {
            return None;
        }
        Some(Self { base_address: range.base(), num_pages: num_pages as u32, attributes })
    }

    /// Returns the size of this region in bytes.
    pub fn size(&self) -> u64 {
        patina::uefi_pages_to_size!(self.num_pages as usize) as u64
    }

    /// Returns the region this entry covers, or `None` for an empty entry or one whose page-rounded
    /// end runs past the end of the address space.
    pub fn range(&self) -> Option<MemoryRange> {
        MemoryRange::new(self.base_address, self.size()).ok()
    }

    /// Returns whether this entry fully contains `range`.
    pub fn contains(&self, range: &MemoryRange) -> bool {
        self.range().is_some_and(|entry| entry.contains_range(range))
    }

    /// Returns whether `range` overlaps this entry.
    pub fn overlaps(&self, range: &MemoryRange) -> bool {
        self.range().is_some_and(|entry| entry.overlaps(range))
    }
}

/// Internal state for the unblocked memory tracker.
struct UnblockedMemoryState {
    /// Array of unblocked memory entries.
    entries: [UnblockedMemoryEntry; MAX_UNBLOCKED_REGIONS],
    /// Number of valid entries in the array.
    count: usize,
}

/// Whether tracking a region added a new entry or found an identical one. Only a new entry
/// needs its page table mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackOutcome {
    /// The region was not recorded before, so a new entry was taken.
    Added,
    /// The region was already recorded with the same base, size and attributes.
    Existing,
}

impl UnblockedMemoryState {
    /// Creates an empty state.
    const fn new() -> Self {
        Self { entries: [UnblockedMemoryEntry::empty(); MAX_UNBLOCKED_REGIONS], count: 0 }
    }

    /// Finds an entry that covers exactly `range`.
    fn find_exact_match(&self, range: &MemoryRange) -> Option<&UnblockedMemoryEntry> {
        self.entries.iter().take(self.count).find(|entry| entry.range() == Some(*range))
    }

    /// Adds `range`, rounded up to whole pages, if there is space. A rounded range that runs past
    /// the end of the address space is refused, so every entry has a valid
    /// [`range`](UnblockedMemoryEntry::range).
    fn add_entry(&mut self, range: MemoryRange, attributes: u32) -> MmSupervisorResult<()> {
        let entry = UnblockedMemoryEntry::new(range, attributes).ok_or_else(|| {
            log::debug!("Cannot track unblocked region {range}: it has more pages than an entry records");
            UnblockError::InvalidParameter
        })?;
        if entry.range().is_none() {
            log::debug!("Cannot track unblocked region {range}: its page-rounded end overflows");
            return Err(UnblockError::AddressOverflow.into());
        }
        let capacity = self.entries.len();
        let slot = self.entries.get_mut(self.count).ok_or_else(|| {
            log::debug!("Cannot track unblocked region {range}: all {capacity} slots are in use");
            UnblockError::TooManyRegions
        })?;
        *slot = entry;
        self.count += 1;
        Ok(())
    }
}

/// Tracks the regions unblocked for MM access.
pub struct UnblockedMemoryTracker {
    /// Whether the tracker has been initialized.
    initialized: AtomicBool,
    /// Whether core initialization is complete. Access checks are enforced and unblock requests
    /// refused once set.
    core_init_complete: AtomicBool,
    /// Internal state.
    state: Mutex<UnblockedMemoryState>,
}

impl UnblockedMemoryTracker {
    /// Creates an empty, uninitialized tracker.
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            core_init_complete: AtomicBool::new(false),
            state: Mutex::new(UnblockedMemoryState::new()),
        }
    }

    /// Initializes the tracker with the initial unblocked regions, given as memory policy
    /// descriptors. Call once during BSP initialization.
    ///
    /// ## Errors
    ///
    /// Fails if already initialized, if there are too many descriptors, or if a descriptor is
    /// empty, overflows, or overlaps MMRAM.
    pub fn init_from_descriptors(&self, descriptors: &[MemDescriptorV1_0]) -> MmSupervisorResult<()> {
        if self.initialized.swap(true, Ordering::SeqCst) {
            log::error!("UnblockedMemoryTracker init rejected: already initialized");
            return Err(UnblockError::AlreadyInitialized.into());
        }

        let mut state = self.state.lock();

        for desc in descriptors {
            let range = MemoryRange::try_from(desc).map_err(UnblockError::from).inspect_err(|err| {
                log::error!(
                    "UnblockedMemoryTracker init rejected: descriptor 0x{:016x} (+0x{:x}): {err}",
                    desc.base_address,
                    desc.size
                );
            })?;

            if range_overlaps_mmram(&range) {
                log::error!("UnblockedMemoryTracker init rejected: descriptor {range} overlaps MMRAM");
                return Err(UnblockError::OverlapsWithMmram.into());
            }

            state.add_entry(range, desc.mem_attributes)?;
        }

        log::debug!("UnblockedMemoryTracker initialized with {} regions", state.count);

        Ok(())
    }

    /// Initializes the tracker from a raw buffer of memory policy descriptors. A null `buffer` or
    /// zero `count` initializes it with no regions.
    ///
    /// ## Safety
    ///
    /// Unless `buffer` is null or `count` is 0, `buffer` must point to `count` valid
    /// `MemDescriptorV1_0` entries.
    pub unsafe fn init_from_buffer(&self, buffer: *const MemDescriptorV1_0, count: usize) -> MmSupervisorResult<()> {
        if buffer.is_null() || count == 0 {
            if self.initialized.swap(true, Ordering::SeqCst) {
                return Err(UnblockError::AlreadyInitialized.into());
            }
            log::info!("UnblockedMemoryTracker initialized with 0 regions (empty)");
            return Ok(());
        }

        // SAFETY: the caller guarantees `buffer` holds `count` valid descriptors.
        let descriptors = unsafe { core::slice::from_raw_parts(buffer, count) };
        self.init_from_descriptors(descriptors)
    }

    /// Marks core initialization as complete. From then on access checks are enforced and
    /// unblock requests are refused.
    pub fn set_core_init_complete(&self) {
        self.core_init_complete.store(true, Ordering::Release);
        log::info!("UnblockedMemoryTracker: Core initialization complete, enforcing checks");
    }

    /// Returns whether core initialization is complete.
    pub fn is_core_init_complete(&self) -> bool {
        self.core_init_complete.load(Ordering::Acquire)
    }

    /// Records a region as unblocked, reporting whether it was newly added.
    ///
    /// An identical request returns [`TrackOutcome::Existing`]. The same range with different
    /// attributes, or any other overlap, is a conflict.
    fn track_unblocked_memory(&self, range: MemoryRange, attributes: u32) -> MmSupervisorResult<TrackOutcome> {
        if range_overlaps_mmram(&range) {
            log::error!("unblock_memory: Region {range} overlaps with MMRAM");
            return Err(UnblockError::OverlapsWithMmram.into());
        }

        let mut state = self.state.lock();

        if let Some(existing) = state.find_exact_match(&range) {
            if existing.attributes == attributes {
                log::debug!("unblock_memory: Region {range} already unblocked with same attributes");
                return Ok(TrackOutcome::Existing);
            }
            log::error!(
                "unblock_memory: Region {range} already unblocked with different attributes (existing: 0x{:x}, requested: 0x{:x})",
                existing.attributes,
                attributes
            );
            return Err(UnblockError::ConflictingAttributes.into());
        }

        // Log every overlap before failing.
        let mut has_overlap = false;
        for entry in state.entries.iter().take(state.count) {
            if entry.overlaps(&range) {
                log::error!(
                    "unblock_memory: Region {range} overlaps with existing region 0x{:016x} (+0x{:x})",
                    entry.base_address,
                    entry.size()
                );
                has_overlap = true;
            }
        }

        if has_overlap {
            return Err(UnblockError::ConflictingAttributes.into());
        }

        state.add_entry(range, attributes)?;

        log::debug!("unblock_memory: Unblocked region {range} with attributes 0x{attributes:x}");

        Ok(TrackOutcome::Added)
    }

    /// Removes the entry matching `range` and `attributes` exactly, returning whether one was
    /// removed. Used to roll back a region whose mapping failed. The last entry moves into the
    /// freed slot.
    fn remove_unblocked_memory(&self, range: &MemoryRange, attributes: u32) -> bool {
        let mut state = self.state.lock();
        let Some(index) = state
            .entries
            .iter()
            .take(state.count)
            .position(|entry| entry.range() == Some(*range) && entry.attributes == attributes)
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

    /// Returns whether `range` is blocked, meaning no unblocked entry fully contains it. Always
    /// `false` before core initialization is complete.
    pub fn is_memory_blocked(&self, range: &MemoryRange) -> bool {
        if !self.core_init_complete.load(Ordering::Acquire) {
            return false;
        }

        let state = self.state.lock();

        for entry in state.entries.iter().take(state.count) {
            if entry.contains(range) {
                log::trace!(
                    "is_memory_blocked: Region {range} is within unblocked region 0x{:016x} (+0x{:x})",
                    entry.base_address,
                    entry.size()
                );
                return false;
            }
        }

        log::trace!("is_memory_blocked: Region {range} is NOT within any unblocked region");

        true
    }

    /// Returns the inverse of [`is_memory_blocked`](Self::is_memory_blocked).
    pub fn is_within_unblocked_region(&self, range: &MemoryRange) -> bool {
        !self.is_memory_blocked(range)
    }

    /// Gets the current count of unblocked regions.
    #[cfg(test)]
    pub fn region_count(&self) -> usize {
        self.state.lock().count
    }

    /// Logs the unblocked regions for debugging.
    pub fn dump_regions(&self) {
        let state = self.state.lock();

        log::info!("UnblockedMemoryTracker: {} regions", state.count);
        for (index, entry) in state.entries.iter().take(state.count).enumerate() {
            let readable = if (entry.attributes & RESOURCE_ATTR_READ) != 0 { "R" } else { "." };
            let writable = if (entry.attributes & RESOURCE_ATTR_WRITE) != 0 { "W" } else { "." };
            let executable = if (entry.attributes & RESOURCE_ATTR_EXECUTE) != 0 { "X" } else { "." };
            log::info!(
                "  [{index}] 0x{:016x} (+0x{:x}) {readable}{writable}{executable}",
                entry.base_address,
                entry.size()
            );
        }
    }
}

/// An unblock request whose parameters have been checked by [`validate_unblock_request`].
#[derive(Clone, Copy, Debug)]
struct ValidatedUnblockRequest {
    /// The region, with a non-zero, page-aligned base.
    range: MemoryRange,
    /// Whether the region is to be mapped supervisor-only rather than user accessible.
    is_supervisor_page: bool,
    /// Attributes recorded with the tracker entry.
    track_attributes: u32,
}

/// Errors produced while updating the page table for an unblock request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageUpdateError {
    /// The page table is not installed yet.
    PageTableNotReady,
    /// The region is already present.
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

/// The global state an unblock request reads and changes, behind a trait so tests can drive
/// [`process_unblock_mem`].
trait UnblockMemoryContext {
    /// Returns whether unblocking is closed.
    fn is_locked(&self) -> bool;
    /// Returns whether the region overlaps MMRAM.
    fn overlaps_mmram(&self, range: &MemoryRange) -> bool;
    /// Records the region, reporting whether it was newly added or already present.
    fn track_unblocked_memory(&self, range: MemoryRange, attributes: u32) -> MmSupervisorResult<TrackOutcome>;
    /// Removes a recorded region after its mapping fails.
    fn remove_unblocked_memory(&self, range: &MemoryRange, attributes: u32) -> bool;
    /// Maps the region with the attributes the request asked for.
    fn update_page_table(&self, request: &ValidatedUnblockRequest) -> MmSupervisorResult<()>;
}

/// The [`UnblockMemoryContext`] backed by the supervisor's global state.
struct SupervisorUnblockMemoryContext<'a> {
    /// Tracker holding the unblocked regions and the lock flag.
    unblocked_tracker: &'a UnblockedMemoryTracker,
}

impl UnblockMemoryContext for SupervisorUnblockMemoryContext<'_> {
    fn is_locked(&self) -> bool {
        self.unblocked_tracker.is_core_init_complete()
    }

    fn overlaps_mmram(&self, range: &MemoryRange) -> bool {
        range_overlaps_mmram(range)
    }

    fn track_unblocked_memory(&self, range: MemoryRange, attributes: u32) -> MmSupervisorResult<TrackOutcome> {
        self.unblocked_tracker.track_unblocked_memory(range, attributes)
    }

    fn remove_unblocked_memory(&self, range: &MemoryRange, attributes: u32) -> bool {
        self.unblocked_tracker.remove_unblocked_memory(range, attributes)
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

/// Maps the region present, writable and non-executable, and supervisor-only if requested.
///
/// A region that is already present is refused rather than changed in place, so a request
/// cannot widen access to memory that is already mapped.
fn update_unblocked_page_table<P: PageTable>(
    page_table: &mut P,
    request: &ValidatedUnblockRequest,
) -> MmSupervisorResult<()> {
    let range = request.range;
    match page_table.query_memory_region(range.base(), range.size()) {
        Ok(current_attrs) => {
            log::error!(
                "UNBLOCK_MEM: pages at {range} are already present (attrs: {current_attrs:?}). \
                 Only not-present pages may be unblocked."
            );
            return Err(PageUpdateError::AlreadyMapped.into());
        }
        Err(PtError::NoMapping) => {}
        Err(err) => {
            log::error!("UNBLOCK_MEM: failed to query page attributes for {range}: {err:?}");
            return Err(PageUpdateError::QueryFailed.into());
        }
    }

    let mut new_attrs = MemoryAttributes::ExecuteProtect;
    if request.is_supervisor_page {
        new_attrs |= MemoryAttributes::Supervisor;
    }

    if let Err(err) = page_table.map_memory_region(range.base(), range.size(), new_attrs) {
        log::error!("UNBLOCK_MEM: failed to update page table for {range}: {err:?}");
        // The range was not present before, so unmapping removes whatever the failed call mapped.
        if let Err(unmap_err) = page_table.unmap_memory_region(range.base(), range.size()) {
            log::error!("UNBLOCK_MEM: failed to unmap {range} after the failed mapping: {unmap_err:?}");
        }
        return Err(PageUpdateError::MapFailed.into());
    }

    Ok(())
}

/// Handles an `UNBLOCK_MEM` request, which maps a memory region outside MMRAM for MM drivers.
///
/// Checks, in order:
///
/// 1. Core initialization is not yet complete.
/// 2. The buffer holds the header and [`MmSupervisorUnblockMemoryParams`].
/// 3. The request passes [`validate_unblock_request`].
/// 4. The region does not overlap MMRAM.
/// 5. The [`UnblockedMemoryTracker`] accepts it. An identical earlier request succeeds without
///    remapping.
/// 6. The region is not present. It is then mapped present, writable and non-executable, and
///    supervisor-only if the request sets `EFI_MEMORY_SP`.
///
/// ## Safety
///
/// `comm_buffer` must be null or point to `*comm_buffer_size` bytes that are readable and
/// writable for the duration of the call.
pub(crate) unsafe fn handle_unblock_mem(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    if comm_buffer.is_null() {
        log::error!("UNBLOCK_MEM: communication buffer is null");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: `comm_buffer` is non-null, and the caller guarantees `*comm_buffer_size` readable
    // bytes.
    let buffer = unsafe { core::slice::from_raw_parts(comm_buffer, *comm_buffer_size) };
    let context = SupervisorUnblockMemoryContext { unblocked_tracker: security_state().unblocked_tracker() };
    let status = process_unblock_mem(buffer, &context);
    *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
    status
}

/// Serves an `UNBLOCK_MEM` request against `context`.
fn process_unblock_mem<C: UnblockMemoryContext>(comm_buffer: &[u8], context: &C) -> efi::Status {
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

    let range = request.range;
    if context.overlaps_mmram(&range) {
        log::error!("UNBLOCK_MEM: region {range} overlaps with MMRAM");
        return efi::Status::SECURITY_VIOLATION;
    }

    let track_outcome = match context.track_unblocked_memory(range, request.track_attributes) {
        Ok(outcome) => outcome,
        Err(MmSupervisorError::Unblock(UnblockError::ConflictingAttributes)) => {
            log::error!("UNBLOCK_MEM: region {range} conflicts with existing entry");
            return efi::Status::SECURITY_VIOLATION;
        }
        Err(err) => {
            log::error!("UNBLOCK_MEM: tracker rejected request for {range}: {err:?}");
            return efi::Status::INVALID_PARAMETER;
        }
    };

    if track_outcome == TrackOutcome::Existing {
        log::debug!("UNBLOCK_MEM: region {range} already unblocked (idempotent)");
        return efi::Status::SUCCESS;
    }

    if let Err(error) = context.update_page_table(&request) {
        if !context.remove_unblocked_memory(&range, request.track_attributes) {
            log::error!("UNBLOCK_MEM: failed to roll back tracker entry for {range}");
        }

        return match error {
            MmSupervisorError::PageUpdate(PageUpdateError::PageTableNotReady) => efi::Status::NOT_READY,
            MmSupervisorError::PageUpdate(PageUpdateError::AlreadyMapped) => efi::Status::SECURITY_VIOLATION,
            _ => efi::Status::DEVICE_ERROR,
        };
    }

    log::debug!(
        "UNBLOCK_MEM: SUCCESS - unblocked {range} ({} pages, {})",
        patina::uefi_size_to_pages!(range.size() as usize),
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

    // SAFETY: `payload` holds exactly one parameter structure. Its fields are integers and GUIDs,
    // for which every bit pattern is valid, and `read_unaligned` has no alignment requirement.
    Ok(unsafe { payload.as_ptr().cast::<MmSupervisorUnblockMemoryParams>().read_unaligned() })
}

/// Checks the request parameters and derives the values the later steps use.
///
/// Checks:
/// - The identifier GUID is not zero.
/// - `PhysicalStart` is not null and is page aligned.
/// - The page count fits the `u32` the tracker records.
/// - The page count is not zero and the range does not wrap the address space.
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

    // The tracker records page counts as `u32`.
    if number_of_pages > u64::from(u32::MAX) {
        log::error!("UNBLOCK_MEM: {number_of_pages} pages exceeds the {} pages one entry can track", u32::MAX);
        return Err(efi::Status::INVALID_PARAMETER);
    }

    let range = match MemoryRange::from_pages(physical_start, number_of_pages) {
        Ok(range) => range,
        Err(err) => {
            log::error!("UNBLOCK_MEM: {number_of_pages} pages at 0x{physical_start:016x} is not a valid range: {err}");
            return Err(efi::Status::INVALID_PARAMETER);
        }
    };

    let is_supervisor_page = (params.memory_descriptor.attribute & efi::MEMORY_SP) != 0;
    let mut track_attributes = policy::RESOURCE_ATTR_READ | policy::RESOURCE_ATTR_WRITE;
    if is_supervisor_page {
        track_attributes |= SUPERVISOR_TRACKING_ATTRIBUTE;
    }

    Ok(ValidatedUnblockRequest { range, is_supervisor_page, track_attributes })
}
