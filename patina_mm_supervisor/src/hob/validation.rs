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
        /// Base of the first overlapping region.
        base_a: u64,
        /// Size of the first overlapping region.
        size_a: u64,
        /// Base of the second overlapping region.
        base_b: u64,
        /// Size of the second overlapping region.
        size_b: u64,
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
        /// Base address of the allocation.
        base: u64,
        /// Length of the allocation in bytes.
        length: u64,
    },
    /// A memory allocation module HOB describes memory outside of MMRAM.
    AllocationModuleOutsideMmram {
        /// Base address of the module allocation.
        base: u64,
        /// Length of the module allocation in bytes.
        length: u64,
    },
    /// The Init allocation cannot be reclaimed as whole pages.
    InitModuleNotPageAligned {
        /// Base address of the Init allocation.
        base: u64,
        /// Length of the Init allocation in bytes.
        length: u64,
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
        /// Base of the first overlapping module.
        base_a: u64,
        /// Size of the first overlapping module.
        size_a: u64,
        /// Base of the second overlapping module.
        base_b: u64,
        /// Size of the second overlapping module.
        size_b: u64,
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
        /// Base address of the module allocation.
        base: u64,
        /// Length of the module allocation in bytes.
        length: u64,
    },
    /// Two resource descriptor HOBs of the same version and address space overlap.
    ResourceDescriptorsOverlap {
        /// The category of the overlapping descriptors (e.g. "v1 memory").
        kind: &'static str,
        /// Base of the first overlapping descriptor.
        base_a: u64,
        /// Size of the first overlapping descriptor.
        size_a: u64,
        /// Base of the second overlapping descriptor.
        base_b: u64,
        /// Size of the second overlapping descriptor.
        size_b: u64,
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
            Self::MmramRegionsOverlap { base_a, size_a, base_b, size_b } => {
                write!(f, "MMRAM regions overlap: [0x{base_a:x}, +0x{size_a:x}) and [0x{base_b:x}, +0x{size_b:x})")
            }
            Self::MmramNotContiguous { covered, span } => {
                write!(f, "MMRAM regions are not contiguous: cover 0x{covered:x} of 0x{span:x} span")
            }
            Self::MemoryAllocationInsideMmram { base, length } => {
                write!(f, "memory allocation HOB [0x{base:x}, +0x{length:x}) overlaps MMRAM")
            }
            Self::AllocationModuleOutsideMmram { base, length } => {
                write!(f, "allocation module HOB [0x{base:x}, +0x{length:x}) is outside MMRAM")
            }
            Self::InitModuleNotPageAligned { base, length } => {
                write!(f, "MM Init allocation [0x{base:x}, +0x{length:x}) is not page aligned")
            }
            Self::MissingModule { has_init, has_supv_core } => {
                write!(f, "required MM module missing: has_init={has_init}, has_supv_core={has_supv_core}")
            }
            Self::AllocationModulesOverlap { base_a, size_a, base_b, size_b } => write!(
                f,
                "allocation module HOBs overlap: [0x{base_a:x}, +0x{size_a:x}) and [0x{base_b:x}, +0x{size_b:x})"
            ),
            Self::TooManyAllocationModules { limit } => {
                write!(f, "more than {limit} allocation module HOBs were supplied")
            }
            Self::AllocationModuleEntryPointOutOfRange { entry_point, base, length } => write!(
                f,
                "allocation module entry point 0x{entry_point:x} is outside its allocation [0x{base:x}, +0x{length:x})"
            ),
            Self::ResourceDescriptorsOverlap { kind, base_a, size_a, base_b, size_b } => write!(
                f,
                "{kind} resource descriptor HOBs overlap: [0x{base_a:x}, +0x{size_a:x}) and [0x{base_b:x}, +0x{size_b:x})"
            ),
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

    if let Some((a, b)) = MemoryRange::find_overlap(ranges.clone()) {
        return Err(HobValidationError::MmramRegionsOverlap {
            base_a: a.base(),
            size_a: a.size(),
            base_b: b.base(),
            size_b: b.size(),
        }
        .into());
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
            if checked_range("memory allocation HOB", base, length)?.overlaps(&mmram) {
                return Err(HobValidationError::MemoryAllocationInsideMmram { base, length }.into());
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
                return Err(HobValidationError::InitModuleNotPageAligned { base, length }.into());
            }
            if !mmram.contains_range(&range) {
                return Err(HobValidationError::AllocationModuleOutsideMmram { base, length }.into());
            }
            let entry_point = module.entry_point;
            if !range.contains(entry_point) {
                return Err(
                    HobValidationError::AllocationModuleEntryPointOutOfRange { entry_point, base, length }.into()
                );
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

    if let Some((a, b)) = MemoryRange::find_overlap(modules.iter().flatten()) {
        return Err(HobValidationError::AllocationModulesOverlap {
            base_a: a.base(),
            size_a: a.size(),
            base_b: b.base(),
            size_b: b.size(),
        }
        .into());
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
    if let Some((a, b)) = MemoryRange::find_overlap(ranges.iter().flatten()) {
        return Err(HobValidationError::ResourceDescriptorsOverlap {
            kind,
            base_a: a.base(),
            size_a: a.size(),
            base_b: b.base(),
            size_b: b.size(),
        }
        .into());
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
    for (i, r) in regions.iter().enumerate() {
        log::debug!("Region {i}: {}, size=0x{:x}, pre_allocated={}", r.range, r.range.size(), r.pre_allocated);
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
        match module.module_name {
            name if name == MM_SUPERVISOR_CORE_GUID => {
                log::info!(
                    "Verifying MM Supervisor Core module page protections: base=0x{base:x}, length=0x{length:x}"
                );
                // Supervisor core pages must be supervisor-owned (SP set).
                verify_page_attributes(
                    page_table,
                    base,
                    length,
                    MemoryAttributes::Supervisor,
                    MemoryAttributes::empty(),
                )?;
                verify_entry_point_on_code_page(page_table, module.entry_point)?;
            }
            name if name == MM_SUPERVISOR_USER_GUID => {
                log::info!(
                    "Verifying MM Supervisor User module page protections: base=0x{base:x}, length=0x{length:x}"
                );
                // User core pages must be user-accessible (SP clear).
                verify_page_attributes(
                    page_table,
                    base,
                    length,
                    MemoryAttributes::empty(),
                    MemoryAttributes::Supervisor,
                )?;
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
pub(crate) fn verify_cpl3_stacks_user_accessible(base: u64, length: u64) -> MmSupervisorResult<()> {
    with_active_page_table(|page_table| {
        verify_page_attributes(
            page_table,
            base,
            length,
            MemoryAttributes::ExecuteProtect,
            MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly | MemoryAttributes::ReadProtect,
        )
    })
}

/// Verifies that every page in `[base, base + length)` has all of the `required`
/// attributes set and none of the `forbidden` attributes set in `page_table`.
///
/// The range is widened to whole pages and each page is queried individually, so
/// attributes that differ across the range are detected per page rather than merged
/// into a single range query.
fn verify_page_attributes(
    page_table: &impl PageTable,
    base: u64,
    length: u64,
    required: MemoryAttributes,
    forbidden: MemoryAttributes,
) -> MmSupervisorResult<()> {
    let pages = MemoryRange::new(base, length)
        .and_then(|range| range.align_outward(UEFI_PAGE_SIZE as u64))
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: base })?;

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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::error::MmSupervisorError;
    use crate::hob::pass_down::{MM_SUPV_PASS_DOWN_HOB_GUID, MM_SUPV_PASS_DOWN_HOB_REVISION};
    use core::mem::size_of;
    use patina_paging::{MockPageTable, PtError};

    use crate::test_support::{PageAlignedMemory, init_global_state_over};

    use patina::pi::BootMode;
    use patina::pi::hob::{
        EFI_RESOURCE_SYSTEM_MEMORY, END_OF_HOB_LIST, GUID_EXTENSION, GuidHob, HANDOFF, HobHeader, MEMORY_ALLOCATION,
        MEMORY_POOL, MemoryAllocation, MemoryAllocationHeader, MemoryAllocationModule, RESOURCE_DESCRIPTOR,
        ResourceDescriptorV2, UNUSED,
    };

    fn region(base: u64, size: u64) -> MmramRegion {
        MmramRegion::new(MemoryRange::new(base, size).unwrap(), false)
    }

    /// Returns `[base, base + size)` as a range.
    fn span(base: u64, size: u64) -> MemoryRange {
        MemoryRange::new(base, size).unwrap()
    }

    /// MMRAM span covering `[0x1000, 0x10_1000)`, which holds every address the HOB fixtures name.
    fn mmram() -> MemoryRange {
        span(0x1000, 0x10_0000)
    }

    fn pass_down() -> MmSupvPassDownHobData {
        MmSupvPassDownHobData {
            revision: MM_SUPV_PASS_DOWN_HOB_REVISION,
            reserved: 0,
            cpl3_stack_base: 0x1000,
            cpl3_stack_size: 0x1000,
            sm_base_array: 0x2000,
            mm_initialized_buffer: 0x3000,
            firmware_policy_buffer: 0x4000,
            firmware_policy_buffer_size: 0x1000,
            mmi_entry_size: 0x100,
        }
    }

    fn memory_allocation(base: u64, length: u64) -> MemoryAllocation {
        MemoryAllocation {
            header: HobHeader { r#type: MEMORY_ALLOCATION, length: size_of::<MemoryAllocation>() as u16, reserved: 0 },
            alloc_descriptor: MemoryAllocationHeader {
                name: MM_SUPERVISOR_CORE_GUID,
                memory_base_address: base,
                memory_length: length,
                memory_type: efi::BOOT_SERVICES_CODE,
                reserved: [0; 4],
            },
        }
    }

    fn allocation_module(
        allocation_name: patina::BinaryGuid,
        module_name: patina::BinaryGuid,
        base: u64,
        length: u64,
        entry_point: u64,
    ) -> MemoryAllocationModule {
        MemoryAllocationModule {
            header: HobHeader {
                r#type: MEMORY_ALLOCATION,
                length: size_of::<MemoryAllocationModule>() as u16,
                reserved: 0,
            },
            alloc_descriptor: MemoryAllocationHeader {
                name: allocation_name,
                memory_base_address: base,
                memory_length: length,
                memory_type: efi::BOOT_SERVICES_CODE,
                reserved: [0; 4],
            },
            module_name,
            entry_point,
        }
    }

    fn resource_descriptor(
        owner: patina::BinaryGuid,
        resource_type: u32,
        base: u64,
        length: u64,
    ) -> ResourceDescriptor {
        ResourceDescriptor {
            header: HobHeader {
                r#type: RESOURCE_DESCRIPTOR,
                length: size_of::<ResourceDescriptor>() as u16,
                reserved: 0,
            },
            owner,
            resource_type,
            resource_attribute: 0,
            physical_start: base,
            resource_length: length,
        }
    }

    fn resource_descriptor_v2(resource_type: u32, base: u64, length: u64, attributes: u64) -> ResourceDescriptorV2 {
        let mut descriptor =
            ResourceDescriptorV2::from(resource_descriptor(MM_SUPERVISOR_CORE_GUID, resource_type, base, length));
        descriptor.attributes = attributes;
        descriptor
    }

    fn guid_hob(name: patina::BinaryGuid, data_len: usize) -> GuidHob {
        GuidHob {
            header: HobHeader { r#type: GUID_EXTENSION, length: (size_of::<GuidHob>() + data_len) as u16, reserved: 0 },
            name,
        }
    }

    fn pass_down_bytes(pass_down: &MmSupvPassDownHobData) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(size_of::<MmSupvPassDownHobData>());
        bytes.extend_from_slice(&pass_down.revision.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.reserved.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.cpl3_stack_base.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.cpl3_stack_size.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.sm_base_array.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.mm_initialized_buffer.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.firmware_policy_buffer.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.firmware_policy_buffer_size.to_ne_bytes());
        bytes.extend_from_slice(&pass_down.mmi_entry_size.to_ne_bytes());
        bytes
    }

    #[test]
    fn test_mm_supervisor_hob_validation_dump_hobs() {
        /// A HOB list with a GUID HOB whose payload must not reach the log.
        #[repr(C, align(8))]
        struct DumpHobList {
            handoff: PhaseHandoffInformationTable,
            guid: GuidHob,
            data: [u8; 8],
            end: HobHeader,
        }

        // log::debug! only formats its arguments when a logger is installed, so the silent test
        // logger makes the dump walk the list and format each HOB.
        crate::test_support::init_test_logger();

        let list = DumpHobList {
            handoff: PhaseHandoffInformationTable {
                header: HobHeader {
                    r#type: HANDOFF,
                    length: size_of::<PhaseHandoffInformationTable>() as u16,
                    reserved: 0,
                },
                version: 1,
                boot_mode: BootMode::BootWithFullConfiguration,
                memory_top: 0x9000,
                memory_bottom: 0x1000,
                free_memory_top: 0x8000,
                free_memory_bottom: 0x2000,
                end_of_hob_list: 0,
            },
            guid: guid_hob(MM_SUPERVISOR_CORE_GUID, 8),
            data: [0xa5; 8],
            end: HobHeader { r#type: END_OF_HOB_LIST, length: size_of::<HobHeader>() as u16, reserved: 0 },
        };

        dump_hobs(&list.handoff);
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pre_paging_accepts_a_complete_hob_list() {
        /// A HOB list with Init, Core, and the required `PassDown` HOB.
        #[repr(C, align(8))]
        struct PassDownHobList {
            handoff: PhaseHandoffInformationTable,
            core: MemoryAllocationModule,
            init: MemoryAllocationModule,
            guid: GuidHob,
            pass_down: MmSupvPassDownHobData,
            end: HobHeader,
        }

        let mut list = PassDownHobList {
            handoff: PhaseHandoffInformationTable {
                header: HobHeader {
                    r#type: HANDOFF,
                    length: size_of::<PhaseHandoffInformationTable>() as u16,
                    reserved: 0,
                },
                version: 1,
                boot_mode: BootMode::BootWithFullConfiguration,
                memory_top: 0x9000,
                memory_bottom: 0x1000,
                free_memory_top: 0x8000,
                free_memory_bottom: 0x2000,
                end_of_hob_list: 0,
            },
            core: allocation_module(
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_CORE_GUID,
                0x1000,
                0x1000,
                0x1000,
            ),
            init: allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x2000, 0x2000, 0x2100),
            guid: guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, size_of::<MmSupvPassDownHobData>()),
            pass_down: pass_down(),
            end: HobHeader { r#type: END_OF_HOB_LIST, length: size_of::<HobHeader>() as u16, reserved: 0 },
        };
        let list_end = (&raw const list).addr() as u64 + size_of::<PassDownHobList>() as u64;
        // One region from the lowest address the producer named up to the end of the list on the
        // stack, so the modules, the PassDown pointers and the list itself are all inside it.
        let regions = [region(0x1000, list_end - 0x1000)];

        // Everything the producer named resolves inside the scanned regions, so the whole
        // pre-paging sequence runs to completion.
        // SAFETY: `list` is a contiguous, END_OF_HOB_LIST-terminated HOB list on the stack.
        let hob_list_len = unsafe { validate_hob_list_structure(&list.handoff) };
        assert_eq!(hob_list_len, Ok(size_of::<PassDownHobList>() as u64));
        let hob_list_len = hob_list_len.expect("the list is well formed");
        assert_eq!(validate_incoming_hobs_pre_paging_init(&list.handoff, hob_list_len, &regions), Ok(()));
        list.init.alloc_descriptor.memory_base_address = 0x1000;
        assert!(matches!(
            validate_incoming_hobs_pre_paging_init(&list.handoff, hob_list_len, &regions),
            Err(MmSupervisorError::HobValidation(HobValidationError::AllocationModulesOverlap { .. }))
        ));

        // The HOB list itself is a buffer at an address the producer chose, so it is rejected
        // when it falls outside the regions it described.
        assert!(matches!(
            validate_incoming_hobs_pre_paging_init(&list.handoff, hob_list_len, &[region(0x1000, 0x8000)]),
            Err(MmSupervisorError::HobValidation(HobValidationError::HobListOutsideMmram { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_dump_regions() {
        // log::debug! skips its arguments when no logger is installed. The silent test logger
        // makes each region format, including one that ends at the top of the address space.
        crate::test_support::init_test_logger();

        dump_regions(&[]);
        dump_regions(&[
            MmramRegion::new(MemoryRange::new(0x1000, 0x1000).unwrap(), false),
            MmramRegion::new(MemoryRange::new(0x2000, 0x2000).unwrap(), true),
        ]);
        dump_regions(&[MmramRegion::new(MemoryRange::new(u64::MAX - 0x1000, 0x1000).unwrap(), false)]);
    }

    #[test]
    fn test_mm_supervisor_hob_validation_checked_range() {
        assert_eq!(checked_range("test", 0x1000, 0x1000), Ok(span(0x1000, 0x1000)));
        assert!(matches!(
            checked_range("test", 0x1000, 0),
            Err(MmSupervisorError::HobValidation(HobValidationError::InvalidAddressRange {
                kind: "test",
                base: 0x1000,
                length: 0
            }))
        ));
        assert!(matches!(
            checked_range("test", u64::MAX, 1),
            Err(MmSupervisorError::HobValidation(HobValidationError::InvalidAddressRange {
                kind: "test",
                base: u64::MAX,
                length: 1
            }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_contiguous_single_region() {
        // A single region maps to itself.
        assert_eq!(validate_mmram_contiguous(&[region(0x1000, 0x2000)]), Ok(span(0x1000, 0x2000)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_contiguous_adjacent_unsorted() {
        // Two adjacent regions provided out of order collapse to one [min_base, max_end) span.
        let regions = [region(0x3000, 0x1000), region(0x1000, 0x2000)];
        assert_eq!(validate_mmram_contiguous(&regions), Ok(span(0x1000, 0x3000)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_empty_regions_rejected() {
        assert_eq!(validate_mmram_contiguous(&[]), Err(HobValidationError::NoMmramRegions.into()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_overlapping_regions_rejected() {
        let regions = [region(0x1000, 0x2000), region(0x2000, 0x2000)];
        assert!(matches!(
            validate_mmram_contiguous(&regions),
            Err(MmSupervisorError::HobValidation(HobValidationError::MmramRegionsOverlap { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_gap_rejected() {
        // Gap between 0x2000 and 0x3000.
        let regions = [region(0x1000, 0x1000), region(0x3000, 0x1000)];
        assert!(matches!(
            validate_mmram_contiguous(&regions),
            Err(MmSupervisorError::HobValidation(HobValidationError::MmramNotContiguous { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_memory_allocations() {
        let outside = memory_allocation(0x3000, 0x1000);
        assert_eq!(validate_memory_allocations([Hob::MemoryAllocation(&outside)], span(0x1000, 0x1000)), Ok(()));

        let inside = memory_allocation(0x1800, 0x1000);
        assert_eq!(
            validate_memory_allocations([Hob::MemoryAllocation(&inside)], span(0x1000, 0x1000)),
            Err(HobValidationError::MemoryAllocationInsideMmram { base: 0x1800, length: 0x1000 }.into())
        );

        let invalid = memory_allocation(u64::MAX, 1);
        assert!(matches!(
            validate_memory_allocations([Hob::MemoryAllocation(&invalid)], span(0x1000, 0x1000)),
            Err(MmSupervisorError::HobValidation(HobValidationError::InvalidAddressRange {
                kind: "memory allocation HOB",
                ..
            }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_allocation_modules() {
        let first = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1000,
        );
        let second = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x2000,
            0x1000,
            0x2800,
        );
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x3000, 0x1000, 0x3000);
        assert_eq!(
            validate_allocation_modules(
                [
                    Hob::MemoryAllocationModule(&first),
                    Hob::MemoryAllocationModule(&second),
                    Hob::MemoryAllocationModule(&init)
                ],
                span(0x1000, 0x3000)
            ),
            Ok(())
        );

        let outside = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x4000,
            0x1000,
            0x4000,
        );
        assert_eq!(
            validate_allocation_modules([Hob::MemoryAllocationModule(&outside)], span(0x1000, 0x3000)),
            Err(HobValidationError::AllocationModuleOutsideMmram { base: 0x4000, length: 0x1000 }.into())
        );

        let bad_entry = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x2000,
        );
        assert_eq!(
            validate_allocation_modules([Hob::MemoryAllocationModule(&bad_entry)], mmram()),
            Err(HobValidationError::AllocationModuleEntryPointOutOfRange {
                entry_point: 0x2000,
                base: 0x1000,
                length: 0x1000,
            }
            .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_allocation_module_overlap_and_limit() {
        let first = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x2000,
            0x1000,
        );
        let second = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x2000,
            0x1000,
            0x2000,
        );
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x4000, 0x1000, 0x4000);
        assert!(matches!(
            validate_allocation_modules(
                [
                    Hob::MemoryAllocationModule(&first),
                    Hob::MemoryAllocationModule(&second),
                    Hob::MemoryAllocationModule(&init)
                ],
                mmram()
            ),
            Err(MmSupervisorError::HobValidation(HobValidationError::AllocationModulesOverlap { .. }))
        ));

        let module = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1000,
        );
        let hobs = (0..=MAX_ALLOC_MODULES).map(|_| Hob::MemoryAllocationModule(&module));
        assert_eq!(
            validate_allocation_modules(hobs, mmram()),
            Err(HobValidationError::TooManyAllocationModules { limit: MAX_ALLOC_MODULES }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_ignores_unrelated_allocation_modules() {
        let module = allocation_module(MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_CORE_GUID, u64::MAX, 1, u64::MAX);
        let core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1000,
        );
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x2000, 0x1000, 0x2000);
        assert_eq!(
            validate_allocation_modules(
                [
                    Hob::MemoryAllocationModule(&module),
                    Hob::MemoryAllocationModule(&core),
                    Hob::MemoryAllocationModule(&init)
                ],
                mmram(),
            ),
            Ok(())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_requires_init_module() {
        let core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1000,
        );
        let wrong_allocation = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_INIT_GUID,
            0x2000,
            0x1000,
            0x2000,
        );
        for (hobs, has_supv_core) in [
            (vec![], false),
            (vec![Hob::MemoryAllocationModule(&core)], true),
            (vec![Hob::MemoryAllocationModule(&core), Hob::MemoryAllocationModule(&wrong_allocation)], true),
        ] {
            assert_eq!(
                validate_allocation_modules(hobs, mmram()),
                Err(HobValidationError::MissingModule { has_init: false, has_supv_core }.into())
            );
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_init_allocation_ranges() {
        let core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1000,
        );
        for (base, length, expected) in [
            (
                0x4000,
                0,
                HobValidationError::InvalidAddressRange { kind: "allocation module HOB", base: 0x4000, length: 0 },
            ),
            (0x4001, 0x2000, HobValidationError::InitModuleNotPageAligned { base: 0x4001, length: 0x2000 }),
            (0x4000, 0x2001, HobValidationError::InitModuleNotPageAligned { base: 0x4000, length: 0x2001 }),
            (
                0,
                0x2000,
                HobValidationError::InvalidAddressRange { kind: "allocation module HOB", base: 0, length: 0x2000 },
            ),
            (
                u64::MAX - 0xfff,
                0x1000,
                HobValidationError::InvalidAddressRange {
                    kind: "allocation module HOB",
                    base: u64::MAX - 0xfff,
                    length: 0x1000,
                },
            ),
        ] {
            let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, base, length, base);
            assert_eq!(
                validate_allocation_modules(
                    [Hob::MemoryAllocationModule(&init), Hob::MemoryAllocationModule(&core)],
                    span(0x1000, 0x8000),
                ),
                Err(expected.into())
            );
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_init_requires_core_and_rejects_overlap() {
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x4000, 0x2000, 0x4100);
        assert_eq!(
            validate_allocation_modules([Hob::MemoryAllocationModule(&init)], mmram()),
            Err(HobValidationError::MissingModule { has_init: true, has_supv_core: false }.into())
        );
        for (base, length) in [(0x4000, 0x2000), (0x3000, 0x2000), (0x5000, 0x2000), (0x4000, 0x1000), (0x3000, 0x4000)]
        {
            let core = allocation_module(
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_CORE_GUID,
                base,
                length,
                base,
            );
            assert!(matches!(
                validate_allocation_modules(
                    [Hob::MemoryAllocationModule(&core), Hob::MemoryAllocationModule(&init)],
                    mmram(),
                ),
                Err(MmSupervisorError::HobValidation(HobValidationError::AllocationModulesOverlap { .. }))
            ));
        }
        for (base, length) in [(0x1000, 0), (u64::MAX - 0xfff, 0x1000), (0, 0x1000)] {
            let core = allocation_module(
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_CORE_GUID,
                base,
                length,
                base,
            );
            assert!(matches!(
                validate_allocation_modules(
                    [Hob::MemoryAllocationModule(&core), Hob::MemoryAllocationModule(&init)],
                    mmram(),
                ),
                Err(MmSupervisorError::HobValidation(HobValidationError::InvalidAddressRange { .. }))
            ));
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_is_io() {
        assert!(is_io(EFI_RESOURCE_IO));
        assert!(is_io(EFI_RESOURCE_IO_RESERVED));
        assert!(!is_io(0)); // EFI_RESOURCE_SYSTEM_MEMORY
    }

    #[test]
    fn test_mm_supervisor_hob_validation_is_memory_type_info() {
        use patina::pi::hob::{HobHeader, RESOURCE_DESCRIPTOR};
        let resource = |owner| ResourceDescriptor {
            header: HobHeader { r#type: RESOURCE_DESCRIPTOR, length: 0, reserved: 0 },
            owner,
            resource_type: 0,
            resource_attribute: 0,
            physical_start: 0,
            resource_length: 0,
        };
        assert!(is_memory_type_info(&resource(MEMORY_TYPE_INFO_HOB_GUID)));
        assert!(!is_memory_type_info(&resource(MM_SUPERVISOR_CORE_GUID)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_check_category_overlap_none() {
        assert_eq!(
            check_category_overlap(&[Some(span(0, 0x1000)), Some(span(0x1000, 0x1000)), None], "v1 memory"),
            Ok(())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_check_category_overlap_reports() {
        let result = check_category_overlap(&[Some(span(0, 0x2000)), Some(span(0x1000, 0x1000))], "v2 I/O");
        assert!(matches!(
            result,
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v2 I/O",
                ..
            }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_resource_descriptor_categories() {
        let memory_a = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        let memory_b = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1800, 0x1000, efi::MEMORY_WB);
        assert!(matches!(
            validate_resource_descriptor_overlaps([
                Hob::ResourceDescriptorV2(&memory_a),
                Hob::ResourceDescriptorV2(&memory_b)
            ]),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v2 memory",
                ..
            }))
        ));

        let io_a = resource_descriptor_v2(EFI_RESOURCE_IO, 0x1000, 0x1000, 0);
        let io_b = resource_descriptor_v2(EFI_RESOURCE_IO, 0x1800, 0x1000, 0);
        assert!(matches!(
            validate_resource_descriptor_overlaps([Hob::ResourceDescriptorV2(&io_a), Hob::ResourceDescriptorV2(&io_b)]),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v2 I/O",
                ..
            }))
        ));

        // Memory and I/O are separate address spaces, so the same range in each is not an overlap.
        assert_eq!(
            validate_resource_descriptor_overlaps([
                Hob::ResourceDescriptorV2(&memory_a),
                Hob::ResourceDescriptorV2(&io_a)
            ]),
            Ok(())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_resource_descriptor_exemptions_and_limits() {
        let normal = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        let memory_type_info = ResourceDescriptorV2::from(resource_descriptor(
            MEMORY_TYPE_INFO_HOB_GUID,
            EFI_RESOURCE_SYSTEM_MEMORY,
            0x1000,
            0x1000,
        ));
        assert_eq!(
            validate_resource_descriptor_overlaps([
                Hob::ResourceDescriptorV2(&normal),
                Hob::ResourceDescriptorV2(&memory_type_info)
            ]),
            Ok(())
        );

        let hobs = (0..=MAX_RESOURCE_HOBS).map(|_| Hob::ResourceDescriptorV2(&normal));
        assert_eq!(
            validate_resource_descriptor_overlaps(hobs),
            Err(HobValidationError::TooManyResourceDescriptors { kind: "v2 memory", limit: MAX_RESOURCE_HOBS }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_resource_descriptor_invalid_range() {
        for (resource_type, kind) in [
            (EFI_RESOURCE_SYSTEM_MEMORY, "v2 memory resource descriptor HOB"),
            (EFI_RESOURCE_IO, "v2 I/O resource descriptor HOB"),
        ] {
            let invalid = resource_descriptor_v2(resource_type, u64::MAX, 1, 0);
            assert_eq!(
                validate_resource_descriptor_overlaps([Hob::ResourceDescriptorV2(&invalid)]),
                Err(HobValidationError::InvalidAddressRange { kind, base: u64::MAX, length: 1 }.into())
            );
        }
    }

    #[test]
    #[cfg(feature = "v1_resource_descriptor_support")]
    fn test_mm_supervisor_hob_validation_v1_resource_descriptors_checked_with_feature() {
        let memory_a = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000);
        let memory_b = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1800, 0x1000);
        assert!(matches!(
            validate_resource_descriptor_overlaps([
                Hob::ResourceDescriptor(&memory_a),
                Hob::ResourceDescriptor(&memory_b)
            ]),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v1 memory",
                ..
            }))
        ));

        let io_a = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_IO, 0x1000, 0x1000);
        let io_b = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_IO, 0x1800, 0x1000);
        assert!(matches!(
            validate_resource_descriptor_overlaps([Hob::ResourceDescriptor(&io_a), Hob::ResourceDescriptor(&io_b)]),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v1 I/O",
                ..
            }))
        ));

        // V2 describes the same ranges as V1, so a V1 and a V2 descriptor for one range do not
        // overlap, and the memory bin descriptor is skipped as it is for V2.
        let memory_type_info =
            resource_descriptor(MEMORY_TYPE_INFO_HOB_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000);
        let v2 = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        assert_eq!(
            validate_resource_descriptor_overlaps([
                Hob::ResourceDescriptor(&memory_a),
                Hob::ResourceDescriptor(&io_a),
                Hob::ResourceDescriptor(&memory_type_info),
                Hob::ResourceDescriptorV2(&v2),
            ]),
            Ok(())
        );

        let hobs = (0..=MAX_RESOURCE_HOBS).map(|_| Hob::ResourceDescriptor(&memory_a));
        assert_eq!(
            validate_resource_descriptor_overlaps(hobs),
            Err(HobValidationError::TooManyResourceDescriptors { kind: "v1 memory", limit: MAX_RESOURCE_HOBS }.into())
        );

        for (resource_type, kind) in [
            (EFI_RESOURCE_SYSTEM_MEMORY, "v1 memory resource descriptor HOB"),
            (EFI_RESOURCE_IO, "v1 I/O resource descriptor HOB"),
        ] {
            let invalid = resource_descriptor(MM_SUPERVISOR_CORE_GUID, resource_type, u64::MAX, 1);
            assert_eq!(
                validate_resource_descriptor_overlaps([Hob::ResourceDescriptor(&invalid)]),
                Err(HobValidationError::InvalidAddressRange { kind, base: u64::MAX, length: 1 }.into())
            );
        }
    }

    #[test]
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    fn test_mm_supervisor_hob_validation_v1_resource_descriptors_ignored_without_feature() {
        // Without the feature V1 descriptors are not consumed, so overlapping and invalid ones are
        // not reported.
        let memory_a = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000);
        let memory_b = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1800, 0x1000);
        let invalid = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, u64::MAX, 1);
        assert_eq!(
            validate_resource_descriptor_overlaps([
                Hob::ResourceDescriptor(&memory_a),
                Hob::ResourceDescriptor(&memory_b),
                Hob::ResourceDescriptor(&invalid),
            ]),
            Ok(())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_memory_valid() {
        use patina::pi::hob::EFI_RESOURCE_SYSTEM_MEMORY;
        // Exactly one cacheability bit is valid.
        assert_eq!(check_v2_attributes(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, efi::MEMORY_WB), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_uce_rejected() {
        use patina::pi::hob::EFI_RESOURCE_SYSTEM_MEMORY;
        let result = check_v2_attributes(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, efi::MEMORY_UCE);
        assert!(matches!(
            result,
            Err(MmSupervisorError::HobValidation(HobValidationError::V2ContainsUceAttribute { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_no_cacheability_rejected() {
        use patina::pi::hob::EFI_RESOURCE_SYSTEM_MEMORY;
        let result = check_v2_attributes(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0);
        assert!(matches!(
            result,
            Err(MmSupervisorError::HobValidation(HobValidationError::V2InvalidCacheability { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_multiple_cacheability_rejected() {
        use patina::pi::hob::EFI_RESOURCE_SYSTEM_MEMORY;
        let result = check_v2_attributes(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, efi::MEMORY_WB | efi::MEMORY_WT);
        assert!(matches!(
            result,
            Err(MmSupervisorError::HobValidation(HobValidationError::V2InvalidCacheability { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_io_zero_valid() {
        assert_eq!(check_v2_attributes(EFI_RESOURCE_IO, 0x1000, 0), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_io_nonzero_rejected() {
        let result = check_v2_attributes(EFI_RESOURCE_IO, 0x1000, efi::MEMORY_WB);
        assert!(matches!(
            result,
            Err(MmSupervisorError::HobValidation(HobValidationError::V2IoAttributesNotZero { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_are_checked_across_hobs() {
        let valid = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        let invalid = resource_descriptor_v2(EFI_RESOURCE_IO, 0x2000, 0x100, efi::MEMORY_WB);
        assert_eq!(
            validate_resource_v2_memory_attributes([
                Hob::ResourceDescriptorV2(&valid),
                Hob::ResourceDescriptorV2(&invalid)
            ]),
            Err(HobValidationError::V2IoAttributesNotZero { base: 0x2000, attributes: efi::MEMORY_WB }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_rejects_null_pointers() {
        // MMRAM never covers address 0, so a null pointer fails the containment check like any
        // other address outside MMRAM.
        assert_eq!(validate_pass_down_pointers(&pass_down(), mmram()), Ok(()));

        for field in ["cpl3_stack_base", "sm_base_array", "mm_initialized_buffer", "firmware_policy_buffer"] {
            let mut pd = pass_down();
            let size = match field {
                "cpl3_stack_base" => {
                    pd.cpl3_stack_base = 0;
                    pd.cpl3_stack_size
                }
                "sm_base_array" => {
                    pd.sm_base_array = 0;
                    size_of::<u64>() as u64
                }
                "mm_initialized_buffer" => {
                    pd.mm_initialized_buffer = 0;
                    1
                }
                _ => {
                    pd.firmware_policy_buffer = 0;
                    pd.firmware_policy_buffer_size
                }
            };
            assert_eq!(
                validate_pass_down_pointers(&pd, mmram()),
                Err(PassDownHobError::PointerOutsideMmram { field, range: MemoryRange::new(0, size).unwrap() }.into()),
                "a null `{field}` must be rejected"
            );
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_hob_list_must_be_inside_mmram() {
        let handoff = PhaseHandoffInformationTable {
            header: HobHeader {
                r#type: HANDOFF,
                length: size_of::<PhaseHandoffInformationTable>() as u16,
                reserved: 0,
            },
            version: 1,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0x9000,
            memory_bottom: 0x1000,
            free_memory_top: 0x8000,
            free_memory_bottom: 0x2000,
            // Deliberately left unset: platforms do not reliably populate it for the MM HOB
            // list, which is why the length is walked from the headers instead.
            end_of_hob_list: 0,
        };
        let base = &raw const handoff as u64;
        let around_list = span(base, 0x40);

        assert_eq!(validate_hob_list_inside_mmram(&handoff, 0x40, around_list), Ok(()));

        // A list the producer placed outside MMRAM would be mapped readable to Ring 3 later.
        assert_eq!(
            validate_hob_list_inside_mmram(&handoff, 0x40, mmram()),
            Err(HobValidationError::HobListOutsideMmram { base, size: 0x40 }.into())
        );

        // A zero-length walk means the list has no HOBs at all, which is not a list.
        assert_eq!(
            validate_hob_list_inside_mmram(&handoff, 0, around_list),
            Err(HobValidationError::HobListOutsideMmram { base, size: 0 }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_hob_list_crossing_mmram_end_is_rejected() {
        let handoff = PhaseHandoffInformationTable {
            header: HobHeader {
                r#type: HANDOFF,
                length: size_of::<PhaseHandoffInformationTable>() as u16,
                reserved: 0,
            },
            version: 1,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0,
            memory_bottom: 0,
            free_memory_top: 0,
            free_memory_bottom: 0,
            end_of_hob_list: 0,
        };
        let base = &raw const handoff as u64;

        // The span covers the start of the list but ends before the list does.
        assert_eq!(
            validate_hob_list_inside_mmram(&handoff, 0x40, span(base, 0x20)),
            Err(HobValidationError::HobListOutsideMmram { base, size: 0x40 }.into())
        );
    }

    /// Lays out HOB headers back to back in an 8-byte aligned buffer, with zeroed payloads.
    ///
    /// Each header starts where the previous length ends, so a malformed length only moves the
    /// headers after it, and the walk stops at the malformed one first. The buffer is at least
    /// one PHIT long, so it can be borrowed as a PHIT even when the first header is not one.
    fn header_list(headers: &[(u16, u16)]) -> Vec<u64> {
        let total: usize = headers.iter().map(|&(_, length)| usize::from(length).max(size_of::<HobHeader>())).sum();
        let total = total.max(size_of::<PhaseHandoffInformationTable>());
        let mut buffer = vec![0u64; total.div_ceil(size_of::<u64>())];
        let mut offset = 0usize;
        for &(hob_type, length) in headers {
            // SAFETY: `total` reserved room for every header, and each one is written at the
            // sum of the lengths before it.
            unsafe {
                buffer.as_mut_ptr().cast::<u8>().add(offset).cast::<HobHeader>().write_unaligned(HobHeader {
                    r#type: hob_type,
                    length,
                    reserved: 0,
                });
            }
            offset += usize::from(length).max(size_of::<HobHeader>());
        }
        buffer
    }

    /// Runs [`validate_hob_list_structure`] over `buffer`.
    fn walk(buffer: &[u64]) -> MmSupervisorResult<u64> {
        // SAFETY: `buffer` is 8-byte aligned and at least one PHIT long.
        let handoff = unsafe { &*buffer.as_ptr().cast::<PhaseHandoffInformationTable>() };
        // SAFETY: the walk reads only headers inside `buffer`, because every test list either
        // ends or is rejected before its last byte.
        unsafe { validate_hob_list_structure(handoff) }
    }

    const PHIT_LENGTH: u16 = size_of::<PhaseHandoffInformationTable>() as u16;
    const HEADER_LENGTH: u16 = size_of::<HobHeader>() as u16;

    #[test]
    fn test_mm_supervisor_hob_validation_structure_accepts_a_well_formed_list() {
        let buffer = header_list(&[(HANDOFF, PHIT_LENGTH), (GUID_EXTENSION, 32), (END_OF_HOB_LIST, HEADER_LENGTH)]);

        // The returned length includes the END_OF_HOB_LIST header, like get_pi_hob_list_size.
        assert_eq!(walk(&buffer), Ok(u64::from(PHIT_LENGTH + 32 + HEADER_LENGTH)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_structure_rejects_malformed_headers() {
        let malformed = |offset: usize, hob_type: u16, length: u16| -> MmSupervisorResult<u64> {
            Err(HobValidationError::MalformedHob { offset, hob_type, length }.into())
        };
        let after_phit = usize::from(PHIT_LENGTH);

        // The list must start with the PHIT.
        let buffer = header_list(&[(GUID_EXTENSION, 24), (END_OF_HOB_LIST, HEADER_LENGTH)]);
        assert_eq!(walk(&buffer), malformed(0, GUID_EXTENSION, 24));

        // A zero length would keep the walk on the same HOB forever.
        let buffer = header_list(&[(HANDOFF, PHIT_LENGTH), (GUID_EXTENSION, 0)]);
        assert_eq!(walk(&buffer), malformed(after_phit, GUID_EXTENSION, 0));

        // A length that is not a multiple of eight would leave the next header unaligned.
        let buffer = header_list(&[(HANDOFF, PHIT_LENGTH), (UNUSED, 12)]);
        assert_eq!(walk(&buffer), malformed(after_phit, UNUSED, 12));

        // A length smaller than the structure for its type would let iteration read past the HOB.
        let buffer = header_list(&[(HANDOFF, PHIT_LENGTH), (RESOURCE_DESCRIPTOR, 16)]);
        assert_eq!(walk(&buffer), malformed(after_phit, RESOURCE_DESCRIPTOR, 16));
        let buffer = header_list(&[(HANDOFF, 16)]);
        assert_eq!(walk(&buffer), malformed(0, HANDOFF, 16));

        // The end of the list is a header too, so it cannot be empty.
        let buffer = header_list(&[(HANDOFF, PHIT_LENGTH), (END_OF_HOB_LIST, 0)]);
        assert_eq!(walk(&buffer), malformed(after_phit, END_OF_HOB_LIST, 0));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_structure_stops_at_the_size_limit() {
        const FILLER_LENGTH: u16 = 0xfff8;
        let too_large: MmSupervisorResult<u64> =
            Err(HobValidationError::HobListTooLarge { limit: MAX_HOB_LIST_SIZE }.into());

        // A PHIT and 16 maximal filler HOBs leave 72 bytes below the limit.
        let mut headers = vec![(HANDOFF, PHIT_LENGTH)];
        headers.extend(core::iter::repeat_n((UNUSED, FILLER_LENGTH), 16));
        let left = MAX_HOB_LIST_SIZE - usize::from(PHIT_LENGTH) - 16 * usize::from(FILLER_LENGTH);
        assert_eq!(left, 72);

        // A list whose END_OF_HOB_LIST header ends exactly at the limit is accepted.
        let mut fits = headers.clone();
        fits.extend([(UNUSED, 64), (END_OF_HOB_LIST, HEADER_LENGTH)]);
        assert_eq!(walk(&header_list(&fits)), Ok(MAX_HOB_LIST_SIZE as u64));

        // Without END_OF_HOB_LIST, no header fits after the last HOB.
        let mut no_end = headers.clone();
        no_end.extend([(UNUSED, 64), (UNUSED, HEADER_LENGTH)]);
        assert_eq!(walk(&header_list(&no_end)), too_large);

        // A HOB whose length runs past the limit is rejected before the walk moves past it.
        let mut runs_past = headers;
        runs_past.extend([(UNUSED, FILLER_LENGTH), (END_OF_HOB_LIST, HEADER_LENGTH)]);
        assert_eq!(walk(&header_list(&runs_past)), too_large);
    }

    #[test]
    fn test_mm_supervisor_hob_validation_min_hob_length_covers_each_type() {
        assert_eq!(min_hob_length(HANDOFF), size_of::<PhaseHandoffInformationTable>());
        assert_eq!(min_hob_length(MEMORY_ALLOCATION), size_of::<MemoryAllocation>());
        assert_eq!(min_hob_length(RESOURCE_DESCRIPTOR), size_of::<ResourceDescriptor>());
        assert_eq!(min_hob_length(GUID_EXTENSION), size_of::<GuidHob>());
        assert_eq!(min_hob_length(FV), size_of::<FirmwareVolume>());
        assert_eq!(min_hob_length(FV2), size_of::<FirmwareVolume2>());
        assert_eq!(min_hob_length(FV3), size_of::<FirmwareVolume3>());
        assert_eq!(min_hob_length(CPU), size_of::<Cpu>());
        assert_eq!(min_hob_length(UEFI_CAPSULE), size_of::<Capsule>());
        assert_eq!(min_hob_length(RESOURCE_DESCRIPTOR2), size_of::<ResourceDescriptorV2>());
        assert_eq!(min_hob_length(MEMORY_POOL), size_of::<HobHeader>());
        assert_eq!(min_hob_length(END_OF_HOB_LIST), size_of::<HobHeader>());
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_pointer_outside_mmram() {
        let result = validate_pass_down_pointers(&pass_down(), span(0x10_0000, 0x1000));
        assert_eq!(
            result,
            Err(PassDownHobError::PointerOutsideMmram {
                field: "cpl3_stack_base",
                range: MemoryRange::new(0x1000, 0x1000).unwrap()
            }
            .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_pointer_past_the_end_of_the_address_space() {
        // A pointer whose buffer would end past u64::MAX is not a valid range.
        let mut pd = pass_down();
        pd.sm_base_array = u64::MAX - 3;
        assert_eq!(
            validate_pass_down_pointers(&pd, mmram()),
            Err(HobValidationError::InvalidAddressRange {
                kind: "sm_base_array",
                base: u64::MAX - 3,
                length: size_of::<u64>() as u64
            }
            .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_bad_revision() {
        let mut pd = pass_down();
        pd.revision = MM_SUPV_PASS_DOWN_HOB_REVISION + 1;
        assert!(matches!(
            validate_pass_down_pointers(&pd, mmram()),
            Err(MmSupervisorError::PassDownHob(PassDownHobError::InvalidRevision { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_pointer_sizes() {
        // Each pointer sits so that the last byte it covers is the last byte of MMRAM. A zero
        // buffer size is checked as one byte and the SMBASE array as one u64, so all of them fit.
        let mmram = mmram();
        let mmram_end = 0x10_1000;
        let mut pd = pass_down();
        pd.cpl3_stack_base = mmram_end - 1;
        pd.cpl3_stack_size = 0;
        pd.sm_base_array = mmram_end - size_of::<u64>() as u64;
        pd.mm_initialized_buffer = mmram_end - 1;
        pd.firmware_policy_buffer = mmram_end - 1;
        pd.firmware_policy_buffer_size = 0;
        assert_eq!(validate_pass_down_pointers(&pd, mmram), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_pointer_crossing_mmram_end_is_rejected() {
        // The SMBASE array is checked as one u64, so starting it 4 bytes before the end of MMRAM
        // makes it cross the boundary.
        let mut pd = pass_down();
        pd.sm_base_array = 0x10_1000 - 4;
        assert_eq!(
            validate_pass_down_pointers(&pd, mmram()),
            Err(PassDownHobError::PointerOutsideMmram {
                field: "sm_base_array",
                range: MemoryRange::new(0x10_1000 - 4, size_of::<u64>() as u64).unwrap()
            }
            .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_hob_errors_and_success() {
        let data = pass_down_bytes(&pass_down());
        let matching = guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, data.len());
        assert_eq!(validate_pass_down([Hob::GuidHob(&matching, &data)], mmram()), Ok(()));

        let unrelated = guid_hob(MM_SUPERVISOR_CORE_GUID, data.len());
        assert_eq!(
            validate_pass_down([Hob::GuidHob(&unrelated, &data)], mmram()),
            Err(PassDownHobError::Missing.into())
        );

        let short_data = [0u8; 4];
        let short = guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, short_data.len());
        assert_eq!(
            validate_pass_down([Hob::GuidHob(&short, &short_data)], mmram()),
            Err(PassDownHobError::TooSmall.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_push_range() {
        let mut ranges = [None; 2];
        let mut count = 0usize;
        assert_eq!(push_range(&mut ranges, &mut count, span(0x1000, 0x100), "test"), Ok(()));
        assert_eq!(push_range(&mut ranges, &mut count, span(0x2000, 0x200), "test"), Ok(()));
        assert_eq!(count, 2);
        assert_eq!(ranges, [Some(span(0x1000, 0x100)), Some(span(0x2000, 0x200))]);
        assert_eq!(
            push_range(&mut ranges, &mut count, span(0x3000, 0x300), "test"),
            Err(HobValidationError::TooManyResourceDescriptors { kind: "test", limit: 2 }.into())
        );
        assert_eq!(count, 2);
        assert_eq!(ranges, [Some(span(0x1000, 0x100)), Some(span(0x2000, 0x200))]);
    }

    #[test]
    fn test_mm_supervisor_hob_validation_module_page_protections() {
        let core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1100,
        );
        let user = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x2000,
            0x2000,
            0x2100,
        );
        // Not one of the supervisor's own module HOBs, so its pages are never queried. The mock
        // has no expectation for 0x4000, so a query there would fail the test.
        let unrelated = allocation_module(MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_CORE_GUID, 0x4000, 0x1000, 0x4100);
        let hobs = [
            Hob::MemoryAllocationModule(&core),
            Hob::MemoryAllocationModule(&user),
            Hob::MemoryAllocationModule(&unrelated),
        ];

        // Each module's first page is queried twice: once for the page walk, once for the entry
        // point that lies in it.
        let page_table = |core_page: MemoryAttributes, user_page: MemoryAttributes| {
            let mut page_table = MockPageTable::new();
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x1000)
                .returning(move |_, _| Ok(core_page));
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x2000 || addr == 0x3000)
                .returning(move |_, _| Ok(user_page));
            page_table
        };

        // Code pages are read-only: supervisor-owned for the core, user-accessible for the user
        // module.
        let core_code = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        let user_code = MemoryAttributes::ReadOnly;
        assert_eq!(verify_module_page_protections(hobs.clone(), &page_table(core_code, user_code)), Ok(()));

        // Core pages must be supervisor pages.
        assert_eq!(
            verify_module_page_protections(hobs.clone(), &page_table(MemoryAttributes::ReadOnly, user_code)),
            Err(HobValidationError::PageMissingAttribute {
                addr: 0x1000,
                desired: MemoryAttributes::Supervisor.bits(),
                found: MemoryAttributes::ReadOnly.bits(),
            }
            .into())
        );

        // User pages must not be supervisor pages.
        let supervisor_code = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        assert_eq!(
            verify_module_page_protections(hobs.clone(), &page_table(core_code, supervisor_code)),
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr: 0x2000,
                forbidden: MemoryAttributes::Supervisor.bits(),
                found: supervisor_code.bits(),
            }
            .into())
        );

        // The entry point must be on a present, read-only, executable page.
        for core_page in [
            MemoryAttributes::Supervisor,
            core_code | MemoryAttributes::ExecuteProtect,
            core_code | MemoryAttributes::ReadProtect,
        ] {
            assert_eq!(
                verify_module_page_protections(hobs.clone(), &page_table(core_page, user_code)),
                Err(HobValidationError::EntryPointNotOnCodePage { entry_point: 0x1100, attributes: core_page.bits() }
                    .into())
            );
        }
        let user_data = MemoryAttributes::ExecuteProtect;
        assert_eq!(
            verify_module_page_protections(hobs, &page_table(core_code, user_data)),
            Err(HobValidationError::EntryPointNotOnCodePage { entry_point: 0x2100, attributes: user_data.bits() }
                .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_page_attributes_checks_each_page() {
        // An unaligned range is widened to whole pages, and each page is queried on its own.
        let mut page_table = MockPageTable::new();
        for page in [0x1000u64, 0x2000] {
            page_table
                .expect_query_memory_region()
                .withf(move |&addr, &size| addr == page && size == 0x1000)
                .times(1)
                .returning(|_, _| Ok(MemoryAttributes::Supervisor));
        }
        assert_eq!(
            verify_page_attributes(
                &page_table,
                0x1800,
                0x1000,
                MemoryAttributes::Supervisor,
                MemoryAttributes::ReadOnly
            ),
            Ok(())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_page_attributes_reports_the_failing_page() {
        let required = MemoryAttributes::Supervisor;

        // A page that cannot be queried is reported by its address.
        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().withf(|&addr, _| addr == 0x1000).returning(move |_, _| Ok(required));
        page_table
            .expect_query_memory_region()
            .withf(|&addr, _| addr == 0x2000)
            .returning(|_, _| Err(PtError::NoMapping));
        assert_eq!(
            verify_page_attributes(&page_table, 0x1000, 0x2000, required, MemoryAttributes::empty()),
            Err(HobValidationError::PageAttributeQueryFailed { addr: 0x2000 }.into())
        );

        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::empty()));
        assert_eq!(
            verify_page_attributes(&page_table, 0x1000, 0x1000, required, MemoryAttributes::empty()),
            Err(HobValidationError::PageMissingAttribute { addr: 0x1000, desired: required.bits(), found: 0 }.into())
        );

        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::ReadOnly));
        assert_eq!(
            verify_page_attributes(&page_table, 0x1000, 0x1000, MemoryAttributes::empty(), MemoryAttributes::ReadOnly),
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr: 0x1000,
                forbidden: MemoryAttributes::ReadOnly.bits(),
                found: MemoryAttributes::ReadOnly.bits(),
            }
            .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_page_attributes_rejects_invalid_range() {
        // The mock has no expectations, so any query would fail the test.
        let page_table = MockPageTable::new();
        let required = MemoryAttributes::Supervisor;
        for (base, length) in [(0x1000, 0), (u64::MAX, 1), (u64::MAX - 0xfff, 0xfff)] {
            assert_eq!(
                verify_page_attributes(&page_table, base, length, required, MemoryAttributes::empty()),
                Err(HobValidationError::PageAttributeQueryFailed { addr: base }.into())
            );
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_post_paging_page_table_unavailable() {
        // The page table is uninitialized in unit tests, so the check reports it as unavailable
        // before it reads any HOB.
        let handoff = PhaseHandoffInformationTable {
            header: HobHeader {
                r#type: HANDOFF,
                length: size_of::<PhaseHandoffInformationTable>() as u16,
                reserved: 0,
            },
            version: 1,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0x9000,
            memory_bottom: 0x1000,
            free_memory_top: 0x8000,
            free_memory_bottom: 0x2000,
            end_of_hob_list: 0,
        };
        assert_eq!(
            validate_incoming_hobs_post_paging_init(&handoff),
            Err(HobValidationError::PageTableUnavailable.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_entry_point_on_code_page() {
        // The page that holds the entry point is the one queried.
        let mut page_table = MockPageTable::new();
        page_table
            .expect_query_memory_region()
            .withf(|&addr, &size| addr == 0x1000 && size == 0x1000)
            .times(1)
            .returning(|_, _| Ok(MemoryAttributes::ReadOnly));
        assert_eq!(verify_entry_point_on_code_page(&page_table, 0x1234), Ok(()));

        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().returning(|_, _| Err(PtError::NoMapping));
        assert_eq!(
            verify_entry_point_on_code_page(&page_table, 0x1234),
            Err(HobValidationError::PageAttributeQueryFailed { addr: 0x1000 }.into())
        );

        // An entry point at the top of the address space is rejected without a query.
        assert_eq!(
            verify_entry_point_on_code_page(&MockPageTable::new(), u64::MAX),
            Err(HobValidationError::PageAttributeQueryFailed { addr: u64::MAX }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_entry_point_attributes() {
        // Present, read-only and executable, with or without the supervisor bit.
        assert_eq!(validate_entry_point_attributes(0x1234, MemoryAttributes::ReadOnly), Ok(()));
        assert_eq!(
            validate_entry_point_attributes(0x1234, MemoryAttributes::ReadOnly | MemoryAttributes::Supervisor),
            Ok(())
        );

        // Writable, not executable, or not present.
        for attributes in [
            MemoryAttributes::empty(),
            MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect,
            MemoryAttributes::ReadOnly | MemoryAttributes::ReadProtect,
        ] {
            assert_eq!(
                validate_entry_point_attributes(0x1234, attributes),
                Err(HobValidationError::EntryPointNotOnCodePage { entry_point: 0x1234, attributes: attributes.bits() }
                    .into())
            );
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_verify_cpl3_stacks_page_table_unavailable() {
        // The page table is uninitialized in unit tests, so the query reports it
        // as unavailable rather than panicking.
        assert_eq!(
            verify_cpl3_stacks_user_accessible(0x1000, 0x2000),
            Err(HobValidationError::PageTableUnavailable.into())
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_mm_supervisor_hob_validation_cpl3_stack_pages() {
        let pool = PageAlignedMemory::new(crate::memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&pool);
        let stack = PageAlignedMemory::new(2);
        let map_stack = |attributes: MemoryAttributes| {
            let mut page_table = security_state().lock_page_table();
            let page_table = page_table.as_mut().expect("a page table is installed");
            page_table.map_memory_region(stack.base(), stack.size(), attributes).expect("map the stack");
        };
        let forbidden = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly | MemoryAttributes::ReadProtect;

        // A present, writable, non-executable user page is a usable stack page.
        map_stack(MemoryAttributes::ExecuteProtect);
        assert_eq!(verify_cpl3_stacks_user_accessible(stack.base(), stack.size()), Ok(()));

        // A supervisor or read-only page cannot serve as a Ring 3 stack.
        for attribute in [MemoryAttributes::Supervisor, MemoryAttributes::ReadOnly] {
            map_stack(MemoryAttributes::ExecuteProtect | attribute);
            let found = security_state()
                .lock_page_table()
                .as_ref()
                .expect("a page table is installed")
                .query_memory_region(stack.base(), UEFI_PAGE_SIZE as u64)
                .expect("query the stack page");
            assert_eq!(
                verify_cpl3_stacks_user_accessible(stack.base(), stack.size()),
                Err(HobValidationError::PageHasForbiddenAttribute {
                    addr: stack.base(),
                    forbidden: forbidden.bits(),
                    found: found.bits(),
                }
                .into())
            );
        }

        // Nor can a page that is not present. The x64 page table reports such a page as unmapped
        // rather than returning ReadProtect, so the query itself fails.
        map_stack(MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadProtect);
        assert_eq!(
            verify_cpl3_stacks_user_accessible(stack.base(), stack.size()),
            Err(HobValidationError::PageAttributeQueryFailed { addr: stack.base() }.into())
        );

        *security_state().lock_page_table() = None;
    }

    #[test]
    fn test_mm_supervisor_hob_validation_error_display_non_empty() {
        // Every error variant must produce a non-empty, human-readable message.
        let errors = [
            HobValidationError::NoMmramRegions,
            HobValidationError::InvalidAddressRange { kind: "MMRAM region", base: 0x1000, length: 0 },
            HobValidationError::MmramRegionsOverlap { base_a: 0, size_a: 0x1000, base_b: 0x800, size_b: 0x1000 },
            HobValidationError::MmramNotContiguous { covered: 0x1000, span: 0x2000 },
            HobValidationError::MemoryAllocationInsideMmram { base: 0x1000, length: 0x1000 },
            HobValidationError::AllocationModuleOutsideMmram { base: 0x1000, length: 0x1000 },
            HobValidationError::InitModuleNotPageAligned { base: 0x1001, length: 0x1000 },
            HobValidationError::MissingModule { has_init: false, has_supv_core: false },
            HobValidationError::MissingModule { has_init: false, has_supv_core: true },
            HobValidationError::MissingModule { has_init: true, has_supv_core: false },
            HobValidationError::AllocationModulesOverlap { base_a: 0, size_a: 0x1000, base_b: 0x800, size_b: 0x1000 },
            HobValidationError::TooManyAllocationModules { limit: MAX_ALLOC_MODULES },
            HobValidationError::AllocationModuleEntryPointOutOfRange {
                entry_point: 0x5000,
                base: 0x1000,
                length: 0x1000,
            },
            HobValidationError::ResourceDescriptorsOverlap {
                kind: "v1 memory",
                base_a: 0,
                size_a: 0x1000,
                base_b: 0x800,
                size_b: 0x1000,
            },
            HobValidationError::TooManyResourceDescriptors { kind: "v1 memory", limit: MAX_RESOURCE_HOBS },
            HobValidationError::V2ContainsUceAttribute { base: 0x1000, attributes: 0x1 },
            HobValidationError::V2InvalidCacheability { base: 0x1000, attributes: 0 },
            HobValidationError::V2IoAttributesNotZero { base: 0x1000, attributes: 0x4 },
            HobValidationError::MalformedHob { offset: 0x38, hob_type: GUID_EXTENSION, length: 0 },
            HobValidationError::HobListTooLarge { limit: MAX_HOB_LIST_SIZE },
            HobValidationError::HobListOutsideMmram { base: 0x1000, size: 0x40 },
            HobValidationError::PageTableUnavailable,
            HobValidationError::PageAttributeQueryFailed { addr: 0x1000 },
            HobValidationError::PageMissingAttribute { addr: 0x1000, desired: 0x1, found: 0 },
            HobValidationError::PageHasForbiddenAttribute { addr: 0x1000, forbidden: 0x1, found: 0x1 },
            HobValidationError::EntryPointNotOnCodePage { entry_point: 0x5000, attributes: 0x1 },
        ];
        for err in errors {
            assert!(!format!("{err}").is_empty());
        }
    }
}
