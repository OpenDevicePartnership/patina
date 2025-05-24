//! DXE Core DXE Services
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{boxed::Box, vec::Vec};
use core::{
    ffi::c_void,
    mem,
    slice::{self, from_raw_parts},
};
use patina_sdk::error::EfiError;

use mu_pi::{dxe_services, fw_fs::FirmwareVolume};
use r_efi::efi;

use crate::{
    allocator::{core_allocate_pool, EFI_RUNTIME_SERVICES_DATA_ALLOCATOR},
    dispatcher::{core_dispatcher, core_schedule, core_trust},
    fv::core_install_firmware_volume,
    gcd, misc_boot_services,
    systemtables::EfiSystemTable,
    GCD,
};

extern "efiapi" fn add_memory_space(
    gcd_memory_type: dxe_services::GcdMemoryType,
    base_address: efi::PhysicalAddress,
    length: u64,
    capabilities: u64,
) -> efi::Status {
    let result = unsafe { GCD.add_memory_space(gcd_memory_type, base_address as usize, length as usize, capabilities) };

    match result {
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn allocate_memory_space(
    gcd_allocate_type: dxe_services::GcdAllocateType,
    gcd_memory_type: dxe_services::GcdMemoryType,
    alignment: usize,
    length: u64,
    base_address: *mut efi::PhysicalAddress,
    image_handle: efi::Handle,
    device_handle: efi::Handle,
) -> efi::Status {
    if base_address.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let allocate_type = match gcd_allocate_type {
        dxe_services::GcdAllocateType::Address => {
            let desired_address = unsafe { *base_address };
            gcd::AllocateType::Address(desired_address as usize)
        }
        dxe_services::GcdAllocateType::AnySearchBottomUp => gcd::AllocateType::BottomUp(None),
        dxe_services::GcdAllocateType::AnySearchTopDown => gcd::AllocateType::TopDown(None),
        dxe_services::GcdAllocateType::MaxAddressSearchBottomUp => {
            let limit = unsafe { *base_address };
            gcd::AllocateType::BottomUp(Some(limit as usize))
        }
        dxe_services::GcdAllocateType::MaxAddressSearchTopDown => {
            let limit = unsafe { *base_address };
            gcd::AllocateType::TopDown(Some(limit as usize))
        }
        _ => return efi::Status::INVALID_PARAMETER,
    };

    let result = GCD.allocate_memory_space(
        allocate_type,
        gcd_memory_type,
        alignment,
        length as usize,
        image_handle,
        if device_handle.is_null() { None } else { Some(device_handle) },
    );

    match result {
        Ok(allocated_addr) => {
            unsafe { base_address.write(allocated_addr as u64) };
            efi::Status::SUCCESS
        }
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn free_memory_space(base_address: efi::PhysicalAddress, length: u64) -> efi::Status {
    let result = GCD.free_memory_space(base_address as usize, length as usize);

    match result {
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn remove_memory_space(base_address: efi::PhysicalAddress, length: u64) -> efi::Status {
    let result = GCD.remove_memory_space(base_address as usize, length as usize);
    match result {
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn get_memory_space_descriptor(
    base_address: efi::PhysicalAddress,
    descriptor: *mut dxe_services::MemorySpaceDescriptor,
) -> efi::Status {
    if descriptor.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    match core_get_memory_space_descriptor(base_address) {
        Err(err) => return err.into(),
        Ok(target_descriptor) => unsafe {
            descriptor.write(target_descriptor);
        },
    }
    efi::Status::SUCCESS
}

pub fn core_get_memory_space_descriptor(
    base_address: efi::PhysicalAddress,
) -> Result<dxe_services::MemorySpaceDescriptor, EfiError> {
    GCD.get_memory_descriptor_for_address(base_address)
}

extern "efiapi" fn set_memory_space_attributes(
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    match core_set_memory_space_attributes(base_address, length, attributes) {
        Err(err) => err.into(),
        Ok(_) => efi::Status::SUCCESS,
    }
}

pub fn core_set_memory_space_attributes(
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> Result<(), EfiError> {
    GCD.set_memory_space_attributes(base_address as usize, length as usize, attributes)
}

extern "efiapi" fn set_memory_space_capabilities(
    base_address: efi::PhysicalAddress,
    length: u64,
    capabilities: u64,
) -> efi::Status {
    match core_set_memory_space_capabilities(base_address, length, capabilities) {
        Err(err) => err.into(),
        Ok(_) => efi::Status::SUCCESS,
    }
}

pub fn core_set_memory_space_capabilities(
    base_address: efi::PhysicalAddress,
    length: u64,
    capabilities: u64,
) -> Result<(), EfiError> {
    GCD.set_memory_space_capabilities(base_address as usize, length as usize, capabilities)
}

extern "efiapi" fn get_memory_space_map(
    number_of_descriptors: *mut usize,
    memory_space_map: *mut *mut dxe_services::MemorySpaceDescriptor,
) -> efi::Status {
    if number_of_descriptors.is_null() || memory_space_map.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    //allocate an empty vector with enough space for all the descriptors with some padding (in the event)
    //that extra descriptors come into being after creation but before usage.
    let mut descriptors: Vec<dxe_services::MemorySpaceDescriptor> =
        Vec::with_capacity(GCD.memory_descriptor_count() + 10);
    let result = GCD.get_memory_descriptors(&mut descriptors);

    if let Err(err) = result {
        return efi::Status::from(err);
    }

    //caller is supposed to free the handle buffer using free pool, so we need to allocate it using allocate pool.
    let buffer_size = descriptors.len() * mem::size_of::<dxe_services::MemorySpaceDescriptor>();
    match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
        Err(err) => err.into(),
        Ok(allocation) => unsafe {
            memory_space_map.write(allocation as *mut dxe_services::MemorySpaceDescriptor);
            number_of_descriptors.write(descriptors.len());
            slice::from_raw_parts_mut(*memory_space_map, descriptors.len()).copy_from_slice(&descriptors);
            efi::Status::SUCCESS
        },
    }
}

extern "efiapi" fn add_io_space(
    gcd_io_type: dxe_services::GcdIoType,
    base_address: efi::PhysicalAddress,
    length: u64,
) -> efi::Status {
    let result = GCD.add_io_space(gcd_io_type, base_address as usize, length as usize);
    match result {
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn allocate_io_space(
    gcd_allocate_type: dxe_services::GcdAllocateType,
    gcd_io_type: dxe_services::GcdIoType,
    alignment: usize,
    length: u64,
    base_address: *mut efi::PhysicalAddress,
    image_handle: efi::Handle,
    device_handle: efi::Handle,
) -> efi::Status {
    if base_address.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let allocate_type = match gcd_allocate_type {
        dxe_services::GcdAllocateType::Address => {
            let desired_address = unsafe { *base_address };
            gcd::AllocateType::Address(desired_address as usize)
        }
        dxe_services::GcdAllocateType::AnySearchBottomUp => gcd::AllocateType::BottomUp(None),
        dxe_services::GcdAllocateType::AnySearchTopDown => gcd::AllocateType::TopDown(None),
        dxe_services::GcdAllocateType::MaxAddressSearchBottomUp => {
            let limit = unsafe { *base_address };
            gcd::AllocateType::BottomUp(Some(limit as usize))
        }
        dxe_services::GcdAllocateType::MaxAddressSearchTopDown => {
            let limit = unsafe { *base_address };
            gcd::AllocateType::TopDown(Some(limit as usize))
        }
        _ => return efi::Status::INVALID_PARAMETER,
    };

    let result = GCD.allocate_io_space(
        allocate_type,
        gcd_io_type,
        alignment,
        length as usize,
        image_handle,
        if device_handle.is_null() { None } else { Some(device_handle) },
    );

    match result {
        Ok(allocated_addr) => {
            unsafe { base_address.write(allocated_addr as u64) };
            efi::Status::SUCCESS
        }
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn free_io_space(base_address: efi::PhysicalAddress, length: u64) -> efi::Status {
    let result = GCD.free_io_space(base_address as usize, length as usize);

    match result {
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn remove_io_space(base_address: efi::PhysicalAddress, length: u64) -> efi::Status {
    let result = GCD.remove_io_space(base_address as usize, length as usize);
    match result {
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => efi::Status::from(err),
    }
}

extern "efiapi" fn get_io_space_descriptor(
    base_address: efi::PhysicalAddress,
    descriptor: *mut dxe_services::IoSpaceDescriptor,
) -> efi::Status {
    //Note: this would be more efficient if it was done in the GCD; rather than retrieving all the descriptors and
    //searching them here. It is done this way for simplicity - it can be optimized if it proves too slow.

    //allocate an empty vector with enough space for all the descriptors with some padding (in the event)
    //that extra descriptors come into being after creation but before usage.
    let mut descriptors: Vec<dxe_services::IoSpaceDescriptor> = Vec::with_capacity(GCD.io_descriptor_count() + 10);
    let result = GCD.get_io_descriptors(&mut descriptors);

    if let Err(err) = result {
        return efi::Status::from(err);
    }

    let target_descriptor =
        descriptors.iter().find(|x| (x.base_address <= base_address) && (base_address < (x.base_address + x.length)));

    if let Some(target_descriptor) = target_descriptor {
        unsafe { descriptor.write(*target_descriptor) };
        efi::Status::SUCCESS
    } else {
        efi::Status::NOT_FOUND
    }
}

extern "efiapi" fn get_io_space_map(
    number_of_descriptors: *mut usize,
    io_space_map: *mut *mut dxe_services::IoSpaceDescriptor,
) -> efi::Status {
    if number_of_descriptors.is_null() || io_space_map.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    //allocate an empty vector with enough space for all the descriptors with some padding (in the event)
    //that extra descriptors come into being after creation but before usage.
    let mut descriptors: Vec<dxe_services::IoSpaceDescriptor> = Vec::with_capacity(GCD.io_descriptor_count() + 10);
    let result = GCD.get_io_descriptors(&mut descriptors);

    if let Err(err) = result {
        return efi::Status::from(err);
    }

    //caller is supposed to free the handle buffer using free pool, so we need to allocate it using allocate pool.
    let buffer_size = descriptors.len() * mem::size_of::<dxe_services::IoSpaceDescriptor>();

    match core_allocate_pool(efi::BOOT_SERVICES_DATA, buffer_size) {
        Err(err) => err.into(),
        Ok(allocation) => unsafe {
            io_space_map.write(allocation as *mut dxe_services::IoSpaceDescriptor);
            number_of_descriptors.write(descriptors.len());
            slice::from_raw_parts_mut(*io_space_map, descriptors.len()).copy_from_slice(&descriptors);
            efi::Status::SUCCESS
        },
    }
}

extern "efiapi" fn dispatch() -> efi::Status {
    match core_dispatcher() {
        Err(err) => err.into(),
        Ok(()) => efi::Status::SUCCESS,
    }
}

extern "efiapi" fn schedule(firmware_volume_handle: efi::Handle, file_name: *const efi::Guid) -> efi::Status {
    let Some(file_name) = (unsafe { file_name.as_ref() }) else {
        return efi::Status::INVALID_PARAMETER;
    };

    match core_schedule(firmware_volume_handle, file_name) {
        Err(status) => status.into(),
        Ok(_) => efi::Status::SUCCESS,
    }
}

extern "efiapi" fn trust(firmware_volume_handle: efi::Handle, file_name: *const efi::Guid) -> efi::Status {
    let Some(file_name) = (unsafe { file_name.as_ref() }) else {
        return efi::Status::INVALID_PARAMETER;
    };

    match core_trust(firmware_volume_handle, file_name) {
        Err(status) => status.into(),
        Ok(_) => efi::Status::SUCCESS,
    }
}

extern "efiapi" fn process_firmware_volume(
    firmware_volume_header: *const c_void,
    size: usize,
    firmware_volume_handle: *mut efi::Handle,
) -> efi::Status {
    if firmware_volume_handle.is_null() || firmware_volume_header.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // construct a FirmwareVolume to verify sanity
    let fv_slice = unsafe { slice::from_raw_parts(firmware_volume_header as *const u8, size) };
    if let Err(_err) = FirmwareVolume::new(fv_slice) {
        return efi::Status::VOLUME_CORRUPTED;
    }

    let handle = match core_install_firmware_volume(firmware_volume_header as u64, None) {
        Ok(handle) => handle,
        Err(err) => return err.into(),
    };

    unsafe {
        firmware_volume_handle.write(handle);
    }

    efi::Status::SUCCESS
}

pub fn init_dxe_services(system_table: &mut EfiSystemTable) {
    let mut dxe_system_table = dxe_services::DxeServicesTable {
        header: efi::TableHeader {
            signature: efi::BOOT_SERVICES_SIGNATURE,
            revision: efi::BOOT_SERVICES_REVISION,
            header_size: mem::size_of::<dxe_services::DxeServicesTable>() as u32,
            crc32: 0,
            reserved: 0,
        },
        add_memory_space,
        allocate_memory_space,
        free_memory_space,
        remove_memory_space,
        get_memory_space_descriptor,
        set_memory_space_attributes,
        get_memory_space_map,
        add_io_space,
        allocate_io_space,
        free_io_space,
        remove_io_space,
        get_io_space_descriptor,
        get_io_space_map,
        dispatch,
        schedule,
        trust,
        process_firmware_volume,
        set_memory_space_capabilities,
    };
    let dxe_system_table_ptr = &dxe_system_table as *const dxe_services::DxeServicesTable;
    let crc32 = unsafe {
        crc32fast::hash(from_raw_parts(
            dxe_system_table_ptr as *const u8,
            mem::size_of::<dxe_services::DxeServicesTable>(),
        ))
    };
    dxe_system_table.header.crc32 = crc32;

    let dxe_system_table = Box::new_in(dxe_system_table, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR);

    let _ = misc_boot_services::core_install_configuration_table(
        dxe_services::DXE_SERVICES_TABLE_GUID,
        unsafe { (Box::into_raw(dxe_system_table) as *mut c_void).as_mut() },
        system_table,
    );
}
