//! Validation of the Page Table Adopted from MM Init
//!
//! MM Init builds the page table the supervisor keeps running on. Before anything is mapped
//! through it for Ring 3, the BSP checks it against what the validated HOB list tells MM Init to
//! map:
//!
//! - the root table at CR3 lies inside MMRAM;
//! - every mapping is an identity mapping;
//! - outside MMRAM, every present page lies in a region a HOB names, and carries the attributes MM
//!   Init gives that region:
//!   - MMIO and firmware-device resource descriptors: user-accessible, non-executable;
//!   - unblock-region HOBs: non-executable, and supervisor-only exactly when the HOB sets
//!     `EFI_MEMORY_SP`;
//!   - the supervisor and user communication buffers and their status mailboxes: supervisor-only,
//!     non-executable;
//!   - IOMMU protected regions: user-accessible, read-only, non-executable.
//!
//! Only a protected region can share pages with another region, and such a page must satisfy
//! both.
//!
//! Inside MMRAM, no page is both writable and executable, and:
//!
//! - user-accessible pages lie in the user core, a driver, or the Ring 3 stacks;
//! - executable pages lie in the supervisor core, the user core, a driver, or the SMI entry code;
//! - every other page is supervisor-only and non-executable.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::mem::size_of;

use patina::management_mode::MmCommBufferStatus;
use patina::management_mode::comm_buffer_hob::{MM_COMM_BUFFER_HOB_GUID, MmCommonBufferHobData};
use patina::management_mode::protocol::mm_supervisor_request::{
    MM_SUPV_UNBLOCK_REGION_HOB_GUID, MmSupervisorUnblockMemoryParams,
};
use patina::management_mode::supervisor::{MM_SUPERVISOR_CORE_GUID, MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID};
use patina::pi::hob::{
    EFI_RESOURCE_FIRMWARE_DEVICE, EFI_RESOURCE_MEMORY_MAPPED_IO, Hob, MEMORY_TYPE_INFO_HOB_GUID,
    PhaseHandoffInformationTable,
};
use patina::standard::efi;
use patina::{MemoryRange, UEFI_PAGE_MASK, UEFI_PAGE_SIZE};
use patina_paging::{MappedRegion, MemoryAttributes};
use zerocopy::FromBytes;

use crate::comm_buffer::{MM_COMMON_REGION_HOB_GUID, MmCommonRegionHobData};
use crate::error::MmSupervisorResult;
use crate::hob::pass_down::MmSupvPassDownHobData;
use crate::hob::protected_region::{MM_PROTECTED_REGION_HOB_GUID, MmProtectedRegion};
use crate::hob::validation::{HobValidationError, check_page_attributes, smbase_array, with_active_page_table};
use crate::intrinsics::read_cr3;
use crate::mmcore::smi_idt_patch::SMM_HANDLER_OFFSET;

/// The attributes MM Init maps a region outside MMRAM with: those every page in it must carry, and
/// those none may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mapping {
    required: MemoryAttributes,
    forbidden: MemoryAttributes,
}

/// User-accessible data: MMIO and firmware-device resources, and unblock regions without
/// `EFI_MEMORY_SP`.
const USER_DATA: Mapping =
    Mapping { required: MemoryAttributes::ExecuteProtect, forbidden: MemoryAttributes::Supervisor };

/// Supervisor-only data: unblock regions with `EFI_MEMORY_SP`, the communication buffers and their
/// status mailboxes.
const SUPERVISOR_DATA: Mapping = Mapping {
    required: MemoryAttributes::Supervisor.union(MemoryAttributes::ExecuteProtect),
    forbidden: MemoryAttributes::empty(),
};

/// User-accessible, read-only data: the IOMMU protected regions, so MM code can read the IOMMU's
/// registers but not reprogram it.
const USER_READ_ONLY: Mapping = Mapping {
    required: MemoryAttributes::ReadOnly.union(MemoryAttributes::ExecuteProtect),
    forbidden: MemoryAttributes::Supervisor,
};

/// Validates the adopted page table against the HOB list at `handoff`. `mmram` is the span the
/// SMRR covers.
///
/// Checks:
/// - The root table at CR3 lies inside MMRAM.
/// - Every leaf mapping passes [`verify_paging_structure`].
///
/// The HOB list must already have passed
/// [`validate_incoming_hobs_pre_paging_init`](crate::hob::validation::validate_incoming_hobs_pre_paging_init)
/// and
/// [`validate_incoming_hobs_post_paging_init`](crate::hob::validation::validate_incoming_hobs_post_paging_init).
/// Must run before anything is mapped outside MMRAM or for Ring 3, and before the unblocked memory
/// tracker is seeded from the page table.
// Reads the live CR3, which host tests leave at 0.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn validate_page_table_post_activation(
    handoff: &PhaseHandoffInformationTable,
    pass_down: &MmSupvPassDownHobData,
    number_of_cpus: usize,
    mmram: MemoryRange,
) -> MmSupervisorResult<()> {
    let root = read_cr3() & !(UEFI_PAGE_MASK as u64);
    if !MemoryRange::new(root, UEFI_PAGE_SIZE as u64).is_ok_and(|table| mmram.contains_range(&table)) {
        return Err(HobValidationError::PageTableOutsideMmram { root }.into());
    }

    // SAFETY: the pre-paging HOB checks placed the SMBASE array's `number_of_cpus` entries in
    // allocated MMRAM.
    let smbases = unsafe { smbase_array(pass_down, number_of_cpus) }?;
    let hob = Hob::Handoff(handoff);
    with_active_page_table(|page_table| {
        verify_paging_structure(&hob, mmram, pass_down, smbases, page_table.iter_mapped_regions(None))
    })?;
    log::info!("Page table at 0x{root:016x} maps only what the HOB list names");
    Ok(())
}

/// Validates `regions`, the leaf mappings in ascending address order. `smbases` has one entry per
/// CPU.
///
/// Checks:
/// - Every mapping is an identity mapping and a valid address range.
/// - Each blanket inside MMRAM passes [`verify_inside_mmram_region`].
/// - Each blanket outside MMRAM passes [`verify_outside_mmram_region`].
///
/// A blanket is a leaf plus the contiguous identity leaves after it with the same attributes, so
/// the HOB list is walked once per blanket rather than once per page. Blankets stop at the MMRAM
/// edges; a single leaf that straddles one fails the outside check.
fn verify_paging_structure<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>> + Clone,
    mmram: MemoryRange,
    pass_down: &MmSupvPassDownHobData,
    smbases: &[u64],
    regions: impl IntoIterator<Item = MappedRegion>,
) -> MmSupervisorResult<()> {
    let mut regions = regions.into_iter().peekable();
    while let Some(region) = regions.next() {
        if region.va != region.pa {
            return Err(HobValidationError::MappingNotIdentity { va: region.va, pa: region.pa }.into());
        }

        let mut blanket = MemoryRange::new(region.pa, region.size).map_err(|_| {
            HobValidationError::InvalidAddressRange { kind: "page table mapping", base: region.pa, length: region.size }
        })?;
        while let Some(next) = regions.next_if(|next| {
            next.va == next.pa
                && next.pa == blanket.end()
                && next.attributes == region.attributes
                && next.pa != mmram.base()
                && next.pa != mmram.end()
        }) {
            blanket = blanket.extend(next.size).map_err(|_| HobValidationError::InvalidAddressRange {
                kind: "page table mapping",
                base: next.pa,
                length: next.size,
            })?;
        }

        if mmram.contains_range(&blanket) {
            verify_inside_mmram_region(hobs.clone(), pass_down, smbases, blanket, region.attributes)?;
        } else {
            verify_outside_mmram_region(hobs.clone(), blanket, region.attributes)?;
        }
    }

    Ok(())
}

/// Validates `blanket`, present pages inside MMRAM that all carry `attributes`.
///
/// Checks:
/// - No page is both writable and executable.
/// - A user-accessible page lies in the user core, a driver or the Ring 3 stacks.
/// - An executable page lies in the supervisor core, the user core, a driver or the SMI entry
///   code.
///
/// Supervisor-only, non-executable pages may lie anywhere in MMRAM. MM Init's own image is in
/// neither set, as it never runs under this page table.
fn verify_inside_mmram_region<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>> + Clone,
    pass_down: &MmSupvPassDownHobData,
    smbases: &[u64],
    blanket: MemoryRange,
    attributes: MemoryAttributes,
) -> MmSupervisorResult<()> {
    if !attributes.intersects(MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect) {
        return Err(
            HobValidationError::PageWritableAndExecutable { addr: blanket.base(), found: attributes.bits() }.into()
        );
    }

    // A blanket can span several regions, such as two drivers loaded back to back, so the covered
    // prefix is extended one region at a time.
    if !attributes.contains(MemoryAttributes::Supervisor) {
        let stacks =
            MemoryRange::new(pass_down.cpl3_stack_base, pass_down.cpl3_stack_size.saturating_mul(smbases.len() as u64))
                .ok();
        let mut covered = blanket.base();
        while covered < blanket.end() {
            let mut reach = stacks.filter(|stacks| stacks.contains(covered)).map(|stacks| stacks.end());
            for current in hobs.clone() {
                if let Hob::MemoryAllocationModule(module) = current
                    && module.alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID
                    && module.module_name != MM_SUPERVISOR_CORE_GUID
                    && let Ok(image) = MemoryRange::new(
                        module.alloc_descriptor.memory_base_address,
                        module.alloc_descriptor.memory_length,
                    )
                    && image.contains(covered)
                {
                    reach = reach.max(Some(image.end()));
                }
            }
            let Some(reach) = reach else {
                let uncovered = MemoryRange::new(covered, blanket.end() - covered).unwrap_or(blanket);
                return Err(HobValidationError::UserPageNotCovered { range: uncovered }.into());
            };
            covered = reach;
        }
    }

    if !attributes.contains(MemoryAttributes::ExecuteProtect) {
        let mut covered = blanket.base();
        while covered < blanket.end() {
            let mut reach = None;
            for current in hobs.clone() {
                if let Hob::MemoryAllocationModule(module) = current
                    && module.alloc_descriptor.name == MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID
                    && let Ok(image) = MemoryRange::new(
                        module.alloc_descriptor.memory_base_address,
                        module.alloc_descriptor.memory_length,
                    )
                    && image.contains(covered)
                {
                    reach = reach.max(Some(image.end()));
                }
            }

            // Or the SMI entry code, on the pages it spans from `SMBASE + SMM_HANDLER_OFFSET`.
            for smbase in smbases {
                let entry_code = smbase
                    .checked_add(SMM_HANDLER_OFFSET)
                    .and_then(|entry| MemoryRange::new(entry, pass_down.mmi_entry_size).ok())
                    .and_then(|entry| entry.align_outward(UEFI_PAGE_SIZE as u64).ok());
                if let Some(entry_code) = entry_code
                    && entry_code.contains(covered)
                {
                    reach = reach.max(Some(entry_code.end()));
                }
            }

            let Some(reach) = reach else {
                let uncovered = MemoryRange::new(covered, blanket.end() - covered).unwrap_or(blanket);
                return Err(HobValidationError::ExecutablePageNotCovered { range: uncovered }.into());
            };
            covered = reach;
        }
    }

    Ok(())
}

/// Validates `blanket`, present pages outside MMRAM that all carry `attributes`, against the
/// regions the HOB list `hobs` names (see [`allowed_outside_regions`]).
///
/// Checks:
/// - Every named region that overlaps the blanket allows `attributes`.
/// - The named regions together cover the whole blanket.
fn verify_outside_mmram_region<'a>(
    hobs: impl IntoIterator<Item = Hob<'a>> + Clone,
    blanket: MemoryRange,
    attributes: MemoryAttributes,
) -> MmSupervisorResult<()> {
    for (allowed, mapping) in hobs.clone().into_iter().flat_map(allowed_outside_regions).flatten() {
        if let Ok(shared) = blanket.intersection(&allowed) {
            check_page_attributes(shared.base(), attributes, mapping.required, mapping.forbidden).inspect_err(
                |err| log::error!("{blanket} outside MMRAM is mapped in a way {allowed} does not allow: {err}"),
            )?;
        }
    }

    let mut covered = blanket.base();
    while covered < blanket.end() {
        let reach = hobs
            .clone()
            .into_iter()
            .flat_map(allowed_outside_regions)
            .flatten()
            .filter(|(allowed, _)| allowed.contains(covered))
            .map(|(allowed, _)| allowed.end())
            .max();
        let Some(reach) = reach else {
            let uncovered = MemoryRange::new(covered, blanket.end() - covered).unwrap_or(blanket);
            log::error!("{uncovered} outside MMRAM is mapped, but no HOB names it");
            return Err(HobValidationError::RangeMapped { range: uncovered }.into());
        };
        covered = reach;
    }

    Ok(())
}

/// Returns the regions outside MMRAM that `hob` names, each with its [`Mapping`]. Only the
/// communication buffer HOBs fill both slots: the buffer and its status mailbox.
fn allowed_outside_regions(hob: Hob<'_>) -> [Option<(MemoryRange, Mapping)>; 2] {
    #[cfg(feature = "v1_resource_descriptor_support")]
    let resource = match hob {
        Hob::ResourceDescriptor(resource) => Some(resource),
        _ => None,
    };
    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    let resource = match hob {
        Hob::ResourceDescriptorV2(resource) => Some(&resource.v1),
        _ => None,
    };
    if let Some(resource) = resource {
        let is_mmio = matches!(resource.resource_type, EFI_RESOURCE_MEMORY_MAPPED_IO | EFI_RESOURCE_FIRMWARE_DEVICE)
            && resource.owner != MEMORY_TYPE_INFO_HOB_GUID;
        let range = MemoryRange::new(resource.physical_start, resource.resource_length)
            .and_then(|range| range.align_outward(UEFI_PAGE_SIZE as u64));
        return [range.ok().filter(|_| is_mmio).map(|range| (range, USER_DATA)), None];
    }

    let Hob::GuidHob(guid_hob, data) = hob else {
        return [None, None];
    };

    if guid_hob.name == MM_SUPV_UNBLOCK_REGION_HOB_GUID {
        let Some(payload) = data.get(..MmSupervisorUnblockMemoryParams::SIZE) else {
            return [None, None];
        };
        // SAFETY: `payload` holds a whole parameter structure, which every bit pattern is valid
        // for, and `read_unaligned` has no alignment requirement.
        let params = unsafe { payload.as_ptr().cast::<MmSupervisorUnblockMemoryParams>().read_unaligned() };
        let descriptor = params.memory_descriptor;
        // MM Init honors only `EFI_MEMORY_SP`, and maps every unblock region as read/write data.
        let mapping = if descriptor.attribute & efi::MEMORY_SP != 0 { SUPERVISOR_DATA } else { USER_DATA };
        let range = MemoryRange::from_pages(descriptor.physical_start, descriptor.number_of_pages);
        return [range.ok().map(|range| (range, mapping)), None];
    }

    if guid_hob.name == MM_PROTECTED_REGION_HOB_GUID {
        let range = MmProtectedRegion::read_from_prefix(data).ok().and_then(|(region, _)| {
            MemoryRange::from_pages(region.protected_region_base, region.protected_region_pages).ok()
        });
        return [range.map(|range| (range, USER_READ_ONLY)), None];
    }

    // The supervisor or user communication buffer.
    let buffer = if guid_hob.name == MM_COMMON_REGION_HOB_GUID {
        MmCommonRegionHobData::read_from_prefix(data)
            .ok()
            .map(|(region, _)| (region.physical_start, region.number_of_pages, region.status_address))
    } else if guid_hob.name == MM_COMM_BUFFER_HOB_GUID {
        MmCommonBufferHobData::read_from_prefix(data)
            .ok()
            .map(|(buffer, _)| (buffer.physical_start, buffer.number_of_pages, buffer.status_buffer))
    } else {
        None
    };
    let Some((base, pages, status)) = buffer else {
        return [None, None];
    };
    let buffer = MemoryRange::from_pages(base, pages);
    let status = MemoryRange::new(status, size_of::<MmCommBufferStatus>() as u64)
        .and_then(|status| status.align_outward(UEFI_PAGE_SIZE as u64));
    [buffer.ok().map(|range| (range, SUPERVISOR_DATA)), status.ok().map(|range| (range, SUPERVISOR_DATA))]
}
