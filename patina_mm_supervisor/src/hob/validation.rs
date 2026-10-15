//! Validation of critical incoming HOBs.
//!
//! The MM Supervisor consumes several HOBs produced outside of MMRAM by the MM
//! IPL. Validation runs in two rounds during BSP initialization.
//!
//! ## Pre-paging checks
//!
//! Run before any HOB content is consumed and before the page table is
//! initialized. Before the SMRAM scan, [`validate_hob_list_structure`] checks
//! that the HOB headers form a list that ends within a fixed size. The rest
//! run once the MMRAM regions are scanned:
//!
//! 1. The scanned MMRAM regions form a single contiguous, non-overlapping span.
//! 2. No `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOB overlaps MMRAM (these describe
//!    allocations outside of MMRAM).
//! 3. The supervisor's module HOBs, including the standard PI Init module HOB,
//!    lie inside MMRAM, do not overlap each other, and have entry points inside
//!    their allocations. The Init allocation is page-aligned, and both the Init
//!    and Core modules must be present.
//! 4. Resource descriptor HOBs do not overlap within the same version and
//!    address space (memory vs I/O). Only v2 descriptors are checked unless the
//!    `v1_resource_descriptor_support` feature is enabled, which adds v1.
//!    Descriptors owned by the Memory Type Information GUID are skipped, since
//!    the PEI memory bin HOB is expected to overlap the system-memory
//!    descriptors backing it.
//! 5. Every v2 resource descriptor carries valid memory attributes (exactly one
//!    cacheability bit for memory, no `EFI_MEMORY_UCE`, no attributes for I/O).
//! 6. The pointers reported in the MM Supervisor `PassDown` HOB reference
//!    memory inside MMRAM (and the HOB reports the expected revision).
//!
//! ## Post-paging checks
//!
//! Run after the page table is initialized, so per-page attributes can be
//! queried from it:
//!
//! 7. Every page of the MM Supervisor Core module is supervisor-owned (the U/S
//!    "SP" bit is set) and every page of the MM Supervisor User module is
//!    user-accessible (the SP bit is clear).
//! 8. Each module's entry point lies on a code page: present (`ReadProtect`
//!    clear), read-only (`ReadOnly` set) and executable (`ExecuteProtect`
//!    clear), i.e. it sits in the module's code section.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use patina::management_mode::supervisor::{
    MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID,
    MM_SUPERVISOR_USER_GUID,
};
use patina::pi::guid::HOB_MEMORY_ALLOC_MODULE_GUID;
use patina::pi::hob::{
    CPU, Capsule, Cpu, EFI_RESOURCE_IO, EFI_RESOURCE_IO_RESERVED, END_OF_HOB_LIST, FV, FV2, FV3, FirmwareVolume,
    FirmwareVolume2, FirmwareVolume3, GUID_EXTENSION, GuidHob, HANDOFF, Hob, HobHeader, MEMORY_ALLOCATION,
    MEMORY_TYPE_INFO_HOB_GUID, MemoryAllocation, PhaseHandoffInformationTable, RESOURCE_DESCRIPTOR,
    RESOURCE_DESCRIPTOR2, ResourceDescriptor, ResourceDescriptorV2, UEFI_CAPSULE,
};
use patina::standard::efi;
use patina::{MemoryRange, UEFI_PAGE_SIZE};
use patina_paging::{MemoryAttributes, PageTable};
use zerocopy::FromBytes;

use crate::error::MmSupervisorResult;
use crate::hob::pass_down::{
    MM_SUPV_PASS_DOWN_HOB_GUID, MM_SUPV_PASS_DOWN_HOB_REVISION, MmSupvPassDownHobData, PassDownHobError,
};
use crate::memory::PAGE_STEP;
use crate::memory::mmram::MmramRegion;
use crate::state::security::SupervisorPageTable;
use crate::state::security_state;

/// Maximum number of `MemoryAllocationModule` HOBs accepted while checking for
/// overlaps. This bounds the stack storage used for the pairwise comparison.
const MAX_ALLOC_MODULES: usize = 64;

/// Maximum number of resource descriptor HOBs accepted per category while
/// checking for overlaps. This bounds the stack storage used for the comparison.
const MAX_RESOURCE_HOBS: usize = 128;

/// Largest HOB list, in bytes, that [`validate_hob_list_structure`] accepts.
///
/// The list is walked before the MMRAM bound is known, so this limits how far header lengths
/// chosen by the producer can move the walk.
const MAX_HOB_LIST_SIZE: usize = 0x10_0000;

/// Errors produced while validating the critical incoming HOBs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HobValidationError {
    /// No MMRAM regions were reported.
    NoMmramRegions,
    /// An address range was empty or overflowed the physical address space.
    InvalidAddressRange {
        /// Description of the range being validated.
        kind: &'static str,
        /// Base address of the invalid range.
        base: u64,
        /// Length of the invalid range in bytes.
        length: u64,
    },
    /// Two MMRAM regions overlap each other.
    MmramRegionsOverlap {
        /// The first overlapping region.
        first: MemoryRange,
        /// The second overlapping region.
        second: MemoryRange,
    },
    /// The MMRAM regions do not cover a single contiguous span (a gap exists).
    MmramNotContiguous {
        /// Total bytes covered by the reported regions.
        covered: u64,
        /// Bytes spanned from the lowest base to the highest end.
        span: u64,
    },
    /// A memory allocation HOB describes memory that overlaps MMRAM.
    MemoryAllocationInsideMmram {
        /// The allocation's range.
        range: MemoryRange,
    },
    /// A memory allocation module HOB describes memory outside of MMRAM.
    AllocationModuleOutsideMmram {
        /// The module allocation's range.
        range: MemoryRange,
    },
    /// The Init allocation cannot be reclaimed as whole pages.
    InitModuleNotPageAligned {
        /// The Init allocation's range.
        range: MemoryRange,
    },
    /// One or more required module allocations were not described.
    MissingModule {
        /// Whether the Init module HOB was present.
        has_init: bool,
        /// Whether the Supervisor Core module HOB was present.
        has_supv_core: bool,
    },
    /// Two memory allocation module HOBs overlap each other.
    AllocationModulesOverlap {
        /// The first overlapping module.
        first: MemoryRange,
        /// The second overlapping module.
        second: MemoryRange,
    },
    /// More allocation module HOBs were supplied than can be checked.
    TooManyAllocationModules {
        /// Maximum number of allocation module HOBs accepted.
        limit: usize,
    },
    /// A memory allocation module HOB's entry point lies outside its own
    /// allocation range.
    AllocationModuleEntryPointOutOfRange {
        /// The reported entry point.
        entry_point: u64,
        /// The module allocation's range.
        range: MemoryRange,
    },
    /// Two resource descriptor HOBs of the same version and address space overlap.
    ResourceDescriptorsOverlap {
        /// The category of the overlapping descriptors (e.g. "v1 memory").
        kind: &'static str,
        /// The first overlapping descriptor.
        first: MemoryRange,
        /// The second overlapping descriptor.
        second: MemoryRange,
    },
    /// More resource descriptor HOBs were supplied than can be checked.
    TooManyResourceDescriptors {
        /// Category whose fixed-capacity storage was exhausted.
        kind: &'static str,
        /// Maximum number of resource descriptor HOBs accepted in the category.
        limit: usize,
    },
    /// A V2 resource descriptor for a memory region carries the prohibited
    /// `EFI_MEMORY_UCE` cacheability attribute.
    V2ContainsUceAttribute {
        /// Base address of the offending descriptor.
        base: u64,
        /// The reported attributes.
        attributes: u64,
    },
    /// A V2 resource descriptor for a memory region does not carry exactly one
    /// valid cacheability attribute.
    V2InvalidCacheability {
        /// Base address of the offending descriptor.
        base: u64,
        /// The reported attributes.
        attributes: u64,
    },
    /// A V2 resource descriptor for an I/O region carries non-zero attributes.
    V2IoAttributesNotZero {
        /// Base address of the offending descriptor.
        base: u64,
        /// The reported attributes.
        attributes: u64,
    },
    /// A HOB header is malformed: the list does not start with the PHIT, or a length is not a
    /// multiple of eight bytes or is too small for the HOB type.
    MalformedHob {
        /// Byte offset of the HOB from the start of the list.
        offset: usize,
        /// The HOB type from its header.
        hob_type: u16,
        /// The HOB length from its header.
        length: u16,
    },
    /// The HOB list does not reach `END_OF_HOB_LIST` within the accepted size.
    HobListTooLarge {
        /// Maximum HOB list size accepted, in bytes.
        limit: usize,
    },
    /// The HOB list itself does not lie inside MMRAM.
    HobListOutsideMmram {
        /// Address the HOB list starts at.
        base: u64,
        /// Byte length of the list, as walked from its headers.
        size: u64,
    },
    /// The active page table was not available to query page attributes.
    PageTableUnavailable,
    /// A page could not be queried in the active page table (e.g. unmapped).
    PageAttributeQueryFailed {
        /// Base address of the page whose query failed.
        addr: u64,
    },
    /// A page did not have all of the desired attributes set.
    PageMissingAttribute {
        /// Base address of the offending page.
        addr: u64,
        /// The attribute bits that were required.
        desired: u64,
        /// The attribute bits that were actually set on the page.
        found: u64,
    },
    /// A page had one of the forbidden attributes set.
    PageHasForbiddenAttribute {
        /// Base address of the offending page.
        addr: u64,
        /// The attribute bits that were forbidden.
        forbidden: u64,
        /// The attribute bits that were actually set on the page.
        found: u64,
    },
    /// A module's entry point does not lie on a present, read-only, executable page.
    EntryPointNotOnCodePage {
        /// The module entry point.
        entry_point: u64,
        /// The attribute bits of the page that holds the entry point.
        attributes: u64,
    },
}

impl core::error::Error for HobValidationError {}

impl fmt::Display for HobValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMmramRegions => write!(f, "no MMRAM regions were reported in the HOB list"),
            Self::InvalidAddressRange { kind, base, length } => {
                write!(f, "{kind} has an empty or overflowing address range [0x{base:x}, +0x{length:x})")
            }
            Self::MmramRegionsOverlap { first, second } => write!(f, "MMRAM regions overlap: {first} and {second}"),
            Self::MmramNotContiguous { covered, span } => {
                write!(f, "MMRAM regions are not contiguous: cover 0x{covered:x} of 0x{span:x} span")
            }
            Self::MemoryAllocationInsideMmram { range } => write!(f, "memory allocation HOB {range} overlaps MMRAM"),
            Self::AllocationModuleOutsideMmram { range } => {
                write!(f, "allocation module HOB {range} is outside MMRAM")
            }
            Self::InitModuleNotPageAligned { range } => write!(f, "MM Init allocation {range} is not page aligned"),
            Self::MissingModule { has_init, has_supv_core } => {
                write!(f, "required MM module missing: has_init={has_init}, has_supv_core={has_supv_core}")
            }
            Self::AllocationModulesOverlap { first, second } => {
                write!(f, "allocation module HOBs overlap: {first} and {second}")
            }
            Self::TooManyAllocationModules { limit } => {
                write!(f, "more than {limit} allocation module HOBs were supplied")
            }
            Self::AllocationModuleEntryPointOutOfRange { entry_point, range } => {
                write!(f, "allocation module entry point 0x{entry_point:x} is outside its allocation {range}")
            }
            Self::ResourceDescriptorsOverlap { kind, first, second } => {
                write!(f, "{kind} resource descriptor HOBs overlap: {first} and {second}")
            }
            Self::TooManyResourceDescriptors { kind, limit } => {
                write!(f, "more than {limit} {kind} resource descriptor HOBs were supplied")
            }
            Self::V2ContainsUceAttribute { base, attributes } => {
                write!(f, "V2 resource descriptor at 0x{base:x} carries prohibited UCE attribute (0x{attributes:x})")
            }
            Self::V2InvalidCacheability { base, attributes } => write!(
                f,
                "V2 resource descriptor at 0x{base:x} does not have exactly one cacheability attribute (0x{attributes:x})"
            ),
            Self::V2IoAttributesNotZero { base, attributes } => {
                write!(f, "V2 I/O resource descriptor at 0x{base:x} has non-zero attributes (0x{attributes:x})")
            }
            Self::MalformedHob { offset, hob_type, length } => {
                write!(f, "HOB at offset 0x{offset:x} (type 0x{hob_type:x}, length 0x{length:x}) is malformed")
            }
            Self::HobListTooLarge { limit } => {
                write!(f, "HOB list does not end within 0x{limit:x} bytes")
            }
            Self::HobListOutsideMmram { base, size } => {
                write!(f, "HOB list at 0x{base:x} (size 0x{size:x}) is not inside MMRAM")
            }
            Self::PageTableUnavailable => write!(f, "active page table is not available to query page attributes"),
            Self::PageAttributeQueryFailed { addr } => {
                write!(f, "failed to query page attributes at 0x{addr:x}")
            }
            Self::PageMissingAttribute { addr, desired, found } => write!(
                f,
                "page at 0x{addr:x} is missing required attributes (desired 0x{desired:x}, found 0x{found:x})"
            ),
            Self::PageHasForbiddenAttribute { addr, forbidden, found } => write!(
                f,
                "page at 0x{addr:x} has forbidden attributes set (forbidden 0x{forbidden:x}, found 0x{found:x})"
            ),
            Self::EntryPointNotOnCodePage { entry_point, attributes } => write!(
                f,
                "module entry point 0x{entry_point:x} is not on a present, read-only, executable page \
                 (attributes 0x{attributes:x})"
            ),
        }
    }
}

/// Returns `[base, base + length)` as a range, or reports it as an invalid address range when it
/// is empty or runs past the end of the address space.
fn checked_range(kind: &'static str, base: u64, length: u64) -> MmSupervisorResult<MemoryRange> {
    MemoryRange::new(base, length).map_err(|_| HobValidationError::InvalidAddressRange { kind, base, length }.into())
}

/// Validates that the scanned MMRAM regions form a single contiguous, non-overlapping span and
/// returns that span. The regions are not required to be sorted.
fn validate_mmram_contiguous(regions: &[MmramRegion]) -> MmSupervisorResult<MemoryRange> {
    let ranges = regions.iter().map(|region| region.range);
    let span = MemoryRange::collapse(ranges.clone()).map_err(|_| HobValidationError::NoMmramRegions)?;

    if let Some((first, second)) = MemoryRange::find_overlap(ranges.clone()) {
        return Err(HobValidationError::MmramRegionsOverlap { first, second }.into());
    }

    if MemoryRange::coalesce(ranges).is_err() {
        // The regions are disjoint, so their sizes sum to less than the span they cover.
        let covered: u64 = regions.iter().map(|region| region.range.size()).sum();
        return Err(HobValidationError::MmramNotContiguous { covered, span: span.size() }.into());
    }

    Ok(span)
}

/// Validates the `PassDown` HOB revision and that every pointer it reports references memory
/// inside the MMRAM span.
///
/// Every pointer is required. A null one needs no separate check: MMRAM never covers address 0,
/// so the containment check rejects it and names the field.
fn validate_pass_down_pointers(pass_down: &MmSupvPassDownHobData, mmram: MemoryRange) -> MmSupervisorResult<()> {
    if pass_down.revision != MM_SUPV_PASS_DOWN_HOB_REVISION {
        return Err(PassDownHobError::InvalidRevision {
            found: pass_down.revision,
            expected: MM_SUPV_PASS_DOWN_HOB_REVISION,
        }
        .into());
    }

    let pointer = |name: &'static str, base: u64, size: u64| checked_range(name, base, size).map(|range| (name, range));
    let checks: [(&'static str, MemoryRange); 4] = [
        pointer("cpl3_stack_base", pass_down.cpl3_stack_base, pass_down.cpl3_stack_size.max(1))?,
        pointer("sm_base_array", pass_down.sm_base_array, core::mem::size_of::<u64>() as u64)?,
        pointer("mm_initialized_buffer", pass_down.mm_initialized_buffer, 1)?,
        pointer(
            "firmware_policy_buffer",
            pass_down.firmware_policy_buffer,
            pass_down.firmware_policy_buffer_size.max(1),
        )?,
    ];

    for (field, range) in checks {
        if !mmram.contains_range(&range) {
            return Err(PassDownHobError::PointerOutsideMmram { field, range }.into());
        }
    }

    Ok(())
}

/// Returns the smallest length a HOB of `hob_type` can have.
///
/// This is the size of the structure [`Hob`] iteration reads for the type, so a list that passes
/// [`validate_hob_list_structure`] is never read past a HOB's own length.
fn min_hob_length(hob_type: u16) -> usize {
    match hob_type {
        HANDOFF => size_of::<PhaseHandoffInformationTable>(),
        MEMORY_ALLOCATION => size_of::<MemoryAllocation>(),
        RESOURCE_DESCRIPTOR => size_of::<ResourceDescriptor>(),
        GUID_EXTENSION => size_of::<GuidHob>(),
        FV => size_of::<FirmwareVolume>(),
        FV2 => size_of::<FirmwareVolume2>(),
        FV3 => size_of::<FirmwareVolume3>(),
        CPU => size_of::<Cpu>(),
        UEFI_CAPSULE => size_of::<Capsule>(),
        RESOURCE_DESCRIPTOR2 => size_of::<ResourceDescriptorV2>(),
        _ => size_of::<HobHeader>(),
    }
}

/// Checks the HOB headers and returns the byte length of the HOB list, including the
/// `END_OF_HOB_LIST` header.
///
/// This must run before anything else walks the list. The first HOB must be the PHIT, and every
/// length must be a multiple of eight bytes and at least the size of its HOB type. The list must
/// reach `END_OF_HOB_LIST` within [`MAX_HOB_LIST_SIZE`] bytes. A list that passes can be walked
/// with [`Hob`] iteration without looping forever or reading past its end.
///
/// ## Safety
///
/// Each header the walk reaches must be readable. At most [`MAX_HOB_LIST_SIZE`] bytes from
/// `handoff` are read.
pub(crate) unsafe fn validate_hob_list_structure(handoff: &PhaseHandoffInformationTable) -> MmSupervisorResult<u64> {
    let base = core::ptr::from_ref(handoff).cast::<HobHeader>();
    let limit = MAX_HOB_LIST_SIZE;
    checked_range("HOB list", base.addr() as u64, limit as u64)?;

    let mut offset = 0usize;
    loop {
        if limit - offset < size_of::<HobHeader>() {
            return Err(HobValidationError::HobListTooLarge { limit }.into());
        }
        // SAFETY: the header lies within `limit` bytes of `handoff`, which the caller guarantees
        // is readable. `handoff` is 8-byte aligned and every earlier length was a multiple of
        // eight, so the header is aligned.
        let header = unsafe { base.byte_add(offset).read() };
        let length = usize::from(header.length);
        if (offset == 0 && header.r#type != HANDOFF)
            || !length.is_multiple_of(size_of::<u64>())
            || length < min_hob_length(header.r#type)
        {
            return Err(
                HobValidationError::MalformedHob { offset, hob_type: header.r#type, length: header.length }.into()
            );
        }
        if limit - offset < length {
            return Err(HobValidationError::HobListTooLarge { limit }.into());
        }
        offset += length;
        if header.r#type == END_OF_HOB_LIST {
            return Ok(offset as u64);
        }
    }
}

/// Validates that the HOB list itself lies inside MMRAM.
///
/// Every other check here reasons about regions the HOB list *describes*. The
/// list is itself a buffer at an address the producer chose, and the supervisor
/// later maps it readable to Ring 3, so it has to be inside MMRAM for the same
/// reason its contents do.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
fn validate_hob_list_inside_mmram(
    handoff: &PhaseHandoffInformationTable,
    length: u64,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    let base = core::ptr::from_ref(handoff) as u64;

    if !MemoryRange::new(base, length).is_ok_and(|range| mmram.contains_range(&range)) {
        return Err(HobValidationError::HobListOutsideMmram { base, size: length }.into());
    }

    Ok(())
}

/// Validates that no memory allocation HOB overlaps MMRAM.
///
/// Plain `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOBs describe allocations outside of
/// MMRAM, so any that overlaps the MMRAM span is rejected. `mmram` is the single
/// contiguous span validated by [`validate_mmram_contiguous`].
fn validate_memory_allocations<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    for current in hobs {
        if let Hob::MemoryAllocation(alloc) = current {
            let base = alloc.alloc_descriptor.memory_base_address;
            let length = alloc.alloc_descriptor.memory_length;
            let range = checked_range("memory allocation HOB", base, length)?;
            if range.overlaps(&mmram) {
                return Err(HobValidationError::MemoryAllocationInsideMmram { range }.into());
            }
        }
    }
    Ok(())
}

/// Validates supervisor module allocations, including the standard PI Init allocation.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
fn validate_allocation_modules<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    let mut modules: [Option<MemoryRange>; MAX_ALLOC_MODULES] = [None; MAX_ALLOC_MODULES];
    let mut count = 0usize;
    let mut has_init = false;
    let mut has_supv_core = false;

    for current in hobs {
        if let Hob::MemoryAllocationModule(module) = current {
            let is_init = module.alloc_descriptor.name == HOB_MEMORY_ALLOC_MODULE_GUID
                && module.module_name == MM_SUPERVISOR_INIT_GUID;
            if module.alloc_descriptor.name != MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID && !is_init {
                continue;
            }
            has_init |= is_init;
            has_supv_core |= module.alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID
                && module.module_name == MM_SUPERVISOR_CORE_GUID;
            let base = module.alloc_descriptor.memory_base_address;
            let length = module.alloc_descriptor.memory_length;
            let range = checked_range("allocation module HOB", base, length)?;
            if base == 0 {
                return Err(
                    HobValidationError::InvalidAddressRange { kind: "allocation module HOB", base, length }.into()
                );
            }
            if is_init && !matches!(range.is_aligned(UEFI_PAGE_SIZE as u64), Ok(true)) {
                return Err(HobValidationError::InitModuleNotPageAligned { range }.into());
            }
            if !mmram.contains_range(&range) {
                return Err(HobValidationError::AllocationModuleOutsideMmram { range }.into());
            }
            let entry_point = module.entry_point;
            if !range.contains(entry_point) {
                return Err(HobValidationError::AllocationModuleEntryPointOutOfRange { entry_point, range }.into());
            }
            let slot = modules
                .get_mut(count)
                .ok_or(HobValidationError::TooManyAllocationModules { limit: MAX_ALLOC_MODULES })?;
            *slot = Some(range);
            count += 1;
        }
    }

    if !has_init || !has_supv_core {
        return Err(HobValidationError::MissingModule { has_init, has_supv_core }.into());
    }

    if let Some((first, second)) = MemoryRange::find_overlap(modules.iter().flatten()) {
        return Err(HobValidationError::AllocationModulesOverlap { first, second }.into());
    }

    Ok(())
}

/// Returns whether a resource type describes an I/O (rather than memory) region.
///
/// I/O and memory resources occupy distinct address spaces, so overlaps are only
/// meaningful within the same space.
fn is_io(resource_type: u32) -> bool {
    resource_type == EFI_RESOURCE_IO || resource_type == EFI_RESOURCE_IO_RESERVED
}

/// Returns whether a resource descriptor is owned by the Memory Type Information
/// GUID, and therefore describes PEI memory bin ranges that are expected to
/// overlap the system-memory resource descriptors backing them.
fn is_memory_type_info(resource: &ResourceDescriptor) -> bool {
    resource.owner == MEMORY_TYPE_INFO_HOB_GUID
}

/// Reports the first overlapping pair within a single resource descriptor
/// category as a [`HobValidationError::ResourceDescriptorsOverlap`].
fn check_category_overlap(ranges: &[Option<MemoryRange>], kind: &'static str) -> MmSupervisorResult<()> {
    if let Some((first, second)) = MemoryRange::find_overlap(ranges.iter().flatten()) {
        return Err(HobValidationError::ResourceDescriptorsOverlap { kind, first, second }.into());
    }
    Ok(())
}

/// Appends `range` to `ranges`, updating `count`.
fn push_range(
    ranges: &mut [Option<MemoryRange>],
    count: &mut usize,
    range: MemoryRange,
    kind: &'static str,
) -> MmSupervisorResult<()> {
    let limit = ranges.len();
    let slot = ranges.get_mut(*count).ok_or(HobValidationError::TooManyResourceDescriptors { kind, limit })?;
    *slot = Some(range);
    *count += 1;
    Ok(())
}

/// Validates that resource descriptor HOBs do not overlap within the same
/// version (v1/v2) and address space (memory/I/O).
///
/// V1 and V2 descriptors are expected to cover the same ranges (V2 is a superset
/// of V1), so overlaps are only flagged within each category, not across them.
///
/// V1 descriptors are only checked with the `v1_resource_descriptor_support`
/// feature. Without it they are ignored, as in `patina_dxe_core`.
fn validate_resource_descriptor_overlaps<'a>(hobs: impl IntoIterator<Item = Hob<'a>>) -> MmSupervisorResult<()> {
    #[cfg(feature = "v1_resource_descriptor_support")]
    let (mut v1_mem, mut v1_io) = ([None; MAX_RESOURCE_HOBS], [None; MAX_RESOURCE_HOBS]);
    #[cfg(feature = "v1_resource_descriptor_support")]
    let (mut v1_mem_n, mut v1_io_n) = (0usize, 0usize);
    let mut v2_mem = [None; MAX_RESOURCE_HOBS];
    let mut v2_io = [None; MAX_RESOURCE_HOBS];
    let (mut v2_mem_n, mut v2_io_n) = (0usize, 0usize);

    for current in hobs {
        match current {
            #[cfg(feature = "v1_resource_descriptor_support")]
            Hob::ResourceDescriptor(rd) => {
                // The PEI memory bin (Memory Type Info) HOB is expected to overlap the
                // system-memory resource descriptors backing it, so skip it.
                if is_memory_type_info(rd) {
                    continue;
                }
                let (base, length) = (rd.physical_start, rd.resource_length);
                if is_io(rd.resource_type) {
                    let range = checked_range("v1 I/O resource descriptor HOB", base, length)?;
                    push_range(&mut v1_io, &mut v1_io_n, range, "v1 I/O")?;
                } else {
                    let range = checked_range("v1 memory resource descriptor HOB", base, length)?;
                    push_range(&mut v1_mem, &mut v1_mem_n, range, "v1 memory")?;
                }
            }
            Hob::ResourceDescriptorV2(rd) => {
                if is_memory_type_info(&rd.v1) {
                    continue;
                }
                let (base, length) = (rd.v1.physical_start, rd.v1.resource_length);
                if is_io(rd.v1.resource_type) {
                    let range = checked_range("v2 I/O resource descriptor HOB", base, length)?;
                    push_range(&mut v2_io, &mut v2_io_n, range, "v2 I/O")?;
                } else {
                    let range = checked_range("v2 memory resource descriptor HOB", base, length)?;
                    push_range(&mut v2_mem, &mut v2_mem_n, range, "v2 memory")?;
                }
            }
            _ => {}
        }
    }

    #[cfg(feature = "v1_resource_descriptor_support")]
    {
        check_category_overlap(&v1_mem, "v1 memory")?;
        check_category_overlap(&v1_io, "v1 I/O")?;
    }
    check_category_overlap(&v2_mem, "v2 memory")?;
    check_category_overlap(&v2_io, "v2 I/O")?;

    Ok(())
}

/// Validates the extended attributes of a single V2 resource descriptor.
///
/// Memory regions must carry exactly one cacheability attribute and must not
/// carry the prohibited `EFI_MEMORY_UCE` bit; I/O regions must carry no
/// attributes at all.
fn check_v2_attributes(resource_type: u32, base: u64, attributes: u64) -> MmSupervisorResult<()> {
    if is_io(resource_type) {
        if attributes != 0 {
            return Err(HobValidationError::V2IoAttributesNotZero { base, attributes }.into());
        }
        return Ok(());
    }

    if attributes & efi::MEMORY_UCE != 0 {
        return Err(HobValidationError::V2ContainsUceAttribute { base, attributes }.into());
    }

    // Exactly one cacheability bit (excluding the prohibited UCE) must be set.
    let cache_bits = attributes & (efi::CACHE_ATTRIBUTE_MASK & !efi::MEMORY_UCE);
    if cache_bits == 0 || (cache_bits & (cache_bits - 1)) != 0 {
        return Err(HobValidationError::V2InvalidCacheability { base, attributes }.into());
    }

    Ok(())
}

/// Validates the extended memory attributes carried by every V2 resource
/// descriptor HOB.
fn validate_resource_v2_memory_attributes<'a>(hobs: impl IntoIterator<Item = Hob<'a>>) -> MmSupervisorResult<()> {
    for current in hobs {
        if let Hob::ResourceDescriptorV2(rd) = current {
            check_v2_attributes(rd.v1.resource_type, rd.v1.physical_start, rd.attributes)?;
        }
    }
    Ok(())
}

/// Logs every HOB in the list, one line each, to aid debugging.
fn dump_hobs(handoff: &PhaseHandoffInformationTable) {
    log::debug!("---- Incoming HOB dump ----");
    let hob = Hob::Handoff(handoff);
    for current in &hob {
        log::debug!("HOB {current}");
    }
    log::debug!("---- End HOB dump ----");
}

/// Logs every scanned MMRAM region to aid debugging.
fn dump_regions(regions: &[MmramRegion]) {
    log::debug!("---- Scanned MMRAM regions ({}) ----", regions.len());
    for (index, region) in regions.iter().enumerate() {
        log::debug!(
            "Region {index}: {}, size=0x{:x}, pre_allocated={}",
            region.range,
            region.range.size(),
            region.pre_allocated
        );
    }
    log::debug!("---- End MMRAM region dump ----");
}

/// Validates the `PassDown` HOB present in the HOB list.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
fn validate_pass_down<'a>(hobs: impl IntoIterator<Item = Hob<'a>>, mmram: MemoryRange) -> MmSupervisorResult<()> {
    let data = hobs
        .into_iter()
        .find_map(|hob| match hob {
            Hob::GuidHob(guid_hob, data) if guid_hob.name == MM_SUPV_PASS_DOWN_HOB_GUID => Some(data),
            _ => None,
        })
        .ok_or(PassDownHobError::Missing)?;
    let (pass_down, _) = MmSupvPassDownHobData::read_from_prefix(data).map_err(|_| PassDownHobError::TooSmall)?;
    validate_pass_down_pointers(&pass_down, mmram)
}

/// Validates the critical incoming HOBs before their contents are consumed.
///
/// `hob_list_len` is the length returned by [`validate_hob_list_structure`], which must already
/// have accepted the list. `regions` are the MMRAM regions scanned from the HOB list. They are
/// checked to form one contiguous span, and every containment check runs against that span
/// rather than the page allocator, so this can run before the allocator has committed to the
/// producer's descriptors.
pub(crate) fn validate_incoming_hobs_pre_paging_init(
    handoff: &PhaseHandoffInformationTable,
    hob_list_len: u64,
    regions: &[MmramRegion],
) -> MmSupervisorResult<()> {
    // The regions were already copied out of the HOB list into a fixed array, so printing them
    // reads nothing beyond that array.
    dump_regions(regions);

    let mmram = validate_mmram_contiguous(regions)?;
    // The HOB list is itself a buffer at an address the producer chose, so it is checked before
    // anything it describes is trusted. Its length is walked from the headers rather than read
    // from the PHIT, which platforms do not reliably populate for the MM HOB list.
    validate_hob_list_inside_mmram(handoff, hob_list_len, mmram)?;
    // Runs before the remaining checks so the whole list is still visible when one of them fails.
    dump_hobs(handoff);
    // MMRAM is a single contiguous span (checked above), so every containment check below
    // tests against that one range instead of iterating the region list.
    let hob = Hob::Handoff(handoff);
    validate_memory_allocations(&hob, mmram)?;
    validate_allocation_modules(&hob, mmram)?;
    validate_resource_descriptor_overlaps(&hob)?;
    validate_resource_v2_memory_attributes(&hob)?;
    validate_pass_down(&hob, mmram)?;

    Ok(())
}

/// Validates memory protections that can only be checked once the page table is
/// active.
///
/// Runs after the page table has been initialized (unlike [`validate_incoming_hobs_pre_paging_init`],
/// which runs before), so it can verify per-page attributes via
/// [`verify_page_attributes`].
pub(crate) fn validate_incoming_hobs_post_paging_init(
    handoff: &PhaseHandoffInformationTable,
) -> MmSupervisorResult<()> {
    let hob = Hob::Handoff(handoff);
    with_active_page_table(|page_table| verify_module_page_protections(&hob, page_table))
}

/// Verifies the page protections of the MM Supervisor Core and User modules.
///
/// Iterates the module HOBs, selecting the MM Supervisor's own
/// `MemoryAllocationModule` HOBs, dispatches on the module GUID to check
/// supervisor/user page ownership in `page_table`, and confirms each module's
/// entry point lies on a code page.
fn verify_module_page_protections<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    page_table: &impl PageTable,
) -> MmSupervisorResult<()> {
    for current in hobs {
        let Hob::MemoryAllocationModule(module) = current else {
            continue;
        };
        if module.alloc_descriptor.name != MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID {
            continue;
        }

        let base = module.alloc_descriptor.memory_base_address;
        let length = module.alloc_descriptor.memory_length;
        let range = checked_range("allocation module HOB", base, length)?;
        match module.module_name {
            name if name == MM_SUPERVISOR_CORE_GUID => {
                log::info!("Verifying MM Supervisor Core module page protections: {range}");
                // Supervisor core pages must be supervisor-owned (SP set).
                verify_page_attributes(page_table, &range, MemoryAttributes::Supervisor, MemoryAttributes::empty())?;
                verify_entry_point_on_code_page(page_table, module.entry_point)?;
            }
            name if name == MM_SUPERVISOR_USER_GUID => {
                log::info!("Verifying MM Supervisor User module page protections: {range}");
                // User core pages must be user-accessible (SP clear).
                verify_page_attributes(page_table, &range, MemoryAttributes::empty(), MemoryAttributes::Supervisor)?;
                verify_entry_point_on_code_page(page_table, module.entry_point)?;
            }
            _ => {}
        }
    }

    Ok(())
}

/// Runs `check` against the active page table, or reports it as unavailable when it has not been
/// initialized yet.
fn with_active_page_table<R>(
    check: impl FnOnce(&SupervisorPageTable) -> MmSupervisorResult<R>,
) -> MmSupervisorResult<R> {
    let page_table = security_state().lock_page_table();
    check(page_table.as_ref().ok_or(HobValidationError::PageTableUnavailable)?)
}

/// Verifies that every page of the Ring 3 stack region is usable by a demoted
/// routine: user-accessible (SP clear), present, writable, and non-executable.
///
/// Unlike the checks above, this validates state the supervisor establishes
/// itself, so it runs after the stacks have been mapped rather than as part of
/// the incoming HOB validation rounds.
pub(crate) fn verify_cpl3_stacks_user_accessible(range: &MemoryRange) -> MmSupervisorResult<()> {
    with_active_page_table(|page_table| {
        verify_page_attributes(
            page_table,
            range,
            MemoryAttributes::ExecuteProtect,
            MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly | MemoryAttributes::ReadProtect,
        )
    })
}

/// Verifies that every page in `range` has all of the `required` attributes set and none of the
/// `forbidden` attributes set in `page_table`.
///
/// The range is widened to whole pages and each page is queried individually, so
/// attributes that differ across the range are detected per page rather than merged
/// into a single range query.
fn verify_page_attributes(
    page_table: &impl PageTable,
    range: &MemoryRange,
    required: MemoryAttributes,
    forbidden: MemoryAttributes,
) -> MmSupervisorResult<()> {
    let pages = range
        .align_outward(UEFI_PAGE_SIZE as u64)
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: range.base() })?;

    for addr in pages.step_by(PAGE_STEP) {
        let attrs = page_table
            .query_memory_region(addr, UEFI_PAGE_SIZE as u64)
            .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr })?;
        if !attrs.contains(required) {
            return Err(HobValidationError::PageMissingAttribute {
                addr,
                desired: required.bits(),
                found: attrs.bits(),
            }
            .into());
        }
        if attrs.intersects(forbidden) {
            return Err(HobValidationError::PageHasForbiddenAttribute {
                addr,
                forbidden: forbidden.bits(),
                found: attrs.bits(),
            }
            .into());
        }
    }

    Ok(())
}

/// Verifies that the page containing `entry_point` is a code page in
/// `page_table`, confirming the entry point lands in the module's code section.
fn verify_entry_point_on_code_page(page_table: &impl PageTable, entry_point: u64) -> MmSupervisorResult<()> {
    let page = MemoryRange::new(entry_point, 1)
        .and_then(|range| range.align_outward(UEFI_PAGE_SIZE as u64))
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: entry_point })?;

    let attrs = page_table
        .query_memory_region(page.base(), page.size())
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: page.base() })?;
    validate_entry_point_attributes(entry_point, attrs)
}

/// Validates the queried page attributes for a module entry point.
///
/// The loader maps module code present, read-only and executable, the same protection
/// `validate_init_code_page` requires of the MM Init image. A page missing any of the three is
/// not code the loader placed there.
fn validate_entry_point_attributes(entry_point: u64, attributes: MemoryAttributes) -> MmSupervisorResult<()> {
    let forbidden = MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadProtect;
    if !attributes.contains(MemoryAttributes::ReadOnly) || attributes.intersects(forbidden) {
        Err(HobValidationError::EntryPointNotOnCodePage { entry_point, attributes: attributes.bits() }.into())
    } else {
        Ok(())
    }
}
