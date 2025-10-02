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

use core::{ffi::c_void, ops::Range, panic};
use mu_pi::{
    dxe_services::{GcdIoType, GcdMemoryType},
    hob::{self, Hob, HobList, PhaseHandoffInformationTable, ResourceDescriptorV2},
};
use patina::base::{align_down, align_up};
use patina::error::EfiError;
use patina_paging::MemoryAttributes;
use r_efi::efi;

#[cfg(feature = "compatibility_mode_allowed")]
use patina::base::{UEFI_PAGE_SIZE, align_range};

use crate::GCD;

pub use spin_locked_gcd::{AllocateType, MapChangeType, SpinLockedGcd};

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
    unsafe {
        GCD.add_memory_space(
            GcdMemoryType::SystemMemory,
            free_memory_start as usize,
            free_memory_size as usize,
            efi::MEMORY_UC
                | efi::MEMORY_WC
                | efi::MEMORY_WT
                | efi::MEMORY_WB
                | efi::MEMORY_WP
                | efi::MEMORY_RP
                | efi::MEMORY_XP
                | efi::MEMORY_RO,
        )
        .expect("Failed to add initial region to GCD.");
    }
}

pub fn init_paging(hob_list: &HobList) {
    GCD.init_paging(hob_list);
}

pub fn add_hob_resource_descriptors_to_gcd(hob_list: &HobList) {
    let phit = hob_list
        .iter()
        .find_map(|x| match x {
            mu_pi::hob::Hob::Handoff(handoff) => Some(*handoff),
            _ => None,
        })
        .expect("Failed to find PHIT Hob");

    let free_memory_start = align_up(phit.free_memory_bottom, 0x1000).expect("Unaligned free memory bottom");
    let free_memory_size =
        align_down(phit.free_memory_top, 0x1000).expect("Unaligned free memory top") - free_memory_start;

    //Iterate over the hob list and map resource descriptor HOBs into the GCD.
    for hob in hob_list.iter() {
        let mut gcd_mem_type: GcdMemoryType = GcdMemoryType::NonExistent;
        let mut mem_range: Range<u64> = 0..0;
        let mut resource_attributes: u32 = 0;

        let mut res_desc_op = None;
        if let Hob::ResourceDescriptor(t_res_desc) = hob {
            res_desc_op = Some(ResourceDescriptorV2::from(**t_res_desc));
        } else if let Hob::ResourceDescriptorV2(t_res_desc) = hob {
            res_desc_op = Some(**t_res_desc);
        }

        match res_desc_op {
            None => (),
            Some(res_desc_v2) => {
                let res_desc = res_desc_v2.v1;
                mem_range = res_desc.physical_start
                    ..res_desc
                        .physical_start
                        .checked_add(res_desc.resource_length)
                        .expect("Invalid resource descriptor hob");

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
                        GCD.add_io_space(
                            GcdIoType::Io,
                            res_desc.physical_start as usize,
                            res_desc.resource_length as usize,
                        )
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
                    assert!(res_desc.attributes_valid());
                }
            }
        }

        if gcd_mem_type != GcdMemoryType::NonExistent {
            let memory_attributes = {
                if let Hob::ResourceDescriptorV2(res_desc) = hob {
                    let mut memory_attributes = MemoryAttributes::from_bits_truncate(res_desc.attributes);
                    memory_attributes &= MemoryAttributes::CacheAttributesMask; //clear everything but caching attributes.
                    if gcd_mem_type == GcdMemoryType::SystemMemory {
                        memory_attributes |= MemoryAttributes::ReadProtect; //force all system memory to be RP by default (since none is allocated yet).
                    }
                    let memory_attributes = memory_attributes.bits();
                    Some(memory_attributes)
                } else {
                    None
                }
            };

            for split_range in
                remove_range_overlap(&mem_range, &(free_memory_start..(free_memory_start + free_memory_size)))
                    .into_iter()
                    .take_while(|r| r.is_some())
                    .flatten()
            {
                log::info!(
                    "Mapping memory range {split_range:#x?} as {gcd_mem_type:?} with attributes {resource_attributes:#x?}",
                );
                unsafe {
                    GCD.add_memory_space(
                        gcd_mem_type,
                        split_range.start as usize,
                        split_range.end.saturating_sub(split_range.start) as usize,
                        spin_locked_gcd::get_capabilities(gcd_mem_type, resource_attributes as u64),
                    )
                    .expect("Failed to add memory space to GCD");
                }
                if let Some(attributes) = memory_attributes {
                    match GCD.set_memory_space_attributes(
                        split_range.start as usize,
                        split_range.end.saturating_sub(split_range.start) as usize,
                        attributes,
                    ) {
                        // NotReady is expected result here since page table is not yet initialized. In this case GCD
                        // will be updated with the appropriate attributes which will then be sync'd to page table
                        // once it is initialized.
                        Err(EfiError::NotReady) => (),
                        _ => {
                            panic!(
                                "GCD failed to set memory attributes {:#X} for base: {:#X}, length: {:#X}",
                                attributes,
                                split_range.start,
                                split_range.end.saturating_sub(split_range.start)
                            );
                        }
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

#[cfg(feature = "compatibility_mode_allowed")]
/// This activates compatibility mode for the GCD.
/// This will:
/// - Activate compatibility mode for the GCD lower layers
/// - Set the memory space attributes for all memory ranges in the loader code and data allocators to be RWX
/// - Uninstall the memory attributes protocol
pub(crate) fn activate_compatibility_mode() {
    GCD.activate_compatibility_mode();
    // if the allocator doesn't have any memory, then when it is used next it will allocate from the GCD
    // and the GCD will be in compatibility mode, so we don't care here
    let mut loader_mem_ranges = crate::allocator::get_memory_ranges_for_memory_type(efi::LOADER_CODE);
    loader_mem_ranges.extend(crate::allocator::get_memory_ranges_for_memory_type(efi::LOADER_DATA));
    for range in loader_mem_ranges.iter() {
        let mut addr = range.start;
        while addr < range.end {
            let mut len = UEFI_PAGE_SIZE as u64;
            match GCD.get_memory_descriptor_for_address(addr) {
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

                    if GCD.set_memory_space_attributes(addr as usize, len as usize, attributes).is_err() {
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
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::ffi::c_void;

    use mu_pi::{
        dxe_services::{GcdIoType, GcdMemoryType, IoSpaceDescriptor, MemorySpaceDescriptor},
        hob::{HobList, PhaseHandoffInformationTable},
    };

    use crate::{
        GCD,
        gcd::init_gcd,
        test_support::{self, build_test_hob_list},
    };

    use super::add_hob_resource_descriptors_to_gcd;

    const MEM_SIZE: u64 = 0x200000;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                GCD.reset();
            }
            f();
        })
        .unwrap();
    }

    fn init_gcd_should_init_gcd(physical_hob_list: *const c_void, mem_base: u64) {
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
}
