//! Memory Bin Manager
//!
//! Tracks memory bin regions for hibernate (S4) resume stability. A "memory bin" is
//! a pre-allocated address range for a specific EFI memory type whose size is defined
//! by the platform's Memory Type Information HOB.
//!
//! This module is responsible for:
//!
//! 1. HOB processing: Extracting bin configuration from the Memory Type Information
//!    GUID HOB and optionally consuming a pre-allocated bin region from a Resource
//!    Descriptor HOB produced by PEI.
//!
//! 2. GetMemoryMap "overlay": Post-processing the EFI memory map so that free
//!    (`EfiConventionalMemory`) pages within a bin region are reported as the bin's
//!    memory type.
//!
//! 3. Statistics and config table: Tracking per-type allocation counts and bin
//!    usage so BDS can recommend bin sizes for the next boot.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::allocator::{DEFAULT_PAGE_ALLOCATION_GRANULARITY, RUNTIME_PAGE_ALLOCATION_GRANULARITY};
use patina::{
    base::{UEFI_PAGE_SHIFT, align_up},
    efi_types::{EFI_MAX_MEMORY_TYPE, INVALID_INFORMATION_INDEX},
    pi::hob::{self, EFiMemoryTypeInformation, Hob, HobList, MEMORY_TYPE_INFO_HOB_GUID},
    uefi_pages_to_size, uefi_size_to_pages,
};
use r_efi::efi;

use alloc::vec::Vec;

/// Maximum number of entries in the memory type information array.
const MAX_MEMORY_TYPE_INFO_ENTRIES: usize = EFI_MAX_MEMORY_TYPE + 1;

/// Maximum allocation address.
const MAX_ALLOC_ADDRESS: efi::PhysicalAddress = u64::MAX >> 1;

/// Log target for all memory bin log messages.
const LOG_TARGET: &str = "memory_bin";

/// Returns a human-readable name for a UEFI memory type.
///
/// Returns a `&'static str` for all standard types. Returns `"Unknown"` for unrecognized values.
#[coverage(off)]
pub(crate) fn memory_type_name(memory_type: efi::MemoryType) -> &'static str {
    match memory_type {
        efi::RESERVED_MEMORY_TYPE => "ReservedMemoryType",
        efi::LOADER_CODE => "LoaderCode",
        efi::LOADER_DATA => "LoaderData",
        efi::BOOT_SERVICES_CODE => "BootServicesCode",
        efi::BOOT_SERVICES_DATA => "BootServicesData",
        efi::RUNTIME_SERVICES_CODE => "RuntimeServicesCode",
        efi::RUNTIME_SERVICES_DATA => "RuntimeServicesData",
        efi::CONVENTIONAL_MEMORY => "ConventionalMemory",
        efi::UNUSABLE_MEMORY => "UnusableMemory",
        efi::ACPI_RECLAIM_MEMORY => "ACPIReclaimMemory",
        efi::ACPI_MEMORY_NVS => "ACPIMemoryNVS",
        efi::MEMORY_MAPPED_IO => "MemoryMappedIO",
        efi::MEMORY_MAPPED_IO_PORT_SPACE => "MemoryMappedIOPortSpace",
        efi::PAL_CODE => "PalCode",
        efi::PERSISTENT_MEMORY => "PersistentMemory",
        efi::UNACCEPTED_MEMORY_TYPE => "UnacceptedMemoryType",
        _ => "Unknown",
    }
}

/// Rounds a UEFI page count up to the nearest multiple of pages that correspond to the given
/// byte-level granularity.
///
/// On architectures with a page allocation granularity larger than `UEFI_PAGE_SIZE` (e.g.,
/// AARCH64 with 64KB runtime pages), the GCD allocates in granularity-sized chunks. This
/// function aligns a raw page count to match the actual GCD consumption.
///
/// # Parameters
/// - `pages`: The raw page count to align.
/// - `granularity`: Must be a non-zero multiple of `UEFI_PAGE_SIZE`.
///
/// # Returns
/// The page count rounded up to the nearest multiple of `granularity / UEFI_PAGE_SIZE`.
const fn align_pages_to_granularity(pages: u64, granularity: usize) -> u64 {
    let granularity_pages: u64 = (granularity >> UEFI_PAGE_SHIFT) as u64;
    if granularity_pages <= 1 {
        return pages;
    }
    pages.div_ceil(granularity_pages) * granularity_pages
}

/// Per-memory-type bin statistics.
///
/// Tracks the bin region, current allocation count, and metadata for a single memory type.
/// Mirrors `EFI_MEMORY_TYPE_STATISTICS` in edk2.
#[derive(Debug, Clone, Copy)]
struct MemoryBinStatistics {
    /// The base (lowest) address of this memory type's bin region.
    base_address: efi::PhysicalAddress,
    /// The maximum (highest) address of this memory type's bin region.
    maximum_address: efi::PhysicalAddress,
    /// The number of pages currently allocated within this bin.
    current_number_of_pages: u64,
    /// The total number of pages reserved for this bin.
    number_of_pages: u64,
    /// Index into the `MemoryTypeInformation` array for this type.
    information_index: usize,
    /// Whether this memory type persists into the OS runtime (affects `GetMemoryMap` behavior).
    special: bool,
    /// Whether this memory type should have `EFI_MEMORY_RUNTIME` attribute in the memory map.
    runtime: bool,
}

impl MemoryBinStatistics {
    /// Creates default statistics for a memory type with the given special/runtime flags.
    #[coverage(off)]
    const fn new(special: bool, runtime: bool) -> Self {
        Self {
            base_address: 0,
            maximum_address: MAX_ALLOC_ADDRESS,
            current_number_of_pages: 0,
            number_of_pages: 0,
            information_index: INVALID_INFORMATION_INDEX,
            special,
            runtime,
        }
    }
}

/// Default `MemoryBinStatistics` initialization for all memory types.
///
/// Indexed by `efi::MemoryType` value. Matches edk2's `mMemoryTypeStatistics` initialization.
const DEFAULT_STATISTICS: [MemoryBinStatistics; EFI_MAX_MEMORY_TYPE + 1] = [
    MemoryBinStatistics::new(true, false),  // EfiReservedMemoryType (0)
    MemoryBinStatistics::new(false, false), // EfiLoaderCode (1)
    MemoryBinStatistics::new(false, false), // EfiLoaderData (2)
    MemoryBinStatistics::new(false, false), // EfiBootServicesCode (3)
    MemoryBinStatistics::new(false, false), // EfiBootServicesData (4)
    MemoryBinStatistics::new(true, true),   // EfiRuntimeServicesCode (5)
    MemoryBinStatistics::new(true, true),   // EfiRuntimeServicesData (6)
    MemoryBinStatistics::new(false, false), // EfiConventionalMemory (7)
    MemoryBinStatistics::new(false, false), // EfiUnusableMemory (8)
    MemoryBinStatistics::new(true, false),  // EfiACPIReclaimMemory (9)
    MemoryBinStatistics::new(true, false),  // EfiACPIMemoryNVS (10)
    MemoryBinStatistics::new(false, false), // EfiMemoryMappedIO (11)
    MemoryBinStatistics::new(false, false), // EfiMemoryMappedIOPortSpace (12)
    MemoryBinStatistics::new(true, true),   // EfiPalCode (13)
    MemoryBinStatistics::new(false, false), // EfiPersistentMemory (14)
    MemoryBinStatistics::new(true, false),  // EfiUnacceptedMemoryType (15)
    MemoryBinStatistics::new(false, false), // EfiMaxMemoryType sentinel (16)
];

/// Manages memory bins for hibernate resume stability.
///
/// The `MemoryBinManager` tracks per-memory-type bin regions and allocation statistics.
pub(crate) struct MemoryBinManager {
    /// Per-memory-type bin statistics, indexed by `efi::MemoryType`.
    statistics: [MemoryBinStatistics; EFI_MAX_MEMORY_TYPE + 1],
    /// Current memory type information with peak usage tracking for the BDS config table.
    /// This is a fixed-size array so that raw pointers to it remain valid for the
    /// lifetime of the static `MEMORY_BIN_MANAGER`.
    memory_type_information: [EFiMemoryTypeInformation; MAX_MEMORY_TYPE_INFO_ENTRIES],
    /// Number of valid entries in `memory_type_information`.
    memory_type_information_count: usize,
    /// Whether bins have been initialized.
    initialized: bool,
}

impl MemoryBinManager {
    /// Creates a new uninitialized `MemoryBinManager`.
    pub(crate) const fn new() -> Self {
        Self {
            statistics: DEFAULT_STATISTICS,
            memory_type_information: [EFiMemoryTypeInformation { memory_type: 0, number_of_pages: 0 };
                MAX_MEMORY_TYPE_INFO_ENTRIES],
            memory_type_information_count: 0,
            initialized: false,
        }
    }

    /// Returns whether memory bins have been initialized.
    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns the allocation granularity for the given memory type.
    pub const fn granularity_for_type(memory_type: efi::MemoryType) -> usize {
        match memory_type {
            efi::RESERVED_MEMORY_TYPE
            | efi::ACPI_MEMORY_NVS
            | efi::RUNTIME_SERVICES_CODE
            | efi::RUNTIME_SERVICES_DATA => RUNTIME_PAGE_ALLOCATION_GRANULARITY,
            _ => DEFAULT_PAGE_ALLOCATION_GRANULARITY,
        }
    }

    /// Calculates the total memory needed for all bins, considering alignment.
    ///
    /// If `bin_top` is non-zero, alignment padding is included in the calculation.
    fn calculate_total_bin_size(memory_type_info: &[EFiMemoryTypeInformation], bin_top: efi::PhysicalAddress) -> u64 {
        let mut total_size: u64 = 0;
        let mut current_top = bin_top;

        for entry in memory_type_info {
            if entry.memory_type as usize >= EFI_MAX_MEMORY_TYPE {
                break;
            }

            let granularity = Self::granularity_for_type(entry.memory_type) as u64;
            let entry_size = uefi_pages_to_size!(entry.number_of_pages as usize) as u64;
            total_size += entry_size;

            if current_top == 0 {
                continue;
            }

            current_top -= entry_size;
            let alignment_padding = current_top & (granularity - 1);
            total_size += alignment_padding;
            current_top &= !(granularity - 1);
        }

        total_size
    }

    /// Calculates a conservative allocation size for a single contiguous bin block.
    ///
    /// Returns `None` if there are no bin entries with pages > 0.
    /// The result includes the raw entry sizes plus worst-case per-entry alignment padding, rounded
    /// up to the maximum bin granularity.
    pub(crate) fn contiguous_alloc_size(memory_type_info: &[EFiMemoryTypeInformation]) -> Option<usize> {
        let mut raw_total: usize = 0;
        let mut entry_count: usize = 0;
        let mut max_granularity = DEFAULT_PAGE_ALLOCATION_GRANULARITY;

        for entry in memory_type_info {
            if entry.memory_type as usize >= EFI_MAX_MEMORY_TYPE {
                break;
            }
            if entry.number_of_pages == 0 {
                continue;
            }
            raw_total += uefi_pages_to_size!(entry.number_of_pages as usize);
            entry_count += 1;
            max_granularity = max_granularity.max(Self::granularity_for_type(entry.memory_type));
        }

        if raw_total == 0 {
            return None;
        }

        // Each entry may need up to (granularity - 1) bytes of alignment padding within the block.
        // Using max_granularity per entry is a safe over-estimate.
        let padded = raw_total + entry_count * max_granularity;
        Some(align_up(padded, max_granularity).unwrap_or(padded))
    }

    /// Returns the maximum allocation granularity across all non-zero bin entries.
    pub(crate) fn max_granularity(memory_type_info: &[EFiMemoryTypeInformation]) -> usize {
        memory_type_info
            .iter()
            .take_while(|e| (e.memory_type as usize) < EFI_MAX_MEMORY_TYPE)
            .filter(|e| e.number_of_pages > 0)
            .map(|e| Self::granularity_for_type(e.memory_type))
            .max()
            .unwrap_or(DEFAULT_PAGE_ALLOCATION_GRANULARITY)
    }

    /// Initializes bins from a pre-allocated range (provided via Resource Descriptor HOB from PEI).
    ///
    /// Divides the range `[start, start + length)` into per-type bins based on `memory_type_info`.
    /// Bins are allocated from the top of the range downward.
    pub(crate) fn initialize_from_range(
        &mut self,
        start: efi::PhysicalAddress,
        length: u64,
        memory_type_info: &[EFiMemoryTypeInformation],
    ) -> bool {
        if self.initialized {
            log::warn!("Memory bins already initialized, ignoring range.");
            return false;
        }

        let end = match start.checked_add(length) {
            Some(end) if end <= MAX_ALLOC_ADDRESS => end,
            _ => {
                log::warn!(
                    target: LOG_TARGET,
                    "Memory bin range invalid: start={:#X} length={:#X} (overflow or exceeds MAX_ALLOC_ADDRESS)",
                    start,
                    length
                );
                return false;
            }
        };

        let total_needed = Self::calculate_total_bin_size(memory_type_info, end);
        if total_needed > length {
            log::warn!(
                target: LOG_TARGET,
                "Memory bin range too small: need {:#X} bytes but only {:#X} available.",
                total_needed,
                length
            );
            return false;
        }

        log::info!(
            target: LOG_TARGET,
            "Initializing memory bins from PEI range: base={:#X} length={:#X} total_needed={:#X}",
            start,
            length,
            total_needed
        );

        let mut top = end;

        for (index, entry) in memory_type_info.iter().enumerate() {
            let mem_type = entry.memory_type;
            if mem_type as usize >= EFI_MAX_MEMORY_TYPE {
                break;
            }
            if entry.number_of_pages == 0 {
                continue;
            }

            let entry_size = uefi_pages_to_size!(entry.number_of_pages as usize) as u64;
            let stats =
                self.statistics.get_mut(mem_type as usize).expect("Defined memory types should be in statistics");
            stats.maximum_address = top - 1;
            top -= entry_size;

            // Align to the type's granularity
            let granularity = Self::granularity_for_type(mem_type) as u64;
            top &= !(granularity - 1);

            stats.base_address = top;
            stats.number_of_pages = entry.number_of_pages as u64;
            stats.information_index = index;

            log::info!(
                target: LOG_TARGET,
                "  Bin[{}] {}: base={:#X} max={:#X} pages={:#X} ({} pages)",
                mem_type,
                memory_type_name(mem_type),
                stats.base_address,
                stats.maximum_address,
                stats.number_of_pages,
                stats.number_of_pages
            );
        }

        self.finalize_information_index(memory_type_info);
        self.copy_memory_type_info(memory_type_info);
        self.initialized = true;

        log::info!(
            target: LOG_TARGET,
            "Memory bins initialized from pre-allocated range."
        );
        true
    }

    /// Sets the `information_index` for each memory type that has a corresponding entry
    /// in the memory type information array.
    fn finalize_information_index(&mut self, memory_type_info: &[EFiMemoryTypeInformation]) {
        for mem_type in 0..EFI_MAX_MEMORY_TYPE {
            let stats = self.statistics.get_mut(mem_type).expect("All defined memory types should be in statistics");
            for (index, entry) in memory_type_info.iter().enumerate() {
                if mem_type == entry.memory_type as usize {
                    stats.information_index = index;
                }
            }
            stats.current_number_of_pages = 0;
        }
        log::trace!(target: LOG_TARGET, "Bin stats: finalized information indices, reset current_number_of_pages to 0 for all types");
    }

    /// Copies memory type information entries into the fixed-size array.
    fn copy_memory_type_info(&mut self, memory_type_info: &[EFiMemoryTypeInformation]) {
        let count = memory_type_info.len().min(self.memory_type_information.len());
        let src = memory_type_info.get(..count).expect("Failed to get source slice");
        let dest = self.memory_type_information.get_mut(..count).expect("Failed to get destination slice");

        dest.copy_from_slice(src);
        self.memory_type_information_count = count;

        if log::log_enabled!(target: LOG_TARGET, log::Level::Trace) {
            log::trace!(
                target: LOG_TARGET,
                "Bin table: initialized with {} entries from HOB",
                count
            );

            if let Some(entries) = self.memory_type_information.get(..count) {
                for entry in entries {
                    log::trace!(
                        target: LOG_TARGET,
                        "  Bin table: {} pages={}",
                        memory_type_name(entry.memory_type),
                        entry.number_of_pages
                    );
                }
            }
        }
    }

    /// Seeds bin statistics from a PEI memory allocation HOB.
    ///
    /// Called for Memory Allocation HOBs that have `name == MEMORY_TYPE_INFO_HOB_GUID`,
    /// indicating they were produced by PEI's bin-aware allocator. All memory bin-type
    /// allocations are expected to be counted regardless of address.
    pub(crate) fn seed_statistics_from_hob(&mut self, memory_type: efi::MemoryType, pages: u64) {
        if !self.initialized {
            return;
        }

        let type_idx = memory_type as usize;
        let Some(stats) = self.statistics.get_mut(type_idx).filter(|s| s.special) else {
            return;
        };

        let aligned_pages = align_pages_to_granularity(pages, Self::granularity_for_type(memory_type));
        stats.current_number_of_pages += aligned_pages;
        log::debug!(
            target: LOG_TARGET,
            "PEI seed: {} +{} pages. total={}",
            memory_type_name(memory_type),
            pages,
            stats.current_number_of_pages
        );
    }

    /// Returns the current tracked page count for the given memory type.
    ///
    /// Returns 0 for invalid memory types.
    #[cfg(test)]
    pub(crate) fn current_pages_for_type(&self, memory_type: efi::MemoryType) -> u64 {
        self.statistics.get(memory_type as usize).map_or(0, |s| s.current_number_of_pages)
    }

    /// Returns an iterator over all active bins: `(memory_type, base_address, max_address, pages)`.
    ///
    /// Only yields bins with `number_of_pages > 0`.
    pub(crate) fn active_bins(
        &self,
    ) -> impl Iterator<Item = (efi::MemoryType, efi::PhysicalAddress, efi::PhysicalAddress, u64)> + '_ {
        self.statistics.iter().enumerate().filter_map(|(idx, stats)| {
            if stats.number_of_pages > 0 && idx < EFI_MAX_MEMORY_TYPE {
                Some((idx as efi::MemoryType, stats.base_address, stats.maximum_address, stats.number_of_pages))
            } else {
                None
            }
        })
    }

    /// Records an allocation for statistics tracking on special (runtime) memory types.
    ///
    /// Only tracks types with active bins (`special == true` and `number_of_pages > 0`).
    /// Updates `current_number_of_pages` and peak tracking in `memory_type_information`.
    pub(crate) fn record_allocation(&mut self, memory_type: efi::MemoryType, pages: u64) {
        if !self.initialized {
            return;
        }

        let type_idx = memory_type as usize;
        let Some(stats) = self.statistics.get_mut(type_idx).filter(|s| s.special) else {
            return;
        };

        let aligned_pages = align_pages_to_granularity(pages, Self::granularity_for_type(memory_type));
        let prev = stats.current_number_of_pages;
        stats.current_number_of_pages += aligned_pages;
        let current = stats.current_number_of_pages;
        let info_idx = stats.information_index;

        log::trace!(
            target: LOG_TARGET,
            "Bin stats: {} current_pages {} -> {} (alloc +{} aligned to {})",
            memory_type_name(memory_type),
            prev,
            current,
            pages,
            aligned_pages
        );

        // Update peak tracking: if current exceeds previous peak, update for BDS
        if let Some(mti_entry) = self.memory_type_information.get_mut(info_idx)
            && current > mti_entry.number_of_pages as u64
        {
            let prev_peak = mti_entry.number_of_pages;
            mti_entry.number_of_pages = current as u32;
            log::trace!(
                target: LOG_TARGET,
                "Bin table: {} pages {} -> {} (peak update)",
                memory_type_name(memory_type),
                prev_peak,
                mti_entry.number_of_pages
            );
        }
    }

    /// Records a deallocation for statistics tracking on special (runtime) memory types.
    ///
    /// Like [`Self::record_allocation`], all special-type frees are counted regardless of address.
    pub(crate) fn record_free(&mut self, memory_type: efi::MemoryType, pages: u64) {
        if !self.initialized {
            return;
        }

        let type_idx = memory_type as usize;
        let Some(stats) = self.statistics.get_mut(type_idx).filter(|s| s.special) else {
            return;
        };

        let aligned_pages = align_pages_to_granularity(pages, Self::granularity_for_type(memory_type));
        let prev = stats.current_number_of_pages;
        stats.current_number_of_pages = stats.current_number_of_pages.saturating_sub(aligned_pages);

        log::trace!(
            target: LOG_TARGET,
            "Bin stats: {} current_pages {} -> {} (free -{} aligned to {})",
            memory_type_name(memory_type),
            prev,
            stats.current_number_of_pages,
            pages,
            aligned_pages
        );
    }

    /// Applies bin descriptors to a populated EFI memory map buffer.
    ///
    /// Post-processes the memory map by converting `EfiConventionalMemory` entries that overlap
    /// with bin regions to the bin's memory type. Entries may be split at bin boundaries.
    ///
    /// `count` is the current number of valid entries in `buffer`.
    /// Returns the new entry count after splitting and conversion.
    ///
    /// # Precondition
    ///
    /// `buffer.len()` must be at least `count + self.max_additional_descriptors()`. Callers
    /// are responsible for reserving this slack so the per-bin split (at most two extra
    /// descriptors per active special bin) always fits.
    pub(crate) fn apply_bin_descriptors(&self, buffer: &mut [efi::MemoryDescriptor], count: usize) -> usize {
        if !self.initialized {
            return count;
        }

        let buffer_len = buffer.len();
        let mut current_count = count;

        for mem_type in 0..(EFI_MAX_MEMORY_TYPE as u32) {
            // Only process special types with actual bin pages
            let Some(stats) = self.statistics.get(mem_type as usize).filter(|s| s.special && s.number_of_pages > 0)
            else {
                continue;
            };

            let bin_start = stats.base_address;
            let bin_end = stats.maximum_address;

            log::debug!(
                target: LOG_TARGET,
                "GetMemoryMap: processing bin[{}] {} range=[{:#X}..{:#X}]",
                mem_type,
                memory_type_name(mem_type),
                bin_start,
                bin_end
            );

            // Repeatedly process until no more modifications are needed.
            // Each pass may split one entry, so restart from the beginning after each modification.
            loop {
                current_count = Self::merge_descriptors(buffer, current_count);

                let entry_count = current_count;
                let mut did_modify = false;

                for i in 0..entry_count {
                    let Some(entry) =
                        buffer.get_mut(i).filter(|e| e.r#type == efi::CONVENTIONAL_MEMORY && e.number_of_pages > 0)
                    else {
                        continue;
                    };

                    let entry_start = entry.physical_start;
                    let entry_end = entry_start + uefi_pages_to_size!(entry.number_of_pages as usize) as u64 - 1;

                    // No overlap
                    if entry_end < bin_start || entry_start > bin_end {
                        continue;
                    }

                    // Case 1: Entry completely within bin
                    if entry_start >= bin_start && entry_end <= bin_end {
                        Self::set_descriptor_type(entry, mem_type, stats.runtime);
                        did_modify = true;
                        break;
                    }

                    // Case 2: Entry starts before bin
                    if entry_start < bin_start {
                        // Calculate the total extra slots needed up front so partial mutation is
                        // prevented if the buffer-sizing precondition is violated.
                        let extra_needed = if entry_end > bin_end { 2 } else { 1 };
                        debug_assert!(
                            current_count + extra_needed <= buffer_len,
                            "apply_bin_descriptors: buffer is too small, caller must reserve max_additional_descriptors()"
                        );
                        if current_count + extra_needed > buffer_len {
                            log::error!(
                                target: LOG_TARGET,
                                "Buffer is too small for memory bin descriptor split, leaving entry as EfiConventionalMemory."
                            );
                            return current_count;
                        }

                        // Shrink original entry to end at bin start
                        let pre_bin_pages = uefi_size_to_pages!((bin_start - entry_start) as usize);
                        entry.number_of_pages = pre_bin_pages as u64;

                        // Insert new entry for in-bin portion
                        current_count = Self::insert_descriptor_after(buffer, current_count, i);
                        let new_idx = i + 1;
                        let new_entry = buffer.get_mut(new_idx).expect("Newly inserted entry not found");
                        new_entry.physical_start = bin_start;
                        new_entry.number_of_pages = uefi_size_to_pages!((entry_end - bin_start + 1) as usize) as u64;
                        Self::set_descriptor_type(new_entry, mem_type, stats.runtime);

                        // If entry also extends past bin end, split again
                        if entry_end > bin_end {
                            new_entry.number_of_pages = uefi_size_to_pages!((bin_end - bin_start + 1) as usize) as u64;

                            current_count = Self::insert_descriptor_after(buffer, current_count, new_idx);
                            let post_idx = new_idx + 1;
                            let post_entry = buffer.get_mut(post_idx).expect("Failed to get post-bin entry");
                            post_entry.physical_start = bin_end + 1;
                            post_entry.number_of_pages = uefi_size_to_pages!((entry_end - bin_end) as usize) as u64;
                            Self::set_descriptor_type(post_entry, efi::CONVENTIONAL_MEMORY, false);
                        }

                        did_modify = true;
                        break;
                    }

                    // Case 3: Entry ends after bin (entry_start >= bin_start implied here)
                    if entry_end > bin_end {
                        debug_assert!(
                            current_count < buffer_len,
                            "apply_bin_descriptors: buffer is too small, caller must reserve max_additional_descriptors() slack"
                        );
                        if current_count + 1 > buffer_len {
                            log::error!(
                                target: LOG_TARGET,
                                "Buffer is too small for memory bin descriptor split, leaving entry as EfiConventionalMemory."
                            );
                            return current_count;
                        }

                        // Shrink original entry to cover only the in-bin portion
                        entry.number_of_pages = uefi_size_to_pages!((bin_end - entry_start + 1) as usize) as u64;
                        Self::set_descriptor_type(entry, mem_type, stats.runtime);

                        // Insert new entry for the post-bin portion
                        current_count = Self::insert_descriptor_after(buffer, current_count, i);
                        let post_idx = i + 1;
                        let post_entry = buffer.get_mut(post_idx).expect("Failed to get newly created post-bin entry");
                        post_entry.physical_start = bin_end + 1;
                        post_entry.number_of_pages = uefi_size_to_pages!((entry_end - bin_end) as usize) as u64;
                        Self::set_descriptor_type(post_entry, efi::CONVENTIONAL_MEMORY, false);

                        did_modify = true;
                        break;
                    }

                    // Reaching here indicates a logic bug. This could potentially be marked unreachable!()
                    // in the future.
                    debug_assert!(
                        false,
                        "apply_bin_descriptors: overlap case fell through; entry=[{:#X}..{:#X}] bin=[{:#X}..{:#X}]",
                        entry_start, entry_end, bin_start, bin_end
                    );
                    break;
                }

                if !did_modify {
                    break;
                }
            }
        }

        Self::merge_descriptors(buffer, current_count)
    }

    /// Returns the current memory type information for config table publishing.
    ///
    /// Contains peak usage data that BDS can use to recommend next-boot bin sizes.
    pub(crate) fn memory_type_information(&self) -> &[EFiMemoryTypeInformation] {
        self.memory_type_information
            .get(..self.memory_type_information_count)
            .expect("Memory Type Info count should be correct")
    }

    /// Returns the maximum number of additional descriptors that bin splitting could add.
    ///
    /// Each active memory bin can cause up to 2 additional descriptor entries (worst case
    /// where an entry spans the entire bin, requiring a triple-split).
    pub(crate) fn max_additional_descriptors(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        self.statistics.iter().filter(|s| s.number_of_pages > 0 && s.special).count() * 2
    }

    /// Sets the type and runtime attribute on a memory descriptor.
    fn set_descriptor_type(descriptor: &mut efi::MemoryDescriptor, memory_type: efi::MemoryType, runtime: bool) {
        descriptor.r#type = memory_type;
        if runtime {
            descriptor.attribute |= efi::MEMORY_RUNTIME;
        } else {
            descriptor.attribute &= !efi::MEMORY_RUNTIME;
        }
    }

    /// Inserts a new descriptor after position `after_idx` by shifting subsequent entries right.
    ///
    /// The new entry is initialized as a copy of `buffer[after_idx]`.
    /// Returns the new total count.
    fn insert_descriptor_after(buffer: &mut [efi::MemoryDescriptor], count: usize, after_idx: usize) -> usize {
        // Shift entries after `after_idx` right by one
        buffer.copy_within((after_idx + 1)..count, after_idx + 2);
        // Copy the current entry as a template for the new one. copy_within will panic if the range is invalid.
        let source = buffer.get(after_idx).copied().expect("Failed to get source entry");
        let dest = buffer.get_mut(after_idx + 1).expect("Failed to get destination entry");
        *dest = source;
        count + 1
    }

    /// Merges consecutive descriptors with the same type and attributes.
    ///
    /// Returns the new count after merging.
    fn merge_descriptors(buffer: &mut [efi::MemoryDescriptor], count: usize) -> usize {
        if count <= 1 {
            return count;
        }

        let mut write_idx = 0;
        for read_idx in 1..count {
            let write_entry = *buffer.get(write_idx).expect("count should be accurate");
            let read_entry = *buffer.get(read_idx).expect("count should be correct");

            let prev_end =
                write_entry.physical_start + uefi_pages_to_size!(write_entry.number_of_pages as usize) as u64;

            if read_entry.r#type == write_entry.r#type
                && read_entry.attribute == write_entry.attribute
                && read_entry.physical_start == prev_end
            {
                // Merge into the current entry
                let write_entry = buffer.get_mut(write_idx).expect("count should be accurate");
                write_entry.number_of_pages += read_entry.number_of_pages;
            } else {
                write_idx += 1;
                if write_idx != read_idx {
                    let write_entry = buffer.get_mut(write_idx).expect("count should be accurate");
                    *write_entry = read_entry;
                }
            }
        }

        write_idx + 1
    }

    /// Resets the bin manager to its initial uninitialized state.
    #[cfg(test)]
    pub(crate) fn reset(&mut self) {
        self.statistics = DEFAULT_STATISTICS;
        self.memory_type_information =
            [EFiMemoryTypeInformation { memory_type: 0, number_of_pages: 0 }; MAX_MEMORY_TYPE_INFO_ENTRIES];
        self.memory_type_information_count = 0;
        self.initialized = false;
    }
}

/// Searches the HOB list for a Resource Descriptor HOB owned by `MEMORY_TYPE_INFO_HOB_GUID`.
///
/// Returns `Some((physical_start, resource_length))` if exactly one valid Resource Descriptor HOB
/// is found with the correct owner, resource type, and attributes. Returns `None` if no match
/// or multiple matches are found.
pub(crate) fn find_memory_type_info_resource_hob(
    hob_list: &HobList,
    memory_type_info: &[EFiMemoryTypeInformation],
) -> Option<(efi::PhysicalAddress, u64)> {
    let target_guid = MEMORY_TYPE_INFO_HOB_GUID;
    let mut count = 0u32;
    let mut result: Option<(efi::PhysicalAddress, u64)> = None;

    for hob_entry in hob_list.iter() {
        let res_desc = match hob_entry {
            Hob::ResourceDescriptor(rd) => rd,
            _ => continue,
        };

        if res_desc.owner != target_guid {
            continue;
        }

        if res_desc.resource_type != hob::EFI_RESOURCE_SYSTEM_MEMORY {
            continue;
        }

        if (res_desc.resource_attribute & hob::MEMORY_ATTRIBUTE_MASK) != hob::TESTED_MEMORY_ATTRIBUTES {
            continue;
        }

        // Reject HOBs whose range overflows or exceeds the maximum allocation address.
        let end = match res_desc.physical_start.checked_add(res_desc.resource_length) {
            Some(end) if end <= MAX_ALLOC_ADDRESS => end,
            _ => {
                log::warn!(
                    target: LOG_TARGET,
                    "Skipping MemoryTypeInformation Resource Descriptor HOB with invalid range: start={:#X} length={:#X}",
                    res_desc.physical_start,
                    res_desc.resource_length
                );
                continue;
            }
        };

        count += 1;

        let total_needed = MemoryBinManager::calculate_total_bin_size(memory_type_info, end);

        if res_desc.resource_length >= total_needed {
            result = Some((res_desc.physical_start, res_desc.resource_length));
        }
    }

    // Reject if multiple Resource Descriptor HOBs with the owner GUID were found to avoid ambiguity
    if count > 1 {
        log::warn!(
            target: LOG_TARGET,
            "Multiple MemoryTypeInformation Resource Descriptor HOBs found ({}), rejecting all.",
            count
        );
        return None;
    }

    if let Some((start, length)) = result {
        log::info!(
            target: LOG_TARGET,
            "Found MemoryTypeInformation Resource Descriptor HOB: base={:#X} length={:#X}",
            start,
            length
        );
    } else {
        log::info!(
            target: LOG_TARGET,
            "No MemoryTypeInformation Resource Descriptor HOB found. DXE will allocate bins."
        );
    }

    result
}

/// Extracts the Memory Type Information from the GUID HOB.
///
/// Returns a Vec of `EFiMemoryTypeInformation` entries with page counts aligned to
/// the appropriate granularity for each memory type.
pub(crate) fn extract_memory_type_info_from_hob(hob_list: &HobList) -> Option<Vec<EFiMemoryTypeInformation>> {
    hob_list.iter().find_map(|hob_entry| {
        if let Hob::GuidHob(hob, data) = hob_entry {
            if hob.name != MEMORY_TYPE_INFO_HOB_GUID.into_inner() {
                return None;
            }

            let entry_size = core::mem::size_of::<EFiMemoryTypeInformation>();
            if data.is_empty() || data.len() > (EFI_MAX_MEMORY_TYPE + 1) * entry_size {
                log::error!(target: LOG_TARGET, "Invalid Memory Type Information HOB data size: {}", data.len());
                return None;
            }

            log::info!(
                target: LOG_TARGET,
                "Found Memory Type Information HOB ({} bytes, {} entries)",
                data.len(),
                data.len() / entry_size
            );

            let ptr = data.as_ptr() as *const EFiMemoryTypeInformation;
            let len = data.len() / entry_size;

            // SAFETY: HOB data is 8-byte aligned per the PI spec.
            // A compile-time assertion in allocator.rs verifies EFiMemoryTypeInformation's alignment requirement
            // is <= 8 bytes.
            let raw_entries = unsafe { core::slice::from_raw_parts(ptr, len) };

            let mut entries: Vec<EFiMemoryTypeInformation> = Vec::with_capacity(len);
            for entry in raw_entries {
                if entry.memory_type as usize >= EFI_MAX_MEMORY_TYPE {
                    // Either the sentinel or an invalid type. Include as-is (since the sentinel terminates processing).
                    entries.push(EFiMemoryTypeInformation {
                        memory_type: entry.memory_type,
                        number_of_pages: entry.number_of_pages,
                    });
                    break;
                }

                // Align page count to the type's allocation granularity for logging.
                // The config table retains the original HOB values. Alignment is only applied when
                // allocating the actual GCD bin region.
                let granularity = MemoryBinManager::granularity_for_type(entry.memory_type);
                let unaligned_size = uefi_pages_to_size!(entry.number_of_pages as usize);
                let aligned_size = align_up(unaligned_size, granularity).unwrap_or(unaligned_size);
                let aligned_pages = uefi_size_to_pages!(aligned_size);

                log::info!(
                    target: LOG_TARGET,
                    "  MemTypeInfo: {} pages={} (GCD alloc will use {})",
                    memory_type_name(entry.memory_type),
                    entry.number_of_pages,
                    aligned_pages,
                );

                entries.push(*entry);
            }

            Some(entries)
        } else {
            None
        }
    })
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use patina::base::{SIZE_64KB, UEFI_PAGE_SIZE};

    const RT_GRAN_PAGES: u64 =
        (MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA) / UEFI_PAGE_SIZE) as u64;

    /// Returns the preferred allocation range for the given memory type.
    ///
    /// Returns `Some((base, max))` if a bin exists for this type with pages > 0.
    fn preferred_range(
        manager: &MemoryBinManager,
        memory_type: efi::MemoryType,
    ) -> Option<(efi::PhysicalAddress, efi::PhysicalAddress)> {
        manager.active_bins().find(|(mt, _, _, _)| *mt == memory_type).map(|(_, base, max, _)| (base, max))
    }

    /// Returns a range size large enough to hold `pages` of a runtime type including alignment padding.
    fn rt_range_size(pages: u32) -> u64 {
        let granularity = MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA);
        // Enough for the pages plus one unit of granularity for alignment padding.
        (pages as u64) * UEFI_PAGE_SIZE as u64 + granularity as u64
    }

    /// Initializes a `MemoryBinManager` from the given memory type info at the given base address.
    ///
    /// Uses `contiguous_alloc_size` to compute a range large enough for all bins.
    #[coverage(off)]
    fn init_bins(manager: &mut MemoryBinManager, base: u64, info: &[EFiMemoryTypeInformation]) {
        crate::test_support::init_test_logger();
        let range_size = MemoryBinManager::contiguous_alloc_size(info).unwrap() as u64;
        assert!(manager.initialize_from_range(base, range_size, info), "init_bins failed");
    }

    #[test]
    fn test_memory_bin_new_uninitialized() {
        let manager = MemoryBinManager::new();
        assert!(!manager.is_initialized());
        assert_eq!(preferred_range(&manager, efi::RUNTIME_SERVICES_DATA), None);
        assert_eq!(manager.max_additional_descriptors(), 0);
    }

    #[test]
    fn test_memory_bin_calculate_total_size_no_alignment() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 10 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 20 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let size = MemoryBinManager::calculate_total_bin_size(&info, 0);
        assert_eq!(size, (10 + 20) * UEFI_PAGE_SIZE as u64);
    }

    #[test]
    fn test_memory_bin_initialize_from_range() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 8 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_size = rt_range_size(4) + rt_range_size(8);
        let range_start = 0x1000_0000u64;

        let result = manager.initialize_from_range(range_start, range_size, &info);
        assert!(result);
        assert!(manager.is_initialized());

        // Bins should have been set up
        let rt_code_range = preferred_range(&manager, efi::RUNTIME_SERVICES_CODE);
        assert!(rt_code_range.is_some());
        let (base, max) = rt_code_range.unwrap();
        assert!(base >= range_start);
        assert!(max < range_start + range_size);

        let rt_data_range = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA);
        assert!(rt_data_range.is_some());

        // Non-bin types should return None
        assert_eq!(preferred_range(&manager, efi::BOOT_SERVICES_DATA), None);
    }

    #[test]
    fn test_memory_bin_initialize_from_range_too_small() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 100 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        // Range is too small for 100 pages
        let result = manager.initialize_from_range(0x1000_0000, UEFI_PAGE_SIZE as u64, &info);
        assert!(!result);
        assert!(!manager.is_initialized());
    }

    #[test]
    fn test_memory_bin_record_allocation_in_bin() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 64 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_start = 0x1000_0000u64;
        let range_size = rt_range_size(64);
        manager.initialize_from_range(range_start, range_size, &info);

        // Record in-bin allocation. The page count is aligned up to granularity.
        manager.record_allocation(efi::RUNTIME_SERVICES_DATA, 4);
        assert_eq!(
            manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages,
            align_pages_to_granularity(4, MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA))
        );

        // Record another in-bin allocation
        let prev = manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages;
        manager.record_allocation(efi::RUNTIME_SERVICES_DATA, 2);
        assert_eq!(
            manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages,
            prev + align_pages_to_granularity(2, MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA))
        );
    }

    #[test]
    fn test_memory_bin_record_free() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 64 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_start = 0x1000_0000u64;
        let range_size = rt_range_size(64);
        manager.initialize_from_range(range_start, range_size, &info);

        manager.record_allocation(efi::RUNTIME_SERVICES_DATA, RT_GRAN_PAGES);
        assert_eq!(manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages, RT_GRAN_PAGES);

        manager.record_free(efi::RUNTIME_SERVICES_DATA, RT_GRAN_PAGES);
        assert_eq!(manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages, 0);

        // Free more than allocated. It should stop at 0.
        manager.record_allocation(efi::RUNTIME_SERVICES_DATA, RT_GRAN_PAGES);
        manager.record_free(efi::RUNTIME_SERVICES_DATA, 100);
        assert_eq!(manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages, 0);
    }

    #[test]
    fn test_memory_bin_peak_tracking() {
        let bin_pages: u32 = 8;
        let alloc_pages = (bin_pages as u64).max(RT_GRAN_PAGES) + RT_GRAN_PAGES;

        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: bin_pages },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_start = 0x1000_0000u64;
        let range_size = rt_range_size(bin_pages).max(rt_range_size(alloc_pages as u32));
        manager.initialize_from_range(range_start, range_size, &info);

        // Allocate enough to exceed the original bin size
        manager.record_allocation(efi::RUNTIME_SERVICES_DATA, alloc_pages);

        // Peak should be updated in memory_type_information
        let expected =
            align_pages_to_granularity(alloc_pages, MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA));
        assert_eq!(manager.memory_type_information()[0].number_of_pages, expected as u32);
    }

    #[test]
    fn test_memory_bin_apply_descriptors_fully_within() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA).unwrap();
        let bin_pages = uefi_size_to_pages!((bin_max - bin_base + 1) as usize);

        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: bin_base,
            virtual_start: 0,
            number_of_pages: bin_pages as u64,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_ne!(buffer[0].attribute & efi::MEMORY_RUNTIME, 0);
    }

    #[test]
    fn test_memory_bin_apply_descriptors_starts_before() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA).unwrap();
        let bin_size = bin_max - bin_base + 1;

        // Entry starts 1 page before bin
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize);

        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: entry_start,
            virtual_start: 0,
            number_of_pages: entry_pages as u64,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert!(count >= 2);

        // First entry should be the pre-bin conventional memory
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);

        // Second entry should be the bin type
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[1].physical_start, bin_base);
    }

    #[test]
    fn test_memory_bin_apply_descriptors_ends_after() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA).unwrap();
        let bin_size = bin_max - bin_base + 1;

        // Entry starts at bin_base, ends 1 page after bin
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize);

        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: bin_base,
            virtual_start: 0,
            number_of_pages: entry_pages as u64,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 2);

        // First entry should be the bin type
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[0].physical_start, bin_base);

        // Second entry should be conventional memory after bin
        assert_eq!(buffer[1].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[1].physical_start, bin_max + 1);
    }

    #[test]
    fn test_memory_bin_apply_descriptors_spans_bin() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA).unwrap();
        let bin_size = bin_max - bin_base + 1;

        // Entry spans before and after bin
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + 2 * UEFI_PAGE_SIZE as u64) as usize);

        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: entry_start,
            virtual_start: 0,
            number_of_pages: entry_pages as u64,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 3);

        // Pre-bin conventional memory
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);

        // Bin region
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[1].physical_start, bin_base);

        // Post-bin conventional memory
        assert_eq!(buffer[2].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[2].physical_start, bin_max + 1);
    }

    #[test]
    fn test_memory_bin_apply_descriptors_no_overlap() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        // Entry far away from bin
        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: 0x8000_0000,
            virtual_start: 0,
            number_of_pages: 4,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
    }

    #[test]
    fn test_memory_bin_apply_descriptors_runtime_attribute() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_CODE).unwrap();
        let bin_pages = uefi_size_to_pages!((bin_max - bin_base + 1) as usize);

        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: bin_base,
            virtual_start: 0,
            number_of_pages: bin_pages as u64,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_CODE);
        // Runtime services code is a runtime type
        assert_ne!(buffer[0].attribute & efi::MEMORY_RUNTIME, 0);
    }

    #[test]
    fn test_memory_bin_max_additional_descriptors() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 8 },
            EFiMemoryTypeInformation { memory_type: efi::ACPI_MEMORY_NVS, number_of_pages: 2 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        // RT Code (special+runtime), RT Data (special+runtime), ACPI NVS (special) = 3 (special) memory bins
        assert_eq!(manager.max_additional_descriptors(), 3 * 2);
    }

    #[test]
    fn test_memory_bin_seed_statistics_from_hob() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 64 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_start = 0x1000_0000u64;
        let range_size = rt_range_size(64);
        manager.initialize_from_range(range_start, range_size, &info);

        manager.seed_statistics_from_hob(efi::RUNTIME_SERVICES_DATA, 3);
        assert_eq!(
            manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages,
            align_pages_to_granularity(3, MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA))
        );
    }

    #[test]
    fn test_memory_bin_no_bins_when_not_initialized() {
        let manager = MemoryBinManager::new();

        // All operations should be no-ops when not initialized
        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: 0x1000_0000,
            virtual_start: 0,
            number_of_pages: 4,
            attribute: 0,
        }; 5];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1); // Unchanged
        assert_eq!(preferred_range(&manager, efi::RUNTIME_SERVICES_DATA), None);
        assert_eq!(manager.max_additional_descriptors(), 0);
    }

    #[test]
    fn test_memory_bin_double_initialization_rejected() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_start = 0x1000_0000u64;
        let range_size = rt_range_size(4);

        assert!(manager.initialize_from_range(range_start, range_size, &info));
        // Second initialization should be rejected
        assert!(!manager.initialize_from_range(range_start + 0x100_0000, range_size, &info));
    }

    #[test]
    fn test_merge_descriptors() {
        let mut buffer = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x2000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::RUNTIME_SERVICES_DATA,
                physical_start: 0x3000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WB | efi::MEMORY_RUNTIME,
            },
            efi::MemoryDescriptor { r#type: 0, physical_start: 0, virtual_start: 0, number_of_pages: 0, attribute: 0 },
            efi::MemoryDescriptor { r#type: 0, physical_start: 0, virtual_start: 0, number_of_pages: 0, attribute: 0 },
        ];

        let count = MemoryBinManager::merge_descriptors(&mut buffer, 3);
        assert_eq!(count, 2);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].number_of_pages, 2);
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
    }

    #[test]
    fn test_merge_descriptors_single_entry() {
        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: 0x1000,
            virtual_start: 0,
            number_of_pages: 4,
            attribute: efi::MEMORY_WB,
        }];
        let count = MemoryBinManager::merge_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_merge_descriptors_gap_prevents_merge() {
        let mut buffer = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
            // Gap: 0x2000 is the end of the first, but second starts at 0x4000
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x4000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
        ];
        let count = MemoryBinManager::merge_descriptors(&mut buffer, 2);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_calculate_total_bin_size_with_alignment() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 1 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let entry_size = UEFI_PAGE_SIZE as u64;

        // With bin_top=0, no alignment padding
        let size_no_align = MemoryBinManager::calculate_total_bin_size(&info, 0);
        assert_eq!(size_no_align, entry_size);

        // With a non-zero bin_top that is already aligned to the type's granularity, no padding
        let granularity = MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA) as u64;
        let aligned_top = 0x1000_0000u64;
        assert_eq!(aligned_top % granularity, 0, "test precondition: top must be granularity-aligned");
        let size_aligned = MemoryBinManager::calculate_total_bin_size(&info, aligned_top);
        if entry_size.is_multiple_of(granularity) {
            assert_eq!(size_aligned, entry_size);
        } else {
            assert!(size_aligned >= entry_size);
        }

        // With an unaligned bin_top, alignment padding is required.
        let size_unaligned = MemoryBinManager::calculate_total_bin_size(&info, 0x1000_0001);
        assert!(size_unaligned > entry_size);
    }

    #[test]
    fn test_active_bins_returns_only_configured_types() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 0 },
            EFiMemoryTypeInformation { memory_type: efi::ACPI_MEMORY_NVS, number_of_pages: 2 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x3000_0000, &info);

        let bins: Vec<_> = manager.active_bins().collect();
        // RTCode (4 pages) and ACPI NVS (2 pages). RTData has 0 pages so excluded.
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0].0, efi::RUNTIME_SERVICES_CODE);
        assert_eq!(bins[1].0, efi::ACPI_MEMORY_NVS);
    }

    #[test]
    fn test_record_allocation_ignored_for_non_special_type() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::BOOT_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x1000_0000, &info);

        // BSData is not a special type, so record_allocation should be a no-op
        manager.record_allocation(efi::BOOT_SERVICES_DATA, 2);
        assert_eq!(manager.statistics[efi::BOOT_SERVICES_DATA as usize].current_number_of_pages, 0);
    }

    #[test]
    fn test_record_allocation_counts_outside_bin_range() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x1000_0000, &info);

        // Address outside the bin range is counted so BDS can see overflow.
        manager.record_allocation(efi::RUNTIME_SERVICES_DATA, 2);
        assert_eq!(
            manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages,
            align_pages_to_granularity(2, MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA))
        );
    }

    #[test]
    fn test_seed_statistics_always_counted_for_special_types() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 16 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        let range_start = 0x1000_0000u64;
        let range_size = rt_range_size(16);
        manager.initialize_from_range(range_start, range_size, &info);

        // All memory bin-type HOB allocations are counted regardless of address
        manager.seed_statistics_from_hob(efi::RUNTIME_SERVICES_DATA, 5);
        assert_eq!(
            manager.statistics[efi::RUNTIME_SERVICES_DATA as usize].current_number_of_pages,
            align_pages_to_granularity(5, MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA))
        );

        // A non-memory bin type is not tracked
        manager.seed_statistics_from_hob(efi::BOOT_SERVICES_DATA, 3);
        assert_eq!(manager.statistics[efi::BOOT_SERVICES_DATA as usize].current_number_of_pages, 0);
    }

    #[test]
    fn test_memory_type_information_returned_after_init() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 8 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x1000_0000, &info);

        let mti = manager.memory_type_information();
        assert_eq!(mti.len(), 3);
        assert_eq!(mti[0].memory_type, efi::RUNTIME_SERVICES_CODE);
        assert_eq!(mti[0].number_of_pages, 4);
        assert_eq!(mti[1].memory_type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(mti[1].number_of_pages, 8);
    }

    #[test]
    fn test_apply_descriptors_skips_non_conventional() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];

        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);

        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA).unwrap();
        let bin_pages = uefi_size_to_pages!((bin_max - bin_base + 1) as usize);

        // Entry is BSCode within the bin range so it should not be converted
        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::BOOT_SERVICES_CODE,
            physical_start: bin_base,
            virtual_start: 0,
            number_of_pages: bin_pages as u64,
            attribute: efi::MEMORY_WB,
        }; 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::BOOT_SERVICES_CODE);
    }

    #[test]
    fn test_memory_type_name_known_and_unknown() {
        assert_eq!(memory_type_name(efi::RUNTIME_SERVICES_DATA), "RuntimeServicesData");
        assert_eq!(memory_type_name(efi::BOOT_SERVICES_CODE), "BootServicesCode");
        assert!(memory_type_name(0xFFFF).starts_with("Unknown"));
    }

    #[test]
    fn test_align_pages_to_granularity_equal_to_page_size() {
        // granularity == UEFI_PAGE_SIZE => granularity_pages == 1, pages returned unchanged
        assert_eq!(align_pages_to_granularity(0, UEFI_PAGE_SIZE), 0);
        assert_eq!(align_pages_to_granularity(1, UEFI_PAGE_SIZE), 1);
        assert_eq!(align_pages_to_granularity(7, UEFI_PAGE_SIZE), 7);
    }

    #[test]
    fn test_align_pages_to_granularity_smaller_than_page_size() {
        // granularity < UEFI_PAGE_SIZE => granularity_pages == 0 <= 1, pages returned unchanged
        assert_eq!(align_pages_to_granularity(5, UEFI_PAGE_SIZE / 2), 5);
    }

    #[test]
    fn test_align_pages_to_granularity_two_pages() {
        assert_eq!(align_pages_to_granularity(0, 2 * UEFI_PAGE_SIZE), 0);
        assert_eq!(align_pages_to_granularity(1, 2 * UEFI_PAGE_SIZE), 2);
        assert_eq!(align_pages_to_granularity(2, 2 * UEFI_PAGE_SIZE), 2);
        assert_eq!(align_pages_to_granularity(3, 2 * UEFI_PAGE_SIZE), 4);
        assert_eq!(align_pages_to_granularity(4, 2 * UEFI_PAGE_SIZE), 4);
    }

    #[test]
    fn test_align_pages_to_granularity_sixteen_pages() {
        assert_eq!(align_pages_to_granularity(0, SIZE_64KB), 0);
        assert_eq!(align_pages_to_granularity(1, SIZE_64KB), 16);
        assert_eq!(align_pages_to_granularity(15, SIZE_64KB), 16);
        assert_eq!(align_pages_to_granularity(16, SIZE_64KB), 16);
        assert_eq!(align_pages_to_granularity(17, SIZE_64KB), 32);
    }

    #[test]
    fn test_contiguous_alloc_size_single_entry() {
        let rt_data_pages = 10;

        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: rt_data_pages },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let size = MemoryBinManager::contiguous_alloc_size(&info).unwrap();
        let raw = rt_data_pages as usize * UEFI_PAGE_SIZE;
        let granularity = MemoryBinManager::granularity_for_type(efi::RUNTIME_SERVICES_DATA);
        // Must be at least raw + one granularity unit of padding, rounded up to granularity.
        assert!(size >= raw + granularity);
        assert_eq!(size % granularity, 0);
    }

    #[test]
    fn test_contiguous_alloc_size_multiple_entries() {
        let rt_code_pages = 4;
        let rt_data_pages = 8;
        let acpi_reclaim_pages = 2;
        let total_pages: usize = (rt_code_pages + rt_data_pages + acpi_reclaim_pages) as usize;

        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: rt_code_pages },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: rt_data_pages },
            EFiMemoryTypeInformation { memory_type: efi::ACPI_RECLAIM_MEMORY, number_of_pages: acpi_reclaim_pages },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let size = MemoryBinManager::contiguous_alloc_size(&info).unwrap();

        let raw = total_pages * UEFI_PAGE_SIZE;
        assert!(size >= raw, "size {:#X} must be >= raw {:#X}", size, raw);
    }

    #[test]
    fn test_contiguous_alloc_size_empty() {
        // Sentinel only, no pages.
        let info = [EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 }];
        assert_eq!(MemoryBinManager::contiguous_alloc_size(&info), None);
    }

    #[test]
    fn test_contiguous_alloc_size_all_zero_pages() {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: 0 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 0 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        assert_eq!(MemoryBinManager::contiguous_alloc_size(&info), None);
    }

    #[test]
    fn test_contiguous_alloc_size_skips_zero_page_entries() {
        let rt_data_pages = 4;

        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 0 },
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: rt_data_pages },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let size = MemoryBinManager::contiguous_alloc_size(&info).unwrap();
        // Only 1 entry with pages > 0, so padding = 1 * max_granularity.
        let raw = rt_data_pages as usize * UEFI_PAGE_SIZE;
        assert!(size >= raw);
    }

    /// Helper that builds a single-bin manager (RUNTIME_SERVICES_DATA) and returns the
    /// (bin_base, bin_max, bin_size) for tests that need to construct entries relative
    /// to the bin.
    fn single_bin_manager(pages: u32) -> (MemoryBinManager, efi::PhysicalAddress, efi::PhysicalAddress, u64) {
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_DATA, number_of_pages: pages },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x2000_0000, &info);
        let (bin_base, bin_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_DATA).unwrap();
        let bin_size = bin_max - bin_base + 1;
        (manager, bin_base, bin_max, bin_size)
    }

    // Helper to create a conventional memory descriptor for testing.
    fn conv_descriptor(physical_start: efi::PhysicalAddress, pages: u64) -> efi::MemoryDescriptor {
        efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start,
            virtual_start: 0,
            number_of_pages: pages,
            attribute: efi::MEMORY_WB,
        }
    }

    #[test]
    fn test_apply_descriptors_count_zero() {
        let (manager, _, _, _) = single_bin_manager(4);
        let mut buffer = [conv_descriptor(0x1000_0000, 4); 10];
        let count = manager.apply_bin_descriptors(&mut buffer, 0);
        assert_eq!(count, 0);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
    }

    #[test]
    fn test_apply_descriptors_entry_just_before_bin() {
        // Entry ends exactly at bin_start - 1 (no overlap).
        let (manager, bin_base, _, _) = single_bin_manager(4);
        let pages = 2u64;
        let entry_start = bin_base - uefi_pages_to_size!(pages as usize) as u64;
        let mut buffer = [conv_descriptor(entry_start, pages); 10];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);
        assert_eq!(buffer[0].number_of_pages, pages);
    }

    #[test]
    fn test_apply_descriptors_entry_just_after_bin() {
        // Entry starts exactly at bin_max + 1 (no overlap).
        let (manager, _, bin_max, _) = single_bin_manager(4);
        let mut buffer = [conv_descriptor(bin_max + 1, 2); 10];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, bin_max + 1);
    }

    #[test]
    fn test_apply_descriptors_entry_equals_bin_exactly() {
        // Case 1 boundary: entry covers exactly [bin_base, bin_max].
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!(bin_size as usize) as u64;
        let mut buffer = [conv_descriptor(bin_base, entry_pages); 10];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[0].physical_start, bin_base);
        let expected_end =
            buffer[0].physical_start + uefi_pages_to_size!(buffer[0].number_of_pages as usize) as u64 - 1;
        assert_eq!(expected_end, bin_max);
    }

    #[test]
    fn test_apply_descriptors_starts_before_ends_at_bin_end() {
        // Case 2 single split: pre-bin tail + in-bin portion ending exactly at bin_max.
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;
        let mut buffer = [conv_descriptor(entry_start, entry_pages); 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 2);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);
        assert_eq!(buffer[0].number_of_pages, 1);
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[1].physical_start, bin_base);
        let post_end = buffer[1].physical_start + uefi_pages_to_size!(buffer[1].number_of_pages as usize) as u64 - 1;
        assert_eq!(post_end, bin_max);
    }

    #[test]
    fn test_apply_descriptors_starts_at_bin_start_ends_after_bin() {
        // Case 3 boundary: entry begins exactly at bin_base and extends past bin_max.
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;
        let mut buffer = [conv_descriptor(bin_base, entry_pages); 10];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 2);
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[0].physical_start, bin_base);
        assert_eq!(buffer[1].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[1].physical_start, bin_max + 1);
        assert_eq!(buffer[1].number_of_pages, 1);
    }

    #[test]
    fn test_apply_descriptors_multiple_bins() {
        // Two active bins. Each conventional entry lies entirely within one bin.
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::RUNTIME_SERVICES_CODE, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: efi::ACPI_MEMORY_NVS, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x3000_0000, &info);

        let (rt_base, rt_max) = preferred_range(&manager, efi::RUNTIME_SERVICES_CODE).unwrap();
        let (acpi_base, acpi_max) = preferred_range(&manager, efi::ACPI_MEMORY_NVS).unwrap();
        let rt_pages = uefi_size_to_pages!((rt_max - rt_base + 1) as usize) as u64;
        let acpi_pages = uefi_size_to_pages!((acpi_max - acpi_base + 1) as usize) as u64;

        // Order entries by ascending physical address.
        let (lo_base, lo_pages, lo_type, hi_base, hi_pages, hi_type) = if rt_base < acpi_base {
            (rt_base, rt_pages, efi::RUNTIME_SERVICES_CODE, acpi_base, acpi_pages, efi::ACPI_MEMORY_NVS)
        } else {
            (acpi_base, acpi_pages, efi::ACPI_MEMORY_NVS, rt_base, rt_pages, efi::RUNTIME_SERVICES_CODE)
        };

        let mut buffer = [conv_descriptor(0, 0); 10];
        buffer[0] = conv_descriptor(lo_base, lo_pages);
        buffer[1] = conv_descriptor(hi_base, hi_pages);

        let count = manager.apply_bin_descriptors(&mut buffer, 2);
        assert_eq!(count, 2);
        assert_eq!(buffer[0].r#type, lo_type);
        assert_eq!(buffer[1].r#type, hi_type);
    }

    #[test]
    fn test_apply_descriptors_multiple_entries_one_overlapping() {
        // Three entries. Only the middle one overlaps the bin. Others must be unchanged.
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!(bin_size as usize) as u64;

        let before_pages = 2u64;
        let before_start = bin_base - 0x1_0000 - uefi_pages_to_size!(before_pages as usize) as u64;
        let after_start = bin_max + 1 + 0x1_0000;

        let mut buffer = [conv_descriptor(0, 0); 10];
        buffer[0] = conv_descriptor(before_start, before_pages);
        buffer[1] = conv_descriptor(bin_base, entry_pages);
        buffer[2] = conv_descriptor(after_start, 2);

        let count = manager.apply_bin_descriptors(&mut buffer, 3);
        assert_eq!(count, 3);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, before_start);
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[1].physical_start, bin_base);
        assert_eq!(buffer[2].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[2].physical_start, after_start);
    }

    #[test]
    fn test_apply_descriptors_exact_fit_buffer_case2_single() {
        // Buffer length is exactly count + 1 (the minimum slack for a Case 2 single split).
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;

        let mut buffer = [conv_descriptor(entry_start, entry_pages); 2];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 2);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        let post_end = buffer[1].physical_start + uefi_pages_to_size!(buffer[1].number_of_pages as usize) as u64 - 1;
        assert_eq!(post_end, bin_max);
    }

    #[test]
    fn test_apply_descriptors_exact_fit_buffer_case2_double() {
        // Buffer length is exactly count + 2 (the minimum slack for a Case 2 spans-bin split).
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + 2 * UEFI_PAGE_SIZE as u64) as usize) as u64;

        let mut buffer = [conv_descriptor(entry_start, entry_pages); 3];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 3);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[1].physical_start, bin_base);
        assert_eq!(buffer[2].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[2].physical_start, bin_max + 1);
    }

    #[test]
    fn test_apply_descriptors_exact_fit_buffer_case3() {
        // Buffer length is exactly count + 1 (the minimum slack for a Case 3 split).
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;

        let mut buffer = [conv_descriptor(bin_base, entry_pages); 2];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 2);
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_eq!(buffer[1].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[1].physical_start, bin_max + 1);
    }

    #[test]
    fn test_apply_descriptors_buffer_too_small_case2_single_fallback() {
        // Case 2 single split needs count + 1, but buffer is sized exactly to count.
        // The function must not mutate the entry and must return the original count.
        let (manager, bin_base, _, bin_size) = single_bin_manager(4);
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;
        let original = conv_descriptor(entry_start, entry_pages);

        let mut buffer = [original; 1];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);

        assert_eq!(count, 1);
        // Buffer should be unchanged (not partially mutated).
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);
        assert_eq!(buffer[0].number_of_pages, entry_pages);
    }

    #[test]
    fn test_apply_descriptors_buffer_too_small_case2_double_fallback() {
        // Case 2 spans-bin needs count + 2, but buffer only has count + 1.
        // The function must not mutate the entry and must return the original count.
        let (manager, bin_base, _, bin_size) = single_bin_manager(4);
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + 2 * UEFI_PAGE_SIZE as u64) as usize) as u64;
        let original = conv_descriptor(entry_start, entry_pages);

        let mut buffer = [original; 2];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);

        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, entry_start);
        assert_eq!(buffer[0].number_of_pages, entry_pages);
    }

    #[test]
    fn test_apply_descriptors_buffer_too_small_case3_fallback() {
        // Case 3 needs count + 1, but buffer is sized exactly to count.
        // The function must not mutate the entry and must return the original count.
        let (manager, bin_base, _, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;
        let original = conv_descriptor(bin_base, entry_pages);

        let mut buffer = [original; 1];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);

        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, bin_base);
        assert_eq!(buffer[0].number_of_pages, entry_pages);
    }

    #[test]
    fn test_apply_descriptors_buffer_too_small_case1_unaffected() {
        // Case 1 (entry fully within bin) does not require any extra slots, so a buffer
        // sized exactly to count is fine and the conversion still happens in place.
        let (manager, bin_base, _, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!(bin_size as usize) as u64;
        let mut buffer = [conv_descriptor(bin_base, entry_pages); 1];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::RUNTIME_SERVICES_DATA);
    }

    #[test]
    fn test_apply_descriptors_initialized_no_special_bins() {
        // Initialize with only a non-special memory type (BOOT_SERVICES_DATA). The manager
        // is initialized but has no active special bins, so apply must be a no-op.
        let info = [
            EFiMemoryTypeInformation { memory_type: efi::BOOT_SERVICES_DATA, number_of_pages: 4 },
            EFiMemoryTypeInformation { memory_type: EFI_MAX_MEMORY_TYPE as u32, number_of_pages: 0 },
        ];
        let mut manager = MemoryBinManager::new();
        init_bins(&mut manager, 0x4000_0000, &info);
        assert_eq!(manager.max_additional_descriptors(), 0);

        let mut buffer = [conv_descriptor(0x4000_0000, 4); 4];
        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
    }

    #[test]
    fn test_apply_descriptors_preserves_attributes_on_pre_bin_split() {
        // The pre-bin conventional remainder retains the original entry's non-runtime
        // attributes; the in-bin portion picks up MEMORY_RUNTIME (runtime bin type).
        let (manager, bin_base, bin_max, bin_size) = single_bin_manager(4);
        let entry_start = bin_base - UEFI_PAGE_SIZE as u64;
        let entry_pages = uefi_size_to_pages!((bin_size + UEFI_PAGE_SIZE as u64) as usize) as u64;

        let mut buffer = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: entry_start,
            virtual_start: 0,
            number_of_pages: entry_pages,
            attribute: efi::MEMORY_WB | efi::MEMORY_WT,
        }; 4];

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 2);

        // Pre-bin conventional keeps original (non-runtime) attributes.
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].attribute & (efi::MEMORY_WB | efi::MEMORY_WT), efi::MEMORY_WB | efi::MEMORY_WT);
        assert_eq!(buffer[0].attribute & efi::MEMORY_RUNTIME, 0);

        // In-bin portion has the runtime attribute set.
        assert_eq!(buffer[1].r#type, efi::RUNTIME_SERVICES_DATA);
        assert_ne!(buffer[1].attribute & efi::MEMORY_RUNTIME, 0);
        assert_eq!(buffer[1].attribute & (efi::MEMORY_WB | efi::MEMORY_WT), efi::MEMORY_WB | efi::MEMORY_WT);
        let in_bin_end = buffer[1].physical_start + uefi_pages_to_size!(buffer[1].number_of_pages as usize) as u64 - 1;
        assert_eq!(in_bin_end, bin_max);
    }

    #[test]
    fn test_apply_descriptors_ignores_entries_beyond_count() {
        // Entries past `count` must be ignored even if they look like overlaps.
        let (manager, bin_base, _, bin_size) = single_bin_manager(4);
        let entry_pages = uefi_size_to_pages!(bin_size as usize) as u64;

        let mut buffer = [conv_descriptor(0, 0); 4];
        // Real entry: far from bin.
        buffer[0] = conv_descriptor(0x8000_0000, 2);
        // Stale data past count: would otherwise overlap the bin.
        buffer[1] = conv_descriptor(bin_base, entry_pages);

        let count = manager.apply_bin_descriptors(&mut buffer, 1);
        assert_eq!(count, 1);
        assert_eq!(buffer[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(buffer[0].physical_start, 0x8000_0000);
        // The stale entry at index 1 was never considered.
        assert_eq!(buffer[1].physical_start, bin_base);
        assert_eq!(buffer[1].r#type, efi::CONVENTIONAL_MEMORY);
    }
}
