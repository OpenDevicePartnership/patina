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
    GCD,
    allocator::{EFI_RUNTIME_SERVICES_DATA_ALLOCATOR, core_allocate_pool},
    config_tables,
    dispatcher::{core_dispatcher, core_schedule, core_trust},
    fv::core_install_firmware_volume,
    gcd,
    systemtables::EfiSystemTable,
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

    let _ = config_tables::core_install_configuration_table(
        dxe_services::DXE_SERVICES_TABLE_GUID,
        Box::into_raw(dxe_system_table) as *mut c_void,
        system_table,
    );
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;
    use dxe_services::GcdMemoryType;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                crate::test_support::init_test_gcd(None);
            }
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_add_memory_space_success() {
        with_locked_state(|| {
            let result = add_memory_space(GcdMemoryType::SystemMemory, 0x80000000, 0x1000, efi::MEMORY_WB);

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_add_memory_space_parameter_validation() {
        with_locked_state(|| {
            // Test: Zero length should return InvalidParameter
            let result = add_memory_space(
                GcdMemoryType::SystemMemory,
                0x80000000,
                0, // zero length should return InvalidParameter
                0,
            );
            assert_eq!(result, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_add_memory_space_overflow_returns_error() {
        with_locked_state(|| {
            // Test: Very large size that would overflow should return an error
            let result = add_memory_space(
                GcdMemoryType::SystemMemory,
                u64::MAX - 100,
                1000, // Would cause overflow
                0,
            );

            // Should return an error status, not SUCCESS
            assert_ne!(result, efi::Status::SUCCESS);
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_add_memory_space_different_memory_types() {
        with_locked_state(|| {
            // Test that our wrapper correctly handles different memory types
            let memory_types = [
                GcdMemoryType::SystemMemory,
                GcdMemoryType::Reserved,
                GcdMemoryType::MemoryMappedIo,
                GcdMemoryType::Persistent,
            ];

            for (i, mem_type) in memory_types.iter().enumerate() {
                let result = add_memory_space(
                    *mem_type,
                    0x100000 + (i as u64 * 0x10000), // Different addresses to avoid conflicts
                    0x1000,
                    0,
                );

                assert_eq!(result, efi::Status::SUCCESS, "Adding memory space for type {:?} failed", mem_type);
            }
        });
    }

    #[test]
    fn test_add_memory_space_reserved_with_specific_attributes() {
        with_locked_state(|| {
            // Demonstrate adding a specific reserved memory region with detailed attributes
            // This example shows a firmware volume region that is:
            // - Memory-mapped I/O type
            // - Uncacheable for device access
            // - Execute-protected (XP) for security
            // - Read-only (RO) for integrity
            let result = add_memory_space(
                GcdMemoryType::Reserved,
                0xF0000000, // Typical firmware region address
                0x100000,   // 1MB firmware volume
                efi::MEMORY_UC | efi::MEMORY_XP | efi::MEMORY_RO,
            );

            assert_eq!(result, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_allocate_memory_space_success() {
        with_locked_state(|| {
            let mut base_address: efi::PhysicalAddress = 0;
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,      // 4KB alignment (2^12 = 4096)
                0x10000, // 64KB length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(result, efi::Status::SUCCESS);
            assert!(base_address != 0, "Base address should be set to a valid address");
        });
    }

    #[test]
    fn test_allocate_memory_space_null_base_address() {
        with_locked_state(|| {
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,                    // 4KB alignment (2^12 = 4096)
                0x10000,               // 64KB length
                core::ptr::null_mut(), // null base address
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(result, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_allocate_memory_space_address_type() {
        with_locked_state(|| {
            let _ = add_memory_space(GcdMemoryType::SystemMemory, 0x100000, 0x100000, efi::MEMORY_WB);

            let mut base_address: efi::PhysicalAddress = 0x110000; // Specific address within the range
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::Address,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment (2^12 = 4096)
                0x1000, // 4KB length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(result, efi::Status::SUCCESS, "Address allocation should succeed");
            assert_eq!(base_address, 0x110000, "Base address should match the requested address");
        });
    }

    #[test]
    fn test_allocate_memory_space_bottom_up() {
        with_locked_state(|| {
            let _ = add_memory_space(GcdMemoryType::SystemMemory, 0x200000, 0x100000, efi::MEMORY_WB);

            let mut base_address: efi::PhysicalAddress = 0;
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(result, efi::Status::SUCCESS, "Bottom-up allocation should succeed");
        });
    }

    #[test]
    fn test_allocate_memory_space_top_down() {
        with_locked_state(|| {
            let _ = add_memory_space(GcdMemoryType::SystemMemory, 0x300000, 0x100000, efi::MEMORY_WB);

            let mut base_address: efi::PhysicalAddress = 0;
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchTopDown,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(result, efi::Status::SUCCESS, "Top-down allocation should succeed");
        });
    }

    #[test]
    fn test_allocate_memory_space_max_address_bottom_up() {
        with_locked_state(|| {
            let _ = add_memory_space(GcdMemoryType::SystemMemory, 0x400000, 0x100000, efi::MEMORY_WB);

            let mut base_address: efi::PhysicalAddress = 0x480000; // Max address limit
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::MaxAddressSearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(result, efi::Status::SUCCESS);
            assert!(base_address <= 0x480000, "Allocated address should be within the limit");
        });
    }

    #[test]
    fn test_allocate_memory_space_max_address_top_down() {
        with_locked_state(|| {
            // First add some memory space to allocate from
            let _ = add_memory_space(GcdMemoryType::SystemMemory, 0x500000, 0x100000, efi::MEMORY_WB);

            let mut base_address: efi::PhysicalAddress = 0x580000; // Max address limit
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::MaxAddressSearchTopDown,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(result, efi::Status::SUCCESS, "Max address top-down allocation should succeed");
            assert!(base_address != 0, "Allocated address should be non-zero");
            assert!(base_address >= 0x500000, "Address should be within added memory range");
        });
    }

    #[test]
    fn test_allocate_memory_space_different_memory_types() {
        with_locked_state(|| {
            // Test allocation with different memory types
            let memory_types = [
                GcdMemoryType::SystemMemory,
                GcdMemoryType::Reserved,
                GcdMemoryType::MemoryMappedIo,
                GcdMemoryType::Persistent,
            ];

            for (i, mem_type) in memory_types.iter().enumerate() {
                // Add memory space for each type
                let base = 0x600000 + (i as u64 * 0x100000);
                let _ = add_memory_space(*mem_type, base, 0x50000, efi::MEMORY_WB);

                let mut base_address: efi::PhysicalAddress = 0;
                let result = allocate_memory_space(
                    dxe_services::GcdAllocateType::AnySearchBottomUp,
                    *mem_type,
                    12,     // 4KB alignment
                    0x1000, // 4KB length
                    &mut base_address,
                    1 as _,
                    core::ptr::null_mut(),
                );

                assert_eq!(result, efi::Status::SUCCESS, "Allocation for memory type {:?} should succeed", mem_type);
            }
        });
    }

    #[test]
    fn test_allocate_memory_space_zero_length() {
        with_locked_state(|| {
            let mut base_address: efi::PhysicalAddress = 0;
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12, // 4KB alignment
                0,  // Zero length
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            // Zero length should return an error
            assert_ne!(result, efi::Status::SUCCESS, "Zero length allocation should fail");
        });
    }

    #[test]
    fn test_allocate_memory_space_excessive_alignment() {
        with_locked_state(|| {
            let mut base_address: efi::PhysicalAddress = 0;
            let result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                63, // Excessive alignment (2^63 would overflow)
                0x1000,
                &mut base_address,
                1 as _,
                core::ptr::null_mut(),
            );

            // Excessive alignment should return an error
            assert_ne!(result, efi::Status::SUCCESS, "Excessive alignment should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_success() {
        with_locked_state(|| {
            // First add memory space
            let base = 0x200000;
            let length = 0x10000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);

            // Then allocate some memory
            let mut allocated_address: efi::PhysicalAddress = 0;
            let allocate_result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut allocated_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(allocate_result, efi::Status::SUCCESS, "Should successfully allocate memory");
            let free_result = free_memory_space(allocated_address, 0x1000);
            assert_eq!(free_result, efi::Status::SUCCESS, "Should successfully free allocated memory");
        });
    }

    #[test]
    fn test_free_memory_space_zero_length() {
        with_locked_state(|| {
            let result = free_memory_space(0x100000, 0);
            // Zero length should return an error
            assert_ne!(result, efi::Status::SUCCESS, "Zero length should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_unallocated_memory() {
        with_locked_state(|| {
            // Try to free memory that was never allocated
            let result = free_memory_space(0x300000, 0x1000);
            // Should return an error since this memory was never allocated
            assert_ne!(result, efi::Status::SUCCESS, "Freeing unallocated memory should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_invalid_address() {
        with_locked_state(|| {
            // Try to free memory at an invalid/unmanaged address
            let result = free_memory_space(0, 0x1000);
            // Should return an error for invalid address
            assert_ne!(result, efi::Status::SUCCESS, "Freeing at address 0 should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_double_free() {
        with_locked_state(|| {
            // Add memory space
            let base = 0x400000;
            let length = 0x10000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);

            // Allocate memory
            let mut allocated_address: efi::PhysicalAddress = 0;
            let allocate_result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut allocated_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(allocate_result, efi::Status::SUCCESS, "Should successfully allocate memory");
            // Free once - should succeed
            let first_free = free_memory_space(allocated_address, 0x1000);
            assert_eq!(first_free, efi::Status::SUCCESS, "First free should succeed");

            // Try to free again - should fail
            let second_free = free_memory_space(allocated_address, 0x1000);
            assert_ne!(second_free, efi::Status::SUCCESS, "Double free should fail");
            assert!(second_free.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_partial_overlap() {
        with_locked_state(|| {
            // Add memory space
            let base = 0x500000;
            let length = 0x10000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);

            // Allocate memory
            let mut allocated_address: efi::PhysicalAddress = 0;
            let allocate_result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x2000, // 8KB length
                &mut allocated_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(allocate_result, efi::Status::SUCCESS, "Should successfully allocate memory");
            let partial_free = free_memory_space(allocated_address, 0x1000); // Only 4KB instead of 8KB
            assert_eq!(partial_free, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_free_memory_space_wrong_length() {
        with_locked_state(|| {
            // Add memory space
            let base = 0x600000;
            let length = 0x10000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);

            // Allocate memory
            let mut allocated_address: efi::PhysicalAddress = 0;
            let allocate_result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut allocated_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(allocate_result, efi::Status::SUCCESS, "Should successfully allocate memory");
            // Try to free with wrong length
            let wrong_length_free = free_memory_space(allocated_address, 0x2000); // 8KB instead of 4KB
            assert_ne!(wrong_length_free, efi::Status::SUCCESS);
            assert!(wrong_length_free.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_large_values() {
        with_locked_state(|| {
            // Test with large but not overflow-causing values
            // Use a high address but leave room to avoid overflow
            let large_base = 0x7FFFFFFF00000000u64; // Large but safe value
            let length = 0x1000u64; // 4KB length

            let result = free_memory_space(large_base, length);
            // Should return an error for invalid large values (not allocated)
            assert_ne!(result, efi::Status::SUCCESS, "Large unallocated values should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_free_memory_space_allocate_free_cycle() {
        with_locked_state(|| {
            // Add memory space
            let base = 0x700000;
            let length = 0x10000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);

            // Test multiple allocate/free cycles
            for i in 0..3 {
                let mut allocated_address: efi::PhysicalAddress = 0;
                let allocate_result = allocate_memory_space(
                    dxe_services::GcdAllocateType::AnySearchBottomUp,
                    GcdMemoryType::SystemMemory,
                    12,     // 4KB alignment
                    0x1000, // 4KB length
                    &mut allocated_address,
                    1 as _,
                    core::ptr::null_mut(),
                );

                assert_eq!(allocate_result, efi::Status::SUCCESS, "Cycle {} allocate should succeed", i);
                let free_result = free_memory_space(allocated_address, 0x1000);
                assert_eq!(free_result, efi::Status::SUCCESS, "Cycle {} free should succeed", i);
            }
        });
    }

    #[test]
    fn test_remove_memory_space_success() {
        with_locked_state(|| {
            let base = 0x800000;
            let length = 0x10000;
            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
            assert_eq!(result, efi::Status::SUCCESS, "Should successfully add memory space");

            let result = remove_memory_space(base, length);
            assert_eq!(result, efi::Status::SUCCESS, "Should successfully remove memory space");
        });
    }

    #[test]
    fn test_remove_memory_space_zero_length() {
        with_locked_state(|| {
            let result = remove_memory_space(0x800000, 0);
            assert_ne!(result, efi::Status::SUCCESS, "Zero length should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_not_found() {
        with_locked_state(|| {
            let result = remove_memory_space(0x900000, 0x1000);
            assert_ne!(result, efi::Status::SUCCESS, "Removing non-existent memory space should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_wrong_base_address() {
        with_locked_state(|| {
            let base = 0xB00000;
            let length = 0x10000;
            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
            assert_eq!(result, efi::Status::SUCCESS, "Should successfully add memory space");

            let result = remove_memory_space(base + 0x1000, length); // Offset base address
            assert_ne!(result, efi::Status::SUCCESS, "Wrong base address should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_wrong_length() {
        with_locked_state(|| {
            let base = 0xC00000;
            let length = 0x10000;
            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
            assert_eq!(result, efi::Status::SUCCESS, "Should successfully add memory space");

            let result = remove_memory_space(base, length * 2); // Double the length
            assert_ne!(result, efi::Status::SUCCESS, "Wrong length should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_double_remove() {
        with_locked_state(|| {
            // Add memory space
            let base = 0xD00000;
            let length = 0x10000;
            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
            assert_eq!(result, efi::Status::SUCCESS, "Should successfully add memory space");

            let first_remove = remove_memory_space(base, length);
            assert_eq!(first_remove, efi::Status::SUCCESS, "First removal should succeed");

            let second_remove = remove_memory_space(base, length);
            assert_ne!(second_remove, efi::Status::SUCCESS, "Double removal should fail");
            assert!(second_remove.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_different_memory_types() {
        with_locked_state(|| {
            // Test removal of different memory types
            let memory_types = [
                GcdMemoryType::SystemMemory,
                GcdMemoryType::Reserved,
                GcdMemoryType::MemoryMappedIo,
                GcdMemoryType::Persistent,
            ];

            for (i, mem_type) in memory_types.iter().enumerate() {
                // Add memory space for each type
                let base = 0xE00000 + (i as u64 * 0x100000);
                let length = 0x10000;
                let result = add_memory_space(*mem_type, base, length, efi::MEMORY_WB);
                assert_eq!(result, efi::Status::SUCCESS, "Adding memory space for type {:?} failed", mem_type);

                // Remove the memory space
                let result = remove_memory_space(base, length);
                assert_eq!(result, efi::Status::SUCCESS, "Removing memory type {:?} should succeed", mem_type);
            }
        });
    }

    #[test]
    fn test_remove_memory_space_with_allocated_memory() {
        with_locked_state(|| {
            let base = 0x1200000;
            let length = 0x10000;
            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
            assert_eq!(result, efi::Status::SUCCESS, "Should successfully add memory space");

            let mut allocated_address: efi::PhysicalAddress = 0;
            let allocate_result = allocate_memory_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdMemoryType::SystemMemory,
                12,     // 4KB alignment
                0x1000, // 4KB length
                &mut allocated_address,
                1 as _,
                core::ptr::null_mut(),
            );

            assert_eq!(allocate_result, efi::Status::SUCCESS, "Should successfully allocate memory");

            let result = remove_memory_space(base, length);
            assert_ne!(result, efi::Status::SUCCESS, "Removing memory space with allocations should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_large_values() {
        with_locked_state(|| {
            let large_base = 0x7FFFFFFF00000000u64; // Large but safe value
            let length = 0x1000u64; // 4KB length

            let result = remove_memory_space(large_base, length);
            assert_ne!(result, efi::Status::SUCCESS, "Large non-existent values should fail");
            assert!(result.as_usize() & 0x8000000000000000 != 0, "Should return an error status");
        });
    }

    #[test]
    fn test_remove_memory_space_add_remove_cycle() {
        with_locked_state(|| {
            let base = 0x1300000;
            let length = 0x10000;

            for i in 0..3 {
                let add_result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
                assert_eq!(add_result, efi::Status::SUCCESS, "Cycle {} add should succeed", i);

                let remove_result = remove_memory_space(base, length);
                assert_eq!(remove_result, efi::Status::SUCCESS, "Cycle {} remove should succeed", i);
            }
        });
    }

    #[test]
    fn test_remove_memory_space_multiple_regions_independence() {
        with_locked_state(|| {
            let regions = [(0x1400000, 0x10000), (0x1500000, 0x20000), (0x1600000, 0x15000)];

            for (base, length) in regions {
                let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
                assert_eq!(result, efi::Status::SUCCESS, "Should add region at 0x{:x}", base);
            }

            let remove_result = remove_memory_space(regions[1].0, regions[1].1);
            assert_eq!(remove_result, efi::Status::SUCCESS, "Should remove middle region");

            let remove_first = remove_memory_space(regions[0].0, regions[0].1);
            assert_eq!(remove_first, efi::Status::SUCCESS, "Should remove first region");

            let remove_third = remove_memory_space(regions[2].0, regions[2].1);
            assert_eq!(remove_third, efi::Status::SUCCESS, "Should remove third region");
        });
    }

    #[test]
    fn test_get_memory_space_descriptor_success() {
        with_locked_state(|| {
            let base = 0x1700000;
            let length = 0x20000;
            let capabilities = efi::MEMORY_WB | efi::MEMORY_WT;

            // Add memory space first
            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, capabilities);
            assert_eq!(result, efi::Status::SUCCESS, "Should add memory space");

            // Get descriptor for the added memory space
            let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            let result = get_memory_space_descriptor(base, descriptor.as_mut_ptr());
            assert_eq!(result, efi::Status::SUCCESS, "Should get memory space descriptor");

            let descriptor = unsafe { descriptor.assume_init() };
            assert_eq!(descriptor.base_address, base, "Base address should match");
            assert_eq!(descriptor.length, length, "Length should match");
            assert_eq!(descriptor.memory_type, GcdMemoryType::SystemMemory, "Memory type should match");
            // Note: GCD may add additional capability flags, so we check that our requested capabilities are present
            assert!(
                descriptor.capabilities & capabilities == capabilities,
                "Requested capabilities should be present. Expected: 0x{:x}, Got: 0x{:x}",
                capabilities,
                descriptor.capabilities
            );
        });
    }

    #[test]
    fn test_get_memory_space_descriptor_null_descriptor() {
        with_locked_state(|| {
            let result = get_memory_space_descriptor(0x1000000, core::ptr::null_mut());
            assert_eq!(result, efi::Status::INVALID_PARAMETER, "Null descriptor should return INVALID_PARAMETER");
        });
    }

    #[test]
    fn test_get_memory_space_descriptor_address_within_range() {
        with_locked_state(|| {
            let base = 0x1800000;
            let length = 0x10000;
            let capabilities = efi::MEMORY_UC;

            // Add memory space
            let result = add_memory_space(GcdMemoryType::Reserved, base, length, capabilities);
            assert_eq!(result, efi::Status::SUCCESS, "Should add memory space");

            // Test getting descriptor for address within the range (not at the base)
            let test_address = base + 0x5000; // Address within the range
            let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            let result = get_memory_space_descriptor(test_address, descriptor.as_mut_ptr());

            assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor for address within range");

            let descriptor = unsafe { descriptor.assume_init() };
            assert_eq!(descriptor.base_address, base, "Base address should be the region base");
            assert_eq!(descriptor.length, length, "Length should match the region length");
            assert_eq!(descriptor.memory_type, GcdMemoryType::Reserved, "Memory type should match");
        });
    }

    #[test]
    fn test_get_memory_space_descriptor_different_memory_types() {
        with_locked_state(|| {
            let memory_types = [
                (GcdMemoryType::SystemMemory, 0x1A00000),
                (GcdMemoryType::Reserved, 0x1B00000),
                (GcdMemoryType::MemoryMappedIo, 0x1C00000),
                (GcdMemoryType::Persistent, 0x1D00000),
            ];

            // Add different memory types
            for (mem_type, base) in memory_types.iter() {
                let result = add_memory_space(*mem_type, *base, 0x10000, efi::MEMORY_WB);
                assert_eq!(result, efi::Status::SUCCESS, "Should add memory space for type {:?}", mem_type);
            }

            // Verify each memory type can be retrieved correctly
            for (expected_type, base) in memory_types.iter() {
                let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
                let result = get_memory_space_descriptor(*base, descriptor.as_mut_ptr());

                assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor for type {:?}", expected_type);

                let descriptor = unsafe { descriptor.assume_init() };
                assert_eq!(descriptor.memory_type, *expected_type, "Memory type should match for {:?}", expected_type);
                assert_eq!(descriptor.base_address, *base, "Base address should match for type {:?}", expected_type);
            }
        });
    }

    #[test]
    fn test_get_memory_space_descriptor_different_capabilities() {
        with_locked_state(|| {
            let capabilities_tests = [
                (0x1E00000, efi::MEMORY_WB),
                (0x1F00000, efi::MEMORY_WT),
                (0x2000000, efi::MEMORY_UC),
                (0x2100000, efi::MEMORY_WB | efi::MEMORY_WT),
                (0x2200000, efi::MEMORY_XP | efi::MEMORY_RO),
            ];

            // Add memory spaces with different capabilities
            for (base, capabilities) in capabilities_tests.iter() {
                let result = add_memory_space(GcdMemoryType::SystemMemory, *base, 0x10000, *capabilities);
                assert_eq!(
                    result,
                    efi::Status::SUCCESS,
                    "Should add memory space with capabilities 0x{:x}",
                    capabilities
                );
            }

            // Verify capabilities are preserved (may have additional flags)
            for (base, expected_capabilities) in capabilities_tests.iter() {
                let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
                let result = get_memory_space_descriptor(*base, descriptor.as_mut_ptr());

                assert_eq!(
                    result,
                    efi::Status::SUCCESS,
                    "Should get descriptor for capabilities 0x{:x}",
                    expected_capabilities
                );

                let descriptor = unsafe { descriptor.assume_init() };
                // Check that our requested capabilities are present (GCD may add additional flags)
                assert!(
                    descriptor.capabilities & expected_capabilities == *expected_capabilities,
                    "Requested capabilities should be present. Expected: 0x{:x}, Got: 0x{:x}",
                    expected_capabilities,
                    descriptor.capabilities
                );
            }
        });
    }

    #[test]
    fn test_get_memory_space_descriptor_boundary_addresses() {
        with_locked_state(|| {
            let base = 0x2300000;
            let length = 0x10000;

            let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
            assert_eq!(result, efi::Status::SUCCESS, "Should add memory space");

            let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            let result = get_memory_space_descriptor(base, descriptor.as_mut_ptr());
            assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor at base address");

            let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            let result = get_memory_space_descriptor(base + length - 1, descriptor.as_mut_ptr());
            assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor at last valid address");

            let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            let result = get_memory_space_descriptor(base - 1, descriptor.as_mut_ptr());
            assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor at address before base");

            let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            let result = get_memory_space_descriptor(base + length, descriptor.as_mut_ptr());
            assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor at address after end of range");
        });
    }
}
