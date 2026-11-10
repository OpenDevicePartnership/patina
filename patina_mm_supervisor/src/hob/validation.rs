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
//! 1. The scanned MMRAM regions form a single contiguous, non-overlapping span
//!    that starts at or above 1 MiB.
//! 2. No `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOB overlaps MMRAM (these describe
//!    allocations outside of MMRAM).
//! 3. The supervisor's module HOBs, including the standard PI Init module HOB,
//!    lie inside MMRAM, do not overlap each other, and have entry points inside
//!    their allocations. Every module allocation is page-aligned, and the Init,
//!    Core and User modules are each described exactly once.
//! 4. Resource descriptor HOBs do not overlap within the same address space
//!    (memory vs I/O). Only v2 descriptors are checked, or only v1 descriptors
//!    when the `v1_resource_descriptor_support` feature is enabled, as in
//!    `patina_dxe_core`.
//!    Descriptors owned by the Memory Type Information GUID are skipped, since
//!    the PEI memory bin HOB is expected to overlap the system-memory
//!    descriptors backing it. One system-memory or reserved-memory descriptor
//!    holds all of MMRAM, so no other memory descriptor, such as MMIO, reaches
//!    into it.
//! 5. Every v2 resource descriptor carries valid memory attributes (exactly one
//!    cacheability bit for memory, no `EFI_MEMORY_UCE`, no attributes for I/O).
//! 6. The pointers reported in the MM Supervisor `PassDown` HOB reference
//!    memory inside MMRAM (and the HOB reports the expected revision). Its
//!    buffers, sized for the CPU count, the module allocations, the HOB list,
//!    the SMBASE area (lowest SMBASE + 0x8000 to highest SMBASE + 0x10000) and
//!    MSEG, when an MSEG SMRAM HOB is present, each lie in pages the producer
//!    marked allocated, no two share a page, and the Ring 3 stacks start and end
//!    on page boundaries. An MSEG SMRAM HOB that is present must describe a
//!    non-empty, page-aligned MSEG below 4 GiB.
//! 7. Every unblock-region and protected-region HOB describes a non-empty,
//!    page-aligned range that does not overlap MMRAM. Unblock regions name their
//!    requester with a non-zero GUID, and protected regions are of the IOMMU
//!    type, the only type the producer applies.
//!
//! ## Post-paging checks
//!
//! Run after the page table is initialized, so per-page attributes can be
//! queried from it:
//!
//! 8. Every page of the MM Supervisor Core module is supervisor-owned (the
//!    `Supervisor` attribute is set, i.e. U/S = 0) and every page of the MM Supervisor
//!    User module and of each MM driver is user-accessible (the `Supervisor` attribute
//!    is clear, i.e. U/S = 1). Every module page is present and is never both writable
//!    and executable.
//! 9. Each module's entry point lies on a code page: present (`ReadProtect`
//!    clear), read-only (`ReadOnly` set) and executable (`ExecuteProtect`
//!    clear), i.e. it sits in the module's code section.
//! 10. Every CPU's SMBASE is page aligned. Its SMI entry code (`SMBASE + 0x8000`)
//!     lies on supervisor-only, read-only, executable pages, and its SMM descriptor
//!     and save-state map (`SMBASE + 0xFB00` to `SMBASE + 0x10000`) on a
//!     supervisor-only, non-executable page.
//! 11. MSEG, when present, lies on supervisor-only, non-executable pages.
//! 12. Page 0 and everything from the physical address width up to the end of
//!     the lower canonical half (2^47 with 4-level paging, 2^56 with 5-level
//!     paging, as CR4.LA57 selects) are not present. The width is the CPU HOB's,
//!     which may not exceed the processor's MAXPHYADDR, or MAXPHYADDR without a
//!     CPU HOB, capped at 47 bits with 4-level paging.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use patina::management_mode::protocol::mm_supervisor_request::{
    MM_SUPV_UNBLOCK_REGION_HOB_GUID, MmSupervisorUnblockMemoryParams,
};
use patina::management_mode::supervisor::{
    MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID,
    MM_SUPERVISOR_USER_GUID,
};
use patina::pi::guid::HOB_MEMORY_ALLOC_MODULE_GUID;
use patina::pi::hob::{
    CPU, Capsule, Cpu, EFI_RESOURCE_IO, EFI_RESOURCE_IO_RESERVED, EFI_RESOURCE_MEMORY_RESERVED,
    EFI_RESOURCE_SYSTEM_MEMORY, END_OF_HOB_LIST, FV, FV2, FV3, FirmwareVolume, FirmwareVolume2, FirmwareVolume3,
    GUID_EXTENSION, GuidHob, HANDOFF, Hob, HobHeader, MEMORY_ALLOCATION, MEMORY_TYPE_INFO_HOB_GUID, MemoryAllocation,
    PhaseHandoffInformationTable, RESOURCE_DESCRIPTOR, RESOURCE_DESCRIPTOR2, ResourceDescriptor, ResourceDescriptorV2,
    UEFI_CAPSULE,
};
use patina::standard::efi;
use patina::{BinaryGuid, MemoryRange, UEFI_PAGE_SIZE};
use patina_paging::{MemoryAttributes, PageTable, PagingType, PtError};
use zerocopy::FromBytes;

use crate::error::MmSupervisorResult;
use crate::hob::pass_down::{
    MM_SUPV_PASS_DOWN_HOB_GUID, MM_SUPV_PASS_DOWN_HOB_REVISION, MmSupvPassDownHobData, PassDownHobError,
};
use crate::hob::protected_region::{MM_PROTECTED_REGION_HOB_GUID, MmProtectedRegion, ProtectedRegionType};
use crate::intrinsics;
use crate::memory::PAGE_STEP;
use crate::memory::mmram::MmramRegion;
use crate::mmcore::mseg;
use crate::mmcore::smi_idt_patch::SMM_HANDLER_OFFSET;
use crate::state::security::SupervisorPageTable;
use crate::state::security_state;

/// Maximum number of `MemoryAllocationModule` HOBs accepted while checking for
/// overlaps. This bounds the stack storage used for the pairwise comparison.
const MAX_ALLOC_MODULES: usize = 64;

/// Largest HOB list, in bytes, that [`validate_hob_list_structure`] accepts.
///
/// The list is walked before the MMRAM bound is known, so this limits how far header lengths
/// chosen by the producer can move the walk.
const MAX_HOB_LIST_SIZE: usize = 0x10_0000;

/// Lowest address MMRAM may start at. The SMRR range was always chosen at or above 1 MiB, and
/// MMRAM is now that range as a whole.
const MMRAM_MIN_BASE: u64 = 0x10_0000;

/// Offset from SMBASE of the processor SMM descriptor (PSD). The save-state map follows it to the
/// end of the CPU's SMRAM window.
const SMM_PSD_OFFSET: u64 = 0xFB00;

/// Size of each CPU's SMRAM window above its SMBASE.
const SMRAM_WINDOW_SIZE: u64 = 0x1_0000;

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
    /// The MMRAM span starts below 1 MiB.
    MmramBelowOneMib {
        /// The span the scanned regions cover.
        span: MemoryRange,
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
    /// A module allocation does not start and end on page boundaries, so its pages cannot be
    /// validated or reclaimed as whole pages.
    AllocationModuleNotPageAligned {
        /// The module allocation's range.
        range: MemoryRange,
    },
    /// One or more required module allocations were not described.
    MissingModule {
        /// Whether the Init module HOB was present.
        has_init: bool,
        /// Whether the Supervisor Core module HOB was present.
        has_supv_core: bool,
        /// Whether the User module HOB was present.
        has_user: bool,
    },
    /// The Init, Core or User module is described by more than one allocation module HOB.
    DuplicateModule {
        /// The name of the module described more than once.
        module_name: patina::BinaryGuid,
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
    /// No memory resource descriptor holds all of MMRAM.
    MmramNotInMemoryDescriptor {
        /// The MMRAM span.
        mmram: MemoryRange,
    },
    /// The memory resource descriptor holding MMRAM is neither system memory nor reserved memory.
    MmramInUnexpectedResourceType {
        /// The descriptor holding MMRAM.
        range: MemoryRange,
        /// The descriptor's resource type.
        resource_type: u32,
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
    /// A region the supervisor remaps for Ring 3 does not start and end on page boundaries, so
    /// the rest of its first or last page would be exposed with it.
    RegionNotPageAligned {
        /// Which region was checked.
        kind: &'static str,
        /// The region's range.
        range: MemoryRange,
    },
    /// A region the supervisor keeps using does not lie in MMRAM the producer marked allocated,
    /// so it would alias pages the page allocator hands out.
    RegionNotInAllocatedMmram {
        /// Which region was checked.
        kind: &'static str,
        /// The region's range, widened to whole pages.
        range: MemoryRange,
    },
    /// Two regions the supervisor keeps using share a page.
    RegionsOverlap {
        /// Which region the first one is.
        first_kind: &'static str,
        /// The first region, widened to whole pages.
        first: MemoryRange,
        /// Which region the second one is.
        second_kind: &'static str,
        /// The second region, widened to whole pages.
        second: MemoryRange,
    },
    /// The MSEG SMRAM HOB is present but does not describe a usable MSEG: it is truncated, empty,
    /// or not page aligned and below 4 GiB.
    InvalidMsegHob,
    /// A GUID extension HOB is too short to hold its payload.
    GuidedHobTooSmall {
        /// The GUID of the HOB that was too small.
        guid: BinaryGuid,
        /// Length of the HOB's payload, in bytes.
        length: usize,
    },
    /// A region the producer has already mapped outside MMRAM, such as an unblock or protected
    /// region, overlaps MMRAM.
    RegionOverlapsMmram {
        /// Which region it is.
        kind: &'static str,
        /// The region.
        range: MemoryRange,
    },
    /// An unblock region has a zero identifier GUID, so it names no requester.
    UnblockRegionWithoutIdentifier {
        /// The unblock region.
        range: MemoryRange,
    },
    /// A protected region is of a type other than IOMMU, the only type the producer applies.
    UnsupportedProtectedRegionType {
        /// The type the HOB reported.
        region_type: u64,
    },
    /// The CPU HOB reports a wider physical address space than the processor supports.
    CpuHobAddressWidthTooLarge {
        /// The CPU HOB's `SizeOfMemorySpace`.
        hob_bits: u8,
        /// The processor's MAXPHYADDR.
        cpuid_bits: u8,
    },
    /// A range that must not be present, such as page 0 or memory above the physical address
    /// width, is at least partly mapped.
    RangeMapped {
        /// The range.
        range: MemoryRange,
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
    /// A page is both writable (`ReadOnly` clear) and executable (`ExecuteProtect` clear).
    PageWritableAndExecutable {
        /// Base address of the offending page.
        addr: u64,
        /// The attribute bits that were actually set on the page.
        found: u64,
    },
    /// A CPU's SMBASE is not page aligned, so its SMI entry code and save state cannot be checked
    /// as whole pages.
    SmBaseNotPageAligned {
        /// Index of the CPU in the SMBASE array.
        cpu: usize,
        /// The SMBASE the array reported.
        smbase: u64,
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
            Self::MmramBelowOneMib { span } => write!(f, "MMRAM span {span} starts below 1 MiB"),
            Self::MemoryAllocationInsideMmram { range } => write!(f, "memory allocation HOB {range} overlaps MMRAM"),
            Self::AllocationModuleOutsideMmram { range } => {
                write!(f, "allocation module HOB {range} is outside MMRAM")
            }
            Self::AllocationModuleNotPageAligned { range } => {
                write!(f, "allocation module HOB {range} is not page aligned")
            }
            Self::MissingModule { has_init, has_supv_core, has_user } => write!(
                f,
                "required MM module missing: has_init={has_init}, has_supv_core={has_supv_core}, has_user={has_user}"
            ),
            Self::DuplicateModule { module_name } => {
                write!(f, "MM module {module_name} is described by more than one allocation module HOB")
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
            Self::MmramNotInMemoryDescriptor { mmram } => {
                write!(f, "MMRAM {mmram} is not inside a single memory resource descriptor")
            }
            Self::MmramInUnexpectedResourceType { range, resource_type } => write!(
                f,
                "resource descriptor {range} holding MMRAM has type 0x{resource_type:x}, not system or reserved memory"
            ),
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
            Self::RegionNotPageAligned { kind, range } => write!(f, "{kind} region {range} is not page aligned"),
            Self::RegionNotInAllocatedMmram { kind, range } => {
                write!(f, "{kind} region {range} is not inside MMRAM the producer marked allocated")
            }
            Self::RegionsOverlap { first_kind, first, second_kind, second } => {
                write!(f, "{first_kind} region {first} and {second_kind} region {second} share a page")
            }
            Self::InvalidMsegHob => write!(f, "MSEG SMRAM HOB does not describe a usable MSEG"),
            Self::GuidedHobTooSmall { guid, length } => {
                write!(f, "HOB with GUID {guid} has payload of {length} bytes which is too small for its parameters")
            }
            Self::RegionOverlapsMmram { kind, range } => write!(f, "{kind} {range} overlaps MMRAM"),
            Self::UnblockRegionWithoutIdentifier { range } => {
                write!(f, "unblock region {range} has a zero identifier GUID")
            }
            Self::UnsupportedProtectedRegionType { region_type } => {
                write!(f, "protected region type 0x{region_type:x} is not supported")
            }
            Self::CpuHobAddressWidthTooLarge { hob_bits, cpuid_bits } => {
                write!(f, "CPU HOB reports {hob_bits} physical address bits, more than the processor's {cpuid_bits}")
            }
            Self::RangeMapped { range } => write!(f, "{range} must not be present but is mapped"),
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
            Self::PageWritableAndExecutable { addr, found } => {
                write!(f, "page at 0x{addr:x} is both writable and executable (found 0x{found:x})")
            }
            Self::SmBaseNotPageAligned { cpu, smbase } => {
                write!(f, "CPU {cpu}: SMBASE 0x{smbase:x} is not page aligned")
            }
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

/// Validates that the scanned MMRAM regions form a single contiguous, non-overlapping span that
/// starts at or above 1 MiB, and returns that span. The regions are not required to be sorted.
pub(crate) fn validate_mmram_contiguous(regions: &[MmramRegion]) -> MmSupervisorResult<MemoryRange> {
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

    if span.base() < MMRAM_MIN_BASE {
        return Err(HobValidationError::MmramBelowOneMib { span }.into());
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
/// Every other check here reasons about regions the HOB list describes. The
/// list is itself a buffer at an address the producer chose, and the supervisor
/// reads it and copies it for Ring 3, so it has to be inside MMRAM for the same
/// reason its contents do.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
pub(crate) fn validate_hob_list_inside_mmram(
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
    let mut has_user = false;

    for current in hobs {
        if let Hob::MemoryAllocationModule(module) = current {
            let is_supv_module = module.alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID;
            let is_init = module.alloc_descriptor.name == HOB_MEMORY_ALLOC_MODULE_GUID
                && module.module_name == MM_SUPERVISOR_INIT_GUID;
            if !is_supv_module && !is_init {
                continue;
            }
            // The Init, Core and User modules are looked up by name, so each must be described once.
            let is_core = is_supv_module && module.module_name == MM_SUPERVISOR_CORE_GUID;
            let is_user = is_supv_module && module.module_name == MM_SUPERVISOR_USER_GUID;
            if (is_init && has_init) || (is_core && has_supv_core) || (is_user && has_user) {
                return Err(HobValidationError::DuplicateModule { module_name: module.module_name }.into());
            }
            has_init |= is_init;
            has_supv_core |= is_core;
            has_user |= is_user;
            let base = module.alloc_descriptor.memory_base_address;
            let length = module.alloc_descriptor.memory_length;
            let range = checked_range("allocation module HOB", base, length)?;
            if base == 0 {
                return Err(
                    HobValidationError::InvalidAddressRange { kind: "allocation module HOB", base, length }.into()
                );
            }
            if !matches!(range.is_aligned(UEFI_PAGE_SIZE as u64), Ok(true)) {
                return Err(HobValidationError::AllocationModuleNotPageAligned { range }.into());
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

    if !has_init || !has_supv_core || !has_user {
        return Err(HobValidationError::MissingModule { has_init, has_supv_core, has_user }.into());
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

/// Reports the first pair of resource descriptors in the same category whose ranges overlap, as a
/// [`HobValidationError::ResourceDescriptorsOverlap`].
///
/// `hobs` is walked again for each descriptor rather than the ranges being stored, so the
/// comparison needs no storage on the small MM stack.
fn check_category_overlap<'a>(hobs: impl IntoIterator<Item = Hob<'a>> + Clone) -> MmSupervisorResult<()> {
    for (index, (kind, first)) in hobs.clone().into_iter().filter_map(descriptor_category).enumerate() {
        for (other_kind, second) in hobs.clone().into_iter().filter_map(descriptor_category).skip(index + 1) {
            if other_kind == kind && first.overlaps(&second) {
                return Err(HobValidationError::ResourceDescriptorsOverlap { kind, first, second }.into());
            }
        }
    }
    Ok(())
}

/// Returns the category ("v1 memory" or "v1 I/O") and range of a V1 resource descriptor HOB, or
/// `None` for any other HOB, a memory bin descriptor, or a descriptor with an invalid range.
#[cfg(feature = "v1_resource_descriptor_support")]
fn descriptor_category(hob: Hob<'_>) -> Option<(&'static str, MemoryRange)> {
    let Hob::ResourceDescriptor(rd) = hob else {
        return None;
    };
    if is_memory_type_info(rd) {
        return None;
    }
    let kind = if is_io(rd.resource_type) { "v1 I/O" } else { "v1 memory" };
    MemoryRange::new(rd.physical_start, rd.resource_length).ok().map(|range| (kind, range))
}

/// Returns the category ("v2 memory" or "v2 I/O") and range of a V2 resource descriptor HOB, or
/// `None` for any other HOB, a memory bin descriptor, or a descriptor with an invalid range.
#[cfg(not(feature = "v1_resource_descriptor_support"))]
fn descriptor_category(hob: Hob<'_>) -> Option<(&'static str, MemoryRange)> {
    let Hob::ResourceDescriptorV2(rd) = hob else {
        return None;
    };
    if is_memory_type_info(&rd.v1) {
        return None;
    }
    let kind = if is_io(rd.v1.resource_type) { "v2 I/O" } else { "v2 memory" };
    MemoryRange::new(rd.v1.physical_start, rd.v1.resource_length).ok().map(|range| (kind, range))
}

/// Validates that the V1 resource descriptor HOBs do not overlap within each address space
/// (memory/I/O), and that one memory descriptor holds all of MMRAM.
///
/// With the `v1_resource_descriptor_support` feature only V1 descriptors are consumed, as in
/// `patina_dxe_core`, and V2 descriptors are ignored. Because memory descriptors may not overlap,
/// the descriptor holding MMRAM is the only one that touches MMRAM, so no MMIO or firmware-device
/// descriptor can overlap it.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
#[cfg(feature = "v1_resource_descriptor_support")]
fn validate_resource_descriptor_overlaps<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>> + Clone,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    let mut found_mmram_region = false;

    for current in hobs.clone() {
        let Hob::ResourceDescriptor(rd) = current else {
            continue;
        };
        // The PEI memory bin (Memory Type Info) HOB is expected to overlap the
        // system-memory resource descriptors backing it, so skip it.
        if is_memory_type_info(rd) {
            continue;
        }
        let (base, length) = (rd.physical_start, rd.resource_length);
        if is_io(rd.resource_type) {
            checked_range("v1 I/O resource descriptor HOB", base, length)?;
        } else {
            let range = checked_range("v1 memory resource descriptor HOB", base, length)?;
            if range.contains_range(&mmram) {
                // Only system memory or reserved memory may hold MMRAM.
                let resource_type = rd.resource_type;
                if resource_type != EFI_RESOURCE_SYSTEM_MEMORY && resource_type != EFI_RESOURCE_MEMORY_RESERVED {
                    return Err(HobValidationError::MmramInUnexpectedResourceType { range, resource_type }.into());
                }
                found_mmram_region = true;
            }
        }
    }

    check_category_overlap(hobs)?;

    if !found_mmram_region {
        return Err(HobValidationError::MmramNotInMemoryDescriptor { mmram }.into());
    }

    Ok(())
}

/// Validates that the V2 resource descriptor HOBs do not overlap within each address space
/// (memory/I/O), and that one memory descriptor holds all of MMRAM.
///
/// Without the `v1_resource_descriptor_support` feature only V2 descriptors are consumed, as in
/// `patina_dxe_core`, and V1 descriptors are ignored, so a platform that describes MMRAM only with
/// a V1 descriptor fails this check. Because memory descriptors may not overlap, the descriptor
/// holding MMRAM is the only one that touches MMRAM, so no MMIO or firmware-device descriptor can
/// overlap it.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
#[cfg(not(feature = "v1_resource_descriptor_support"))]
fn validate_resource_descriptor_overlaps<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>> + Clone,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    let mut found_mmram_region = false;

    for current in hobs.clone() {
        let Hob::ResourceDescriptorV2(rd) = current else {
            continue;
        };
        // The PEI memory bin (Memory Type Info) HOB is expected to overlap the
        // system-memory resource descriptors backing it, so skip it.
        if is_memory_type_info(&rd.v1) {
            continue;
        }
        let (base, length) = (rd.v1.physical_start, rd.v1.resource_length);
        if is_io(rd.v1.resource_type) {
            checked_range("v2 I/O resource descriptor HOB", base, length)?;
        } else {
            let range = checked_range("v2 memory resource descriptor HOB", base, length)?;
            if range.contains_range(&mmram) {
                // Only system memory or reserved memory may hold MMRAM.
                let resource_type = rd.v1.resource_type;
                if resource_type != EFI_RESOURCE_SYSTEM_MEMORY && resource_type != EFI_RESOURCE_MEMORY_RESERVED {
                    return Err(HobValidationError::MmramInUnexpectedResourceType { range, resource_type }.into());
                }
                found_mmram_region = true;
            }
        }
    }

    check_category_overlap(hobs)?;

    if !found_mmram_region {
        return Err(HobValidationError::MmramNotInMemoryDescriptor { mmram }.into());
    }

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

/// Validates the `PassDown` HOB present in the HOB list and returns its payload.
///
/// `mmram` is the span returned by [`validate_mmram_contiguous`].
fn validate_pass_down<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    mmram: MemoryRange,
) -> MmSupervisorResult<MmSupvPassDownHobData> {
    let data = hobs
        .into_iter()
        .find_map(|hob| match hob {
            Hob::GuidHob(guid_hob, data) if guid_hob.name == MM_SUPV_PASS_DOWN_HOB_GUID => Some(data),
            _ => None,
        })
        .ok_or(PassDownHobError::Missing)?;
    let (pass_down, _) = MmSupvPassDownHobData::read_from_prefix(data).map_err(|_| PassDownHobError::TooSmall)?;
    validate_pass_down_pointers(&pass_down, mmram)?;
    Ok(pass_down)
}

/// Returns the MSEG the MSEG SMRAM HOB describes, or `None` when the platform publishes no such
/// HOB because it has no STM.
///
/// A HOB that is present but describes no usable MSEG is an error rather than a missing MSEG.
fn validate_mseg<'a>(hobs: impl IntoIterator<Item = Hob<'a>>) -> MmSupervisorResult<Option<MemoryRange>> {
    let Some(data) = hobs.into_iter().find_map(|hob| match hob {
        Hob::GuidHob(guid_hob, data) if guid_hob.name == mseg::MSEG_SMRAM_HOB_GUID => Some(data),
        _ => None,
    }) else {
        return Ok(None);
    };
    let (base, length) = mseg::parse_mseg_smram_hob(data).ok_or(HobValidationError::InvalidMsegHob)?;
    checked_range("MSEG", base, length).map(Some)
}

/// Returns the range of a module HOB that [`validate_allocation_modules`] checks: the supervisor's
/// own modules and the standard PI Init module. Returns `None` for any other HOB.
fn supervisor_module_range(hob: Hob<'_>) -> Option<(&'static str, MmSupervisorResult<MemoryRange>)> {
    let Hob::MemoryAllocationModule(module) = hob else {
        return None;
    };
    let is_init =
        module.alloc_descriptor.name == HOB_MEMORY_ALLOC_MODULE_GUID && module.module_name == MM_SUPERVISOR_INIT_GUID;
    if module.alloc_descriptor.name != MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID && !is_init {
        return None;
    }
    let base = module.alloc_descriptor.memory_base_address;
    let length = module.alloc_descriptor.memory_length;
    Some(("allocation module", checked_range("allocation module HOB", base, length)))
}

/// Widens `range` to whole pages, requires every page to lie in MMRAM the producer marked
/// allocated, and returns the widened range.
///
/// Free MMRAM belongs to the page allocator, so a region there would alias pages it hands out,
/// such as the paging pool. `regions` are the scanned MMRAM regions.
fn allocated_pages(kind: &'static str, range: MemoryRange, regions: &[MmramRegion]) -> MmSupervisorResult<MemoryRange> {
    let pages = range
        .align_outward(UEFI_PAGE_SIZE as u64)
        .map_err(|_| HobValidationError::RegionNotInAllocatedMmram { kind, range })?;

    // The scanned regions do not overlap, so the parts they share with `pages` add up to the
    // bytes of `pages` they cover.
    let mut allocated = 0u64;
    for region in regions {
        if region.pre_allocated
            && let Ok(shared) = region.range.intersection(&pages)
        {
            allocated += shared.size();
        }
    }
    if allocated != pages.size() {
        return Err(HobValidationError::RegionNotInAllocatedMmram { kind, range: pages }.into());
    }

    Ok(pages)
}

/// Validates the regions in MMRAM the producer named and the supervisor keeps using: the module
/// allocations, the HOB list, the `PassDown` buffers, each at its full size for `number_of_cpus`,
/// MSEG when the platform has one, and the SMBASE area (see [`smbase_area`]).
///
/// Each region is widened to whole pages and must lie in MMRAM the producer marked allocated (see
/// [`allocated_pages`]). No two regions may share a page, and the Ring 3 stacks, which are
/// remapped user-writable, must start and end on page boundaries.
///
/// `regions` are the scanned MMRAM regions. `hobs` must already have passed
/// [`validate_allocation_modules`], and `mseg` is what [`validate_mseg`] returned.
fn validate_named_regions<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    hob_list: MemoryRange,
    pass_down: &MmSupvPassDownHobData,
    mseg: Option<MemoryRange>,
    number_of_cpus: usize,
    regions: &[MmramRegion],
) -> MmSupervisorResult<()> {
    // The regions the producer always names besides the module allocations.
    // - HOB list
    // - CPL3 stack base
    // - SMBASE area
    // - MM initialized buffer
    // - Firmware policy buffer
    const FIXED_NAMED_REGIONS: usize = 5;
    // The regions only some platforms name, skipped when absent.
    // - MSEG.
    const OPTIONAL_NAMED_REGIONS: usize = 1;

    let cpus = number_of_cpus as u64;
    let stacks =
        checked_range("cpl3_stack_base", pass_down.cpl3_stack_base, pass_down.cpl3_stack_size.saturating_mul(cpus))?;
    if !matches!(stacks.is_aligned(UEFI_PAGE_SIZE as u64), Ok(true)) {
        return Err(HobValidationError::RegionNotPageAligned { kind: "cpl3_stack_base", range: stacks }.into());
    }
    let others: [_; FIXED_NAMED_REGIONS] = [
        ("HOB list", Ok(hob_list)),
        ("cpl3_stack_base", Ok(stacks)),
        (
            "sm_base_array",
            checked_range("sm_base_array", pass_down.sm_base_array, (size_of::<u64>() as u64).saturating_mul(cpus)),
        ),
        ("mm_initialized_buffer", checked_range("mm_initialized_buffer", pass_down.mm_initialized_buffer, cpus)),
        (
            "firmware_policy_buffer",
            checked_range(
                "firmware_policy_buffer",
                pass_down.firmware_policy_buffer,
                pass_down.firmware_policy_buffer_size,
            ),
        ),
    ];

    let optional: [_; OPTIONAL_NAMED_REGIONS] = [mseg.map(|range| ("MSEG", Ok(range)))];

    // Every module allocation, which `validate_allocation_modules` capped at `MAX_ALLOC_MODULES`,
    // followed by the fixed and the optional regions.
    const MAX_NAMED_REGIONS: usize = MAX_ALLOC_MODULES + FIXED_NAMED_REGIONS + OPTIONAL_NAMED_REGIONS;
    let mut named: [Option<(&'static str, MemoryRange)>; MAX_NAMED_REGIONS] = [None; MAX_NAMED_REGIONS];
    for (index, (kind, range)) in hobs
        .into_iter()
        .filter_map(supervisor_module_range)
        .chain(others)
        .chain(optional.into_iter().flatten())
        .enumerate()
    {
        let slot =
            named.get_mut(index).ok_or(HobValidationError::TooManyAllocationModules { limit: MAX_ALLOC_MODULES })?;
        *slot = Some((kind, allocated_pages(kind, range?, regions)?));
    }

    // SAFETY: the loop above placed the SMBASE array's `number_of_cpus` entries in allocated MMRAM.
    let smbases = unsafe { smbase_array(pass_down, number_of_cpus) }?;
    let smbase_area = ("SMBASE area", allocated_pages("SMBASE area", smbase_area(smbases)?, regions)?);

    let named = named.iter().flatten().chain(core::iter::once(&smbase_area));
    for (index, &(first_kind, first)) in named.clone().enumerate() {
        for &(second_kind, second) in named.clone().skip(index + 1) {
            if first.overlaps(&second) {
                return Err(HobValidationError::RegionsOverlap { first_kind, first, second_kind, second }.into());
            }
        }
    }

    Ok(())
}

/// Returns the entries of the SMBASE array the `PassDown` HOB names, one per CPU, as raw bytes.
///
/// The entries are read as bytes, so the array need not be aligned.
///
/// ## Safety
///
/// The array's `number_of_cpus` entries must be readable and must not change while the returned
/// slice is in use. [`validate_named_regions`] establishes this by placing them in allocated
/// MMRAM.
unsafe fn smbase_array<'a>(
    pass_down: &MmSupvPassDownHobData,
    number_of_cpus: usize,
) -> MmSupervisorResult<&'a [[u8; size_of::<u64>()]]> {
    let size = number_of_cpus.checked_mul(size_of::<u64>()).ok_or(HobValidationError::InvalidAddressRange {
        kind: "sm_base_array",
        base: pass_down.sm_base_array,
        length: u64::MAX,
    })?;
    // SAFETY: the caller guarantees the `size` bytes at the array are readable and stable.
    let bytes = unsafe { core::slice::from_raw_parts(pass_down.sm_base_array as *const u8, size) };
    Ok(bytes.as_chunks().0)
}

/// Returns the SMRAM the CPUs use at their SMBASEs: from the lowest SMBASE + `0x8000`, where the
/// SMI entry code starts, to the highest SMBASE + `0x10000`, where the save-state map ends.
///
/// The SMBASEs are closer together than a CPU's window is long, so one CPU's entry code sits
/// beside another's save state. The windows are therefore one area rather than one per CPU.
fn smbase_area(smbases: &[[u8; size_of::<u64>()]]) -> MmSupervisorResult<MemoryRange> {
    let smbases = smbases.iter().copied().map(u64::from_ne_bytes);
    let (Some(lowest), Some(highest)) = (smbases.clone().min(), smbases.max()) else {
        return Err(HobValidationError::InvalidAddressRange { kind: "SMBASE area", base: 0, length: 0 }.into());
    };
    // Each window must fit the address space, so the offsets below cannot overflow.
    let lowest = checked_range("SMBASE area", lowest, SMRAM_WINDOW_SIZE)?;
    let highest = checked_range("SMBASE area", highest, SMRAM_WINDOW_SIZE)?;
    let base = lowest.base() + SMM_HANDLER_OFFSET;
    checked_range("SMBASE area", base, highest.end() - base)
}

/// Validates every unblock-region HOB, each a region outside MMRAM that the producer has already
/// mapped for MM code to access.
///
/// Each HOB must hold a full [`MmSupervisorUnblockMemoryParams`] describing a non-empty,
/// page-aligned range that does not overlap MMRAM, with a non-zero identifier GUID naming its
/// requester. `mmram` is the span returned by [`validate_mmram_contiguous`].
fn validate_unblock_regions<'a>(hobs: impl IntoIterator<Item = Hob<'a>>, mmram: MemoryRange) -> MmSupervisorResult<()> {
    for current in hobs {
        let Hob::GuidHob(guid_hob, data) = current else {
            continue;
        };
        if guid_hob.name != MM_SUPV_UNBLOCK_REGION_HOB_GUID {
            continue;
        }
        let payload =
            data.get(..MmSupervisorUnblockMemoryParams::SIZE).ok_or(HobValidationError::GuidedHobTooSmall {
                guid: MM_SUPV_UNBLOCK_REGION_HOB_GUID,
                length: data.len(),
            })?;
        // SAFETY: `payload` holds exactly one parameter structure from validated hob list. By
        // verifying the size above, and `read_unaligned` has no alignment requirement, the read
        // here is safe.
        let params = unsafe { payload.as_ptr().cast::<MmSupervisorUnblockMemoryParams>().read_unaligned() };

        let range = pages_outside_mmram(
            "unblock region",
            params.memory_descriptor.physical_start,
            params.memory_descriptor.number_of_pages,
            mmram,
        )?;

        if params.identifier_guid == BinaryGuid::ZERO {
            return Err(HobValidationError::UnblockRegionWithoutIdentifier { range }.into());
        }
    }

    Ok(())
}

/// Validates every protected-region HOB, each an MMIO region outside MMRAM, such as the IOMMU's
/// registers, that the producer has already mapped read-only and user-accessible so MM code cannot
/// reprogram it.
///
/// Each HOB must hold a full [`MmProtectedRegion`] of the IOMMU type, the only type the producer
/// applies, describing a non-empty, page-aligned range that does not overlap MMRAM. `mmram` is the
/// span returned by [`validate_mmram_contiguous`].
fn validate_protected_regions<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    for current in hobs {
        let Hob::GuidHob(guid_hob, data) = current else {
            continue;
        };
        if guid_hob.name != MM_PROTECTED_REGION_HOB_GUID {
            continue;
        }
        let (region, _) = MmProtectedRegion::read_from_prefix(data).map_err(|_| {
            HobValidationError::GuidedHobTooSmall { guid: MM_PROTECTED_REGION_HOB_GUID, length: data.len() }
        })?;

        if region.protected_region_type != ProtectedRegionType::Iommu as u64 {
            return Err(HobValidationError::UnsupportedProtectedRegionType {
                region_type: region.protected_region_type,
            }
            .into());
        }

        pages_outside_mmram("protected region", region.protected_region_base, region.protected_region_pages, mmram)?;
    }

    Ok(())
}

/// Returns the `pages` pages at `base` as a range, which must be non-empty, page aligned and clear
/// of MMRAM. `kind` names the region in errors.
///
/// The producer maps these regions for MM code with attributes meant for memory outside MMRAM, so
/// a region that reaches even partly into MMRAM would expose MMRAM pages with them.
fn pages_outside_mmram(
    kind: &'static str,
    base: u64,
    pages: u64,
    mmram: MemoryRange,
) -> MmSupervisorResult<MemoryRange> {
    let range = MemoryRange::from_pages(base, pages).map_err(|_| HobValidationError::InvalidAddressRange {
        kind,
        base,
        length: pages.saturating_mul(UEFI_PAGE_SIZE as u64),
    })?;

    if base == 0 || pages == 0 {
        return Err(HobValidationError::InvalidAddressRange {
            kind,
            base,
            length: pages.saturating_mul(UEFI_PAGE_SIZE as u64),
        }
        .into());
    }

    // The length is a page count, so only the base can be misaligned.
    if !matches!(range.is_aligned(UEFI_PAGE_SIZE as u64), Ok(true)) {
        return Err(HobValidationError::RegionNotPageAligned { kind, range }.into());
    }

    if range.overlaps(&mmram) {
        return Err(HobValidationError::RegionOverlapsMmram { kind, range }.into());
    }

    Ok(range)
}

/// Validates the critical incoming HOBs before their contents are consumed.
///
/// `mmram` is the span [`establish_mmram_bound`](crate::memory::mmram::establish_mmram_bound)
/// returned for `regions`, the MMRAM regions scanned from the HOB list. Every containment check
/// runs against that span rather than the page allocator, so this can run before the allocator has
/// committed to the producer's descriptors. `hob_list_len` is the length returned by
/// [`validate_hob_list_structure`]. `number_of_cpus` is the MP Information HOB's count, which
/// sizes the per-CPU `PassDown` buffers.
///
/// Returns the `PassDown` payload it validated, which the post-paging checks use.
pub(crate) fn validate_incoming_hobs_pre_paging_init(
    handoff: &PhaseHandoffInformationTable,
    hob_list_len: u64,
    regions: &[MmramRegion],
    mmram: MemoryRange,
    number_of_cpus: usize,
) -> MmSupervisorResult<MmSupvPassDownHobData> {
    // Runs before the remaining checks so the whole list is still visible when one of them fails.
    dump_hobs(handoff);
    dump_regions(regions);

    // `establish_mmram_bound` derived `mmram` with `validate_mmram_contiguous`, so MMRAM is that one
    // span and every containment check below tests against it instead of iterating the regions.
    let hob = Hob::Handoff(handoff);
    validate_memory_allocations(&hob, mmram)?;
    validate_allocation_modules(&hob, mmram)?;
    validate_resource_descriptor_overlaps(&hob, mmram)?;
    validate_resource_v2_memory_attributes(&hob)?;
    let pass_down = validate_pass_down(&hob, mmram)?;
    let mseg = validate_mseg(&hob)?;
    let hob_list = checked_range("HOB list", core::ptr::from_ref(handoff) as u64, hob_list_len)?;
    validate_named_regions(&hob, hob_list, &pass_down, mseg, number_of_cpus, regions)?;
    validate_unblock_regions(&hob, mmram)?;
    validate_protected_regions(&hob, mmram)?;

    Ok(pass_down)
}

/// Validates memory protections that can only be checked once the page table is
/// active.
///
/// Runs after the page table has been initialized (unlike [`validate_incoming_hobs_pre_paging_init`],
/// which runs before), so it can verify per-page attributes via
/// [`verify_page_attributes`]. `pass_down` and `number_of_cpus` are the payload and CPU count the
/// pre-paging checks accepted.
pub(crate) fn validate_incoming_hobs_post_paging_init(
    handoff: &PhaseHandoffInformationTable,
    pass_down: &MmSupvPassDownHobData,
    number_of_cpus: usize,
) -> MmSupervisorResult<()> {
    let hob = Hob::Handoff(handoff);
    // The MMI entry code sets CR4.LA57 when the producer built a 5-level table.
    let paging_type = if intrinsics::read_cr4() & intrinsics::CR4_LA57 != 0 {
        PagingType::Paging5Level
    } else {
        PagingType::Paging4Level
    };
    with_active_page_table(|page_table| {
        verify_module_page_protections(&hob, page_table)?;
        verify_smbase_page_protections(page_table, pass_down, number_of_cpus)?;
        verify_mseg_page_protections(page_table, validate_mseg(&hob)?)?;
        verify_mem_bound_page_protections(&hob, page_table, intrinsics::cpuid_physical_address_bits(), paging_type)
    })
}

/// Verifies the page protections of the MM Supervisor Core and User modules.
///
/// For each of the MM Supervisor's own `MemoryAllocationModule` HOBs, checks in `page_table`
/// that the Core module is supervisor-owned and the User module is user-accessible, and that
/// each module's entry point lies on a code page.
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
                // Supervisor core pages must be supervisor-owned (`Supervisor` set).
                verify_page_attributes(
                    page_table,
                    &range,
                    MemoryAttributes::Supervisor,
                    MemoryAttributes::ReadProtect,
                )?;
                verify_entry_point_on_code_page(page_table, module.entry_point)?;
            }
            _ => {
                log::info!("Verifying MM Supervisor user level module page protections: {range}");
                // User core and driver pages must be user-accessible (`Supervisor` clear).
                verify_page_attributes(
                    page_table,
                    &range,
                    MemoryAttributes::empty(),
                    MemoryAttributes::Supervisor | MemoryAttributes::ReadProtect,
                )?;
                verify_entry_point_on_code_page(page_table, module.entry_point)?;
            }
        }
    }

    Ok(())
}

/// Verifies the page protections of every CPU's SMRAM window, at each SMBASE in the `PassDown`
/// HOB's SMBASE array.
///
/// Every SMBASE must be page aligned. The SMI entry code at `SMBASE + 0x8000`, `mmi_entry_size`
/// bytes long, must lie on supervisor-only, read-only, executable pages: every SMI runs it. The
/// SMM descriptor and the save-state map, from `SMBASE + 0xFB00` to the end of the 64 KiB window,
/// share one page, which must be supervisor-only and non-executable.
///
/// The SMBASE array must already have passed [`validate_named_regions`], which places its
/// `number_of_cpus` entries in allocated MMRAM.
fn verify_smbase_page_protections(
    page_table: &impl PageTable,
    pass_down: &MmSupvPassDownHobData,
    number_of_cpus: usize,
) -> MmSupervisorResult<()> {
    // SAFETY: `validate_named_regions` placed the SMBASE array's `number_of_cpus` entries in
    // allocated MMRAM.
    let entries = unsafe { smbase_array(pass_down, number_of_cpus) }?;

    for (cpu, entry) in entries.iter().enumerate() {
        let smbase = u64::from_ne_bytes(*entry);
        if !smbase.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            return Err(HobValidationError::SmBaseNotPageAligned { cpu, smbase }.into());
        }
        // Both offsets below lie inside the window, so neither addition can overflow.
        let window = checked_range("SMRAM window", smbase, SMRAM_WINDOW_SIZE)?;

        let entry_code = checked_range("SMI entry code", window.base() + SMM_HANDLER_OFFSET, pass_down.mmi_entry_size)?;
        verify_page_attributes(
            page_table,
            &entry_code,
            MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly,
            MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadProtect,
        )?;

        let descriptor_and_save_state = checked_range(
            "SMM descriptor and save state",
            window.base() + SMM_PSD_OFFSET,
            SMRAM_WINDOW_SIZE - SMM_PSD_OFFSET,
        )?;
        verify_page_attributes(
            page_table,
            &descriptor_and_save_state,
            MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect,
            MemoryAttributes::ReadProtect,
        )?;
    }

    Ok(())
}

/// Verifies that MSEG, when the platform has one, lies on supervisor-only, non-executable pages.
///
/// `mseg` is what [`validate_mseg`] returned. [`validate_named_regions`] already placed it in
/// allocated MMRAM, apart from every other region.
fn verify_mseg_page_protections(page_table: &impl PageTable, mseg: Option<MemoryRange>) -> MmSupervisorResult<()> {
    let Some(mseg) = mseg else {
        return Ok(());
    };
    verify_page_attributes(
        page_table,
        &mseg,
        MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect,
        MemoryAttributes::ReadProtect,
    )
}

/// Verifies the bounds of what the page table maps: page 0 and everything at or above the physical
/// address width must not be present.
///
/// The width is the first CPU HOB's `SizeOfMemorySpace`, which may not exceed the processor's
/// MAXPHYADDR (`cpuid_bits`), or `cpuid_bits` when there is no CPU HOB, as the producer chooses it.
/// The check stops at the top of the lower canonical half `paging_type` identity-maps, 2^47 for
/// 4-level and 2^56 for 5-level paging. Like the producer, the width is capped there, which only
/// takes effect for 4-level paging since MAXPHYADDR is at most 52.
///
/// `patina_paging` reports a not-present page the same as an unmapped one, so both pass.
fn verify_mem_bound_page_protections<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>>,
    page_table: &impl PageTable,
    cpuid_bits: u8,
    paging_type: PagingType,
) -> MmSupervisorResult<()> {
    let canonical_bits = match paging_type {
        PagingType::Paging4Level => 47,
        PagingType::Paging5Level => 56,
    };
    let hob_bits = hobs.into_iter().find_map(|hob| match hob {
        Hob::Cpu(cpu) => Some(cpu.size_of_memory_space),
        _ => None,
    });
    let bit_width = match hob_bits {
        Some(hob_bits) if hob_bits > cpuid_bits => {
            return Err(HobValidationError::CpuHobAddressWidthTooLarge { hob_bits, cpuid_bits }.into());
        }
        Some(hob_bits) => hob_bits,
        None => cpuid_bits,
    }
    .min(canonical_bits);

    // Page 0 stays unmapped so a null pointer dereference faults, and nothing exists above the
    // physical address width.
    let top = 1u64 << canonical_bits;
    let high_addr = 1u64 << bit_width;
    let page_zero = checked_range("page 0", 0, UEFI_PAGE_SIZE as u64)?;
    let above_width = if high_addr < top {
        Some(checked_range("above the physical address width", high_addr, top - high_addr)?)
    } else {
        None
    };
    for range in [Some(page_zero), above_width].into_iter().flatten() {
        if !matches!(page_table.query_memory_region(range.base(), range.size()), Err(PtError::NoMapping)) {
            return Err(HobValidationError::RangeMapped { range }.into());
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
/// routine: user-accessible (`Supervisor` clear), present, writable, and non-executable.
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
        // Writable and executable are the absence of `ReadOnly` and `ExecuteProtect`, so a page
        // carrying neither is an automatic failure.
        if !attrs.intersects(MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect) {
            return Err(HobValidationError::PageWritableAndExecutable { addr, found: attrs.bits() }.into());
        }

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
