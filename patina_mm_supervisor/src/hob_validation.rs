//! Validation of critical incoming HOBs.
//!
//! The MM Supervisor consumes several HOBs produced outside of MMRAM by the MM
//! IPL. Because that producer is not part of the supervisor's trust boundary,
//! the data it hands down must be validated before it is acted upon. Validation
//! runs in two rounds during BSP initialization.
//!
//! ## Pre-paging checks
//!
//! Run before any HOB content is consumed and before the page table is
//! initialized:
//!
//! 1. The scanned MMRAM regions form a single contiguous, non-overlapping span.
//! 2. No `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOB overlaps MMRAM (these describe
//!    allocations outside of MMRAM).
//! 3. Every `MemoryAllocationModule` HOB lies inside MMRAM, the modules do not
//!    overlap each other, and each module's entry point lies within its own
//!    allocation range.
//! 4. Resource descriptor HOBs (v1 and v2) do not overlap within the same
//!    version and address space (memory vs I/O). Descriptors owned by the
//!    Memory Type Information GUID are skipped, since the PEI memory bin HOB is
//!    expected to overlap the system-memory descriptors backing it.
//! 5. Every v2 resource descriptor carries valid memory attributes (exactly one
//!    cacheability bit for memory, no `EFI_MEMORY_UCE`, no attributes for I/O).
//! 6. The pointers reported in the MM Supervisor PassDown HOB reference memory
//!    inside MMRAM (and the HOB reports the expected revision).
//!
//! ## Post-paging checks
//!
//! Run after the page table is initialized, so per-page attributes can be
//! queried from it:
//!
//! 7. Every page of the MM Supervisor Core module is supervisor-owned (the U/S
//!    "SP" bit is set) and every page of the MM Supervisor User module is
//!    user-accessible (the SP bit is clear).
//! 8. Each module's entry point lies on an executable page (its `ExecuteProtect`
//!    bit is clear), i.e. it sits in the module's code section.
//!
//! Before the checks run, both the scanned MMRAM regions and every discovered
//! HOB are logged (at debug level) to aid debugging.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use core::fmt;

use patina::management_mode::supervisor::{
    MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_USER_GUID,
};
use patina::pi::hob::{
    EFI_RESOURCE_IO, EFI_RESOURCE_IO_RESERVED, Hob, MEMORY_TYPE_INFO_HOB_GUID, PhaseHandoffInformationTable,
    ResourceDescriptor,
};
use patina::standard::efi;
use patina::{UEFI_PAGE_SIZE, align_range};
use patina_paging::{MemoryAttributes, PageTable};
use zerocopy::FromBytes;

use crate::init::{MmSupvPassDownHobData, find_guid_hob};
use crate::is_buffer_inside_mmram;
use crate::smrr::SmramRegion;
use crate::state::security_state;

/// Maximum number of `MemoryAllocationModule` HOBs considered while checking for
/// overlaps. This bounds the stack storage used for the pairwise comparison.
const MAX_ALLOC_MODULES: usize = 64;

/// Maximum number of resource descriptor HOBs collected per category while
/// checking for overlaps. This bounds the stack storage used for the comparison.
const MAX_RESOURCE_HOBS: usize = 128;

/// Errors produced while validating the critical incoming HOBs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HobValidationError {
    /// The HOB list pointer was null.
    NullHobList,
    /// No MMRAM regions were reported.
    NoMmramRegions,
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
    /// The PassDown HOB was not present in the HOB list.
    PassDownHobMissing,
    /// The PassDown HOB payload was smaller than its defined structure.
    PassDownHobTooSmall,
    /// The PassDown HOB reported an unexpected revision.
    PassDownInvalidRevision {
        /// The revision found in the HOB.
        found: u32,
        /// The revision the supervisor expected.
        expected: u32,
    },
    /// A pointer reported in the PassDown HOB references memory outside MMRAM.
    PassDownPointerOutsideMmram {
        /// Name of the offending PassDown field.
        field: &'static str,
        /// The reported address.
        addr: u64,
        /// The size that was checked for containment.
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
    /// A module's entry point does not lie on an executable page.
    EntryPointNotExecutable {
        /// The module entry point that is not executable.
        entry_point: u64,
    },
}

impl core::error::Error for HobValidationError {}

impl fmt::Display for HobValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullHobList => write!(f, "HOB list pointer is null"),
            Self::NoMmramRegions => write!(f, "no MMRAM regions were reported in the HOB list"),
            Self::MmramRegionsOverlap { base_a, size_a, base_b, size_b } => write!(
                f,
                "MMRAM regions overlap: [0x{:x}, +0x{:x}) and [0x{:x}, +0x{:x})",
                base_a, size_a, base_b, size_b
            ),
            Self::MmramNotContiguous { covered, span } => {
                write!(f, "MMRAM regions are not contiguous: cover 0x{:x} of 0x{:x} span", covered, span)
            }
            Self::MemoryAllocationInsideMmram { base, length } => {
                write!(f, "memory allocation HOB [0x{:x}, +0x{:x}) overlaps MMRAM", base, length)
            }
            Self::AllocationModuleOutsideMmram { base, length } => {
                write!(f, "allocation module HOB [0x{:x}, +0x{:x}) is outside MMRAM", base, length)
            }
            Self::AllocationModulesOverlap { base_a, size_a, base_b, size_b } => write!(
                f,
                "allocation module HOBs overlap: [0x{:x}, +0x{:x}) and [0x{:x}, +0x{:x})",
                base_a, size_a, base_b, size_b
            ),
            Self::AllocationModuleEntryPointOutOfRange { entry_point, base, length } => write!(
                f,
                "allocation module entry point 0x{:x} is outside its allocation [0x{:x}, +0x{:x})",
                entry_point, base, length
            ),
            Self::ResourceDescriptorsOverlap { kind, base_a, size_a, base_b, size_b } => write!(
                f,
                "{} resource descriptor HOBs overlap: [0x{:x}, +0x{:x}) and [0x{:x}, +0x{:x})",
                kind, base_a, size_a, base_b, size_b
            ),
            Self::V2ContainsUceAttribute { base, attributes } => {
                write!(
                    f,
                    "V2 resource descriptor at 0x{:x} carries prohibited UCE attribute (0x{:x})",
                    base, attributes
                )
            }
            Self::V2InvalidCacheability { base, attributes } => write!(
                f,
                "V2 resource descriptor at 0x{:x} does not have exactly one cacheability attribute (0x{:x})",
                base, attributes
            ),
            Self::V2IoAttributesNotZero { base, attributes } => {
                write!(f, "V2 I/O resource descriptor at 0x{:x} has non-zero attributes (0x{:x})", base, attributes)
            }
            Self::PassDownHobMissing => write!(f, "PassDown HOB is missing from the HOB list"),
            Self::PassDownHobTooSmall => write!(f, "PassDown HOB payload is too small"),
            Self::PassDownInvalidRevision { found, expected } => {
                write!(f, "PassDown HOB revision {} does not match expected {}", found, expected)
            }
            Self::PassDownPointerOutsideMmram { field, addr, size } => {
                write!(f, "PassDown pointer `{}` = 0x{:x} (size 0x{:x}) is outside MMRAM", field, addr, size)
            }
            Self::PageTableUnavailable => write!(f, "active page table is not available to query page attributes"),
            Self::PageAttributeQueryFailed { addr } => {
                write!(f, "failed to query page attributes at 0x{:x}", addr)
            }
            Self::PageMissingAttribute { addr, desired, found } => write!(
                f,
                "page at 0x{:x} is missing required attributes (desired 0x{:x}, found 0x{:x})",
                addr, desired, found
            ),
            Self::PageHasForbiddenAttribute { addr, forbidden, found } => write!(
                f,
                "page at 0x{:x} has forbidden attributes set (forbidden 0x{:x}, found 0x{:x})",
                addr, forbidden, found
            ),
            Self::EntryPointNotExecutable { entry_point } => {
                write!(f, "module entry point 0x{:x} does not lie on an executable page", entry_point)
            }
        }
    }
}

/// Returns whether the two `[base, base + size)` ranges overlap.
fn ranges_overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    let (a_base, a_size) = a;
    let (b_base, b_size) = b;
    let a_end = a_base.saturating_add(a_size);
    let b_end = b_base.saturating_add(b_size);
    a_base < b_end && b_base < a_end
}

/// Validates that the scanned MMRAM regions form a single contiguous,
/// non-overlapping span.
///
/// The regions are not required to be sorted. Overlaps are detected pairwise,
/// and a gap is detected by comparing the total covered bytes against the span
/// from the lowest base to the highest end.
fn validate_mmram_contiguous(regions: &[SmramRegion]) -> Result<(), HobValidationError> {
    if regions.is_empty() {
        return Err(HobValidationError::NoMmramRegions);
    }

    for (i, a) in regions.iter().enumerate() {
        for b in regions.iter().skip(i + 1) {
            if ranges_overlap((a.base, a.size), (b.base, b.size)) {
                return Err(HobValidationError::MmramRegionsOverlap {
                    base_a: a.base,
                    size_a: a.size,
                    base_b: b.base,
                    size_b: b.size,
                });
            }
        }
    }

    let min_base = regions.iter().map(|r| r.base).min().unwrap_or(0);
    let max_end = regions.iter().map(|r| r.base.saturating_add(r.size)).max().unwrap_or(0);
    let covered: u64 = regions.iter().map(|r| r.size).sum();
    let span = max_end - min_base;

    // With no overlaps (checked above), equal covered/span implies no gaps.
    if covered != span {
        return Err(HobValidationError::MmramNotContiguous { covered, span });
    }

    Ok(())
}

/// Validates the PassDown HOB revision and that every non-null pointer it
/// reports references memory inside MMRAM.
fn validate_pass_down_pointers(pass_down: &MmSupvPassDownHobData) -> Result<(), HobValidationError> {
    if pass_down.revision != crate::MM_SUPV_PASS_DOWN_HOB_REVISION {
        return Err(HobValidationError::PassDownInvalidRevision {
            found: pass_down.revision,
            expected: crate::MM_SUPV_PASS_DOWN_HOB_REVISION,
        });
    }

    let checks: [(&'static str, u64, u64); 4] = [
        (
            "mm_supervisor_cpl3_stack_base",
            pass_down.mm_supervisor_cpl3_stack_base,
            pass_down.mm_supervisor_cpl3_per_core_stack_size.max(1),
        ),
        ("sm_base", pass_down.sm_base, core::mem::size_of::<u64>() as u64),
        ("mm_initialized_buffer", pass_down.mm_initialized_buffer, 1),
        (
            "mm_supv_firmware_policy_buffer",
            pass_down.mm_supv_firmware_policy_buffer,
            pass_down.mm_supv_firmware_policy_buffer_size.max(1),
        ),
    ];

    for (field, addr, size) in checks {
        if addr != 0 && !is_buffer_inside_mmram(addr, size) {
            return Err(HobValidationError::PassDownPointerOutsideMmram { field, addr, size });
        }
    }

    Ok(())
}

/// Returns the single contiguous MMRAM span `(base, size)` covered by `regions`.
///
/// Only meaningful once [`validate_mmram_contiguous`] has confirmed the regions
/// form one gap-free span; the union then equals `[min_base, max_end)`.
fn mmram_span(regions: &[SmramRegion]) -> (u64, u64) {
    let base = regions.iter().map(|r| r.base).min().unwrap_or(0);
    let end = regions.iter().map(|r| r.base.saturating_add(r.size)).max().unwrap_or(0);
    (base, end.saturating_sub(base))
}

/// Returns whether `[base, base + length)` overlaps the MMRAM span.
fn buffer_overlaps_mmram(mmram: (u64, u64), base: u64, length: u64) -> bool {
    ranges_overlap((base, length), mmram)
}

/// Validates that no memory allocation HOB overlaps MMRAM.
///
/// Plain `EFI_HOB_TYPE_MEMORY_ALLOCATION` HOBs describe allocations outside of
/// MMRAM, so any that overlaps the MMRAM span is rejected. `mmram` is the single
/// contiguous span validated by [`validate_mmram_contiguous`].
fn validate_memory_allocations(
    handoff: &PhaseHandoffInformationTable,
    mmram: (u64, u64),
) -> Result<(), HobValidationError> {
    let hob = Hob::Handoff(handoff);
    for current in &hob {
        if let Hob::MemoryAllocation(alloc) = current {
            let base = alloc.alloc_descriptor.memory_base_address;
            let length = alloc.alloc_descriptor.memory_length;
            if buffer_overlaps_mmram(mmram, base, length) {
                return Err(HobValidationError::MemoryAllocationInsideMmram { base, length });
            }
        }
    }
    Ok(())
}

/// Validates that every allocation module HOB lies inside MMRAM and that the
/// modules do not overlap each other.
fn validate_allocation_modules(handoff: &PhaseHandoffInformationTable) -> Result<(), HobValidationError> {
    let mut modules: [(u64, u64); MAX_ALLOC_MODULES] = [(0, 0); MAX_ALLOC_MODULES];
    let mut count = 0usize;

    let hob = Hob::Handoff(handoff);
    for current in &hob {
        if let Hob::MemoryAllocationModule(module) = current {
            // Only the MM Supervisor's own module allocations are validated here.
            if module.alloc_descriptor.name != MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID {
                continue;
            }
            let base = module.alloc_descriptor.memory_base_address;
            let length = module.alloc_descriptor.memory_length;
            if !is_buffer_inside_mmram(base, length) {
                return Err(HobValidationError::AllocationModuleOutsideMmram { base, length });
            }
            let entry_point = module.entry_point;
            if !entry_point_in_range(entry_point, base, length) {
                return Err(HobValidationError::AllocationModuleEntryPointOutOfRange { entry_point, base, length });
            }
            if let Some(slot) = modules.get_mut(count) {
                *slot = (base, length);
                count += 1;
            } else {
                log::warn!(
                    "More than {} allocation module HOBs; overlap check limited to the first {}",
                    MAX_ALLOC_MODULES,
                    MAX_ALLOC_MODULES
                );
                break;
            }
        }
    }

    if let Some((a, b)) = find_overlap(modules.get_mut(..count).unwrap_or(&mut [])) {
        return Err(HobValidationError::AllocationModulesOverlap {
            base_a: a.0,
            size_a: a.1,
            base_b: b.0,
            size_b: b.1,
        });
    }

    Ok(())
}

/// Returns whether `entry_point` lies within the `[base, base + length)` range.
fn entry_point_in_range(entry_point: u64, base: u64, length: u64) -> bool {
    entry_point >= base && entry_point < base.saturating_add(length)
}

/// Returns the first pair of overlapping `[base, size)` ranges, if any.
fn find_overlap(ranges: &mut [(u64, u64)]) -> Option<((u64, u64), (u64, u64))> {
    ranges.sort_unstable_by_key(|&(base, _)| base);
    let mut prev = *ranges.first()?;
    let mut prev_end = prev.0.saturating_add(prev.1);
    for &cur in ranges.iter().skip(1) {
        if ranges_overlap(prev, cur) {
            return Some((prev, cur));
        }
        // Keep the interval with the greatest end so a later range is compared
        // against the widest predecessor (catches nested/contained ranges).
        let cur_end = cur.0.saturating_add(cur.1);
        if cur_end > prev_end {
            prev = cur;
            prev_end = cur_end;
        }
    }
    None
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
fn check_category_overlap(ranges: &mut [(u64, u64)], kind: &'static str) -> Result<(), HobValidationError> {
    if let Some((a, b)) = find_overlap(ranges) {
        return Err(HobValidationError::ResourceDescriptorsOverlap {
            kind,
            base_a: a.0,
            size_a: a.1,
            base_b: b.0,
            size_b: b.1,
        });
    }
    Ok(())
}

/// Appends `range` to `ranges`, updating `count`, warning if capacity is exceeded.
fn push_range(ranges: &mut [(u64, u64)], count: &mut usize, range: (u64, u64), kind: &str) {
    if let Some(slot) = ranges.get_mut(*count) {
        *slot = range;
        *count += 1;
    } else {
        log::warn!("More than {} {} resource descriptor HOBs; overlap check limited", MAX_RESOURCE_HOBS, kind);
    }
}

/// Validates that resource descriptor HOBs do not overlap within the same
/// version (v1/v2) and address space (memory/I/O).
///
/// V1 and V2 descriptors are expected to cover the same ranges (V2 is a superset
/// of V1), so overlaps are only flagged within each of the four categories, not
/// across them.
fn validate_resource_descriptor_overlaps(handoff: &PhaseHandoffInformationTable) -> Result<(), HobValidationError> {
    let mut v1_mem = [(0u64, 0u64); MAX_RESOURCE_HOBS];
    let mut v1_io = [(0u64, 0u64); MAX_RESOURCE_HOBS];
    let mut v2_mem = [(0u64, 0u64); MAX_RESOURCE_HOBS];
    let mut v2_io = [(0u64, 0u64); MAX_RESOURCE_HOBS];
    let (mut v1_mem_n, mut v1_io_n, mut v2_mem_n, mut v2_io_n) = (0usize, 0usize, 0usize, 0usize);

    let hob = Hob::Handoff(handoff);
    for current in &hob {
        match current {
            Hob::ResourceDescriptor(rd) => {
                // The PEI memory bin (Memory Type Info) HOB is expected to overlap the
                // system-memory resource descriptors backing it, so skip it.
                if is_memory_type_info(rd) {
                    continue;
                }
                let range = (rd.physical_start, rd.resource_length);
                if is_io(rd.resource_type) {
                    push_range(&mut v1_io, &mut v1_io_n, range, "v1 I/O");
                } else {
                    push_range(&mut v1_mem, &mut v1_mem_n, range, "v1 memory");
                }
            }
            Hob::ResourceDescriptorV2(rd) => {
                if is_memory_type_info(&rd.v1) {
                    continue;
                }
                let range = (rd.v1.physical_start, rd.v1.resource_length);
                if is_io(rd.v1.resource_type) {
                    push_range(&mut v2_io, &mut v2_io_n, range, "v2 I/O");
                } else {
                    push_range(&mut v2_mem, &mut v2_mem_n, range, "v2 memory");
                }
            }
            _ => {}
        }
    }

    check_category_overlap(v1_mem.get_mut(..v1_mem_n).unwrap_or(&mut []), "v1 memory")?;
    check_category_overlap(v1_io.get_mut(..v1_io_n).unwrap_or(&mut []), "v1 I/O")?;
    check_category_overlap(v2_mem.get_mut(..v2_mem_n).unwrap_or(&mut []), "v2 memory")?;
    check_category_overlap(v2_io.get_mut(..v2_io_n).unwrap_or(&mut []), "v2 I/O")?;

    Ok(())
}

/// Validates the extended attributes of a single V2 resource descriptor.
///
/// Memory regions must carry exactly one cacheability attribute and must not
/// carry the prohibited `EFI_MEMORY_UCE` bit; I/O regions must carry no
/// attributes at all.
fn check_v2_attributes(resource_type: u32, base: u64, attributes: u64) -> Result<(), HobValidationError> {
    if is_io(resource_type) {
        if attributes != 0 {
            return Err(HobValidationError::V2IoAttributesNotZero { base, attributes });
        }
        return Ok(());
    }

    if attributes & efi::MEMORY_UCE != 0 {
        return Err(HobValidationError::V2ContainsUceAttribute { base, attributes });
    }

    // Exactly one cacheability bit (excluding the prohibited UCE) must be set.
    let cache_bits = attributes & (efi::CACHE_ATTRIBUTE_MASK & !efi::MEMORY_UCE);
    if cache_bits == 0 || (cache_bits & (cache_bits - 1)) != 0 {
        return Err(HobValidationError::V2InvalidCacheability { base, attributes });
    }

    Ok(())
}

/// Validates the extended memory attributes carried by every V2 resource
/// descriptor HOB.
fn validate_resource_v2_memory_attributes(handoff: &PhaseHandoffInformationTable) -> Result<(), HobValidationError> {
    let hob = Hob::Handoff(handoff);
    for current in &hob {
        if let Hob::ResourceDescriptorV2(rd) = current {
            check_v2_attributes(rd.v1.resource_type, rd.v1.physical_start, rd.attributes)?;
        }
    }
    Ok(())
}

/// Logs every discovered HOB, grouped by type, to aid debugging.
fn dump_hobs(handoff: &PhaseHandoffInformationTable) {
    log::debug!("---- Incoming HOB dump ----");
    let hob = Hob::Handoff(handoff);
    for current in &hob {
        match current {
            Hob::Handoff(h) => log::debug!(
                "HOB Handoff: boot_mode={:?}, memory_top=0x{:x}, memory_bottom=0x{:x}",
                h.boot_mode,
                h.memory_top,
                h.memory_bottom
            ),
            Hob::MemoryAllocation(a) => log::debug!(
                "HOB MemoryAllocation: base=0x{:x}, len=0x{:x}, name={}",
                a.alloc_descriptor.memory_base_address,
                a.alloc_descriptor.memory_length,
                a.alloc_descriptor.name.as_guid()
            ),
            Hob::MemoryAllocationModule(m) => log::debug!(
                "HOB MemoryAllocationModule: base=0x{:x}, len=0x{:x}, module={}, entry=0x{:x}",
                m.alloc_descriptor.memory_base_address,
                m.alloc_descriptor.memory_length,
                m.module_name.as_guid(),
                m.entry_point
            ),
            Hob::ResourceDescriptor(rd) => log::debug!(
                "HOB ResourceDescriptor(v1): base=0x{:x}, len=0x{:x}, type=0x{:x}, attr=0x{:x}",
                rd.physical_start,
                rd.resource_length,
                rd.resource_type,
                rd.resource_attribute
            ),
            Hob::ResourceDescriptorV2(rd) => log::debug!(
                "HOB ResourceDescriptor(v2): base=0x{:x}, len=0x{:x}, type=0x{:x}, attributes=0x{:x}",
                rd.v1.physical_start,
                rd.v1.resource_length,
                rd.v1.resource_type,
                rd.attributes
            ),
            Hob::GuidHob(g, data) => log::debug!("HOB Guid: {}, data_len=0x{:x}", g.name.as_guid(), data.len()),
            Hob::FirmwareVolume(_) | Hob::FirmwareVolume2(_) | Hob::FirmwareVolume3(_) => {
                log::debug!("HOB FirmwareVolume")
            }
            Hob::Cpu(_) => log::debug!("HOB Cpu"),
            Hob::Capsule(_) => log::debug!("HOB Capsule"),
            Hob::Misc(t) => log::debug!("HOB Misc: type=0x{:x}", t),
        }
    }
    log::debug!("---- End HOB dump ----");
}

/// Logs every scanned MMRAM region to aid debugging.
fn dump_regions(regions: &[SmramRegion]) {
    log::debug!("---- Scanned MMRAM regions ({}) ----", regions.len());
    for (i, r) in regions.iter().enumerate() {
        log::debug!(
            "Region {}: base=0x{:x}, size=0x{:x}, end=0x{:x}, pre_allocated={}",
            i,
            r.base,
            r.size,
            r.base.saturating_add(r.size),
            r.pre_allocated
        );
    }
    log::debug!("---- End MMRAM region dump ----");
}

/// Validates the PassDown HOB present in the HOB list.
fn validate_pass_down(handoff: &PhaseHandoffInformationTable) -> Result<(), HobValidationError> {
    let data =
        find_guid_hob(handoff, crate::MM_SUPV_PASS_DOWN_HOB_GUID).ok_or(HobValidationError::PassDownHobMissing)?;
    let (pass_down, _) =
        MmSupvPassDownHobData::read_from_prefix(data).map_err(|_| HobValidationError::PassDownHobTooSmall)?;
    validate_pass_down_pointers(&pass_down)
}

/// Validates the critical incoming HOBs before their contents are consumed.
///
/// `regions` are the MMRAM regions scanned from the HOB list; the page allocator
/// must already be initialized so the MMRAM containment checks are meaningful.
///
/// ## Safety
///
/// The caller must ensure that `hob_list` points to a valid HOB list.
pub(crate) unsafe fn validate_incoming_hobs_pre_paging_init(
    hob_list: *const c_void,
    regions: &[SmramRegion],
) -> Result<(), HobValidationError> {
    // SAFETY: `hob_list` points to a valid HOB list per this function's contract,
    // so reinterpreting its first entry as the handoff table header is sound.
    let handoff =
        unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref() }.ok_or(HobValidationError::NullHobList)?;

    // Dump both inputs before validating either, so the full picture is visible
    // even when a later check fails.
    dump_regions(regions);
    dump_hobs(handoff);

    validate_mmram_contiguous(regions)?;
    // MMRAM is a single contiguous span (checked above), so allocations can be
    // tested against one range instead of iterating the region list.
    validate_memory_allocations(handoff, mmram_span(regions))?;
    validate_allocation_modules(handoff)?;
    validate_resource_descriptor_overlaps(handoff)?;
    validate_resource_v2_memory_attributes(handoff)?;
    validate_pass_down(handoff)?;

    Ok(())
}

/// Validates memory protections that can only be checked once the page table is
/// active.
///
/// Runs after the page table has been initialized (unlike [`validate_incoming_hobs_pre_paging_init`],
/// which runs before), so it can verify per-page attributes via
/// [`verify_pages_have_attributes`].
///
/// ## Safety
///
/// The caller must ensure that `hob_list` points to a valid HOB list.
pub(crate) unsafe fn validate_incoming_hobs_post_paging_init(
    hob_list: *const c_void,
) -> Result<(), HobValidationError> {
    // SAFETY: `hob_list` points to a valid HOB list per this function's contract,
    // so reinterpreting its first entry as the handoff table header is sound.
    let handoff =
        unsafe { (hob_list as *const PhaseHandoffInformationTable).as_ref() }.ok_or(HobValidationError::NullHobList)?;

    verify_module_page_protections(handoff)?;

    Ok(())
}

/// Verifies the page protections of the MM Supervisor Core and User modules.
///
/// Iterates the module HOBs, selecting the MM Supervisor's own
/// `MemoryAllocationModule` HOBs, dispatches on the module GUID to check
/// supervisor/user page ownership, and confirms each module's entry point lies
/// on an executable page.
fn verify_module_page_protections(handoff: &PhaseHandoffInformationTable) -> Result<(), HobValidationError> {
    let hob = Hob::Handoff(handoff);
    for current in &hob {
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
                    "Verifying MM Supervisor Core module page protections: base=0x{:x}, length=0x{:x}",
                    base,
                    length
                );
                // Supervisor core pages must be supervisor-owned (SP set).
                verify_page_attributes(base, length, MemoryAttributes::Supervisor, MemoryAttributes::empty())?;
                verify_entry_point_executable(module.entry_point)?;
            }
            name if name == MM_SUPERVISOR_USER_GUID => {
                log::info!(
                    "Verifying MM Supervisor User module page protections: base=0x{:x}, length=0x{:x}",
                    base,
                    length
                );
                // User core pages must be user-accessible (SP clear).
                verify_page_attributes(base, length, MemoryAttributes::empty(), MemoryAttributes::Supervisor)?;
                verify_entry_point_executable(module.entry_point)?;
            }
            _ => {}
        }
    }

    Ok(())
}

/// Verifies that every page of the Ring 3 stack region is usable by a demoted
/// routine: user-accessible (SP clear), writable, and non-executable.
///
/// Unlike the checks above, this validates state the supervisor establishes
/// itself, so it runs after the stacks have been mapped rather than as part of
/// the incoming HOB validation rounds.
pub(crate) fn verify_cpl3_stacks_user_accessible(base: u64, length: u64) -> Result<(), HobValidationError> {
    verify_page_attributes(
        base,
        length,
        MemoryAttributes::ExecuteProtect,
        MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly,
    )
}

/// Verifies that every page in `[base, base + length)` has all of the `required`
/// attributes set and none of the `forbidden` attributes set in the active page
/// table.
///
/// The range is page-aligned (base rounded down, length rounded up) and each
/// page is queried individually, so attributes that differ across the range are
/// detected per page rather than merged into a single range query.
fn verify_page_attributes(
    base: u64,
    length: u64,
    required: MemoryAttributes,
    forbidden: MemoryAttributes,
) -> Result<(), HobValidationError> {
    let (aligned_base, aligned_len) = align_range(base, length, UEFI_PAGE_SIZE as u64)
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: base })?;

    let page_table = security_state().lock_page_table();
    let pt = page_table.as_ref().ok_or(HobValidationError::PageTableUnavailable)?;

    let page_size = UEFI_PAGE_SIZE as u64;
    let end = aligned_base.saturating_add(aligned_len);
    let mut addr = aligned_base;
    while addr < end {
        let attrs = pt
            .query_memory_region(addr, page_size)
            .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr })?;
        if !attrs.contains(required) {
            return Err(HobValidationError::PageMissingAttribute {
                addr,
                desired: required.bits(),
                found: attrs.bits(),
            });
        }
        if attrs.intersects(forbidden) {
            return Err(HobValidationError::PageHasForbiddenAttribute {
                addr,
                forbidden: forbidden.bits(),
                found: attrs.bits(),
            });
        }
        addr = addr.saturating_add(page_size);
    }

    Ok(())
}

/// Verifies that the page containing `entry_point` is executable (its
/// `ExecuteProtect` bit is clear) in the active page table, confirming the entry
/// point lands in the module's code section.
fn verify_entry_point_executable(entry_point: u64) -> Result<(), HobValidationError> {
    let page_size = UEFI_PAGE_SIZE as u64;
    let (page_base, _) = align_range(entry_point, 1, page_size)
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: entry_point })?;

    let page_table = security_state().lock_page_table();
    let pt = page_table.as_ref().ok_or(HobValidationError::PageTableUnavailable)?;

    let attrs = pt
        .query_memory_region(page_base, page_size)
        .map_err(|_| HobValidationError::PageAttributeQueryFailed { addr: page_base })?;
    if attrs.contains(MemoryAttributes::ExecuteProtect) {
        return Err(HobValidationError::EntryPointNotExecutable { entry_point });
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    fn region(base: u64, size: u64) -> SmramRegion {
        SmramRegion { base, size, pre_allocated: false }
    }

    fn pass_down() -> MmSupvPassDownHobData {
        MmSupvPassDownHobData {
            revision: crate::MM_SUPV_PASS_DOWN_HOB_REVISION,
            reserved: 0,
            mm_supervisor_cpl3_stack_base: 0x1000,
            mm_supervisor_cpl3_per_core_stack_size: 0x1000,
            sm_base: 0x2000,
            mm_initialized_buffer: 0x3000,
            mm_supv_firmware_policy_buffer: 0x4000,
            mm_supv_firmware_policy_buffer_size: 0x1000,
            mmi_entrypoint_size: 0x100,
        }
    }

    #[test]
    fn test_mm_supervisor_hob_validation_ranges_overlap() {
        assert!(ranges_overlap((0, 0x1000), (0x800, 0x1000)));
        assert!(!ranges_overlap((0, 0x1000), (0x1000, 0x1000)));
        assert!(!ranges_overlap((0x1000, 0x1000), (0, 0x1000)));
        // A zero-sized range at a shared boundary does not overlap.
        assert!(!ranges_overlap((0x1000, 0), (0, 0x1000)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_contiguous_single_region() {
        assert_eq!(validate_mmram_contiguous(&[region(0x1000, 0x2000)]), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_contiguous_adjacent_unsorted() {
        // Two adjacent regions provided out of order still validate as contiguous.
        let regions = [region(0x3000, 0x1000), region(0x1000, 0x2000)];
        assert_eq!(validate_mmram_contiguous(&regions), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_empty_regions_rejected() {
        assert_eq!(validate_mmram_contiguous(&[]), Err(HobValidationError::NoMmramRegions));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_overlapping_regions_rejected() {
        let regions = [region(0x1000, 0x2000), region(0x2000, 0x2000)];
        assert!(matches!(validate_mmram_contiguous(&regions), Err(HobValidationError::MmramRegionsOverlap { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_gap_rejected() {
        // Gap between 0x2000 and 0x3000.
        let regions = [region(0x1000, 0x1000), region(0x3000, 0x1000)];
        assert!(matches!(validate_mmram_contiguous(&regions), Err(HobValidationError::MmramNotContiguous { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_find_overlap() {
        assert!(find_overlap(&mut [(0, 0x1000), (0x2000, 0x1000)]).is_none());
        assert!(find_overlap(&mut [(0, 0x1000), (0x800, 0x1000)]).is_some());
        // Unsorted input with a contained range is still detected after sorting.
        assert!(find_overlap(&mut [(0x4000, 0x1000), (0, 0x8000), (0x1000, 0x100)]).is_some());
    }

    #[test]
    fn test_mm_supervisor_hob_validation_mmram_span() {
        // Contiguous regions (unsorted) collapse to one [min_base, max_end) span.
        let regions = [region(0x2000, 0x2000), region(0x1000, 0x1000)];
        assert_eq!(mmram_span(&regions), (0x1000, 0x3000));
        // A single region maps to itself.
        assert_eq!(mmram_span(&[region(0x8000, 0x1000)]), (0x8000, 0x1000));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_buffer_overlaps_mmram() {
        let mmram = (0x1000u64, 0x1000u64); // [0x1000, 0x2000)
        // An allocation outside the MMRAM span does not overlap.
        assert!(!buffer_overlaps_mmram(mmram, 0x2000, 0x1000));
        // An allocation landing inside the MMRAM span overlaps.
        assert!(buffer_overlaps_mmram(mmram, 0x1400, 0x100));
        // A partial overlap at the span boundary is detected.
        assert!(buffer_overlaps_mmram(mmram, 0x0800, 0x1000));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_entry_point_in_range() {
        assert!(entry_point_in_range(0x1000, 0x1000, 0x1000)); // at base
        assert!(entry_point_in_range(0x1800, 0x1000, 0x1000)); // interior
        assert!(!entry_point_in_range(0x2000, 0x1000, 0x1000)); // at end (exclusive)
        assert!(!entry_point_in_range(0x800, 0x1000, 0x1000)); // below base
        assert!(!entry_point_in_range(0, 0x1000, 0x1000)); // null
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
        assert_eq!(check_category_overlap(&mut [(0, 0x1000), (0x1000, 0x1000)], "v1 memory"), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_check_category_overlap_reports() {
        let result = check_category_overlap(&mut [(0, 0x2000), (0x1000, 0x1000)], "v2 I/O");
        assert!(matches!(result, Err(HobValidationError::ResourceDescriptorsOverlap { kind: "v2 I/O", .. })));
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
        assert!(matches!(result, Err(HobValidationError::V2ContainsUceAttribute { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_no_cacheability_rejected() {
        use patina::pi::hob::EFI_RESOURCE_SYSTEM_MEMORY;
        let result = check_v2_attributes(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, 0);
        assert!(matches!(result, Err(HobValidationError::V2InvalidCacheability { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_multiple_cacheability_rejected() {
        use patina::pi::hob::EFI_RESOURCE_SYSTEM_MEMORY;
        let result = check_v2_attributes(EFI_RESOURCE_SYSTEM_MEMORY, 0x1000, efi::MEMORY_WB | efi::MEMORY_WT);
        assert!(matches!(result, Err(HobValidationError::V2InvalidCacheability { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_io_zero_valid() {
        assert_eq!(check_v2_attributes(EFI_RESOURCE_IO, 0x1000, 0), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_v2_attributes_io_nonzero_rejected() {
        let result = check_v2_attributes(EFI_RESOURCE_IO, 0x1000, efi::MEMORY_WB);
        assert!(matches!(result, Err(HobValidationError::V2IoAttributesNotZero { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_all_null_pointers_ok() {
        // Null pointers are skipped, so an all-null (correct-revision) HOB validates.
        let mut pd = pass_down();
        pd.mm_supervisor_cpl3_stack_base = 0;
        pd.sm_base = 0;
        pd.mm_initialized_buffer = 0;
        pd.mm_supv_firmware_policy_buffer = 0;
        assert_eq!(validate_pass_down_pointers(&pd), Ok(()));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_pointer_outside_mmram() {
        // The page allocator is uninitialized in unit tests, so any non-null
        // pointer is reported as outside MMRAM.
        let result = validate_pass_down_pointers(&pass_down());
        assert!(matches!(result, Err(HobValidationError::PassDownPointerOutsideMmram { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_pass_down_bad_revision() {
        let mut pd = pass_down();
        pd.revision = crate::MM_SUPV_PASS_DOWN_HOB_REVISION + 1;
        assert!(matches!(validate_pass_down_pointers(&pd), Err(HobValidationError::PassDownInvalidRevision { .. })));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_push_range() {
        let mut ranges = [(0u64, 0u64); 2];
        let mut count = 0usize;
        push_range(&mut ranges, &mut count, (0x1000, 0x100), "test");
        push_range(&mut ranges, &mut count, (0x2000, 0x200), "test");
        assert_eq!(count, 2);
        assert_eq!(ranges.first(), Some(&(0x1000, 0x100)));
        assert_eq!(ranges.get(1), Some(&(0x2000, 0x200)));
        // Exceeding capacity leaves both the count and the stored ranges unchanged.
        push_range(&mut ranges, &mut count, (0x3000, 0x300), "test");
        assert_eq!(count, 2);
        assert_eq!(ranges.get(1), Some(&(0x2000, 0x200)));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_verify_page_attributes_page_table_unavailable() {
        // The page table is uninitialized in unit tests, so the query reports it
        // as unavailable rather than panicking.
        let result = verify_page_attributes(0x1000, 0x1000, MemoryAttributes::Supervisor, MemoryAttributes::empty());
        assert_eq!(result, Err(HobValidationError::PageTableUnavailable));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_verify_entry_point_executable_page_table_unavailable() {
        // The page table is uninitialized in unit tests, so the query reports it
        // as unavailable rather than panicking.
        assert_eq!(verify_entry_point_executable(0x1000), Err(HobValidationError::PageTableUnavailable));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_verify_cpl3_stacks_page_table_unavailable() {
        // The page table is uninitialized in unit tests, so the query reports it
        // as unavailable rather than panicking.
        assert_eq!(verify_cpl3_stacks_user_accessible(0x1000, 0x2000), Err(HobValidationError::PageTableUnavailable));
    }

    #[test]
    fn test_mm_supervisor_hob_validation_error_display_non_empty() {
        // Every error variant must produce a non-empty, human-readable message.
        let errors = [
            HobValidationError::NullHobList,
            HobValidationError::NoMmramRegions,
            HobValidationError::MmramRegionsOverlap { base_a: 0, size_a: 0x1000, base_b: 0x800, size_b: 0x1000 },
            HobValidationError::MmramNotContiguous { covered: 0x1000, span: 0x2000 },
            HobValidationError::MemoryAllocationInsideMmram { base: 0x1000, length: 0x1000 },
            HobValidationError::AllocationModuleOutsideMmram { base: 0x1000, length: 0x1000 },
            HobValidationError::AllocationModulesOverlap { base_a: 0, size_a: 0x1000, base_b: 0x800, size_b: 0x1000 },
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
            HobValidationError::V2ContainsUceAttribute { base: 0x1000, attributes: 0x1 },
            HobValidationError::V2InvalidCacheability { base: 0x1000, attributes: 0 },
            HobValidationError::V2IoAttributesNotZero { base: 0x1000, attributes: 0x4 },
            HobValidationError::PassDownHobMissing,
            HobValidationError::PassDownHobTooSmall,
            HobValidationError::PassDownInvalidRevision { found: 3, expected: 2 },
            HobValidationError::PassDownPointerOutsideMmram { field: "sm_base", addr: 0x1000, size: 8 },
            HobValidationError::PageTableUnavailable,
            HobValidationError::PageAttributeQueryFailed { addr: 0x1000 },
            HobValidationError::PageMissingAttribute { addr: 0x1000, desired: 0x1, found: 0 },
            HobValidationError::PageHasForbiddenAttribute { addr: 0x1000, forbidden: 0x1, found: 0x1 },
            HobValidationError::EntryPointNotExecutable { entry_point: 0x5000 },
        ];
        for err in errors {
            assert!(!format!("{err}").is_empty());
        }
    }
}
