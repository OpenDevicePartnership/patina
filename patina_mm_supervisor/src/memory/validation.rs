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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::error::MmSupervisorError;
    use crate::hob::pass_down::MM_SUPV_PASS_DOWN_HOB_REVISION;
    use crate::hob::protected_region::ProtectedRegionType;
    use crate::test_support::RawHobList;
    use patina::BinaryGuid;
    use patina::management_mode::supervisor::{MM_SUPERVISOR_INIT_GUID, MM_SUPERVISOR_USER_GUID};
    use patina::pi::guid::HOB_MEMORY_ALLOC_MODULE_GUID;
    use patina::pi::hob::{
        EFI_RESOURCE_SYSTEM_MEMORY, HobHeader, MEMORY_ALLOCATION, MemoryAllocationHeader, MemoryAllocationModule,
        RESOURCE_DESCRIPTOR, ResourceDescriptor, ResourceDescriptorV2,
    };
    use zerocopy::IntoBytes;

    const USER_NX: MemoryAttributes = MemoryAttributes::ExecuteProtect;
    const SUPERVISOR_NX: MemoryAttributes = MemoryAttributes::Supervisor.union(MemoryAttributes::ExecuteProtect);
    const USER_RO_NX: MemoryAttributes = MemoryAttributes::ReadOnly.union(MemoryAttributes::ExecuteProtect);
    const USER_CODE: MemoryAttributes = MemoryAttributes::ReadOnly;
    const SUPERVISOR_CODE: MemoryAttributes = MemoryAttributes::Supervisor.union(MemoryAttributes::ReadOnly);

    /// The two CPUs' SMBASEs, which put their SMI entry code at `0x7500_8000` and `0x7500_A000`.
    const SMBASES: [u64; 2] = [0x7500_0000, 0x7500_2000];

    fn span(base: u64, size: u64) -> MemoryRange {
        MemoryRange::new(base, size).unwrap()
    }

    fn mmram() -> MemoryRange {
        span(0x7400_0000, 0x200_0000)
    }

    /// The `PassDown` payload for [`SMBASES`]: the two CPUs' Ring 3 stacks end at the top of MMRAM.
    fn pass_down() -> MmSupvPassDownHobData {
        MmSupvPassDownHobData {
            revision: MM_SUPV_PASS_DOWN_HOB_REVISION,
            reserved: 0,
            cpl3_stack_base: 0x75FF_E000,
            cpl3_stack_size: 0x1000,
            sm_base_array: 0,
            mm_initialized_buffer: 0,
            firmware_policy_buffer: 0,
            firmware_policy_buffer_size: 0,
            mmi_entry_size: 0x100,
        }
    }

    fn verify(hob: &Hob<'_>, regions: impl IntoIterator<Item = MappedRegion>) -> MmSupervisorResult<()> {
        verify_paging_structure(hob, mmram(), &pass_down(), &SMBASES, regions)
    }

    /// An identity mapping of `size` bytes at `base`.
    fn leaf(base: u64, size: u64, attributes: MemoryAttributes) -> MappedRegion {
        MappedRegion { va: base, pa: base, size, attributes }
    }

    /// Adds a resource descriptor in both its V1 and V2 forms, so the list reads the same with or
    /// without V1 descriptor support.
    fn push_resource(list: &mut RawHobList, owner: BinaryGuid, resource_type: u32, base: u64, length: u64) {
        let v1 = ResourceDescriptor {
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
        };
        // `From` keeps the V1 header length, so a HOB list walk would read the V2 descriptor short.
        let mut v2 = ResourceDescriptorV2::from(v1);
        v2.v1.header.length = size_of::<ResourceDescriptorV2>() as u16;
        list.push_struct(v1);
        list.push_struct(v2);
    }

    fn push_module(list: &mut RawHobList, name: BinaryGuid, module_name: BinaryGuid, base: u64, length: u64) {
        list.push_struct(MemoryAllocationModule {
            header: HobHeader {
                r#type: MEMORY_ALLOCATION,
                length: size_of::<MemoryAllocationModule>() as u16,
                reserved: 0,
            },
            alloc_descriptor: MemoryAllocationHeader {
                name,
                memory_base_address: base,
                memory_length: length,
                memory_type: efi::RUNTIME_SERVICES_CODE,
                reserved: [0; 4],
            },
            module_name,
            entry_point: base,
        });
    }

    fn hob_list() -> RawHobList {
        let owner = BinaryGuid::from_string("0f1e2d3c-4b5a-4968-8776-a5b4c3d2e1f0");
        let mut list = RawHobList::new();
        push_resource(&mut list, owner, EFI_RESOURCE_SYSTEM_MEMORY, 0, 0x8000_0000);
        push_resource(&mut list, owner, EFI_RESOURCE_MEMORY_MAPPED_IO, 0xFC80_0000, 0x8_0000);
        push_resource(&mut list, owner, EFI_RESOURCE_FIRMWARE_DEVICE, 0xFED0_0000, 0x1000);
        for base in [0x659C_7000, 0x65DF_E000, 0x73FF_F000, 0x7600_0000, 0xFED0_1000] {
            push_resource(&mut list, owner, EFI_RESOURCE_MEMORY_MAPPED_IO, base, 0x1000);
        }
        // A memory bin descriptor, which MM Init skips even when it names MMIO.
        push_resource(&mut list, MEMORY_TYPE_INFO_HOB_GUID, EFI_RESOURCE_MEMORY_MAPPED_IO, 0xE000_0000, 0x1000);

        // Each unblock region: the memory descriptor (type, start, virtual start, pages, attribute)
        // and a non-zero identifier GUID.
        for (base, pages, attribute) in [(0x6000_0000u64, 1, efi::MEMORY_SP), (0x659F_E000, 0x400, 0)] {
            let params: [u64; 7] = [0, base, 0, pages, attribute, 1, 1];
            list.push_guid_hob(MM_SUPV_UNBLOCK_REGION_HOB_GUID, params.as_bytes());
        }
        for base in [0xFC80_0000, 0xFED2_0000] {
            let region = MmProtectedRegion {
                protected_region_type: ProtectedRegionType::Iommu as u64,
                protected_region_base: base,
                protected_region_pages: 0x10,
            };
            list.push_guid_hob(MM_PROTECTED_REGION_HOB_GUID, region.as_bytes());
        }
        // Region type, buffer, pages and mailbox; then the user buffer, pages and mailbox.
        list.push_guid_hob(MM_COMMON_REGION_HOB_GUID, [0u64, 0x659E_B000, 0x12, 0x659E_A000].as_bytes());
        list.push_guid_hob(MM_COMM_BUFFER_HOB_GUID, [0x659C_9000u64, 0x21, 0x659C_8000].as_bytes());

        let module = MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID;
        let second_driver = BinaryGuid::from_string("1f2e3d4c-5b6a-4978-8695-b4c3d2e1f0a5");
        push_module(&mut list, module, owner, 0x7400_0000, 0x2000);
        push_module(&mut list, module, second_driver, 0x7400_2000, 0x2000);
        push_module(&mut list, module, MM_SUPERVISOR_CORE_GUID, 0x7410_0000, 0x1_0000);
        push_module(&mut list, module, MM_SUPERVISOR_USER_GUID, 0x7420_0000, 0x1_0000);
        push_module(&mut list, HOB_MEMORY_ALLOC_MODULE_GUID, MM_SUPERVISOR_INIT_GUID, 0x7430_0000, 0x1_0000);
        list.finish()
    }

    fn mapped(base: u64, size: u64) -> MmSupervisorResult<()> {
        Err(HobValidationError::RangeMapped { range: span(base, size) }.into())
    }

    fn missing(addr: u64, desired: MemoryAttributes, found: MemoryAttributes) -> MmSupervisorResult<()> {
        Err(HobValidationError::PageMissingAttribute { addr, desired: desired.bits(), found: found.bits() }.into())
    }

    #[test]
    fn test_page_table_validation_accepts_what_mm_init_maps_outside_mmram() {
        let list = hob_list();
        let hob = Hob::Handoff(list.handoff());
        let regions = [
            // The unblock region with `EFI_MEMORY_SP` is supervisor-only.
            leaf(0x6000_0000, 0x1000, SUPERVISOR_NX),
            // The user mailbox and buffer, then the supervisor's, back to back: one blanket that
            // four regions from two HOBs cover.
            leaf(0x659C_8000, 0x3_5000, SUPERVISOR_NX),
            // An unblock region and the MMIO page after it: one blanket that a GUID HOB and a
            // resource descriptor cover.
            leaf(0x659F_E000, 0x40_0000, USER_NX),
            leaf(0x65DF_E000, 0x1000, USER_NX),
            // MMIO pages on both sides of MMRAM. No blanket spreads into MMRAM, where the pages
            // beside them are user data in the drivers and the Ring 3 stacks.
            leaf(0x73FF_F000, 0x1000, USER_NX),
            leaf(0x7400_0000, 0x1000, USER_NX),
            leaf(0x75FF_E000, 0x2000, USER_NX),
            leaf(0x7600_0000, 0x1000, USER_NX),
            // An IOMMU region inside an MMIO range is read-only, the rest of the range is not.
            leaf(0xFC80_0000, 0x1_0000, USER_RO_NX),
            leaf(0xFC81_0000, 0x7_0000, USER_NX),
            // A firmware device and an MMIO range back to back, mapped page by page.
            leaf(0xFED0_0000, 0x1000, USER_NX),
            leaf(0xFED0_1000, 0x1000, USER_NX),
            // An IOMMU region no MMIO range holds.
            leaf(0xFED2_0000, 0x1_0000, USER_RO_NX),
        ];
        assert_eq!(verify(&hob, regions), Ok(()));
        assert_eq!(verify(&hob, []), Ok(()));
    }

    #[test]
    fn test_page_table_validation_rejects_pages_no_hob_names() {
        let list = hob_list();
        let hob = Hob::Handoff(list.handoff());
        let check = |region: MappedRegion| verify(&hob, [region]);

        // System memory, and MMIO only a memory bin descriptor names.
        assert_eq!(check(leaf(0x1000_0000, 0x1000, USER_NX)), mapped(0x1000_0000, 0x1000));
        assert_eq!(check(leaf(0xE000_0000, 0x1000, USER_NX)), mapped(0xE000_0000, 0x1000));
        // A blanket is reported from where the regions stop covering it.
        assert_eq!(check(leaf(0xFED0_0000, 0x3000, USER_NX)), mapped(0xFED0_2000, 0x1000));
        // A mapping that reaches from MMRAM out past its end.
        assert_eq!(check(leaf(0x75E0_0000, 0x40_0000, USER_NX)), mapped(0x75E0_0000, 0x40_0000));

        let alias = MappedRegion { va: 0xFED0_0000, pa: 0xFED0_1000, size: 0x1000, attributes: USER_NX };
        assert_eq!(
            check(alias),
            Err(HobValidationError::MappingNotIdentity { va: 0xFED0_0000, pa: 0xFED0_1000 }.into())
        );
    }

    #[test]
    fn test_page_table_validation_rejects_attributes_mm_init_does_not_give() {
        let list = hob_list();
        let hob = Hob::Handoff(list.handoff());
        let check = |base, size, attributes| verify(&hob, [leaf(base, size, attributes)]);
        let supervisor = |addr| -> MmSupervisorResult<()> {
            Err(HobValidationError::PageHasForbiddenAttribute {
                addr,
                forbidden: MemoryAttributes::Supervisor.bits(),
                found: SUPERVISOR_NX.bits(),
            }
            .into())
        };

        // Ring 3 must not reach a communication buffer, a mailbox, or an unblock region the HOB
        // keeps supervisor-only.
        for base in [0x659E_B000, 0x659C_8000, 0x6000_0000] {
            assert_eq!(check(base, 0x1000, USER_NX), missing(base, SUPERVISOR_NX, USER_NX));
        }
        // MMIO, and unblock regions without `EFI_MEMORY_SP`, are user data.
        assert_eq!(check(0xFED0_0000, 0x1000, SUPERVISOR_NX), supervisor(0xFED0_0000));
        assert_eq!(check(0x659F_E000, 0x1000, SUPERVISOR_NX), supervisor(0x659F_E000));
        // An IOMMU region is read-only, inside an MMIO range or not.
        assert_eq!(check(0xFED2_0000, 0x1_0000, USER_NX), missing(0xFED2_0000, USER_RO_NX, USER_NX));
        assert_eq!(check(0xFC80_0000, 0x8_0000, USER_NX), missing(0xFC80_0000, USER_RO_NX, USER_NX));
        // An MMIO page and the user mailbox after it, mapped alike, form one blanket. Whatever they
        // carry, one of the two regions does not allow it.
        let mmio_then_mailbox =
            |attributes| verify(&hob, [leaf(0x659C_7000, 0x1000, attributes), leaf(0x659C_8000, 0x1000, attributes)]);
        assert_eq!(mmio_then_mailbox(USER_NX), missing(0x659C_8000, SUPERVISOR_NX, USER_NX));
        assert_eq!(mmio_then_mailbox(SUPERVISOR_NX), supervisor(0x659C_7000));
        // Nothing outside MMRAM is executable.
        assert_eq!(
            check(0xFED0_0000, 0x1000, MemoryAttributes::empty()),
            Err(HobValidationError::PageWritableAndExecutable { addr: 0xFED0_0000, found: 0 }.into())
        );
        assert_eq!(
            check(0xFED0_0000, 0x1000, MemoryAttributes::ReadOnly),
            missing(0xFED0_0000, USER_NX, MemoryAttributes::ReadOnly)
        );
    }

    #[test]
    fn test_page_table_validation_accepts_what_mm_init_maps_inside_mmram() {
        let list = hob_list();
        let hob = Hob::Handoff(list.handoff());
        let regions = [
            // Data spanning the two drivers forms one blanket, which both images cover.
            leaf(0x7400_0000, 0x3000, USER_NX),
            leaf(0x7400_3000, 0x1000, USER_CODE),
            leaf(0x7410_0000, 0x1000, SUPERVISOR_CODE),
            leaf(0x7410_1000, 0xF000, SUPERVISOR_NX),
            leaf(0x7420_0000, 0x1000, USER_CODE),
            leaf(0x7420_1000, 0xF000, USER_RO_NX),
            // MM Init's image, and everything else, is supervisor data.
            leaf(0x7430_0000, 0x1_0000, SUPERVISOR_NX),
            leaf(0x7440_0000, 0x10_0000, SUPERVISOR_NX | MemoryAttributes::ReadOnly),
            // Each CPU's SMI entry code page, between the save-state pages of others.
            leaf(0x7500_8000, 0x1000, SUPERVISOR_CODE),
            leaf(0x7500_9000, 0x1000, SUPERVISOR_NX),
            leaf(0x7500_A000, 0x1000, SUPERVISOR_CODE),
            leaf(0x75FF_E000, 0x2000, USER_NX),
        ];
        assert_eq!(verify(&hob, regions), Ok(()));
    }

    #[test]
    fn test_page_table_validation_rejects_user_and_executable_pages_outside_their_regions() {
        let list = hob_list();
        let hob = Hob::Handoff(list.handoff());
        let check = |base, size, attributes| verify(&hob, [leaf(base, size, attributes)]);
        let user = |base, size| -> MmSupervisorResult<()> {
            Err(HobValidationError::UserPageNotCovered { range: span(base, size) }.into())
        };
        let executable = |base, size| -> MmSupervisorResult<()> {
            Err(HobValidationError::ExecutablePageNotCovered { range: span(base, size) }.into())
        };

        // User pages in free MMRAM or the supervisor core, and a blanket that runs off the end of
        // the second driver, reported from where it leaves it.
        assert_eq!(check(0x7440_0000, 0x1000, USER_NX), user(0x7440_0000, 0x1000));
        assert_eq!(check(0x7410_0000, 0x1000, USER_CODE), user(0x7410_0000, 0x1000));
        assert_eq!(check(0x7400_2000, 0x3000, USER_NX), user(0x7400_4000, 0x1000));
        // Executable pages in the Ring 3 stacks, in MM Init's image, in free MMRAM, or on a
        // save-state page.
        assert_eq!(check(0x75FF_E000, 0x1000, USER_CODE), executable(0x75FF_E000, 0x1000));
        for base in [0x7430_0000, 0x7440_0000, 0x7500_F000] {
            assert_eq!(check(base, 0x1000, SUPERVISOR_CODE), executable(base, 0x1000));
        }
        // No page in MMRAM is writable and executable, not even in a module.
        for attributes in [MemoryAttributes::empty(), MemoryAttributes::Supervisor] {
            assert_eq!(
                check(0x7420_0000, 0x1000, attributes),
                Err(HobValidationError::PageWritableAndExecutable { addr: 0x7420_0000, found: attributes.bits() }
                    .into())
            );
        }
    }

    #[test]
    fn test_page_table_validation_rejects_mappings_that_are_not_address_ranges() {
        let list = hob_list();
        let hob = Hob::Handoff(list.handoff());
        let invalid = |base, length| -> MmSupervisorResult<()> {
            Err(HobValidationError::InvalidAddressRange { kind: "page table mapping", base, length }.into())
        };
        let last_page = u64::MAX - 0xFFF;

        // An empty leaf, and a leaf that runs past the end of the address space.
        assert_eq!(verify(&hob, [leaf(0xFED0_0000, 0, USER_NX)]), invalid(0xFED0_0000, 0));
        assert_eq!(verify(&hob, [leaf(last_page, 0x1000, USER_NX)]), invalid(last_page, 0x1000));
        // A blanket that the next leaf would carry past the end of the address space.
        assert_eq!(
            verify(&hob, [leaf(last_page - 0x1000, 0x1000, USER_NX), leaf(last_page, 0x1000, USER_NX)]),
            invalid(last_page, 0x1000)
        );
    }

    #[test]
    fn test_page_table_validation_ignores_guid_hobs_that_name_no_region() {
        // Each payload would name the page at `base` if it were read, but each is either too short
        // for the structure its GUID names or carries a GUID MM Init does not map from.
        let base = 0x1000_0000u64;
        let unknown = BinaryGuid::from_string("2a3b4c5d-6e7f-4081-92a3-b4c5d6e7f809");
        let truncated_unblock: [u64; 6] = [0, base, 0, 1, 0, 1];
        let truncated_common_region: [u64; 3] = [0, base, 1];
        let truncated_comm_buffer: [u64; 2] = [base, 1];
        let comm_buffer_layout: [u64; 3] = [base, 1, base];

        for (name, payload) in [
            (MM_SUPV_UNBLOCK_REGION_HOB_GUID, truncated_unblock.as_bytes()),
            (MM_COMMON_REGION_HOB_GUID, truncated_common_region.as_bytes()),
            (MM_COMM_BUFFER_HOB_GUID, truncated_comm_buffer.as_bytes()),
            (unknown, comm_buffer_layout.as_bytes()),
        ] {
            let mut list = RawHobList::new();
            list.push_guid_hob(name, payload);
            let list = list.finish();
            let hob = Hob::Handoff(list.handoff());

            assert_eq!(verify(&hob, [leaf(base, 0x1000, SUPERVISOR_NX)]), mapped(base, 0x1000));
        }
    }

    #[test]
    fn test_page_table_validation_requires_the_root_inside_mmram() {
        // Host tests read CR3 as 0, which is outside MMRAM.
        let list = hob_list();
        assert_eq!(
            validate_page_table_post_activation(list.handoff(), &pass_down(), SMBASES.len(), mmram()),
            Err(MmSupervisorError::from(HobValidationError::PageTableOutsideMmram { root: 0 }))
        );
    }
}
