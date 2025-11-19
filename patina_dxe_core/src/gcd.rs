//! DXE Core Global Coherency Domain (GCD)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
mod io_block;
mod memory_block;
mod spin_locked_gcd;

use alloc::string::String;
use goblin::pe::section_table;

use core::{cell::Cell, ffi::c_void, ops::Range};
use patina::{
    base::{align_down, align_up},
    error::EfiError,
    pi::{
        dxe_services::{GcdIoType, GcdMemoryType, MemorySpaceDescriptor},
        hob::{self, Hob, HobList, PhaseHandoffInformationTable},
    },
};
use patina_internal_cpu::paging::create_cpu_paging;
use r_efi::efi;

#[cfg(feature = "compatibility_mode_allowed")]
use patina::base::{UEFI_PAGE_SIZE, align_range};

use crate::{GCD, gcd::spin_locked_gcd::PagingAllocator, pecoff};

pub use spin_locked_gcd::{AllocateType, MapChangeType, SpinLockedGcd};

/// The MemoryProtectionPolicy struct is the source of truth for Patina's memory protection rules.
/// All memory protection decisions in Patina are driven by functions in this struct to have one
/// easily auditable location.
///
/// All rules in this struct are associated functions (don't require an instantiation of the struct)
/// except for the apply_default_allocated_memory_protection_policy function, because this relies on
/// internal state. The GCD contains a MemoryProtectionPolicy instance to manage this state.
pub(crate) struct MemoryProtectionPolicy {
    /// The default attributes for memory allocations. This will be efi::MEMORY_XP unless
    /// we have entered compatibility mode, in which case it is 0, e.g. no protection
    memory_allocation_default_attributes: Cell<u64>,
}

impl MemoryProtectionPolicy {
    /// Create a new MemoryProtectionPolicy instance with default settings.
    pub(crate) const fn new() -> Self {
        Self { memory_allocation_default_attributes: Cell::new(efi::MEMORY_XP) }
    }

    /// Rule: All memory allocations will be marked as the set cache type with NX applied. If compatibility mode
    /// has been activated, no protections will be applied.
    ///
    /// Arguments
    /// * `attributes` - The cache attributes to apply to the allocated memory
    ///
    /// Use Case: This is called whenever memory is allocated via the GCD to ensure
    /// allocated memory is NX by default.
    pub(crate) const fn apply_allocated_memory_protection_policy(&self, attributes: u64) -> u64 {
        (attributes & efi::CACHE_ATTRIBUTE_MASK) | self.memory_allocation_default_attributes.get()
    }

    /// Rule: All resource descriptor HOBs are initially mapped as the supplied cache attribute
    /// (for Resource Descriptor v2 HOBs) with NX applied. All system memory is marked as RP because
    /// we default to it being unmapped until allocated. Reserved and MMIO memory we must map by default
    /// to preserve compatibility with drivers that expect to be able to access those regions automatically.
    ///
    /// Arguments
    /// * `attributes` - The memory attributes from the resource descriptor HOB (if applicable)
    /// * `memory_type` - The GCD memory type being mapped
    ///
    /// Use Case: This is called when we are processing the initial resource descriptor HOBs into the GCD.
    pub(crate) fn apply_resc_desc_hobs_protection_policy(attributes: u64, memory_type: GcdMemoryType) -> u64 {
        let mut new_attributes = (attributes & efi::CACHE_ATTRIBUTE_MASK) | efi::MEMORY_XP;

        if memory_type == GcdMemoryType::SystemMemory {
            new_attributes |= efi::MEMORY_RP;
        }

        new_attributes
    }

    /// Rule: If we have Uncached memory, we must also apply NX to it.
    /// In DXE, we should never be executing from UC memory. On AArch64, this is defined as
    /// a programming error to have executable device memory (which UC maps to).
    ///
    /// Arguments
    /// * `attributes` - The memory attributes to evaluate whether NX needs to be applied
    ///
    /// Use Case: This is called whenever memory attributes are being set in the GCD to ensure
    /// that Uncached memory is not executable.
    pub(crate) const fn apply_nx_to_uc_policy(attributes: u64) -> u64 {
        let mut new_attributes = attributes;
        if new_attributes & efi::MEMORY_UC == efi::MEMORY_UC {
            new_attributes |= efi::MEMORY_XP;
        }

        new_attributes
    }

    /// Rule: The Memory Attributes Table, per UEFI spec, may only have RO, XP, and Runtime set. Only
    /// RuntimeServicesCode and RuntimeServicesData are reported in the MAT. RuntimeServicesCode memory consists
    /// of code sections, data sections, and potentially extra unused memory for padding.
    ///   - If a Runtime Services Code region has no attributes set, mark it as RO, XP, and Runtime. This will
    ///     prevent unused memory from being executed or written to.
    ///   - If a Runtime Services Data region has no attributes set, mark it as XP and Runtime to ensure data can be
    ///     used but not executed.
    ///   - Otherwise, filter the attributes to only the allowed attributes
    ///
    /// Arguments
    /// * `attributes` - The memory attributes for this region
    /// * `memory_type` - The GCD memory type for this region
    ///
    /// Use Case: This is called when building the Memory Attributes Table prior to installing it.
    pub(crate) const fn apply_memory_attributes_table_policy(attributes: u64, memory_type: efi::MemoryType) -> u64 {
        const ALLOWED_MAT_ATTRIBUTES: u64 = efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_RUNTIME;

        match attributes & (efi::MEMORY_RO | efi::MEMORY_XP) {
            // if we don't have any attributes set here, we should mark code as RO and XP. These are
            // likely extra sections in the memory bins and so should not be used
            // Data we will mark as XP only, as likely the caching attributes were changed, which
            // dropped the XP attribute, so we need to set it here.
            0 if memory_type == efi::RUNTIME_SERVICES_CODE => ALLOWED_MAT_ATTRIBUTES,
            0 if memory_type == efi::RUNTIME_SERVICES_DATA => efi::MEMORY_RUNTIME | efi::MEMORY_XP,
            _ => attributes & ALLOWED_MAT_ATTRIBUTES,
        }
    }

    /// Rule: All loaded image stacks must have a guard page that is unmapped. We accomplish that by setting
    /// the RP attribute on the guard page. We also set XP to align with other unmapped pages.
    ///
    /// Arguments
    /// * `attributes` - The memory attributes for the stack guard page
    ///
    /// Use Case: This is called when loading an image to ensure the stack guard page is protected.
    pub(crate) const fn apply_image_stack_guard_policy(attributes: u64) -> u64 {
        attributes | efi::MEMORY_RP | efi::MEMORY_XP
    }

    /// Rule: All loaded image sections must have memory protections applied based on the section type. The cache
    /// attributes from the memory space descriptor are preserved.
    ///   - Code sections are marked as Read Only and Executable
    ///   - Data sections are marked as Read/Write and Non-Executable
    ///   - Sections w/o the write characteristic are marked as Read Only
    ///
    /// Arguments
    /// * `section_base_addr` - The base address of the section being loaded
    /// * `section_characteristics` - The PE/COFF section characteristics
    ///
    /// Returns a tuple of (attributes, capabilities) to be applied to the section
    ///
    /// Use Case: This is called when loading an image to ensure each section has the proper memory protections.
    pub(crate) const fn apply_image_protection_policy(
        section_characteristics: u32,
        descriptor: &MemorySpaceDescriptor,
    ) -> (u64, u64) {
        let mut attributes = efi::MEMORY_XP;
        if section_characteristics & pecoff::IMAGE_SCN_CNT_CODE == pecoff::IMAGE_SCN_CNT_CODE {
            attributes = efi::MEMORY_RO;
        }

        if section_characteristics & section_table::IMAGE_SCN_MEM_WRITE == 0
            && ((section_characteristics & section_table::IMAGE_SCN_MEM_READ) == section_table::IMAGE_SCN_MEM_READ)
        {
            attributes |= efi::MEMORY_RO;
        }

        attributes |= descriptor.attributes & efi::CACHE_ATTRIBUTE_MASK;

        let capabilities = attributes | descriptor.capabilities;

        (attributes, capabilities)
    }

    /// Rule: The EFI_MEMORY_MAP descriptor.attributes field is actually a capability field that must not have
    /// access attributes in it; some OSes treat these as actually set attributes, not capabilities. The runtime
    /// attribute is taken from the attributes, not the capabilities. Persistent memory must have EFI_MEMORY_NV set.
    ///
    /// Arguments
    /// * `attributes` - The memory attributes from the EFI_MEMORY_MAP descriptor
    /// * `capabilities` - The memory capabilities from the EFI_MEMORY_MAP descriptor
    /// * `gcd_memory_type` - The GCD memory type for this region
    ///
    /// Use Case: This is called when building the EFI_MEMORY_MAP to ensure the attributes are correctly set.
    pub(crate) fn apply_efi_memory_map_policy(
        attributes: u64,
        capabilities: u64,
        gcd_memory_type: GcdMemoryType,
    ) -> u64 {
        let mut final_attributes =
            capabilities & !(efi::MEMORY_ACCESS_MASK | efi::MEMORY_RUNTIME) | (attributes & efi::MEMORY_RUNTIME);

        if gcd_memory_type == GcdMemoryType::Persistent {
            final_attributes |= efi::MEMORY_NV;
        }

        final_attributes
    }

    /// Rule: All new memory should support all access capabilities and runtime. These are generally applicable, not
    /// specific to any memory. All new memory is marked as EFI_MEMORY_RP to start with and will not be mapped until
    /// SetMemorySpaceAttributes() is called to set the attributes. EFI_MEMORY_XP is also set to allow merging with
    /// other free memory blocks.
    ///
    /// Arguments
    /// - * `capabilities` - The existing capabilities for the memory region
    ///
    /// Returns the updated capabilities and the attributes to set
    ///
    /// Use Case: This is called whenever new memory is added to the GCD
    pub(crate) const fn apply_add_memory_policy(capabilities: u64) -> (u64, u64) {
        (capabilities | efi::MEMORY_ACCESS_MASK | efi::MEMORY_RUNTIME, efi::MEMORY_RP | efi::MEMORY_XP)
    }

    /// Rule: All free memory should be marked as EFI_MEMORY_RP, EFI_MEMORY_XP, and the preserved cache attributes.
    /// EFI_MEMORY_RP will cause the memory to be unmapped in the page table, but we still set EFI_MEMORY_XP to align
    /// with the originally added memory so that free memory can be coalesced into fewer blocks.
    ///
    /// Arguments
    /// - * `attributes` - The existing attributes for the memory region
    ///
    /// Use Case: This is called whenever memory is freed in the GCD
    pub(crate) const fn apply_free_memory_policy(attributes: u64) -> u64 {
        (attributes & efi::CACHE_ATTRIBUTE_MASK) | efi::MEMORY_RP | efi::MEMORY_XP
    }

    /// Rule: Page 0 should be unmapped to catch null pointer dereferences. Cache attributes should be preserved.
    ///
    /// Arguments
    /// - * `attributes` - The existing attributes for page 0
    ///
    /// Use Case: This is called when initializing paging to ensure page 0 is unmapped.
    pub(crate) const fn apply_null_page_policy(attributes: u64) -> u64 {
        (attributes & efi::CACHE_ATTRIBUTE_MASK) | efi::MEMORY_RP | efi::MEMORY_XP
    }

    /// Rule: If the compatibility_mode_allowed feature flag is not set, we will fail to load
    /// the image that would crash the system with memory protections enabled
    ///
    /// Arguments
    /// * `image_base_page` - The base page of the image being loaded
    /// * `image_num_pages` - The number of pages in the image being loaded
    /// * `filename` - The name of the image being loaded
    ///
    /// Use Case: This is called when the platform has not allowed compatibility mode and we are attempting to load
    /// an EFI_APPLICATION that is not NX compatible.
    #[cfg(not(feature = "compatibility_mode_allowed"))]
    pub(crate) fn activate_compatibility_mode(
        _gcd: &SpinLockedGcd,
        _image_base_page: usize,
        _image_num_pages: usize,
        filename: String,
    ) -> Result<(), EfiError> {
        log::error!(
            "Attempting to load {} that is not NX compatible. Compatibility mode is not allowed in this build, not loading image.",
            filename
        );
        Err(EfiError::LoadError)
    }

    /// Rule: If the platform allows compatibility mode, activate it when an EFI_APPLICATION without the NX_COMPAT flag
    /// is loaded.
    /// This will:
    /// - Activate compatibility mode for the GCD lower layers
    /// - Set the memory space attributes for all memory ranges in the loader code and data allocators to be RWX
    /// - Uninstall the memory attributes protocol
    ///
    /// Arguments
    /// * `image_base_page` - The base page of the image being loaded
    /// * `image_num_pages` - The number of pages in the image being loaded
    /// * `filename` - The name of the image being loaded
    ///
    /// Use Case: This is called when the platform has allowed compatibility mode and we are attempting to load
    /// an EFI_APPLICATION that is not NX compatible.
    #[cfg(feature = "compatibility_mode_allowed")]
    pub(crate) fn activate_compatibility_mode(
        gcd: &SpinLockedGcd,
        image_base_page: usize,
        image_num_pages: usize,
        filename: String,
    ) -> Result<(), EfiError> {
        // remove default NX protection
        gcd.memory_protection_policy.memory_allocation_default_attributes.set(0);

        const LEGACY_BIOS_WB_ADDRESS: usize = 0xA0000;

        log::warn!("Attempting to load an application image that is not NX compatible. Activating compatibility mode.");

        // always map page 0 if it exists in this system, as grub will attempt to read it for legacy boot structures
        // map it WB by default, because 0 is being used as the null page, it may not have gotten cache attributes
        // populated
        match gcd.get_memory_descriptor_for_address(0) {
            Ok(descriptor) if descriptor.memory_type == GcdMemoryType::SystemMemory => {
                // set_memory_space_attributes will set both the GCD and paging attributes
                if let Err(e) = gcd.set_memory_space_attributes(
                    0,
                    UEFI_PAGE_SIZE,
                    descriptor.attributes & efi::CACHE_ATTRIBUTE_MASK,
                ) {
                    log::error!("Failed to map page 0 for compat mode. Status: {e:#x?}");
                    debug_assert!(false);
                }
            }
            _ => {}
        }

        // map legacy region if system mem
        let mut address = UEFI_PAGE_SIZE; // start at 0x1000, as we already mapped page 0
        while address < LEGACY_BIOS_WB_ADDRESS {
            let mut size = UEFI_PAGE_SIZE;
            if let Ok(descriptor) = gcd.get_memory_descriptor_for_address(address as efi::PhysicalAddress) {
                // if the legacy region is not system memory, we should not map it
                if descriptor.memory_type == GcdMemoryType::SystemMemory {
                    size = match address + descriptor.length as usize {
                        end_addr if end_addr > LEGACY_BIOS_WB_ADDRESS => LEGACY_BIOS_WB_ADDRESS - address,
                        _ => descriptor.length as usize,
                    };

                    // set_memory_space_attributes will set both the GCD and paging attributes
                    match gcd.set_memory_space_attributes(
                        address,
                        size,
                        descriptor.attributes & efi::CACHE_ATTRIBUTE_MASK,
                    ) {
                        Ok(_) => {}
                        Err(e) => {
                            log::error!(
                                "Failed to map legacy bios region at {:#x?} of length {:#x?} with attributes {:#x?}. Status: {:#x?}",
                                address,
                                size,
                                descriptor.attributes & efi::CACHE_ATTRIBUTE_MASK,
                                e
                            );
                            debug_assert!(false);
                        }
                    }
                }
            }
            address += size;
        }

        // if the allocator doesn't have any memory, then when it is used next it will allocate from the GCD
        // and the GCD will be in compatibility mode, so we don't care here
        let mut loader_mem_ranges = crate::allocator::get_memory_ranges_for_memory_type(efi::LOADER_CODE);
        loader_mem_ranges.extend(crate::allocator::get_memory_ranges_for_memory_type(efi::LOADER_DATA));
        for range in loader_mem_ranges.iter() {
            let mut addr = range.start;
            while addr < range.end {
                let mut len = UEFI_PAGE_SIZE as u64;
                match gcd.get_memory_descriptor_for_address(addr) {
                    Ok(descriptor) => {
                        let attributes = descriptor.attributes & !efi::MEMORY_XP;
                        len = match descriptor.base_address + descriptor.length {
                            end if end > range.end => range.end - addr,
                            _ => descriptor.length,
                        };

                        // We need to ensure we are operating on page aligned addresses and lengths, as the image(s) that
                        // were allocated here may not be page aligned. We don't share pools across types, so this is safe.
                        (addr, len) = match align_range(addr, len, UEFI_PAGE_SIZE as u64) {
                            Ok((aligned_addr, aligned_len)) => (aligned_addr, aligned_len),
                            Err(_) => {
                                log::error!(
                                    "Failed to align address {addr:#x?} + {len:#x?} to page size, compatibility mode may fail",
                                );
                                debug_assert!(false);

                                // If we can't align the address, we can't set the attributes, so try the next range
                                addr += len;
                                continue;
                            }
                        };

                        if gcd.set_memory_space_attributes(addr as usize, len as usize, attributes).is_err() {
                            log::error!(
                                "Failed to set memory space attributes for range {addr:#x?} - {len:#x?}, compatibility mode may fail",
                            );
                            debug_assert!(false);
                        }
                    }
                    _ => {
                        log::error!(
                            "Failed to get memory space descriptor for range {:#x?} - {:#x?}, compatibility mode may fail",
                            range.start,
                            range.end,
                        );
                        debug_assert!(false);
                    }
                }
                addr += len;
            }
        }
        crate::memory_attributes_protocol::uninstall_memory_attributes_protocol();

        // for this image map all mem RWX preserving cache attributes if we find them
        let stripped_attrs = gcd
            .get_memory_descriptor_for_address(image_base_page as u64)
            .map(|desc| desc.attributes & efi::CACHE_ATTRIBUTE_MASK)
            .unwrap_or(patina::base::DEFAULT_CACHE_ATTR);
        if gcd
            .set_memory_space_attributes(image_base_page, patina::uefi_pages_to_size!(image_num_pages), stripped_attrs)
            .is_err()
        {
            // if we failed to map this image RWX, we should still attempt to execute it, it may succeed
            log::error!("Failed to set GCD attributes for image {}", filename);
            debug_assert!(false);
        }
        Ok(())
    }
}

pub fn init_gcd(physical_hob_list: *const c_void) {
    let mut free_memory_start: u64 = 0;
    let mut free_memory_size: u64 = 0;
    let mut memory_start: u64 = 0;
    let mut memory_end: u64 = 0;

    let hob_list = Hob::Handoff(unsafe {
        (physical_hob_list as *const PhaseHandoffInformationTable)
            .as_ref::<'static>()
            .expect("Physical hob list pointer is null, but it must exist and be valid.")
    });
    for hob in &hob_list {
        match hob {
            Hob::Handoff(handoff) => {
                free_memory_start = align_up(handoff.free_memory_bottom, 0x1000).expect("Unaligned free memory bottom");
                free_memory_size =
                    align_down(handoff.free_memory_top, 0x1000).expect("Unaligned free memory top") - free_memory_start;
                memory_start = handoff.memory_bottom;
                memory_end = handoff.memory_top;
            }
            Hob::Cpu(cpu) => {
                GCD.init(cpu.size_of_memory_space as u32, cpu.size_of_io_space as u32);
            }
            _ => (),
        }
    }

    log::info!("memory_start: {memory_start:#x?}");
    log::info!("memory_size: {:#x?}", memory_end - memory_start);
    log::info!("free_memory_start: {free_memory_start:#x?}");
    log::info!("free_memory_size: {free_memory_size:#x?}");
    log::info!("physical_hob_list: {:#x?}", physical_hob_list as u64);

    // make sure the PHIT is present and it was reasonable.
    assert!(free_memory_size > 0, "Not enough free memory for DXE core to start");
    assert!(memory_start < memory_end, "Not enough memory available for DXE core to start.");

    // initialize the GCD with an initial memory space. Note: this will fail if GCD.init() above didn't happen.
    // SAFETY: We are directly using the free memory space from the PHIT HOB, which must be valid and reserved for use
    // per spec.
    unsafe {
        GCD.init_memory_blocks(
            GcdMemoryType::SystemMemory,
            free_memory_start as usize,
            free_memory_size as usize,
            efi::MEMORY_ACCESS_MASK | efi::CACHE_ATTRIBUTE_MASK,
        )
        .expect("Failed to add initial region to GCD.");
    }
}

#[coverage(off)]
/// Initialize the patina-paging crate
///
/// # Arguments
/// * `hob_list` - The HOB list as passed to DXE Core
///
/// This function installs the new Patina controlled page tables based
/// on the HOB list provided. Note that coverage is disabled for the
/// wrapper function because this simply wraps the actual implementation
/// in the SpinLockedGcd struct, which is covered by unit tests.
pub fn init_paging(hob_list: &HobList) {
    let page_allocator = PagingAllocator::new(&GCD);
    let page_table = create_cpu_paging(page_allocator).expect("Failed to create CPU page table");
    GCD.init_paging_with(hob_list, page_table);
}

pub fn add_hob_resource_descriptors_to_gcd(hob_list: &HobList) {
    #[cfg(feature = "v1_resource_descriptor_support")]
    {
        log::debug!("v1_resource_descriptor_support feature is active (V1 ResourceDescriptor HOBs only)");
    }

    #[cfg(not(feature = "v1_resource_descriptor_support"))]
    {
        log::debug!("v1_resource_descriptor_support feature is NOT active (V2 ResourceDescriptor HOBs only)");
    }

    let phit = hob_list
        .iter()
        .find_map(|x| match x {
            patina::pi::hob::Hob::Handoff(handoff) => Some(*handoff),
            _ => None,
        })
        .expect("Failed to find PHIT Hob");

    let free_memory_start = align_up(phit.free_memory_bottom, 0x1000).expect("Unaligned free memory bottom");
    let free_memory_size =
        align_down(phit.free_memory_top, 0x1000).expect("Unaligned free memory top") - free_memory_start;

    //Iterate over the hob list and map resource descriptor HOBs into the GCD.
    for hob in hob_list.iter() {
        let mut gcd_mem_type: GcdMemoryType = GcdMemoryType::NonExistent;
        let mut resource_attributes: u32 = 0;

        // Only process Resource Descriptor HOBs according to the selected version
        let (res_desc, cache_attributes) = match parse_resource_descriptor_hob(hob) {
            Some((desc, Some(attrs))) => (desc, attrs),
            Some((desc, None)) => (desc, 0u64),
            None => continue, // Not a resource descriptor HOB or unsupported version for this build
        };

        let mem_range = res_desc.physical_start
            ..res_desc.physical_start.checked_add(res_desc.resource_length).expect("Invalid resource descriptor hob");

        match res_desc.resource_type {
            hob::EFI_RESOURCE_SYSTEM_MEMORY => {
                resource_attributes = res_desc.resource_attribute;

                if resource_attributes & hob::MEMORY_ATTRIBUTE_MASK == hob::TESTED_MEMORY_ATTRIBUTES {
                    if resource_attributes & hob::EFI_RESOURCE_ATTRIBUTE_MORE_RELIABLE
                        == hob::EFI_RESOURCE_ATTRIBUTE_MORE_RELIABLE
                    {
                        gcd_mem_type = GcdMemoryType::MoreReliable;
                    } else {
                        gcd_mem_type = GcdMemoryType::SystemMemory;
                    }
                }

                if (resource_attributes & hob::MEMORY_ATTRIBUTE_MASK == (hob::INITIALIZED_MEMORY_ATTRIBUTES))
                    || (resource_attributes & hob::MEMORY_ATTRIBUTE_MASK == (hob::PRESENT_MEMORY_ATTRIBUTES))
                {
                    gcd_mem_type = GcdMemoryType::Reserved;
                }

                if resource_attributes & hob::EFI_RESOURCE_ATTRIBUTE_PERSISTENT
                    == hob::EFI_RESOURCE_ATTRIBUTE_PERSISTENT
                {
                    gcd_mem_type = GcdMemoryType::Persistent;
                }
            }
            hob::EFI_RESOURCE_MEMORY_MAPPED_IO | hob::EFI_RESOURCE_FIRMWARE_DEVICE => {
                resource_attributes = res_desc.resource_attribute;
                gcd_mem_type = GcdMemoryType::MemoryMappedIo;
            }
            hob::EFI_RESOURCE_MEMORY_MAPPED_IO_PORT | hob::EFI_RESOURCE_MEMORY_RESERVED => {
                resource_attributes = res_desc.resource_attribute;
                gcd_mem_type = GcdMemoryType::Reserved;
            }
            hob::EFI_RESOURCE_IO => {
                log::info!(
                    "Mapping io range {:#x?} as {:?}",
                    res_desc.physical_start..res_desc.resource_length,
                    GcdIoType::Io
                );
                GCD.add_io_space(GcdIoType::Io, res_desc.physical_start as usize, res_desc.resource_length as usize)
                    .expect("Failed to add IO space to GCD");
            }
            hob::EFI_RESOURCE_IO_RESERVED => {
                log::info!(
                    "Mapping io range {:#x?} as {:?}",
                    res_desc.physical_start..res_desc.resource_length,
                    GcdIoType::Reserved
                );
                GCD.add_io_space(
                    GcdIoType::Reserved,
                    res_desc.physical_start as usize,
                    res_desc.resource_length as usize,
                )
                .expect("Failed to add IO space to GCD");
            }
            _ => {
                debug_assert!(false, "Unknown resource type in HOB");
            }
        };

        if gcd_mem_type != GcdMemoryType::NonExistent {
            debug_assert!(res_desc.attributes_valid());
        }

        if gcd_mem_type != GcdMemoryType::NonExistent {
            let memory_attributes =
                MemoryProtectionPolicy::apply_resc_desc_hobs_protection_policy(cache_attributes, gcd_mem_type);

            for split_range in
                remove_range_overlap(&mem_range, &(free_memory_start..(free_memory_start + free_memory_size)))
                    .into_iter()
                    .take_while(|r| r.is_some())
                    .flatten()
            {
                unsafe {
                    GCD.add_memory_space(
                        gcd_mem_type,
                        split_range.start as usize,
                        split_range.end.saturating_sub(split_range.start) as usize,
                        spin_locked_gcd::get_capabilities(gcd_mem_type, resource_attributes as u64),
                    )
                    .expect("Failed to add memory space to GCD");
                }

                log::info!(
                    "Mapping memory range {split_range:#x?} as {gcd_mem_type:?} with attributes {memory_attributes:#x?}",
                );

                match GCD.set_memory_space_attributes(
                    split_range.start as usize,
                    split_range.end.saturating_sub(split_range.start) as usize,
                    memory_attributes,
                ) {
                    // NotReady is expected result here since page table is not yet initialized. In this case GCD
                    // will be updated with the appropriate attributes which will then be sync'd to page table
                    // once it is initialized.
                    Err(EfiError::NotReady) => (),
                    _ => {
                        // In debug builds, assert to catch GCD attribute setting failures during development.
                        // In production, allow the system to continue with a potentially torn state,
                        // matching EDK2 behavior where non-critical GCD operations can fail gracefully.
                        debug_assert!(
                            false,
                            "GCD failed to set memory attributes {:#X} for base: {:#X}, length: {:#X}",
                            memory_attributes,
                            split_range.start,
                            split_range.end.saturating_sub(split_range.start),
                        );
                    }
                }
            }
        }
    }
}

fn remove_range_overlap<T: PartialOrd + Copy>(a: &Range<T>, b: &Range<T>) -> [Option<Range<T>>; 2] {
    if a.start < b.end && a.end > b.start {
        // Check if `a` has a portion before the overlap
        let first_range = if a.start < b.start { Some(a.start..b.start) } else { None };

        // Check if `a` has a portion after the overlap
        let second_range = if a.end > b.end { Some(b.end..a.end) } else { None };

        [first_range, second_range]
    } else {
        // No overlap
        [Some(a.start..a.end), None]
    }
}

/// Parse Resource Descriptor HOB v2
///
/// This function takes in a HOB and returns:
/// - Some((Resource Descriptor, Some(cache_attributes))) if cache attributes are present
/// - Some((Resource Descriptor, None)) if no cache attributes are present
/// - None if not a v2 resource descriptor HOB
#[cfg(not(feature = "v1_resource_descriptor_support"))]
fn parse_resource_descriptor_hob(hob: &Hob) -> Option<(hob::ResourceDescriptor, Option<u64>)> {
    match hob {
        Hob::ResourceDescriptorV2(v2_res_desc) => {
            let attrs = if v2_res_desc.attributes != 0 { Some(v2_res_desc.attributes) } else { None };
            Some((v2_res_desc.v1, attrs))
        }
        _ => None, // Not a resource descriptor HOB or a v1 HOB
    }
}

/// Parse Resource Descriptor HOB v1
///
/// This function takes in a HOB and returns:
/// - Some((Resource Descriptor, None))
/// - None if not a v1 resource descriptor HOB
#[cfg(feature = "v1_resource_descriptor_support")]
fn parse_resource_descriptor_hob(hob: &Hob) -> Option<(hob::ResourceDescriptor, Option<u64>)> {
    match hob {
        Hob::ResourceDescriptor(v1_res_desc) => {
            // Legacy platforms: Process v1 HOBs normally
            Some((**v1_res_desc, None)) // v1 HOBs have no cache attributes
        }
        _ => None, // Not a resource descriptor HOB or a v2 HOB
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::ffi::c_void;

    use patina::pi::{
        dxe_services::{GcdIoType, GcdMemoryType, IoSpaceDescriptor, MemorySpaceDescriptor},
        hob::{HobList, PhaseHandoffInformationTable},
    };

    use crate::{
        GCD,
        gcd::init_gcd,
        test_support::{self, build_test_hob_list},
    };

    use super::*;

    const MEM_SIZE: u64 = 0x200000;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            // SAFETY: Test code - resetting the global GCD state for test isolation.
            // The test lock is used to prevent concurrent access.
            unsafe {
                GCD.reset();
            }
            f();
        })
        .unwrap();
    }

    fn init_gcd_should_init_gcd(physical_hob_list: *const c_void, mem_base: u64) {
        // SAFETY: Test code - the physical_hob_list pointer is pointing to a HOBs list
        // constructed in test_full_gcd_init() and directly passed to this function.
        let handoff = unsafe {
            (physical_hob_list as *const PhaseHandoffInformationTable)
                .as_ref::<'static>()
                .expect("Physical hob list pointer is null, but it must exist and be valid.")
        };

        let free_memory_start = handoff.free_memory_bottom;
        let free_memory_size = handoff.free_memory_top - handoff.free_memory_bottom;

        init_gcd(physical_hob_list);
        assert!(free_memory_start >= mem_base && free_memory_start < mem_base + MEM_SIZE);
        assert!(free_memory_size <= 0x100000);
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::with_capacity(GCD.memory_descriptor_count() + 10);
        GCD.get_memory_descriptors(&mut descriptors).expect("get_memory_descriptors failed.");
        assert!(
            descriptors
                .iter()
                .any(|x| x.base_address == free_memory_start && x.memory_type == GcdMemoryType::SystemMemory)
        )
    }

    fn add_resource_descriptors_should_add_resource_descriptors(hob_list: &HobList, mem_base: u64) {
        add_hob_resource_descriptors_to_gcd(hob_list);
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::with_capacity(GCD.memory_descriptor_count() + 10);
        GCD.get_memory_descriptors(&mut descriptors).expect("get_memory_descriptors failed.");
        descriptors
            .iter()
            .find(|x| x.base_address == mem_base + 0xE0000 && x.memory_type == GcdMemoryType::SystemMemory)
            .unwrap();
        descriptors
            .iter()
            .find(|x| x.base_address == mem_base + 0xF0000 && x.memory_type == GcdMemoryType::Reserved)
            .unwrap();
        //Note: resource descriptors 3 & are merged into a single contiguous region in GCD, so no separate entry exists.
        //So verify the length of the entry encompasses both.
        let mmio_3_4 = descriptors
            .iter()
            .find(|x| x.base_address == 0x10000000 && x.memory_type == GcdMemoryType::MemoryMappedIo)
            .unwrap();
        assert_eq!(mmio_3_4.length, 0x2000000);
        descriptors.iter().find(|x| x.base_address == 0x12000000 && x.memory_type == GcdMemoryType::Reserved).unwrap();

        let mut descriptors: Vec<IoSpaceDescriptor> = Vec::with_capacity(GCD.io_descriptor_count() + 10);
        GCD.get_io_descriptors(&mut descriptors).expect("get_io_descriptors failed.");
        descriptors.iter().find(|x| x.base_address == 0x0000 && x.io_type == GcdIoType::Reserved).unwrap();
        descriptors.iter().find(|x| x.base_address == 0x1000 && x.io_type == GcdIoType::Io).unwrap();
    }

    #[test]
    fn test_full_gcd_init() {
        with_locked_state(|| {
            let physical_hob_list = build_test_hob_list(MEM_SIZE);
            init_gcd_should_init_gcd(physical_hob_list, physical_hob_list as u64);

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);

            add_resource_descriptors_should_add_resource_descriptors(&hob_list, physical_hob_list as u64);
        });
    }

    #[test]
    fn test_remove_range_overlap() {
        // Test case 1: No overlap
        let a = 10..20;
        let b = 30..40;
        let result = remove_range_overlap(&a, &b);
        assert_eq!(result, [Some(10..20), None]);

        // Test case 2: Partial overlap at the front of 'b'
        let a = 5..20;
        let b = 10..30;
        let result = remove_range_overlap(&a, &b);
        assert_eq!(result, [Some(5..10), None]);

        // Test case 3: Partial overlap at the end of 'b'
        let a = 20..40;
        let b = 10..30;
        let result = remove_range_overlap(&a, &b);
        assert_eq!(result, [None, Some(30..40)]);

        // Test case 4: 'a' is completely inside 'b' (middle overlap)
        let a = 20..30;
        let b = 10..40;
        let result = remove_range_overlap(&a, &b);
        assert_eq!(result, [None, None]);
    }

    #[test]
    fn test_memory_protection_policy_apply_allocated_memory_protection_policy() {
        let policy = MemoryProtectionPolicy::new();
        let attributes = efi::MEMORY_WB;
        let result = policy.apply_allocated_memory_protection_policy(attributes);
        // Should preserve cache attributes and add default XP
        assert_eq!(result, efi::MEMORY_WB | efi::MEMORY_XP);
    }

    #[test]
    fn test_memory_protection_policy_apply_resc_desc_hobs_protection_policy() {
        // system memory, adds XP and RP
        let attributes = efi::MEMORY_WB;
        let memory_type = GcdMemoryType::SystemMemory;
        let result = MemoryProtectionPolicy::apply_resc_desc_hobs_protection_policy(attributes, memory_type);
        assert_eq!(result, efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RP);

        // not system memory, only adds XP
        let attributes = efi::MEMORY_UC;
        let memory_type = GcdMemoryType::MemoryMappedIo;
        let result = MemoryProtectionPolicy::apply_resc_desc_hobs_protection_policy(attributes, memory_type);
        assert_eq!(result, efi::MEMORY_UC | efi::MEMORY_XP);
    }

    #[test]
    fn test_memory_protection_policy_apply_nx_to_uc_policy() {
        // UC set, should add XP
        let attributes = efi::MEMORY_UC;
        let result = MemoryProtectionPolicy::apply_nx_to_uc_policy(attributes);
        assert_eq!(result, efi::MEMORY_UC | efi::MEMORY_XP);

        // UC not set, should not change
        let attributes = efi::MEMORY_WB;
        let result = MemoryProtectionPolicy::apply_nx_to_uc_policy(attributes);
        assert_eq!(result, efi::MEMORY_WB);
    }

    #[test]
    fn test_memory_protection_policy_apply_memory_attributes_table_policy() {
        // Test case 1: RUNTIME_SERVICES_CODE with no MAT attributes
        let attributes = efi::MEMORY_WB;
        let memory_type = efi::RUNTIME_SERVICES_CODE;
        let result = MemoryProtectionPolicy::apply_memory_attributes_table_policy(attributes, memory_type);
        assert_eq!(result, efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_RUNTIME);

        // Test case 2: RUNTIME_SERVICES_DATA with no MAT attributes
        let attributes = efi::MEMORY_WB;
        let memory_type = efi::RUNTIME_SERVICES_DATA;
        let result = MemoryProtectionPolicy::apply_memory_attributes_table_policy(attributes, memory_type);
        assert_eq!(result, efi::MEMORY_RUNTIME | efi::MEMORY_XP);

        // Test case 3: Attributes already set
        let attributes = efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_RUNTIME | efi::MEMORY_WB;
        let memory_type = efi::RUNTIME_SERVICES_CODE;
        let result = MemoryProtectionPolicy::apply_memory_attributes_table_policy(attributes, memory_type);
        assert_eq!(result, efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_RUNTIME);
    }

    #[test]
    fn test_memory_protection_policy_apply_image_stack_guard_policy() {
        let attributes = efi::MEMORY_WB;
        let result = MemoryProtectionPolicy::apply_image_stack_guard_policy(attributes);
        assert_eq!(result, efi::MEMORY_RP | efi::MEMORY_XP | efi::MEMORY_WB);
    }

    #[test]
    fn test_memory_protection_policy_apply_image_protection_policy() {
        let descriptor = MemorySpaceDescriptor {
            base_address: 0,
            length: 0x1000,
            memory_type: GcdMemoryType::SystemMemory,
            attributes: efi::MEMORY_WB,
            capabilities: efi::MEMORY_WB | efi::MEMORY_ACCESS_MASK,
            image_handle: std::ptr::null_mut(),
            device_handle: std::ptr::null_mut(),
        };

        // Code section
        let section_characteristics = pecoff::IMAGE_SCN_CNT_CODE;
        let (attributes, capabilities) =
            MemoryProtectionPolicy::apply_image_protection_policy(section_characteristics, &descriptor);
        assert_eq!(attributes, efi::MEMORY_RO | efi::MEMORY_WB);
        assert_eq!(capabilities, efi::MEMORY_WB | efi::MEMORY_ACCESS_MASK);

        // Data section (no write, but read)
        let section_characteristics = section_table::IMAGE_SCN_MEM_READ;
        let (attributes, _) =
            MemoryProtectionPolicy::apply_image_protection_policy(section_characteristics, &descriptor);
        assert_eq!(attributes, efi::MEMORY_RO | efi::MEMORY_WB | efi::MEMORY_XP);

        // Data section (write allowed)
        let section_characteristics = section_table::IMAGE_SCN_MEM_WRITE;
        let (attributes, _) =
            MemoryProtectionPolicy::apply_image_protection_policy(section_characteristics, &descriptor);
        assert_eq!(attributes, efi::MEMORY_XP | efi::MEMORY_WB);
    }

    #[test]
    fn test_memory_protection_policy_apply_efi_memory_map_policy() {
        // Persistent memory type
        let attributes = efi::MEMORY_RUNTIME | efi::MEMORY_WB;
        let capabilities = efi::MEMORY_WB | efi::MEMORY_RO | efi::MEMORY_RUNTIME;
        let gcd_memory_type = GcdMemoryType::Persistent;
        let result = MemoryProtectionPolicy::apply_efi_memory_map_policy(attributes, capabilities, gcd_memory_type);
        assert_eq!(result, efi::MEMORY_NV | efi::MEMORY_RUNTIME | efi::MEMORY_WB);

        // Non-persistent, should not set NV, attributes don't have runtime, shouldn't be set
        let attributes = efi::MEMORY_WB;
        let gcd_memory_type = GcdMemoryType::SystemMemory;
        let result = MemoryProtectionPolicy::apply_efi_memory_map_policy(attributes, capabilities, gcd_memory_type);
        assert_eq!(result, efi::MEMORY_WB);
    }

    #[test]
    fn test_memory_protection_policy_apply_add_memory_policy() {
        let capabilities = efi::MEMORY_WB | efi::MEMORY_UC;
        let (new_capabilities, attributes) = MemoryProtectionPolicy::apply_add_memory_policy(capabilities);
        assert_eq!(new_capabilities, efi::MEMORY_ACCESS_MASK | efi::MEMORY_RUNTIME | efi::MEMORY_WB | efi::MEMORY_UC);
        assert_eq!(attributes, efi::MEMORY_RP | efi::MEMORY_XP);
    }

    #[test]
    fn test_memory_protection_policy_apply_free_memory_policy() {
        let attributes = efi::MEMORY_WB | efi::MEMORY_RO | efi::MEMORY_RUNTIME;
        let result = MemoryProtectionPolicy::apply_free_memory_policy(attributes);
        assert_eq!(result, efi::MEMORY_RP | efi::MEMORY_XP | efi::MEMORY_WB);
    }

    #[test]
    fn test_memory_protection_policy_apply_null_page_policy() {
        let attributes = efi::MEMORY_WB | efi::MEMORY_RO;
        let result = MemoryProtectionPolicy::apply_null_page_policy(attributes);
        assert_eq!(result, efi::MEMORY_RP | efi::MEMORY_XP | efi::MEMORY_WB);
    }

    #[test]
    #[cfg(not(feature = "compatibility_mode_allowed"))]
    fn test_activate_compatibility_mode_not_allowed() {
        let gcd: SpinLockedGcd = SpinLockedGcd::new(None);

        let result =
            MemoryProtectionPolicy::activate_compatibility_mode(&gcd, 0x1000, 0x10, "test_image.efi".to_string());
        assert_eq!(result, Err(EfiError::LoadError));
    }

    #[test]
    #[cfg(feature = "compatibility_mode_allowed")]
    fn test_activate_compatibility_mode_allowed() {
        use crate::{
            allocator,
            test_support::{MockPageTable, MockPageTableWrapper},
        };
        use std::{cell::RefCell, rc::Rc};
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(spin_locked_gcd::MEMORY_BLOCK_SLICE_SIZE * 10) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    spin_locked_gcd::MEMORY_BLOCK_SLICE_SIZE * 10,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
                GCD.add_memory_space(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    0x0,
                    0xA0000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let policy = &GCD.memory_protection_policy;

            // 1. Default attributes should be MEMORY_XP before activation
            assert_eq!(policy.memory_allocation_default_attributes.get(), efi::MEMORY_XP);

            // allocate some loader code/data memory to test later
            let mut loader_code_mem = 0;
            let mut loader_data_mem = 0;

            allocator::core_allocate_pages(efi::ALLOCATE_ANY_PAGES, efi::LOADER_CODE, 2, &mut loader_code_mem, None)
                .expect("Failed to allocate loader code memory");
            allocator::core_allocate_pages(efi::ALLOCATE_ANY_PAGES, efi::LOADER_DATA, 2, &mut loader_data_mem, None)
                .expect("Failed to allocate loader data memory");

            // loader code/data should be XP by default
            let loader_code_ranges = allocator::get_memory_ranges_for_memory_type(efi::LOADER_CODE);
            let loader_data_ranges = allocator::get_memory_ranges_for_memory_type(efi::LOADER_DATA);

            for range in loader_code_ranges.iter().chain(loader_data_ranges.iter()) {
                let mut addr = range.start;
                while addr < range.end {
                    let mut len = 0x1000;
                    if let Ok(desc) = GCD.get_memory_descriptor_for_address(addr) {
                        assert_eq!(desc.attributes & efi::MEMORY_XP, efi::MEMORY_XP);
                        len = desc.length;
                    }
                    addr += len;
                }
            }

            let mut image_base_page = 0;
            allocator::core_allocate_pages(
                efi::ALLOCATE_ANY_PAGES,
                efi::BOOT_SERVICES_DATA,
                4,
                &mut image_base_page,
                None,
            )
            .expect("Failed to allocate loader code memory");
            let image_num_pages = 4;
            let filename = "legacy_app.efi".to_string();

            let desc = GCD.get_memory_descriptor_for_address(image_base_page).unwrap();
            assert_eq!(desc.attributes & efi::MEMORY_XP, efi::MEMORY_XP);

            // 2. Activate compatibility mode
            let result = MemoryProtectionPolicy::activate_compatibility_mode(
                &GCD,
                image_base_page as usize,
                image_num_pages,
                filename.clone(),
            );
            assert!(result.is_ok());

            // 3. After activation, default attributes should be 0
            assert_eq!(policy.memory_allocation_default_attributes.get(), 0);

            // 4. Page 0 should be mapped
            let desc = GCD.get_memory_descriptor_for_address(0).unwrap();
            assert_eq!(desc.attributes & efi::CACHE_ATTRIBUTE_MASK, efi::MEMORY_WB);

            // 5. Legacy BIOS region (0xA0000) should be mapped if system memory
            let legacy_desc = GCD.get_memory_descriptor_for_address(0xA0000);
            if let Ok(desc) = legacy_desc
                && desc.memory_type == GcdMemoryType::SystemMemory
            {
                assert_eq!(desc.base_address, 0xA0000);
            }

            // 6. All loader code/data memory should have XP cleared (RWX)
            let loader_code_ranges = allocator::get_memory_ranges_for_memory_type(efi::LOADER_CODE);
            let loader_data_ranges = allocator::get_memory_ranges_for_memory_type(efi::LOADER_DATA);

            for range in loader_code_ranges.iter().chain(loader_data_ranges.iter()) {
                let mut addr = range.start;
                while addr < range.end {
                    let mut len = 0x1000;
                    if let Ok(desc) = GCD.get_memory_descriptor_for_address(addr) {
                        assert_eq!(desc.attributes & efi::MEMORY_XP, 0);
                        len = desc.length;
                    }
                    addr += len;
                }
            }

            // 7. The image region should be mapped RWX (XP cleared)
            let desc = GCD.get_memory_descriptor_for_address(image_base_page).unwrap();
            assert_eq!(desc.attributes & efi::MEMORY_XP, 0);
        });
    }
}
