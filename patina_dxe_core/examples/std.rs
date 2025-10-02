//! DXE Core STD Binary
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg(feature = "std")]

extern crate alloc;

use mu_pi::{
    BootMode,
    hob::{self, header},
};
use patina_dxe_core::Core;
use r_efi::efi;
use std::ffi::c_void;

static LOGGER: patina::log::SerialLogger<patina::serial::Terminal> = patina::log::SerialLogger::new(
    patina::log::Format::Standard,
    &[
        ("goblin", log::LevelFilter::Off),
        ("patina_internal_depex", log::LevelFilter::Off),
        ("gcd_measure", log::LevelFilter::Off),
    ],
    log::LevelFilter::Trace,
    patina::serial::Terminal {},
);

fn main() -> patina::error::Result<()> {
    if log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Trace)).is_err() {
        log::warn!("Global logger has already been set.");
    }

    let hob_list = build_hob_list();
    Core::default()
        // Add any config knob functions for pre-gcd-init Core
        // .with_some_config(true)
        .init_memory(hob_list) // We can make allocations now!
        // Add any config knob functions for post-gcd-init Core
        // .with_some_config(true)
        .with_service(patina_ffs_extractors::CompositeSectionExtractor::default())
        .start()
}

const MEM_SIZE: u64 = 0x2000000;

unsafe fn get_memory(size: usize) -> &'static mut [u8] {
    let addr = unsafe {
        alloc::alloc::alloc(
            alloc::alloc::Layout::from_size_align(size, 0x1000)
                .unwrap_or_else(|_| panic!("Failed to allocate {size:#x} bytes for hob list.")),
        )
    };
    unsafe { core::slice::from_raw_parts_mut(addr, size) }
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

    let end =
        header::Hob { r#type: hob::END_OF_HOB_LIST, length: core::mem::size_of::<header::Hob>() as u16, reserved: 0 };

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
