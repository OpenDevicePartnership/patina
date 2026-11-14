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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::error::MmSupervisorError;
    use crate::hob::pass_down::{MM_SUPV_PASS_DOWN_HOB_GUID, MM_SUPV_PASS_DOWN_HOB_REVISION};
    use core::mem::size_of;
    use patina_paging::{MockPageTable, PtError};
    use zerocopy::IntoBytes;

    use crate::test_support::{PageAlignedMemory, init_global_state_over};

    use patina::pi::BootMode;
    use patina::pi::hob::{
        EFI_RESOURCE_FIRMWARE_DEVICE, EFI_RESOURCE_MEMORY_MAPPED_IO, EFI_RESOURCE_SYSTEM_MEMORY, END_OF_HOB_LIST,
        GUID_EXTENSION, GuidHob, HANDOFF, HobHeader, MEMORY_ALLOCATION, MEMORY_POOL, MemoryAllocation,
        MemoryAllocationHeader, MemoryAllocationModule, RESOURCE_DESCRIPTOR, ResourceDescriptorV2, UNUSED,
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

    /// Name of an MM driver module: not the Init, Core or User module.
    const TEST_DRIVER_GUID: patina::BinaryGuid =
        patina::BinaryGuid::from_string("0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1f0");

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
        // `From` keeps the V1 header length, so a HOB list walk would read the V2 descriptor short.
        descriptor.v1.header.length = size_of::<ResourceDescriptorV2>() as u16;
        descriptor.attributes = attributes;
        descriptor
    }

    /// MMRAM for the resource descriptor tests, clear of the ranges they describe, and the reserved
    /// memory descriptor holding it, as QEMU describes TSEG.
    fn descriptor_test_mmram() -> (MemoryRange, ResourceDescriptorV2) {
        let mmram = span(0x100_0000, 0x10_0000);
        (mmram, resource_descriptor_v2(EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size(), efi::MEMORY_WB))
    }

    fn guid_hob(name: patina::BinaryGuid, data_len: usize) -> GuidHob {
        GuidHob {
            header: HobHeader { r#type: GUID_EXTENSION, length: (size_of::<GuidHob>() + data_len) as u16, reserved: 0 },
            name,
        }
    }

    /// The unblock parameters for `pages` pages at `start`, as the producer lays them out: an
    /// `EFI_MEMORY_DESCRIPTOR` followed by the identifier GUID, which is non-zero when `identified`.
    fn unblock_params_bytes(start: u64, pages: u64, identified: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(MmSupervisorUnblockMemoryParams::SIZE);
        bytes.extend_from_slice(&0u32.to_ne_bytes()); // Type
        bytes.extend_from_slice(&0u32.to_ne_bytes()); // Padding
        bytes.extend_from_slice(&start.to_ne_bytes()); // PhysicalStart
        bytes.extend_from_slice(&0u64.to_ne_bytes()); // VirtualStart
        bytes.extend_from_slice(&pages.to_ne_bytes()); // NumberOfPages
        bytes.extend_from_slice(&0u64.to_ne_bytes()); // Attribute
        bytes.extend_from_slice(&[u8::from(identified); 16]); // IdentifierGuid
        assert_eq!(bytes.len(), MmSupervisorUnblockMemoryParams::SIZE);
        bytes
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
        /// A HOB list with Init, Core, User, the required `PassDown` HOB, and the V1 and V2
        /// descriptors holding MMRAM, so it validates with or without V1 descriptor support.
        #[repr(C, align(8))]
        struct PassDownHobList {
            handoff: PhaseHandoffInformationTable,
            core: MemoryAllocationModule,
            init: MemoryAllocationModule,
            user: MemoryAllocationModule,
            guid: GuidHob,
            pass_down: MmSupvPassDownHobData,
            tseg_v1: ResourceDescriptor,
            tseg_v2: ResourceDescriptorV2,
            end: HobHeader,
        }

        // Two SMBASEs 8 KiB apart, as the producer tiles them, place the SMBASE area at
        // [0x20_8000, 0x21_2000), clear of every other buffer the producer named.
        let smbase_array = PageAlignedMemory::new(1);
        // SAFETY: `smbase_array` is a live, page-aligned page, large enough for two u64s.
        unsafe { core::ptr::write(smbase_array.base() as *mut [u64; 2], [0x20_0000, 0x20_2000]) };

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
            user: allocation_module(
                MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
                MM_SUPERVISOR_USER_GUID,
                0x9000,
                0x1000,
                0x9000,
            ),
            guid: guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, size_of::<MmSupvPassDownHobData>()),
            // Each buffer in its own pages after the modules, as the producer allocates them. The
            // SMBASE array is read, so it is real memory.
            pass_down: MmSupvPassDownHobData {
                cpl3_stack_base: 0x4000,
                sm_base_array: smbase_array.base(),
                mm_initialized_buffer: 0x7000,
                firmware_policy_buffer: 0x8000,
                ..pass_down()
            },
            // Both cover MMRAM once its span is known below.
            tseg_v1: resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_MEMORY_RESERVED, 0, 0x1000),
            tseg_v2: resource_descriptor_v2(EFI_RESOURCE_MEMORY_RESERVED, 0, 0x1000, efi::MEMORY_WB),
            end: HobHeader { r#type: END_OF_HOB_LIST, length: size_of::<HobHeader>() as u16, reserved: 0 },
        };
        let list_end = (&raw const list).addr() as u64 + size_of::<PassDownHobList>() as u64;
        // One allocated region from the lowest address the producer named up to the page holding
        // the end of the list on the stack or of the SMBASE array, so the modules, the PassDown
        // buffers, the SMBASE area and every page of the list itself are inside it.
        let top = list_end.max(smbase_array.base() + smbase_array.size()).next_multiple_of(0x1000);
        let mmram = span(0x1000, top - 0x1000);
        list.tseg_v1 =
            resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size());
        list.tseg_v2 = resource_descriptor_v2(EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size(), efi::MEMORY_WB);
        let regions = [MmramRegion::new(mmram, true)];
        let cpus = 2;

        // Everything the producer named resolves inside the scanned regions, so the whole
        // pre-paging sequence runs to completion.
        // SAFETY: `list` is a contiguous, END_OF_HOB_LIST-terminated HOB list on the stack.
        let hob_list_len = unsafe { validate_hob_list_structure(&list.handoff) };
        assert_eq!(hob_list_len, Ok(size_of::<PassDownHobList>() as u64));
        let hob_list_len = hob_list_len.expect("the list is well formed");
        assert_eq!(validate_hob_list_inside_mmram(&list.handoff, hob_list_len, mmram), Ok(()));
        assert!(validate_incoming_hobs_pre_paging_init(&list.handoff, hob_list_len, &regions, mmram, cpus).is_ok());
        list.init.alloc_descriptor.memory_base_address = 0x1000;
        assert!(matches!(
            validate_incoming_hobs_pre_paging_init(&list.handoff, hob_list_len, &regions, mmram, cpus),
            Err(MmSupervisorError::HobValidation(HobValidationError::AllocationModulesOverlap { .. }))
        ));

        // The HOB list itself is a buffer at an address the producer chose, so it is rejected
        // when it falls outside the regions it described.
        assert!(matches!(
            validate_hob_list_inside_mmram(&list.handoff, hob_list_len, span(0x1000, 0x8000)),
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
        assert_eq!(validate_mmram_contiguous(&[region(0x10_1000, 0x2000)]), Ok(span(0x10_1000, 0x2000)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_contiguous_adjacent_unsorted() {
        // Two adjacent regions provided out of order collapse to one [min_base, max_end) span.
        let regions = [region(0x10_3000, 0x1000), region(0x10_1000, 0x2000)];
        assert_eq!(validate_mmram_contiguous(&regions), Ok(span(0x10_1000, 0x3000)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_empty_regions_rejected() {
        assert_eq!(validate_mmram_contiguous(&[]), Err(HobValidationError::NoMmramRegions.into()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_overlapping_regions_rejected() {
        let regions = [region(0x10_1000, 0x2000), region(0x10_2000, 0x2000)];
        assert!(matches!(
            validate_mmram_contiguous(&regions),
            Err(MmSupervisorError::HobValidation(HobValidationError::MmramRegionsOverlap { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_gap_rejected() {
        // Gap between 0x10_2000 and 0x10_3000.
        let regions = [region(0x10_1000, 0x1000), region(0x10_3000, 0x1000)];
        assert!(matches!(
            validate_mmram_contiguous(&regions),
            Err(MmSupervisorError::HobValidation(HobValidationError::MmramNotContiguous { .. }))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_mmram_below_one_mib_rejected() {
        // Starting at 1 MiB is accepted.
        assert_eq!(validate_mmram_contiguous(&[region(0x10_0000, 0x1000)]), Ok(span(0x10_0000, 0x1000)));

        // A span that starts lower is rejected, even when it reaches above 1 MiB.
        for regions in [vec![region(0xF_F000, 0x1000)], vec![region(0xF_F000, 0x1000), region(0x10_0000, 0x1000)]] {
            let span = MemoryRange::collapse(regions.iter().map(|region| region.range)).unwrap();
            assert_eq!(validate_mmram_contiguous(&regions), Err(HobValidationError::MmramBelowOneMib { span }.into()));
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_memory_allocations() {
        let outside = memory_allocation(0x3000, 0x1000);
        assert_eq!(validate_memory_allocations([Hob::MemoryAllocation(&outside)], span(0x1000, 0x1000)), Ok(()));

        let inside = memory_allocation(0x1800, 0x1000);
        assert_eq!(
            validate_memory_allocations([Hob::MemoryAllocation(&inside)], span(0x1000, 0x1000)),
            Err(HobValidationError::MemoryAllocationInsideMmram { range: span(0x1800, 0x1000) }.into())
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
            Err(HobValidationError::AllocationModuleOutsideMmram { range: span(0x4000, 0x1000) }.into())
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
                range: span(0x1000, 0x1000)
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

        // Drivers are not limited to one HOB each, so repeating one reaches the module limit.
        let module =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, TEST_DRIVER_GUID, 0x1000, 0x1000, 0x1000);
        let hobs = (0..=MAX_ALLOC_MODULES).map(|_| Hob::MemoryAllocationModule(&module));
        assert_eq!(
            validate_allocation_modules(hobs, mmram()),
            Err(HobValidationError::TooManyAllocationModules { limit: MAX_ALLOC_MODULES }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_rejects_duplicate_modules() {
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x1000, 0x1000, 0x1000);
        let core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x2000,
            0x1000,
            0x2000,
        );
        let user = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x3000,
            0x1000,
            0x3000,
        );

        // A second, non-overlapping HOB for the Init, Core or User module is rejected by name.
        for (alloc_name, module_name) in [
            (HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID),
            (MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_CORE_GUID),
            (MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID),
        ] {
            let duplicate = allocation_module(alloc_name, module_name, 0x5000, 0x1000, 0x5000);
            assert_eq!(
                validate_allocation_modules(
                    [
                        Hob::MemoryAllocationModule(&init),
                        Hob::MemoryAllocationModule(&core),
                        Hob::MemoryAllocationModule(&user),
                        Hob::MemoryAllocationModule(&duplicate),
                    ],
                    span(0x1000, 0x8000),
                ),
                Err(HobValidationError::DuplicateModule { module_name }.into())
            );
        }

        // Two drivers with the same name are not the modules looked up by name, so both pass.
        let driver =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, TEST_DRIVER_GUID, 0x4000, 0x1000, 0x4000);
        let same_driver =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, TEST_DRIVER_GUID, 0x5000, 0x1000, 0x5000);
        assert_eq!(
            validate_allocation_modules(
                [
                    Hob::MemoryAllocationModule(&init),
                    Hob::MemoryAllocationModule(&core),
                    Hob::MemoryAllocationModule(&user),
                    Hob::MemoryAllocationModule(&driver),
                    Hob::MemoryAllocationModule(&same_driver),
                ],
                span(0x1000, 0x8000),
            ),
            Ok(())
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
        let user = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x3000,
            0x1000,
            0x3000,
        );
        assert_eq!(
            validate_allocation_modules(
                [
                    Hob::MemoryAllocationModule(&module),
                    Hob::MemoryAllocationModule(&core),
                    Hob::MemoryAllocationModule(&init),
                    Hob::MemoryAllocationModule(&user)
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
                Err(HobValidationError::MissingModule { has_init: false, has_supv_core, has_user: false }.into())
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
            (0x4001, 0x2000, HobValidationError::AllocationModuleNotPageAligned { range: span(0x4001, 0x2000) }),
            (0x4000, 0x2001, HobValidationError::AllocationModuleNotPageAligned { range: span(0x4000, 0x2001) }),
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
    fn test_mm_supervisor_hob_validation_requires_every_module_page_aligned() {
        // Not only the Init allocation: every module is checked and reclaimed in whole pages.
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x1000, 0x1000, 0x1000);
        for module_name in [MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_USER_GUID, TEST_DRIVER_GUID] {
            for (base, length) in [(0x2001, 0x1000), (0x2000, 0x1001)] {
                let module =
                    allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, module_name, base, length, base);
                assert_eq!(
                    validate_allocation_modules(
                        [Hob::MemoryAllocationModule(&init), Hob::MemoryAllocationModule(&module)],
                        span(0x1000, 0x8000),
                    ),
                    Err(HobValidationError::AllocationModuleNotPageAligned { range: span(base, length) }.into())
                );
            }
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_init_requires_core_and_rejects_overlap() {
        let init = allocation_module(HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x4000, 0x2000, 0x4100);
        assert_eq!(
            validate_allocation_modules([Hob::MemoryAllocationModule(&init)], mmram()),
            Err(HobValidationError::MissingModule { has_init: true, has_supv_core: false, has_user: false }.into())
        );
        let user = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_USER_GUID,
            0x8000,
            0x1000,
            0x8000,
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
                    [
                        Hob::MemoryAllocationModule(&core),
                        Hob::MemoryAllocationModule(&init),
                        Hob::MemoryAllocationModule(&user)
                    ],
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
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    fn test_mm_supervisor_hob_validation_resource_descriptor_categories() {
        let (mmram, tseg) = descriptor_test_mmram();
        let memory_a = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        let memory_b = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1800, 0x1000, efi::MEMORY_WB);
        // Descriptors that only touch do not overlap.
        let memory_c = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x2000, 0x1000, efi::MEMORY_WB);
        assert_eq!(
            validate_resource_descriptor_overlaps(
                [
                    Hob::ResourceDescriptorV2(&memory_a),
                    Hob::ResourceDescriptorV2(&memory_c),
                    Hob::ResourceDescriptorV2(&tseg)
                ],
                mmram
            ),
            Ok(())
        );
        assert!(matches!(
            validate_resource_descriptor_overlaps(
                [Hob::ResourceDescriptorV2(&memory_a), Hob::ResourceDescriptorV2(&memory_b)],
                mmram
            ),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v2 memory",
                ..
            }))
        ));

        let io_a = resource_descriptor_v2(EFI_RESOURCE_IO, 0x1000, 0x1000, 0);
        let io_b = resource_descriptor_v2(EFI_RESOURCE_IO, 0x1800, 0x1000, 0);
        assert!(matches!(
            validate_resource_descriptor_overlaps(
                [Hob::ResourceDescriptorV2(&io_a), Hob::ResourceDescriptorV2(&io_b)],
                mmram
            ),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v2 I/O",
                ..
            }))
        ));

        // Memory and I/O are separate address spaces, so the same range in each is not an overlap.
        assert_eq!(
            validate_resource_descriptor_overlaps(
                [
                    Hob::ResourceDescriptorV2(&memory_a),
                    Hob::ResourceDescriptorV2(&io_a),
                    Hob::ResourceDescriptorV2(&tseg)
                ],
                mmram
            ),
            Ok(())
        );
    }

    #[test]
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    fn test_mm_supervisor_hob_validation_resource_descriptor_exemptions() {
        let (mmram, tseg) = descriptor_test_mmram();
        let normal = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        let memory_type_info = ResourceDescriptorV2::from(resource_descriptor(
            MEMORY_TYPE_INFO_HOB_GUID,
            EFI_RESOURCE_SYSTEM_MEMORY,
            0x1000,
            0x1000,
        ));
        assert_eq!(
            validate_resource_descriptor_overlaps(
                [
                    Hob::ResourceDescriptorV2(&normal),
                    Hob::ResourceDescriptorV2(&memory_type_info),
                    Hob::ResourceDescriptorV2(&tseg)
                ],
                mmram
            ),
            Ok(())
        );
    }

    #[test]
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    fn test_mm_supervisor_hob_validation_resource_descriptor_invalid_range() {
        let (mmram, _) = descriptor_test_mmram();
        for (resource_type, kind) in [
            (EFI_RESOURCE_SYSTEM_MEMORY, "v2 memory resource descriptor HOB"),
            (EFI_RESOURCE_IO, "v2 I/O resource descriptor HOB"),
        ] {
            let invalid = resource_descriptor_v2(resource_type, u64::MAX, 1, 0);
            assert_eq!(
                validate_resource_descriptor_overlaps([Hob::ResourceDescriptorV2(&invalid)], mmram),
                Err(HobValidationError::InvalidAddressRange { kind, base: u64::MAX, length: 1 }.into())
            );
        }
    }

    #[test]
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    fn test_mm_supervisor_hob_validation_resource_descriptor_holding_mmram() {
        let (mmram, tseg) = descriptor_test_mmram();
        let check = |resource_type: u32, base: u64, length: u64| {
            let descriptor = resource_descriptor_v2(resource_type, base, length, efi::MEMORY_WB);
            validate_resource_descriptor_overlaps([Hob::ResourceDescriptorV2(&descriptor)], mmram)
        };

        // Reserved memory, as QEMU describes TSEG, or system memory may hold MMRAM, including as part
        // of a larger descriptor.
        assert_eq!(check(EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size()), Ok(()));
        assert_eq!(check(EFI_RESOURCE_SYSTEM_MEMORY, 0x80_0000, 0x100_0000), Ok(()));

        // MMIO or firmware-device memory may not.
        for resource_type in [EFI_RESOURCE_MEMORY_MAPPED_IO, EFI_RESOURCE_FIRMWARE_DEVICE] {
            assert_eq!(
                check(resource_type, mmram.base(), mmram.size()),
                Err(HobValidationError::MmramInUnexpectedResourceType { range: mmram, resource_type }.into())
            );
        }

        // One descriptor must hold all of MMRAM, not just part of it.
        assert_eq!(
            check(EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size() / 2),
            Err(HobValidationError::MmramNotInMemoryDescriptor { mmram }.into())
        );

        // No other memory descriptor may reach into MMRAM, which shows up as an overlap.
        let mmio = resource_descriptor_v2(EFI_RESOURCE_MEMORY_MAPPED_IO, mmram.end() - 0x1000, 0x2000, efi::MEMORY_UC);
        assert!(matches!(
            validate_resource_descriptor_overlaps(
                [Hob::ResourceDescriptorV2(&tseg), Hob::ResourceDescriptorV2(&mmio)],
                mmram
            ),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v2 memory",
                ..
            }))
        ));
    }

    #[test]
    #[cfg(feature = "v1_resource_descriptor_support")]
    fn test_mm_supervisor_hob_validation_v1_resource_descriptors_checked_with_feature() {
        let (mmram, _) = descriptor_test_mmram();
        let memory_a = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000);
        let memory_b = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1800, 0x1000);
        assert!(matches!(
            validate_resource_descriptor_overlaps(
                [Hob::ResourceDescriptor(&memory_a), Hob::ResourceDescriptor(&memory_b)],
                mmram
            ),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v1 memory",
                ..
            }))
        ));

        let io_a = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_IO, 0x1000, 0x1000);
        let io_b = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_IO, 0x1800, 0x1000);
        assert!(matches!(
            validate_resource_descriptor_overlaps(
                [Hob::ResourceDescriptor(&io_a), Hob::ResourceDescriptor(&io_b)],
                mmram
            ),
            Err(MmSupervisorError::HobValidation(HobValidationError::ResourceDescriptorsOverlap {
                kind: "v1 I/O",
                ..
            }))
        ));

        // V2 descriptors are ignored with the feature, so one that overlaps a V1 descriptor, or that
        // holds MMRAM, does not count. The memory bin descriptor is skipped. A V1 descriptor of
        // reserved memory holds MMRAM.
        let memory_type_info =
            resource_descriptor(MEMORY_TYPE_INFO_HOB_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000);
        let v2 = resource_descriptor_v2(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000, efi::MEMORY_WB);
        let v1_tseg =
            resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size());
        assert_eq!(
            validate_resource_descriptor_overlaps(
                [
                    Hob::ResourceDescriptor(&memory_a),
                    Hob::ResourceDescriptor(&io_a),
                    Hob::ResourceDescriptor(&memory_type_info),
                    Hob::ResourceDescriptorV2(&v2),
                    Hob::ResourceDescriptor(&v1_tseg),
                ],
                mmram
            ),
            Ok(())
        );
        let (_, v2_tseg) = descriptor_test_mmram();
        assert_eq!(
            validate_resource_descriptor_overlaps([Hob::ResourceDescriptorV2(&v2_tseg)], mmram),
            Err(HobValidationError::MmramNotInMemoryDescriptor { mmram }.into())
        );

        // Neither MMIO nor firmware-device memory may hold MMRAM.
        for resource_type in [EFI_RESOURCE_MEMORY_MAPPED_IO, EFI_RESOURCE_FIRMWARE_DEVICE] {
            let v1_device = resource_descriptor(MM_SUPERVISOR_CORE_GUID, resource_type, mmram.base(), mmram.size());
            assert_eq!(
                validate_resource_descriptor_overlaps([Hob::ResourceDescriptor(&v1_device)], mmram),
                Err(HobValidationError::MmramInUnexpectedResourceType { range: mmram, resource_type }.into())
            );
        }

        for (resource_type, kind) in [
            (EFI_RESOURCE_SYSTEM_MEMORY, "v1 memory resource descriptor HOB"),
            (EFI_RESOURCE_IO, "v1 I/O resource descriptor HOB"),
        ] {
            let invalid = resource_descriptor(MM_SUPERVISOR_CORE_GUID, resource_type, u64::MAX, 1);
            assert_eq!(
                validate_resource_descriptor_overlaps([Hob::ResourceDescriptor(&invalid)], mmram),
                Err(HobValidationError::InvalidAddressRange { kind, base: u64::MAX, length: 1 }.into())
            );
        }
    }

    #[test]
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    fn test_mm_supervisor_hob_validation_v1_resource_descriptors_ignored_without_feature() {
        // Without the feature V1 descriptors are not consumed, so overlapping and invalid ones are
        // not reported, and a V1 descriptor holding MMRAM does not count.
        let (mmram, tseg) = descriptor_test_mmram();
        let memory_a = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0x1000);
        let memory_b = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, 0x1800, 0x1000);
        let invalid = resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_SYSTEM_MEMORY, u64::MAX, 1);
        let v1_tseg =
            resource_descriptor(MM_SUPERVISOR_CORE_GUID, EFI_RESOURCE_MEMORY_RESERVED, mmram.base(), mmram.size());
        let v1 = [
            Hob::ResourceDescriptor(&memory_a),
            Hob::ResourceDescriptor(&memory_b),
            Hob::ResourceDescriptor(&invalid),
            Hob::ResourceDescriptor(&v1_tseg),
        ];
        assert_eq!(
            validate_resource_descriptor_overlaps(v1.clone(), mmram),
            Err(HobValidationError::MmramNotInMemoryDescriptor { mmram }.into())
        );
        assert_eq!(
            validate_resource_descriptor_overlaps(v1.into_iter().chain([Hob::ResourceDescriptorV2(&tseg)]), mmram),
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
        assert!(validate_pass_down([Hob::GuidHob(&matching, &data)], mmram()).is_ok());

        let unrelated = guid_hob(MM_SUPERVISOR_CORE_GUID, data.len());
        assert!(matches!(
            validate_pass_down([Hob::GuidHob(&unrelated, &data)], mmram()),
            Err(MmSupervisorError::PassDownHob(PassDownHobError::Missing))
        ));

        let short_data = [0u8; 4];
        let short = guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, short_data.len());
        assert!(matches!(
            validate_pass_down([Hob::GuidHob(&short, &short_data)], mmram()),
            Err(MmSupervisorError::PassDownHob(PassDownHobError::TooSmall))
        ));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_mseg() {
        let data = crate::test_support::mseg_smram_hob_data(0x40_0000, 0x40_0000, 0x2_0000);
        let mseg = guid_hob(mseg::MSEG_SMRAM_HOB_GUID, data.len());
        assert_eq!(validate_mseg([Hob::GuidHob(&mseg, &data)]), Ok(Some(span(0x40_0000, 0x2_0000))));

        // A platform without an STM publishes no MSEG SMRAM HOB.
        let unrelated = guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, data.len());
        assert_eq!(validate_mseg([Hob::GuidHob(&unrelated, &data)]), Ok(None));

        // A HOB that is present but unusable is rejected, not treated as missing.
        let empty = crate::test_support::mseg_smram_hob_data(0x40_0000, 0x40_0000, 0);
        assert_eq!(validate_mseg([Hob::GuidHob(&mseg, &empty)]), Err(HobValidationError::InvalidMsegHob.into()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_unblock_regions() {
        // `mmram()` is [0x1000, 0x10_1000), so 0x20_0000 lies outside it.
        let check = |data: &[u8]| {
            let hob = guid_hob(MM_SUPV_UNBLOCK_REGION_HOB_GUID, data.len());
            validate_unblock_regions([Hob::GuidHob(&hob, data)], mmram())
        };
        assert_eq!(check(&unblock_params_bytes(0x20_0000, 2, true)), Ok(()));

        // Other GUID HOBs are not unblock regions.
        let other = guid_hob(MM_SUPV_PASS_DOWN_HOB_GUID, 4);
        assert_eq!(validate_unblock_regions([Hob::GuidHob(&other, &[0; 4])], mmram()), Ok(()));

        let params = unblock_params_bytes(0x20_0000, 2, true);
        assert_eq!(
            check(&params[..params.len() - 1]),
            Err(HobValidationError::GuidedHobTooSmall {
                guid: MM_SUPV_UNBLOCK_REGION_HOB_GUID,
                length: params.len() - 1
            }
            .into())
        );
        assert_eq!(
            check(&unblock_params_bytes(0x20_0000, 0, true)),
            Err(HobValidationError::InvalidAddressRange { kind: "unblock region", base: 0x20_0000, length: 0 }.into())
        );
        assert_eq!(
            check(&unblock_params_bytes(0x20_0800, 1, true)),
            Err(HobValidationError::RegionNotPageAligned { kind: "unblock region", range: span(0x20_0800, 0x1000) }
                .into())
        );

        // Inside MMRAM, or reaching into it across its end.
        for (start, range) in [(0x2000, span(0x2000, 0x2000)), (0x10_0000, span(0x10_0000, 0x2000))] {
            assert_eq!(
                check(&unblock_params_bytes(start, 2, true)),
                Err(HobValidationError::RegionOverlapsMmram { kind: "unblock region", range }.into())
            );
        }

        assert_eq!(
            check(&unblock_params_bytes(0x20_0000, 2, false)),
            Err(HobValidationError::UnblockRegionWithoutIdentifier { range: span(0x20_0000, 0x2000) }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_protected_regions() {
        // `mmram()` is [0x1000, 0x10_1000), so 0x20_0000 lies outside it.
        let check = |region_type: u64, base: u64, pages: u64| {
            let region = MmProtectedRegion {
                protected_region_type: region_type,
                protected_region_base: base,
                protected_region_pages: pages,
            };
            let hob = guid_hob(MM_PROTECTED_REGION_HOB_GUID, size_of::<MmProtectedRegion>());
            validate_protected_regions([Hob::GuidHob(&hob, region.as_bytes())], mmram())
        };
        let iommu = ProtectedRegionType::Iommu as u64;
        assert_eq!(check(iommu, 0x20_0000, 2), Ok(()));

        let data = [0u8; size_of::<MmProtectedRegion>() - 1];
        let short = guid_hob(MM_PROTECTED_REGION_HOB_GUID, data.len());
        assert_eq!(
            validate_protected_regions([Hob::GuidHob(&short, &data)], mmram()),
            Err(HobValidationError::GuidedHobTooSmall { guid: MM_PROTECTED_REGION_HOB_GUID, length: data.len() }.into())
        );

        // The producer applies only IOMMU regions; it rejects the secure BIO type (1) and any other.
        for region_type in [1, 2] {
            assert_eq!(
                check(region_type, 0x20_0000, 2),
                Err(HobValidationError::UnsupportedProtectedRegionType { region_type }.into())
            );
        }

        // The producer makes these pages user-readable, so none may reach into MMRAM.
        assert_eq!(
            check(iommu, 0x10_0000, 2),
            Err(HobValidationError::RegionOverlapsMmram { kind: "protected region", range: span(0x10_0000, 0x2000) }
                .into())
        );
        assert_eq!(
            check(iommu, 0x20_0800, 1),
            Err(HobValidationError::RegionNotPageAligned { kind: "protected region", range: span(0x20_0800, 0x1000) }
                .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_named_regions() {
        // The SMBASE array is read, so it is real memory, described as allocated MMRAM. Its two
        // SMBASEs place the SMBASE area at [0x2_0000, 0x2_A000).
        let smbase_array = PageAlignedMemory::new(1);
        let set_smbases = |smbases: [u64; 2]| {
            // SAFETY: `smbase_array` is a live, page-aligned page, large enough for two u64s.
            unsafe { core::ptr::write(smbase_array.base() as *mut [u64; 2], smbases) };
        };
        set_smbases([0x1_8000, 0x1_A000]);

        // [0x1000, 0x9000) and [0x2_0000, 0x3_0000) are what the producer allocated; [0x9000,
        // 0x11000) is free MMRAM, which the page allocator hands out, for example as the paging pool.
        // MSEG takes the rest of [0x2_0000, 0x3_0000) after the SMBASE area.
        let regions = [
            MmramRegion::new(span(0x1000, 0x8000), true),
            MmramRegion::new(span(0x9000, 0x8000), false),
            MmramRegion::new(span(0x2_0000, 0x1_0000), true),
            MmramRegion::new(smbase_array.range(), true),
        ];
        let mseg = span(0x2_A000, 0x6000);
        let core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x1000,
            0x1000,
            0x1000,
        );
        let hob_list = span(0x2000, 0x800);
        // Two CPUs, each buffer in its own allocated pages, as the producer lays them out.
        let pd = MmSupvPassDownHobData {
            cpl3_stack_base: 0x3000,
            cpl3_stack_size: 0x1000,
            sm_base_array: smbase_array.base(),
            mm_initialized_buffer: 0x6000,
            firmware_policy_buffer: 0x7000,
            firmware_policy_buffer_size: 0x1000,
            ..pass_down()
        };
        let check = |pd: MmSupvPassDownHobData| {
            validate_named_regions([Hob::MemoryAllocationModule(&core)], hob_list, &pd, Some(mseg), 2, &regions)
        };
        assert_eq!(check(pd), Ok(()));

        // The Ring 3 stacks are remapped user-writable, so they must start and end on a page.
        for (base, size, range) in [(0x3800, 0x1000, span(0x3800, 0x2000)), (0x3000, 0x1100, span(0x3000, 0x2200))] {
            assert_eq!(
                check(MmSupvPassDownHobData { cpl3_stack_base: base, cpl3_stack_size: size, ..pd }),
                Err(HobValidationError::RegionNotPageAligned { kind: "cpl3_stack_base", range }.into())
            );
        }

        // Nothing may sit in free MMRAM, including stacks that only fit for the first CPU.
        for (pd, kind, range) in [
            (MmSupvPassDownHobData { cpl3_stack_base: 0x9000, ..pd }, "cpl3_stack_base", span(0x9000, 0x2000)),
            (MmSupvPassDownHobData { cpl3_stack_base: 0x8000, ..pd }, "cpl3_stack_base", span(0x8000, 0x2000)),
            (MmSupvPassDownHobData { sm_base_array: 0x9000, ..pd }, "sm_base_array", span(0x9000, 0x1000)),
            (
                MmSupvPassDownHobData { mm_initialized_buffer: 0x9000, ..pd },
                "mm_initialized_buffer",
                span(0x9000, 0x1000),
            ),
            (
                MmSupvPassDownHobData { firmware_policy_buffer: 0x9000, ..pd },
                "firmware_policy_buffer",
                span(0x9000, 0x1000),
            ),
        ] {
            assert_eq!(check(pd), Err(HobValidationError::RegionNotInAllocatedMmram { kind, range }.into()));
        }
        let free_core = allocation_module(
            MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID,
            MM_SUPERVISOR_CORE_GUID,
            0x9000,
            0x1000,
            0x9000,
        );
        assert_eq!(
            validate_named_regions([Hob::MemoryAllocationModule(&free_core)], hob_list, &pd, Some(mseg), 2, &regions),
            Err(HobValidationError::RegionNotInAllocatedMmram {
                kind: "allocation module",
                range: span(0x9000, 0x1000)
            }
            .into())
        );

        // No two regions may share a page.
        for (pd, first_kind, first, second_kind, second) in [
            (
                MmSupvPassDownHobData { cpl3_stack_base: 0x1000, ..pd },
                "allocation module",
                span(0x1000, 0x1000),
                "cpl3_stack_base",
                span(0x1000, 0x2000),
            ),
            (
                MmSupvPassDownHobData { mm_initialized_buffer: smbase_array.base() + 0x800, ..pd },
                "sm_base_array",
                smbase_array.range(),
                "mm_initialized_buffer",
                smbase_array.range(),
            ),
            (
                MmSupvPassDownHobData { firmware_policy_buffer: 0x2800, firmware_policy_buffer_size: 0x100, ..pd },
                "HOB list",
                span(0x2000, 0x1000),
                "firmware_policy_buffer",
                span(0x2000, 0x1000),
            ),
            (
                MmSupvPassDownHobData { firmware_policy_buffer: 0x2_0000, ..pd },
                "firmware_policy_buffer",
                span(0x2_0000, 0x1000),
                "SMBASE area",
                span(0x2_0000, 0xA000),
            ),
        ] {
            assert_eq!(
                check(pd),
                Err(HobValidationError::RegionsOverlap { first_kind, first, second_kind, second }.into())
            );
        }

        // MSEG is optional, but when present it must lie in allocated MMRAM apart from the rest.
        let check_mseg = |mseg: Option<MemoryRange>| {
            validate_named_regions([Hob::MemoryAllocationModule(&core)], hob_list, &pd, mseg, 2, &regions)
        };
        assert_eq!(check_mseg(None), Ok(()));
        assert_eq!(
            check_mseg(Some(span(0x9000, 0x1000))),
            Err(HobValidationError::RegionNotInAllocatedMmram { kind: "MSEG", range: span(0x9000, 0x1000) }.into())
        );
        assert_eq!(
            check_mseg(Some(span(0x2_8000, 0x4000))),
            Err(HobValidationError::RegionsOverlap {
                first_kind: "MSEG",
                first: span(0x2_8000, 0x4000),
                second_kind: "SMBASE area",
                second: span(0x2_0000, 0xA000),
            }
            .into())
        );

        // The SMBASE area may not reach into free MMRAM either.
        set_smbases([0x1000, 0x3000]);
        assert_eq!(
            check(pd),
            Err(HobValidationError::RegionNotInAllocatedMmram { kind: "SMBASE area", range: span(0x9000, 0xA000) }
                .into())
        );

        // Nor run past the end of the address space.
        let top: u64 = !0xFFF;
        set_smbases([0x1_8000, top]);
        assert_eq!(
            check(pd),
            Err(HobValidationError::InvalidAddressRange { kind: "SMBASE area", base: top, length: 0x1_0000 }.into())
        );
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
                forbidden: (MemoryAttributes::Supervisor | MemoryAttributes::ReadProtect).bits(),
                found: supervisor_code.bits(),
            }
            .into())
        );

        // No module page may be both writable and executable.
        assert_eq!(
            verify_module_page_protections(hobs.clone(), &page_table(MemoryAttributes::Supervisor, user_code)),
            Err(HobValidationError::PageWritableAndExecutable {
                addr: 0x1000,
                found: MemoryAttributes::Supervisor.bits()
            }
            .into())
        );

        // Nor may one be missing.
        let missing_core = core_code | MemoryAttributes::ReadProtect;
        assert_eq!(
            verify_module_page_protections(hobs.clone(), &page_table(missing_core, user_code)),
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr: 0x1000,
                forbidden: MemoryAttributes::ReadProtect.bits(),
                found: missing_core.bits(),
            }
            .into())
        );

        // The entry point must be on a read-only, executable page.
        for core_page in [
            core_code | MemoryAttributes::ExecuteProtect,
            MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect,
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
    fn test_mm_supervisor_hob_validation_driver_page_protections() {
        // Drivers run in Ring 3 like the User module, so their pages are held to the same rules.
        let driver =
            allocation_module(MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, TEST_DRIVER_GUID, 0x5000, 0x2000, 0x5100);
        let hobs = [Hob::MemoryAllocationModule(&driver)];

        // The entry point lies in the first page, which is queried for both the page walk and the
        // entry point.
        let page_table = |first: MemoryAttributes, second: MemoryAttributes| {
            let mut page_table = MockPageTable::new();
            page_table.expect_query_memory_region().withf(|&addr, _| addr == 0x5000).returning(move |_, _| Ok(first));
            page_table.expect_query_memory_region().withf(|&addr, _| addr == 0x6000).returning(move |_, _| Ok(second));
            page_table
        };

        // RO+X code with either RO+NX or RW+NX data is accepted.
        let code = MemoryAttributes::ReadOnly;
        for data in [MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect, MemoryAttributes::ExecuteProtect] {
            assert_eq!(verify_module_page_protections(hobs.clone(), &page_table(code, data)), Ok(()));
        }

        // A driver page must not be a supervisor page.
        let supervisor_data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
        assert_eq!(
            verify_module_page_protections(hobs.clone(), &page_table(code, supervisor_data)),
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr: 0x6000,
                forbidden: (MemoryAttributes::Supervisor | MemoryAttributes::ReadProtect).bits(),
                found: supervisor_data.bits(),
            }
            .into())
        );

        // Nor both writable and executable.
        assert_eq!(
            verify_module_page_protections(hobs.clone(), &page_table(code, MemoryAttributes::empty())),
            Err(HobValidationError::PageWritableAndExecutable { addr: 0x6000, found: 0 }.into())
        );

        // The driver's entry point must be on a code page.
        let data = MemoryAttributes::ExecuteProtect;
        assert_eq!(
            verify_module_page_protections(hobs, &page_table(data, code)),
            Err(HobValidationError::EntryPointNotOnCodePage { entry_point: 0x5100, attributes: data.bits() }.into())
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
                .returning(|_, _| Ok(MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect));
        }
        assert_eq!(
            verify_page_attributes(
                &page_table,
                &span(0x1800, 0x1000),
                MemoryAttributes::Supervisor,
                MemoryAttributes::ReadOnly
            ),
            Ok(())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_page_attributes_rejects_writable_executable_pages() {
        // Read-only or non-executable pages pass, whatever else they carry.
        for attributes in [
            MemoryAttributes::ReadOnly,
            MemoryAttributes::ExecuteProtect,
            MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect,
            MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly,
        ] {
            let mut page_table = MockPageTable::new();
            page_table.expect_query_memory_region().returning(move |_, _| Ok(attributes));
            assert_eq!(
                verify_page_attributes(
                    &page_table,
                    &span(0x1000, 0x1000),
                    MemoryAttributes::empty(),
                    MemoryAttributes::empty()
                ),
                Ok(())
            );
        }

        // A writable, executable page is rejected even when the caller requires and forbids nothing.
        for attributes in [MemoryAttributes::empty(), MemoryAttributes::Supervisor] {
            let mut page_table = MockPageTable::new();
            page_table.expect_query_memory_region().returning(move |_, _| Ok(attributes));
            assert_eq!(
                verify_page_attributes(
                    &page_table,
                    &span(0x1000, 0x1000),
                    MemoryAttributes::empty(),
                    MemoryAttributes::empty()
                ),
                Err(HobValidationError::PageWritableAndExecutable { addr: 0x1000, found: attributes.bits() }.into())
            );
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_page_attributes_reports_the_failing_page() {
        let required = MemoryAttributes::Supervisor;
        let data = MemoryAttributes::ExecuteProtect;

        // A page that cannot be queried is reported by its address.
        let mut page_table = MockPageTable::new();
        page_table
            .expect_query_memory_region()
            .withf(|&addr, _| addr == 0x1000)
            .returning(move |_, _| Ok(required | data));
        page_table
            .expect_query_memory_region()
            .withf(|&addr, _| addr == 0x2000)
            .returning(|_, _| Err(PtError::NoMapping));
        assert_eq!(
            verify_page_attributes(&page_table, &span(0x1000, 0x2000), required, MemoryAttributes::empty()),
            Err(HobValidationError::PageAttributeQueryFailed { addr: 0x2000 }.into())
        );

        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().returning(move |_, _| Ok(data));
        assert_eq!(
            verify_page_attributes(&page_table, &span(0x1000, 0x1000), required, MemoryAttributes::empty()),
            Err(HobValidationError::PageMissingAttribute {
                addr: 0x1000,
                desired: required.bits(),
                found: data.bits()
            }
            .into())
        );

        let mut page_table = MockPageTable::new();
        page_table.expect_query_memory_region().returning(|_, _| Ok(MemoryAttributes::ReadOnly));
        assert_eq!(
            verify_page_attributes(
                &page_table,
                &span(0x1000, 0x1000),
                MemoryAttributes::empty(),
                MemoryAttributes::ReadOnly
            ),
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr: 0x1000,
                forbidden: MemoryAttributes::ReadOnly.bits(),
                found: MemoryAttributes::ReadOnly.bits(),
            }
            .into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_page_attributes_rejects_a_range_that_cannot_be_widened() {
        // The mock has no expectations, so any query would fail the test. A range in the last page
        // cannot be widened to whole pages, because the rounded end would pass u64::MAX.
        let page_table = MockPageTable::new();
        let base = u64::MAX - 0xfff;
        assert_eq!(
            verify_page_attributes(
                &page_table,
                &span(base, 0xfff),
                MemoryAttributes::Supervisor,
                MemoryAttributes::empty()
            ),
            Err(HobValidationError::PageAttributeQueryFailed { addr: base }.into())
        );
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
            validate_incoming_hobs_post_paging_init(&handoff, &pass_down(), 1),
            Err(HobValidationError::PageTableUnavailable.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_smbase_page_protections() {
        // Two CPUs a tile apart, as init lays them out. Each has one page of entry code at
        // SMBASE + 0x8000 and its descriptor and save state in the page at SMBASE + 0xF000.
        let smbases: [u64; 2] = [0x10_0000, 0x10_2000];
        let pd = MmSupvPassDownHobData {
            sm_base_array: smbases.as_ptr().addr() as u64,
            mmi_entry_size: 0x100,
            ..pass_down()
        };
        let page_table = |code_page: MemoryAttributes, data_page: MemoryAttributes| {
            let mut page_table = MockPageTable::new();
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x10_8000 || addr == 0x10_A000)
                .returning(move |_, _| Ok(code_page));
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x10_F000 || addr == 0x11_1000)
                .returning(move |_, _| Ok(data_page));
            page_table
        };
        let code = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        let data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
        assert_eq!(verify_smbase_page_protections(&page_table(code, data), &pd, 2), Ok(()));

        // The entry code must be supervisor-only and executable.
        let user_code = MemoryAttributes::ReadOnly;
        assert_eq!(
            verify_smbase_page_protections(&page_table(user_code, data), &pd, 2),
            Err(HobValidationError::PageMissingAttribute {
                addr: 0x10_8000,
                desired: code.bits(),
                found: user_code.bits()
            }
            .into())
        );
        let no_execute = code | MemoryAttributes::ExecuteProtect;
        assert_eq!(
            verify_smbase_page_protections(&page_table(no_execute, data), &pd, 2),
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr: 0x10_8000,
                forbidden: (MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadProtect).bits(),
                found: no_execute.bits(),
            }
            .into())
        );

        // The descriptor and save state must not be executable.
        let executable_data = MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly;
        assert_eq!(
            verify_smbase_page_protections(&page_table(code, executable_data), &pd, 2),
            Err(HobValidationError::PageMissingAttribute {
                addr: 0x10_F000,
                desired: data.bits(),
                found: executable_data.bits()
            }
            .into())
        );

        // An unaligned SMBASE is rejected before any page is queried.
        let unaligned: [u64; 1] = [0x10_0800];
        let pd = MmSupvPassDownHobData { sm_base_array: unaligned.as_ptr().addr() as u64, ..pd };
        assert_eq!(
            verify_smbase_page_protections(&MockPageTable::new(), &pd, 1),
            Err(HobValidationError::SmBaseNotPageAligned { cpu: 0, smbase: 0x10_0800 }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_mem_bound_page_protections() {
        use PagingType::{Paging4Level, Paging5Level};

        let cpu = |bits: u8| Cpu {
            header: HobHeader { r#type: CPU, length: size_of::<Cpu>() as u16, reserved: 0 },
            size_of_memory_space: bits,
            size_of_io_space: 16,
            reserved: [0; 6],
        };
        let page_zero = span(0, 0x1000);
        // A page table that reports page 0 and `above` as mapped or not, and fails any other query.
        // A range that is only partly mapped is reported as inconsistent.
        let page_table = |above: MemoryRange, page_zero_mapped: bool, above_mapped: bool| {
            let mut page_table = MockPageTable::new();
            page_table
                .expect_query_memory_region()
                .withf(move |&addr, &size| addr == page_zero.base() && size == page_zero.size())
                .returning(move |_, _| {
                    if page_zero_mapped { Ok(MemoryAttributes::ExecuteProtect) } else { Err(PtError::NoMapping) }
                });
            page_table
                .expect_query_memory_region()
                .withf(move |&addr, &size| addr == above.base() && size == above.size())
                .returning(move |_, _| {
                    if above_mapped { Err(PtError::InconsistentMappingAcrossRange) } else { Err(PtError::NoMapping) }
                });
            page_table
        };
        let above_39_bits = span(1 << 39, (1 << 47) - (1 << 39));
        let unmapped = page_table(above_39_bits, false, false);

        // The CPU HOB's width bounds the check, or MAXPHYADDR when there is no CPU HOB.
        let cpu_39 = cpu(39);
        assert_eq!(verify_mem_bound_page_protections([Hob::Cpu(&cpu_39)], &unmapped, 40, Paging4Level), Ok(()));
        assert_eq!(verify_mem_bound_page_protections([], &unmapped, 39, Paging4Level), Ok(()));

        // A CPU HOB may not claim more bits than the processor has; no page is queried.
        let cpu_41 = cpu(41);
        assert_eq!(
            verify_mem_bound_page_protections([Hob::Cpu(&cpu_41)], &MockPageTable::new(), 40, Paging4Level),
            Err(HobValidationError::CpuHobAddressWidthTooLarge { hob_bits: 41, cpuid_bits: 40 }.into())
        );

        // Page 0 must not be present, even as a non-executable page.
        assert_eq!(
            verify_mem_bound_page_protections([], &page_table(above_39_bits, true, false), 39, Paging4Level),
            Err(HobValidationError::RangeMapped { range: page_zero }.into())
        );

        // Nor may any page above the width, even when most of the range is unmapped.
        assert_eq!(
            verify_mem_bound_page_protections([], &page_table(above_39_bits, false, true), 39, Paging4Level),
            Err(HobValidationError::RangeMapped { range: above_39_bits }.into())
        );

        // 4-level paging identity-maps at most 47 bits, so a wider width leaves nothing above it to
        // check, and only page 0 is.
        let mut page_zero_only = MockPageTable::new();
        page_zero_only.expect_query_memory_region().times(1).returning(|addr, _| {
            assert_eq!(addr, 0);
            Err(PtError::NoMapping)
        });
        let cpu_48 = cpu(48);
        assert_eq!(verify_mem_bound_page_protections([Hob::Cpu(&cpu_48)], &page_zero_only, 52, Paging4Level), Ok(()));

        // 5-level paging is not capped at 47 bits, and its lower canonical half runs to 2^56.
        let above_48_bits = span(1 << 48, (1 << 56) - (1 << 48));
        assert_eq!(
            verify_mem_bound_page_protections(
                [Hob::Cpu(&cpu_48)],
                &page_table(above_48_bits, false, false),
                52,
                Paging5Level
            ),
            Ok(())
        );
        let above_52_bits = span(1 << 52, (1 << 56) - (1 << 52));
        assert_eq!(
            verify_mem_bound_page_protections([], &page_table(above_52_bits, false, true), 52, Paging5Level),
            Err(HobValidationError::RangeMapped { range: above_52_bits }.into())
        );
    }

    #[test]
    fn test_mm_supervisor_hob_validation_mseg_page_protections() {
        // Nothing to check without an MSEG, so no page is queried.
        assert_eq!(verify_mseg_page_protections(&MockPageTable::new(), None), Ok(()));

        let page_table = |attributes: MemoryAttributes| {
            let mut page_table = MockPageTable::new();
            page_table
                .expect_query_memory_region()
                .withf(|&addr, _| addr == 0x40_0000 || addr == 0x40_1000)
                .returning(move |_, _| Ok(attributes));
            page_table
        };
        let mseg = Some(span(0x40_0000, 0x2000));
        let data = MemoryAttributes::Supervisor | MemoryAttributes::ExecuteProtect;
        assert_eq!(verify_mseg_page_protections(&page_table(data), mseg), Ok(()));

        let user = MemoryAttributes::ExecuteProtect;
        assert_eq!(
            verify_mseg_page_protections(&page_table(user), mseg),
            Err(HobValidationError::PageMissingAttribute { addr: 0x40_0000, desired: data.bits(), found: user.bits() }
                .into())
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
            verify_cpl3_stacks_user_accessible(&span(0x1000, 0x2000)),
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
        assert_eq!(verify_cpl3_stacks_user_accessible(&stack.range()), Ok(()));

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
                verify_cpl3_stacks_user_accessible(&stack.range()),
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
            verify_cpl3_stacks_user_accessible(&stack.range()),
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
            HobValidationError::MmramRegionsOverlap { first: span(0, 0x1000), second: span(0x800, 0x1000) },
            HobValidationError::MmramNotContiguous { covered: 0x1000, span: 0x2000 },
            HobValidationError::MmramBelowOneMib { span: span(0xF_F000, 0x1000) },
            HobValidationError::MemoryAllocationInsideMmram { range: span(0x1000, 0x1000) },
            HobValidationError::AllocationModuleOutsideMmram { range: span(0x1000, 0x1000) },
            HobValidationError::AllocationModuleNotPageAligned { range: span(0x1001, 0x1000) },
            HobValidationError::MissingModule { has_init: false, has_supv_core: false, has_user: false },
            HobValidationError::MissingModule { has_init: false, has_supv_core: true, has_user: true },
            HobValidationError::MissingModule { has_init: true, has_supv_core: false, has_user: true },
            HobValidationError::MissingModule { has_init: true, has_supv_core: true, has_user: false },
            HobValidationError::DuplicateModule { module_name: MM_SUPERVISOR_CORE_GUID },
            HobValidationError::AllocationModulesOverlap { first: span(0, 0x1000), second: span(0x800, 0x1000) },
            HobValidationError::TooManyAllocationModules { limit: MAX_ALLOC_MODULES },
            HobValidationError::AllocationModuleEntryPointOutOfRange {
                entry_point: 0x5000,
                range: span(0x1000, 0x1000),
            },
            HobValidationError::ResourceDescriptorsOverlap {
                kind: "v1 memory",
                first: span(0, 0x1000),
                second: span(0x800, 0x1000),
            },
            HobValidationError::MmramNotInMemoryDescriptor { mmram: span(0x1000, 0x1000) },
            HobValidationError::MmramInUnexpectedResourceType { range: span(0x1000, 0x1000), resource_type: 1 },
            HobValidationError::V2ContainsUceAttribute { base: 0x1000, attributes: 0x1 },
            HobValidationError::V2InvalidCacheability { base: 0x1000, attributes: 0 },
            HobValidationError::V2IoAttributesNotZero { base: 0x1000, attributes: 0x4 },
            HobValidationError::MalformedHob { offset: 0x38, hob_type: GUID_EXTENSION, length: 0 },
            HobValidationError::HobListTooLarge { limit: MAX_HOB_LIST_SIZE },
            HobValidationError::HobListOutsideMmram { base: 0x1000, size: 0x40 },
            HobValidationError::RegionNotPageAligned { kind: "cpl3_stack_base", range: span(0x1800, 0x1000) },
            HobValidationError::RegionNotInAllocatedMmram { kind: "sm_base_array", range: span(0x1000, 0x1000) },
            HobValidationError::RegionsOverlap {
                first_kind: "HOB list",
                first: span(0x1000, 0x1000),
                second_kind: "firmware_policy_buffer",
                second: span(0x1000, 0x1000),
            },
            HobValidationError::InvalidMsegHob,
            HobValidationError::GuidedHobTooSmall { guid: MM_PROTECTED_REGION_HOB_GUID, length: 4 },
            HobValidationError::RegionOverlapsMmram { kind: "protected region", range: span(0x1000, 0x1000) },
            HobValidationError::UnblockRegionWithoutIdentifier { range: span(0x1000, 0x1000) },
            HobValidationError::UnsupportedProtectedRegionType { region_type: 1 },
            HobValidationError::CpuHobAddressWidthTooLarge { hob_bits: 52, cpuid_bits: 39 },
            HobValidationError::RangeMapped { range: span(0, 0x1000) },
            HobValidationError::PageTableUnavailable,
            HobValidationError::PageAttributeQueryFailed { addr: 0x1000 },
            HobValidationError::PageMissingAttribute { addr: 0x1000, desired: 0x1, found: 0 },
            HobValidationError::PageHasForbiddenAttribute { addr: 0x1000, forbidden: 0x1, found: 0x1 },
            HobValidationError::PageWritableAndExecutable { addr: 0x1000, found: 0 },
            HobValidationError::SmBaseNotPageAligned { cpu: 1, smbase: 0x10_0800 },
            HobValidationError::EntryPointNotOnCodePage { entry_point: 0x5000, attributes: 0x1 },
        ];
        for err in errors {
            assert!(!format!("{err}").is_empty());
        }
    }
}
