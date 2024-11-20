//! DXE Core Global Coherency Domain (GCD)
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
mod io_block;
mod memory_block;
mod spin_locked_gcd;

use core::{ffi::c_void, ops::Range};
use mu_pi::{
    dxe_services::{self, GcdIoType, GcdMemoryType},
    hob::{self, Hob, HobList, PhaseHandoffInformationTable},
};
use mu_rust_helpers::function;
use r_efi::efi;

use crate::{dxe_services::core_get_memory_space_descriptor, protocol_db, GCD};

pub use spin_locked_gcd::{AllocateType, Error, MapChangeType, SpinLockedGcd};

// Align address downwards.
//
// Returns the greatest `x` with alignment `align` so that `x <= addr`.
//
// Panics if the alignment is not a power of two.
#[inline]
const fn align_down(addr: u64, align: u64) -> u64 {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    addr & !(align - 1)
}

// Align address upwards.
//
// Returns the smallest `x` with alignment `align` so that `x >= addr`.
//
// Panics if the alignment is not a power of two or if an overflow occurs.
#[inline]
const fn align_up(addr: u64, align: u64) -> u64 {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    let align_mask = align - 1;
    if addr & align_mask == 0 {
        addr // already aligned
    } else {
        // FIXME: Replace with .expect, once `Option::expect` is const.
        if let Some(aligned) = (addr | align_mask).checked_add(1) {
            aligned
        } else {
            panic!("attempt to add with overflow")
        }
    }
}

pub fn init_gcd(physical_hob_list: *const c_void) -> (u64, u64) {
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
                free_memory_start = align_up(handoff.free_memory_bottom, 0x1000);
                free_memory_size = align_down(handoff.free_memory_top, 0x1000) - free_memory_start;
                memory_start = handoff.memory_bottom;
                memory_end = handoff.memory_top;
            }
            Hob::Cpu(cpu) => {
                GCD.init(cpu.size_of_memory_space as u32, cpu.size_of_io_space as u32);
            }
            _ => (),
        }
    }

    log::info!("memory_start: {:#x?}", memory_start);
    log::info!("memory_size: {:#x?}", memory_end - memory_start);
    log::info!("free_memory_start: {:#x?}", free_memory_start);
    log::info!("free_memory_size: {:#x?}", free_memory_size);

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
        // Mark the first page of memory as non-existent
        GCD.add_memory_space(GcdMemoryType::Reserved, 0, 0x1000, 0)
            .expect("Failed to mark the first page as non-existent in the GCD.");
    };
    (free_memory_start, free_memory_size)
}

pub fn add_hob_resource_descriptors_to_gcd(hob_list: &HobList, free_memory_start: u64, free_memory_size: u64) {
    //Iterate over the hob list and map resource descriptor HOBs into the GCD.
    for hob in hob_list.iter() {
        let mut gcd_mem_type: GcdMemoryType = GcdMemoryType::NonExistent;
        let mut mem_range: Range<u64> = 0..0;
        let mut resource_attributes: u32 = 0;

        if let Hob::ResourceDescriptor(res_desc) = hob {
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

                    if res_desc.physical_start < 0x1000 {
                        let adjusted_base: u64 = 0x1000;
                        mem_range = adjusted_base
                            ..adjusted_base
                                .checked_add(res_desc.resource_length - adjusted_base)
                                .expect("Invalid resource descriptor hob length");
                    }
                }
                hob::EFI_RESOURCE_MEMORY_MAPPED_IO | hob::EFI_RESOURCE_FIRMWARE_DEVICE => {
                    resource_attributes = res_desc.resource_attribute;
                    gcd_mem_type = GcdMemoryType::MemoryMappedIo;
                }
                hob::EFI_RESOURCE_MEMORY_MAPPED_IO_PORT | hob::EFI_RESOURCE_MEMORY_RESERVED => {
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

        if gcd_mem_type != GcdMemoryType::NonExistent {
            for split_range in
                remove_range_overlap(&mem_range, &(free_memory_start..(free_memory_start + free_memory_size)))
                    .into_iter()
                    .take_while(|r| r.is_some())
                    .flatten()
            {
                log::info!(
                    "Mapping memory range {:#x?} as {:?} with attributes {:#x?}",
                    split_range,
                    gcd_mem_type,
                    resource_attributes
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
            }
        }
    }
}

pub fn add_hob_allocations_to_gcd(hob_list: &HobList) {
    for hob in hob_list.iter() {
        match hob {
            Hob::MemoryAllocation(hob::MemoryAllocation { header: _, alloc_descriptor: desc })
            | Hob::MemoryAllocationModule(hob::MemoryAllocationModule {
                header: _,
                alloc_descriptor: desc,
                module_name: _,
                entry_point: _,
            }) => {
                log::trace!("[{}] Processing Memory Allocation HOB:\n{:#x?}\n\n", function!(), hob);

                if let Ok(descriptor) = core_get_memory_space_descriptor(desc.memory_base_address) {
                    let allocator_handle = match desc.memory_type {
                        efi::RESERVED_MEMORY_TYPE => protocol_db::RESERVED_MEMORY_ALLOCATOR_HANDLE,
                        efi::LOADER_CODE => protocol_db::EFI_LOADER_CODE_ALLOCATOR_HANDLE,
                        efi::LOADER_DATA => protocol_db::EFI_LOADER_DATA_ALLOCATOR_HANDLE,
                        efi::BOOT_SERVICES_CODE => protocol_db::EFI_BOOT_SERVICES_CODE_ALLOCATOR_HANDLE,
                        efi::BOOT_SERVICES_DATA => protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                        efi::RUNTIME_SERVICES_CODE => protocol_db::EFI_RUNTIME_SERVICES_CODE_ALLOCATOR_HANDLE,
                        efi::RUNTIME_SERVICES_DATA => protocol_db::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR_HANDLE,
                        efi::ACPI_RECLAIM_MEMORY => protocol_db::EFI_ACPI_RECLAIM_MEMORY_ALLOCATOR_HANDLE,
                        efi::ACPI_MEMORY_NVS => protocol_db::EFI_ACPI_MEMORY_NVS_ALLOCATOR_HANDLE,
                        _ => protocol_db::DXE_CORE_HANDLE,
                    };
                    if let Err(e) = GCD.allocate_memory_space(
                        spin_locked_gcd::AllocateType::Address(desc.memory_base_address as usize),
                        descriptor.memory_type,
                        0,
                        desc.memory_length as usize,
                        allocator_handle,
                        None,
                    ) {
                        log::error!(
                            "Failed to allocate memory space for memory allocation HOB at {:#x?} of length {:#x?}. Error: {:?}",
                            desc.memory_base_address,
                            desc.memory_length,
                            e
                        );
                    }
                }
            }
            Hob::FirmwareVolume(hob::FirmwareVolume { header: _, base_address, length })
            | Hob::FirmwareVolume2(hob::FirmwareVolume2 {
                header: _,
                base_address,
                length,
                fv_name: _,
                file_name: _,
            })
            | Hob::FirmwareVolume3(hob::FirmwareVolume3 {
                header: _,
                base_address,
                length,
                authentication_status: _,
                extracted_fv: _,
                fv_name: _,
                file_name: _,
            }) => {
                log::trace!("[{}] Processing Firmware Volume HOB:\n{:#x?}\n\n", function!(), hob);

                let result = GCD.allocate_memory_space(
                    spin_locked_gcd::AllocateType::Address(*base_address as usize),
                    dxe_services::GcdMemoryType::MemoryMappedIo,
                    0,
                    *length as usize,
                    protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                    None,
                );
                if result.is_err() {
                    log::warn!(
                        "Memory space is not yet available for the FV at {:#x?} of length {:#x?}.",
                        base_address,
                        length
                    );
                }
            }
            _ => continue,
        };
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

#[cfg(test)]
mod tests {
    use core::ffi::c_void;

    use mu_pi::{
        dxe_services::{GcdIoType, GcdMemoryType, IoSpaceDescriptor, MemorySpaceDescriptor},
        hob::{self, header, HobList},
        BootMode,
    };
    use r_efi::efi;

    use crate::{gcd::init_gcd, protocol_db, test_support, GCD};

    use super::{add_hob_allocations_to_gcd, add_hob_resource_descriptors_to_gcd};

    const MEM_SIZE: u64 = 0x200000;

    unsafe fn get_memory(size: usize) -> &'static mut [u8] {
        let addr = alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, 0x1000).unwrap());
        core::slice::from_raw_parts_mut(addr, size)
    }

    fn with_locked_state<F: Fn()>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                GCD.reset();
            }
            f();
        });
    }

    fn build_hob_list() -> *const c_void {
        let mem = unsafe { get_memory(MEM_SIZE as usize) };
        let mem_base = mem.as_mut_ptr() as u64;

        // Build a test HOB list that describes memory layout as follows:
        //
        // Base:         offset 0                   ************
        // HobList:      offset base+0              HOBS
        // Empty:        offset base+HobListSize    N/A
        // SystemMemory  offset base+0xE0000        SystemMemory (resource_descriptor1)
        // Reserved      offset base+0xF0000        Untested SystemMemory (resource_descriptor2)
        // FreeMemory    offset base+0x100000       FreeMemory (phit)
        // End           offset base+0x200000       ************
        //
        // THe test HOB list will also include resource descriptor hobs that describe MMIO/IO as follows:
        // MMIO at 0x10000000 size 0x1000000 (resource_descriptor3)
        // FirmwareDevice at 0x11000000 size 0x1000000 (resource_descriptor4)
        // Reserved at 0x12000000 size 0x1000000 (resource_descriptor5)
        // Legacy I/O at 0x1000 size 0xF000 (resource_descriptor6)
        // Reserved Legacy I/O at 0x0000 size 0x1000 (resource_descriptor7)
        //
        // The test HOB list will also include resource allocation hobs that describe allocations as follows:
        // A Memory ALlocation Hob for each memory type. This will be placed in the SystemMemory region at base+0xE0000 as
        // 4K allocations.
        // A Firmware Volume HOB located in the FirmwareDevice region at 0x10000000
        //
        let phit = hob::PhaseHandoffInformationTable {
            header: header::Hob {
                r#type: hob::HANDOFF,
                length: core::mem::size_of::<hob::PhaseHandoffInformationTable>() as u16,
                reserved: 0x00000000,
            },
            version: 0x0009,
            boot_mode: BootMode::BootAssumingNoConfigurationChanges,
            memory_top: mem_base + MEM_SIZE,
            memory_bottom: mem_base,
            free_memory_top: mem_base + MEM_SIZE,
            free_memory_bottom: mem_base + 0x100000,
            end_of_hob_list: mem_base
                + core::mem::size_of::<hob::PhaseHandoffInformationTable>() as u64
                + core::mem::size_of::<hob::Cpu>() as u64
                + (core::mem::size_of::<hob::ResourceDescriptor>() as u64) * 7
                + core::mem::size_of::<header::Hob>() as u64,
        };

        let cpu = hob::Cpu {
            header: header::Hob { r#type: hob::CPU, length: core::mem::size_of::<hob::Cpu>() as u16, reserved: 0 },
            size_of_memory_space: 48,
            size_of_io_space: 16,
            reserved: Default::default(),
        };

        let resource_descriptor1 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
            physical_start: mem_base + 0xE0000,
            resource_length: 0x10000,
        };

        let resource_descriptor2 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::INITIALIZED_MEMORY_ATTRIBUTES,
            physical_start: mem_base + 0xF0000,
            resource_length: 0x10000,
        };

        let resource_descriptor3 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_MEMORY_MAPPED_IO,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
            physical_start: 0x10000000,
            resource_length: 0x1000000,
        };

        let resource_descriptor4 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_FIRMWARE_DEVICE,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
            physical_start: 0x11000000,
            resource_length: 0x1000000,
        };

        let resource_descriptor5 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_MEMORY_RESERVED,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
            physical_start: 0x12000000,
            resource_length: 0x1000000,
        };

        let resource_descriptor6 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_IO,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
            physical_start: 0x1000,
            resource_length: 0xF000,
        };

        let resource_descriptor7 = hob::ResourceDescriptor {
            header: header::Hob {
                r#type: hob::RESOURCE_DESCRIPTOR,
                length: core::mem::size_of::<hob::ResourceDescriptor>() as u16,
                reserved: 0x00000000,
            },
            owner: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
            resource_type: hob::EFI_RESOURCE_IO_RESERVED,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
            physical_start: 0x0000,
            resource_length: 0x1000,
        };

        let mut allocation_hob_template = hob::MemoryAllocation {
            header: header::Hob {
                r#type: hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                reserved: 0x00000000,
            },
            alloc_descriptor: header::MemoryAllocation {
                name: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0u8; 6]),
                memory_base_address: 0,
                memory_length: 0x1000,
                memory_type: efi::RESERVED_MEMORY_TYPE,
                reserved: Default::default(),
            },
        };

        let firmware_volume_hob = hob::FirmwareVolume {
            header: header::Hob {
                r#type: hob::FV,
                length: core::mem::size_of::<hob::FirmwareVolume>() as u16,
                reserved: 0x00000000,
            },
            base_address: resource_descriptor4.physical_start,
            length: 0x80000,
        };

        let end = header::Hob {
            r#type: hob::END_OF_HOB_LIST,
            length: core::mem::size_of::<header::Hob>() as u16,
            reserved: 0,
        };

        unsafe {
            let mut cursor = mem.as_mut_ptr();

            //PHIT HOB
            core::ptr::copy(&phit, cursor as *mut hob::PhaseHandoffInformationTable, 1);
            cursor = cursor.offset(phit.header.length as isize);

            //CPU HOB
            core::ptr::copy(&cpu, cursor as *mut hob::Cpu, 1);
            cursor = cursor.offset(cpu.header.length as isize);

            //resource descriptor HOBs - see above comment
            core::ptr::copy(&resource_descriptor1, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor1.header.length as isize);

            core::ptr::copy(&resource_descriptor2, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor2.header.length as isize);

            core::ptr::copy(&resource_descriptor3, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor3.header.length as isize);

            core::ptr::copy(&resource_descriptor4, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor4.header.length as isize);

            core::ptr::copy(&resource_descriptor5, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor5.header.length as isize);

            core::ptr::copy(&resource_descriptor6, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor6.header.length as isize);

            core::ptr::copy(&resource_descriptor7, cursor as *mut hob::ResourceDescriptor, 1);
            cursor = cursor.offset(resource_descriptor7.header.length as isize);

            //memory allocation HOBs.
            for (idx, memory_type) in [
                efi::RESERVED_MEMORY_TYPE,
                efi::LOADER_CODE,
                efi::LOADER_DATA,
                efi::BOOT_SERVICES_CODE,
                efi::BOOT_SERVICES_DATA,
                efi::RUNTIME_SERVICES_CODE,
                efi::RUNTIME_SERVICES_DATA,
                efi::ACPI_RECLAIM_MEMORY,
                efi::ACPI_MEMORY_NVS,
                efi::PAL_CODE,
            ]
            .iter()
            .enumerate()
            {
                allocation_hob_template.alloc_descriptor.memory_base_address =
                    resource_descriptor1.physical_start + idx as u64 * 0x1000;
                allocation_hob_template.alloc_descriptor.memory_type = *memory_type;

                core::ptr::copy(&allocation_hob_template, cursor as *mut hob::MemoryAllocation, 1);
                cursor = cursor.offset(allocation_hob_template.header.length as isize);
            }

            //FV HOB.
            core::ptr::copy(&firmware_volume_hob, cursor as *mut hob::FirmwareVolume, 1);
            cursor = cursor.offset(firmware_volume_hob.header.length as isize);

            core::ptr::copy(&end, cursor as *mut header::Hob, 1);
        }
        mem.as_ptr() as *const c_void
    }

    fn init_gcd_should_init_gcd(physical_hob_list: *const c_void, mem_base: u64) -> (u64, u64) {
        let (free_memory_start, free_memory_size) = init_gcd(physical_hob_list);
        assert!(free_memory_start >= mem_base && free_memory_start < mem_base + MEM_SIZE);
        assert!(free_memory_size <= 0x100000);
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::with_capacity(GCD.memory_descriptor_count() + 10);
        GCD.get_memory_descriptors(&mut descriptors).expect("get_memory_descriptors failed.");
        assert!(descriptors
            .iter()
            .any(|x| x.base_address == free_memory_start && x.memory_type == GcdMemoryType::SystemMemory));
        (free_memory_start, free_memory_size)
    }

    fn add_resource_descriptors_should_add_resource_descriptors(
        hob_list: &HobList,
        free_memory_start: u64,
        free_memory_size: u64,
        mem_base: u64,
    ) {
        add_hob_resource_descriptors_to_gcd(hob_list, free_memory_start, free_memory_size);
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

    fn add_allocations_should_add_allocations(hob_list: &HobList, mem_base: u64) {
        add_hob_allocations_to_gcd(hob_list);
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::with_capacity(GCD.memory_descriptor_count() + 10);
        GCD.get_memory_descriptors(&mut descriptors).expect("get_memory_descriptors failed.");
        log::info!("Descriptors: {:#x?}", descriptors);
        for (idx, handle) in [
            protocol_db::RESERVED_MEMORY_ALLOCATOR_HANDLE,
            protocol_db::EFI_LOADER_CODE_ALLOCATOR_HANDLE,
            protocol_db::EFI_LOADER_DATA_ALLOCATOR_HANDLE,
            protocol_db::EFI_BOOT_SERVICES_CODE_ALLOCATOR_HANDLE,
            protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
            protocol_db::EFI_RUNTIME_SERVICES_CODE_ALLOCATOR_HANDLE,
            protocol_db::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR_HANDLE,
            protocol_db::EFI_ACPI_RECLAIM_MEMORY_ALLOCATOR_HANDLE,
            protocol_db::EFI_ACPI_MEMORY_NVS_ALLOCATOR_HANDLE,
            protocol_db::DXE_CORE_HANDLE,
        ]
        .iter()
        .enumerate()
        {
            log::info!("Testing allocation descriptor idx: {:#x?} handle: {:#x?}", idx, handle);
            descriptors
                .iter()
                .find(|x| {
                    x.base_address == mem_base + 0xE0000 + idx as u64 * 0x1000
                        && x.length == 0x1000
                        && x.memory_type == GcdMemoryType::SystemMemory
                        && x.image_handle == *handle
                })
                .unwrap();
        }
        //FV allocation
        descriptors
            .iter()
            .find(|x| {
                x.base_address == 0x11000000
                    && x.length == 0x80000
                    && x.memory_type == GcdMemoryType::MemoryMappedIo
                    && x.image_handle == protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE
            })
            .unwrap();
    }

    #[test]
    fn test_full_gcd_init() {
        with_locked_state(|| {
            let physical_hob_list = build_hob_list();
            let (free_memory_start, free_memory_size) =
                init_gcd_should_init_gcd(physical_hob_list, physical_hob_list as u64);

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);

            add_resource_descriptors_should_add_resource_descriptors(
                &hob_list,
                free_memory_start,
                free_memory_size,
                physical_hob_list as u64,
            );

            add_allocations_should_add_allocations(&hob_list, physical_hob_list as u64);
        });
    }
}
