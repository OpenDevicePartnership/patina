//! DXE Core DXE Services
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::{boxed::Box, vec::Vec};
use core::{
    ffi::c_void,
    mem,
    slice::{self, from_raw_parts},
};
use patina_ffs::volume::VolumeRef;
use patina_sdk::error::EfiError;

use mu_pi::dxe_services;
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
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            let desired_address = unsafe { base_address.read_unaligned() };
            gcd::AllocateType::Address(desired_address as usize)
        }
        dxe_services::GcdAllocateType::AnySearchBottomUp => gcd::AllocateType::BottomUp(None),
        dxe_services::GcdAllocateType::AnySearchTopDown => gcd::AllocateType::TopDown(None),
        dxe_services::GcdAllocateType::MaxAddressSearchBottomUp => {
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            let limit = unsafe { base_address.read_unaligned() };
            gcd::AllocateType::BottomUp(Some(limit as usize))
        }
        dxe_services::GcdAllocateType::MaxAddressSearchTopDown => {
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            let limit = unsafe { base_address.read_unaligned() };
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
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            unsafe { base_address.write_unaligned(allocated_addr as u64) };
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
        Ok(target_descriptor) =>
        // Safety: caller must ensure that descriptor is a valid pointer. It is null-checked above.
        unsafe {
            descriptor.write_unaligned(target_descriptor);
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
        Ok(_) => efi::Status::SUCCESS,
        Err(err) => err.into(),
    }
}

pub fn core_set_memory_space_attributes(
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> Result<(), EfiError> {
    match GCD.set_memory_space_attributes(base_address as usize, length as usize, attributes) {
        Ok(()) => Ok(()),
        Err(EfiError::NotReady) => {
            // Disambiguate "NotReady": if the GCD is initialized but paging
            // isn’t installed yet, the GCD state has been updated and callers
            // of the DXE Services wrapper expect SUCCESS. Only surface
            // NOT_READY when the GCD itself is uninitialized.
            if GCD.is_ready() {
                Ok(()) // GCD ready, paging not ready -> treat as success
            } else {
                Err(EfiError::NotReady) // GCD not initialized
            }
        }
        Err(e) => Err(e),
    }
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
        Ok(allocation) =>
        // Safety: caller must ensure that number_of_descriptors and memory_space_map are valid pointers. They are
        // null-checked above.
        unsafe {
            memory_space_map.write_unaligned(allocation as *mut dxe_services::MemorySpaceDescriptor);
            number_of_descriptors.write_unaligned(descriptors.len());
            slice::from_raw_parts_mut(memory_space_map.read_unaligned(), descriptors.len())
                .copy_from_slice(&descriptors);
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
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            let desired_address = unsafe { base_address.read_unaligned() };
            gcd::AllocateType::Address(desired_address as usize)
        }
        dxe_services::GcdAllocateType::AnySearchBottomUp => gcd::AllocateType::BottomUp(None),
        dxe_services::GcdAllocateType::AnySearchTopDown => gcd::AllocateType::TopDown(None),
        dxe_services::GcdAllocateType::MaxAddressSearchBottomUp => {
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            let limit = unsafe { base_address.read_unaligned() };
            gcd::AllocateType::BottomUp(Some(limit as usize))
        }
        dxe_services::GcdAllocateType::MaxAddressSearchTopDown => {
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            let limit = unsafe { base_address.read_unaligned() };
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
            // Safety: caller must ensure that base_address is a valid pointer. It is null-checked above.
            unsafe { base_address.write_unaligned(allocated_addr as u64) };
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
    if descriptor.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

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
        // Safety: caller must ensure that descriptor is a valid pointer. It is null-checked above.
        unsafe { descriptor.write_unaligned(*target_descriptor) };
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
        Ok(allocation) =>
        // Safety: caller must ensure that number_of_descriptors and io_space_map are valid pointers. They are null-checked above.
        unsafe {
            io_space_map.write_unaligned(allocation as *mut dxe_services::IoSpaceDescriptor);
            number_of_descriptors.write_unaligned(descriptors.len());
            slice::from_raw_parts_mut(io_space_map.read_unaligned(), descriptors.len()).copy_from_slice(&descriptors);
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
    if file_name.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    // Safety: caller must ensure that file_name is a valid pointer. It is null-checked above.
    let file_name = unsafe { file_name.read_unaligned() };

    match core_schedule(firmware_volume_handle, &file_name) {
        Err(status) => status.into(),
        Ok(_) => efi::Status::SUCCESS,
    }
}

extern "efiapi" fn trust(firmware_volume_handle: efi::Handle, file_name: *const efi::Guid) -> efi::Status {
    if file_name.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    // Safety: caller must ensure that file_name is a valid pointer. It is null-checked above.
    let file_name = unsafe { file_name.read_unaligned() };

    match core_trust(firmware_volume_handle, &file_name) {
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
    // Safety: caller must ensure that firmware_volume_header is a valid pointer. It is null-checked above.
    let fv_slice = unsafe { slice::from_raw_parts(firmware_volume_header as *const u8, size) };
    if let Err(err) = VolumeRef::new(fv_slice) {
        return err.into();
    }
    // Safety: caller must ensure that firmware_volume_header is a valid firmware volume that will not be freed
    // or moved after being sent to the core for processing.
    let res = unsafe { core_install_firmware_volume(firmware_volume_header as u64, None) };
    let handle = match res {
        Ok(handle) => handle,
        Err(err) => return err.into(),
    };

    // Safety: caller must ensure that firmware_volume_handle is a valid pointer. It is null-checked above.
    unsafe {
        firmware_volume_handle.write_unaligned(handle);
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
        Box::into_raw_with_allocator(dxe_system_table).0 as *mut c_void,
        system_table,
    );
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;
    use dxe_services::{GcdIoType, GcdMemoryType};

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

                assert_eq!(result, efi::Status::SUCCESS, "Adding memory space for type {mem_type:?} failed");
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

                assert_eq!(result, efi::Status::SUCCESS, "Allocation for memory type {mem_type:?} should succeed");
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

                assert_eq!(allocate_result, efi::Status::SUCCESS, "Cycle {i} allocate should succeed");
                let free_result = free_memory_space(allocated_address, 0x1000);
                assert_eq!(free_result, efi::Status::SUCCESS, "Cycle {i} free should succeed");
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
                assert_eq!(result, efi::Status::SUCCESS, "Adding memory space for type {mem_type:?} failed");

                // Remove the memory space
                let result = remove_memory_space(base, length);
                assert_eq!(result, efi::Status::SUCCESS, "Removing memory type {mem_type:?} should succeed");
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
                assert_eq!(add_result, efi::Status::SUCCESS, "Cycle {i} add should succeed");

                let remove_result = remove_memory_space(base, length);
                assert_eq!(remove_result, efi::Status::SUCCESS, "Cycle {i} remove should succeed");
            }
        });
    }

    #[test]
    fn test_remove_memory_space_multiple_regions_independence() {
        with_locked_state(|| {
            let regions = [(0x1400000, 0x10000), (0x1500000, 0x20000), (0x1600000, 0x15000)];

            for (base, length) in regions {
                let result = add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB);
                assert_eq!(result, efi::Status::SUCCESS, "Should add region at 0x{base:x}");
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
                assert_eq!(result, efi::Status::SUCCESS, "Should add memory space for type {mem_type:?}");
            }

            // Verify each memory type can be retrieved correctly
            for (expected_type, base) in memory_types.iter() {
                let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
                let result = get_memory_space_descriptor(*base, descriptor.as_mut_ptr());

                assert_eq!(result, efi::Status::SUCCESS, "Should get descriptor for type {expected_type:?}");

                let descriptor = unsafe { descriptor.assume_init() };
                assert_eq!(descriptor.memory_type, *expected_type, "Memory type should match for {expected_type:?}");
                assert_eq!(descriptor.base_address, *base, "Base address should match for type {expected_type:?}");
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
                    "Should add memory space with capabilities 0x{capabilities:x}",
                );
            }

            // Verify capabilities are preserved (may have additional flags)
            for (base, expected_capabilities) in capabilities_tests.iter() {
                let mut descriptor = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
                let result = get_memory_space_descriptor(*base, descriptor.as_mut_ptr());

                assert_eq!(
                    result,
                    efi::Status::SUCCESS,
                    "Should get descriptor for capabilities 0x{expected_capabilities:x}",
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

    #[test]
    fn test_set_memory_space_attributes_success_and_readback() {
        with_locked_state(|| {
            let base = 0x2400000;
            let length = 0x2000; // 2 pages

            // Prepare a region
            assert_eq!(
                add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB),
                efi::Status::SUCCESS
            );

            // Allow changing RO/XP and keep WB. Also include RP to continue supporting current attributes.
            let allowed = efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB | efi::MEMORY_RP;
            assert_eq!(set_memory_space_capabilities(base, length, allowed), efi::Status::SUCCESS);

            // Apply RO + XP + keep WB caching
            let attrs = efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB;
            let s = set_memory_space_attributes(base, length, attrs);
            assert_eq!(s, efi::Status::SUCCESS);

            // Read back and verify bits are set
            let mut d = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            assert_eq!(get_memory_space_descriptor(base, d.as_mut_ptr()), efi::Status::SUCCESS);
            let d = unsafe { d.assume_init() };
            assert_eq!(d.base_address, base);
            assert_eq!(d.length, length);
            assert!(d.attributes & attrs == attrs, "expected attrs 0x{:x} to be set in 0x{:x}", attrs, d.attributes);
        });
    }

    #[test]
    fn test_set_memory_space_attributes_partial_range_only_affects_subset() {
        with_locked_state(|| {
            let base = 0x2410000;
            let length = 0x4000; // 4 pages
            assert_eq!(
                add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB),
                efi::Status::SUCCESS
            );

            // Allow RO and keep existing WB/XP so current attributes remain supported. Include RP to cover current attrs.
            assert_eq!(
                set_memory_space_capabilities(
                    base,
                    length,
                    efi::MEMORY_RO | efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RP
                ),
                efi::Status::SUCCESS
            );

            // Change attributes for first page only
            let first_len = 0x1000u64;
            let attrs = efi::MEMORY_RO | efi::MEMORY_WB;
            let s = set_memory_space_attributes(base, first_len, attrs);
            assert_eq!(s, efi::Status::SUCCESS);

            // The first page should have RO set
            let mut d0 = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            assert_eq!(get_memory_space_descriptor(base, d0.as_mut_ptr()), efi::Status::SUCCESS);
            let d0 = unsafe { d0.assume_init() };
            assert!(d0.attributes & efi::MEMORY_RO != 0);

            // A later page should not necessarily have RO (split expected). We only assert that RO is not set there.
            let mut d1 = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            assert_eq!(get_memory_space_descriptor(base + 0x3000, d1.as_mut_ptr()), efi::Status::SUCCESS);
            let d1 = unsafe { d1.assume_init() };
            assert!(d1.attributes & efi::MEMORY_RO == 0, "RO should not be set on untouched pages");
        });
    }

    #[test]
    fn test_set_memory_space_attributes_not_ready() {
        with_locked_state(|| {
            unsafe { GCD.reset() };
            let s = set_memory_space_attributes(0x2421000, 0x1000, efi::MEMORY_WB);
            assert_eq!(s, efi::Status::NOT_READY);
        });
    }

    // Note: We intentionally do not test out-of-range behavior for set_memory_space_attributes here,
    // as the debug build asserts on internal GCD errors for this path. The out-of-range case is covered
    // by set_memory_space_capabilities tests above, which return UNSUPPORTED without asserting.

    #[test]
    fn test_get_memory_space_map_invalid_parameters() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };

            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::MemorySpaceDescriptor = core::ptr::null_mut();

            let s = get_memory_space_map(core::ptr::null_mut(), &mut out_ptr);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);

            let s = get_memory_space_map(&mut out_count, core::ptr::null_mut());
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_get_memory_space_map_not_ready() {
        with_locked_state(|| {
            unsafe { GCD.reset() };

            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::MemorySpaceDescriptor = core::ptr::null_mut();

            let s = get_memory_space_map(&mut out_count, &mut out_ptr);
            assert_eq!(s, efi::Status::NOT_READY, "Expected NOT_READY when GCD is uninitialized");
        });
    }

    #[test]
    fn test_get_memory_space_map_success_and_contents() {
        with_locked_state(|| {
            unsafe {
                crate::test_support::reset_allocators();
                crate::test_support::init_test_gcd(None);
            }

            let expected_count = GCD.memory_descriptor_count();
            let mut expected: Vec<dxe_services::MemorySpaceDescriptor> = Vec::with_capacity(expected_count + 10);
            GCD.get_memory_descriptors(&mut expected).expect("get_memory_descriptors failed");
            assert!(!expected.is_empty());

            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::MemorySpaceDescriptor = core::ptr::null_mut();
            let s = get_memory_space_map(&mut out_count, &mut out_ptr);
            assert_eq!(s, efi::Status::SUCCESS);
            assert_eq!(out_count, expected.len());
            assert!(!out_ptr.is_null());

            let out_slice = unsafe { core::slice::from_raw_parts(out_ptr, out_count) };
            assert_eq!(out_slice, expected.as_slice());

            assert!(crate::allocator::core_free_pool(out_ptr as *mut core::ffi::c_void).is_ok());
        });
    }

    #[test]
    fn test_get_memory_space_map_with_additional_regions() {
        with_locked_state(|| {
            unsafe {
                crate::test_support::reset_allocators();
                crate::test_support::init_test_gcd(None);
            }

            // Add a few extra regions of varying types
            let _ = add_memory_space(GcdMemoryType::SystemMemory, 0x2600000, 0x20000, efi::MEMORY_WB);
            let _ = add_memory_space(GcdMemoryType::Reserved, 0x2700000, 0x10000, efi::MEMORY_UC | efi::MEMORY_XP);
            let _ = add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2800000, 0x30000, efi::MEMORY_UC);

            // Fetch expected
            let expected_count = GCD.memory_descriptor_count();
            let mut expected: Vec<dxe_services::MemorySpaceDescriptor> = Vec::with_capacity(expected_count + 10);
            GCD.get_memory_descriptors(&mut expected).expect("get_memory_descriptors failed");
            assert!(expected.len() >= 3);

            // Call API
            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::MemorySpaceDescriptor = core::ptr::null_mut();
            let s = get_memory_space_map(&mut out_count, &mut out_ptr);
            assert_eq!(s, efi::Status::SUCCESS);
            assert_eq!(out_count, expected.len());

            // Verify first and last few entries match (order should be the same as GCD enumeration)
            let out_slice = unsafe { core::slice::from_raw_parts(out_ptr, out_count) };
            assert_eq!(out_slice, expected.as_slice());

            // cleanup
            assert!(crate::allocator::core_free_pool(out_ptr as *mut core::ffi::c_void).is_ok());
        });
    }

    #[test]
    fn test_get_io_space_map_invalid_parameters() {
        with_locked_state(|| {
            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::IoSpaceDescriptor = core::ptr::null_mut();

            let s = get_io_space_map(core::ptr::null_mut(), &mut out_ptr);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);

            let s = get_io_space_map(&mut out_count, core::ptr::null_mut());
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_get_io_space_map_not_ready() {
        with_locked_state(|| {
            unsafe { GCD.reset() };

            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::IoSpaceDescriptor = core::ptr::null_mut();

            let s = get_io_space_map(&mut out_count, &mut out_ptr);
            assert_eq!(s, efi::Status::NOT_READY, "Expected NOT_READY when GCD is uninitialized");
        });
    }

    #[test]
    fn test_get_io_space_map_success_and_contents() {
        with_locked_state(|| {
            unsafe {
                crate::test_support::reset_allocators();
                crate::test_support::init_test_gcd(None);
            }

            let expected_count = GCD.io_descriptor_count();
            let mut expected: Vec<dxe_services::IoSpaceDescriptor> = Vec::with_capacity(expected_count + 10);
            GCD.get_io_descriptors(&mut expected).expect("get_io_descriptors failed");
            assert!(!expected.is_empty());

            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::IoSpaceDescriptor = core::ptr::null_mut();
            let s = get_io_space_map(&mut out_count, &mut out_ptr);
            assert_eq!(s, efi::Status::SUCCESS);
            assert_eq!(out_count, expected.len());
            assert!(!out_ptr.is_null());

            let out_slice = unsafe { core::slice::from_raw_parts(out_ptr, out_count) };
            assert_eq!(out_slice, expected.as_slice());

            assert!(crate::allocator::core_free_pool(out_ptr as *mut core::ffi::c_void).is_ok());
        });
    }

    #[test]
    fn test_get_io_space_map_with_additional_regions() {
        with_locked_state(|| {
            unsafe {
                crate::test_support::reset_allocators();
                crate::test_support::init_test_gcd(None);
            }

            assert_eq!(add_io_space(GcdIoType::Io, 0x2000, 0x100), efi::Status::SUCCESS);
            assert_eq!(add_io_space(GcdIoType::Reserved, 0x2400, 0x80), efi::Status::SUCCESS);
            assert_eq!(add_io_space(GcdIoType::Io, 0x2600, 0x180), efi::Status::SUCCESS);

            let expected_count = GCD.io_descriptor_count();
            let mut expected: Vec<dxe_services::IoSpaceDescriptor> = Vec::with_capacity(expected_count + 10);
            GCD.get_io_descriptors(&mut expected).expect("get_io_descriptors failed");
            assert!(!expected.is_empty());

            let mut out_count: usize = 0;
            let mut out_ptr: *mut dxe_services::IoSpaceDescriptor = core::ptr::null_mut();
            let s = get_io_space_map(&mut out_count, &mut out_ptr);
            assert_eq!(s, efi::Status::SUCCESS);
            assert_eq!(out_count, expected.len());

            let out_slice = unsafe { core::slice::from_raw_parts(out_ptr, out_count) };
            assert_eq!(out_slice, expected.as_slice());

            assert!(crate::allocator::core_free_pool(out_ptr as *mut core::ffi::c_void).is_ok());
        });
    }

    #[test]
    fn test_set_memory_space_capabilities_success() {
        with_locked_state(|| {
            // Add a page-aligned region we can operate on
            let base = 0x2A00000;
            let length = 0x2000; // 2 pages
            assert_eq!(
                add_memory_space(GcdMemoryType::SystemMemory, base, length, efi::MEMORY_WB),
                efi::Status::SUCCESS
            );

            // Set a combination of reasonable capabilities
            let caps = efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP;
            let s = set_memory_space_capabilities(base, length, caps);
            assert_eq!(s, efi::Status::SUCCESS);

            // Verify capabilities include the requested bits
            let mut d = core::mem::MaybeUninit::<dxe_services::MemorySpaceDescriptor>::uninit();
            assert_eq!(get_memory_space_descriptor(base, d.as_mut_ptr()), efi::Status::SUCCESS);
            let d = unsafe { d.assume_init() };
            assert_eq!(d.base_address, base);
            assert!(d.capabilities & caps == caps, "Expected caps 0x{:x} to be set in 0x{:x}", caps, d.capabilities);
        });
    }

    #[test]
    fn test_set_memory_space_capabilities_zero_length_invalid_param() {
        with_locked_state(|| {
            let s = set_memory_space_capabilities(0x2B00000, 0, efi::MEMORY_WB);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_set_memory_space_capabilities_not_ready() {
        with_locked_state(|| {
            // Force GCD to an uninitialized state
            unsafe { GCD.reset() };
            let s = set_memory_space_capabilities(0x200000, 0x1000, efi::MEMORY_WB);
            assert_eq!(s, efi::Status::NOT_READY, "Expected NOT_READY when GCD is reset");
        });
    }

    #[test]
    fn test_set_memory_space_capabilities_unaligned_length() {
        with_locked_state(|| {
            // Add a valid region first
            let base = 0x2C00000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, 0x4000, efi::MEMORY_WB);
            // Use an unaligned length
            let s = set_memory_space_capabilities(base, 0x1234, efi::MEMORY_WB);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_set_memory_space_capabilities_unaligned_base() {
        with_locked_state(|| {
            // Add a valid region first
            let base = 0x2D00000;
            let _ = add_memory_space(GcdMemoryType::SystemMemory, base, 0x4000, efi::MEMORY_WB);
            // Use an unaligned base
            let s = set_memory_space_capabilities(base + 1, 0x1000, efi::MEMORY_WB);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_set_memory_space_capabilities_out_of_range_unsupported() {
        with_locked_state(|| {
            // The test GCD uses 48-bit physical address space
            let max_addr = 1u64 << 48;
            // Choose a base such that base + len > maximum_address
            let base = max_addr - 0x1000;
            let len = 0x2000;
            let s = set_memory_space_capabilities(base, len, efi::MEMORY_WB);
            assert_eq!(s, efi::Status::UNSUPPORTED);
        });
    }

    #[test]
    fn test_add_io_space_success_io() {
        with_locked_state(|| {
            // Initialize GCD (sets IO address bits = 16)
            unsafe { crate::test_support::init_test_gcd(None) };

            let base = 0x2000u64;
            let len = 0x100u64;
            let s = add_io_space(GcdIoType::Io, base, len);
            assert_eq!(s, efi::Status::SUCCESS);

            // Verify via descriptor query
            let mut desc = core::mem::MaybeUninit::<dxe_services::IoSpaceDescriptor>::uninit();
            let s = get_io_space_descriptor(base, desc.as_mut_ptr());
            assert_eq!(s, efi::Status::SUCCESS);
            let desc = unsafe { desc.assume_init() };
            assert_eq!(desc.base_address, base);
            assert_eq!(desc.length, len);
            assert_eq!(desc.io_type, GcdIoType::Io);
        });
    }

    #[test]
    fn test_add_io_space_success_reserved() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };

            let base = 0x3000u64;
            let len = 0x80u64;
            let s = add_io_space(GcdIoType::Reserved, base, len);
            assert_eq!(s, efi::Status::SUCCESS);

            let mut desc = core::mem::MaybeUninit::<dxe_services::IoSpaceDescriptor>::uninit();
            assert_eq!(get_io_space_descriptor(base, desc.as_mut_ptr()), efi::Status::SUCCESS);
            let desc = unsafe { desc.assume_init() };
            assert_eq!(desc.base_address, base);
            assert_eq!(desc.length, len);
            assert_eq!(desc.io_type, GcdIoType::Reserved);
        });
    }

    #[test]
    fn test_add_io_space_zero_length_invalid_parameter() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let s = add_io_space(GcdIoType::Io, 0x1000, 0);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_add_io_space_out_of_range_unsupported() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            // IO address space is 16 bits in tests => maximum address 0x10000
            // Pick a range that exceeds the maximum
            let s = add_io_space(GcdIoType::Io, 0xFF80, 0x200);
            assert_eq!(s, efi::Status::UNSUPPORTED);
        });
    }

    #[test]
    fn test_add_io_space_not_ready() {
        with_locked_state(|| {
            // Force GCD to uninitialized state for IO
            unsafe { GCD.reset() };
            let s = add_io_space(GcdIoType::Io, 0x1000, 0x10);
            assert_eq!(s, efi::Status::NOT_READY);
        });
    }

    #[test]
    fn test_add_io_space_overlap_access_denied() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let base = 0x4000u64;
            let len = 0x100u64;
            assert_eq!(add_io_space(GcdIoType::Io, base, len), efi::Status::SUCCESS);
            // Overlapping add should be denied since region is no longer NonExistent
            let s = add_io_space(GcdIoType::Reserved, base, 0x80);
            assert_eq!(s, efi::Status::ACCESS_DENIED);
        });
    }

    #[test]
    fn test_allocate_io_space_null_base_ptr_invalid_parameter() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdIoType::Io,
                3,
                0x10,
                core::ptr::null_mut(),
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_allocate_io_space_not_ready() {
        with_locked_state(|| {
            unsafe { GCD.reset() };
            let mut out: efi::PhysicalAddress = 0;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdIoType::Io,
                3,
                0x10,
                &mut out,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::NOT_READY);
        });
    }

    #[test]
    fn test_allocate_io_space_zero_length_invalid_parameter() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            // Need an IO region present to allocate from, but length 0 should still fail early
            assert_eq!(add_io_space(GcdIoType::Io, 0x2000, 0x200), efi::Status::SUCCESS);
            let mut out: efi::PhysicalAddress = 0;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdIoType::Io,
                3,
                0,
                &mut out,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_ne!(s, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_allocate_io_space_null_image_handle_invalid_parameter() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(add_io_space(GcdIoType::Io, 0x2200, 0x200), efi::Status::SUCCESS);
            let mut out: efi::PhysicalAddress = 0;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdIoType::Io,
                3,
                0x20,
                &mut out,
                core::ptr::null_mut(), // null image handle should be invalid
                core::ptr::null_mut(),
            );
            assert_ne!(s, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_allocate_io_space_bottom_up_success_and_sets_base() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            // Prepare IO space to allocate from
            assert_eq!(add_io_space(GcdIoType::Io, 0x3000, 0x300), efi::Status::SUCCESS);

            let mut out: efi::PhysicalAddress = 0;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::AnySearchBottomUp,
                GcdIoType::Io,
                3,    // 8-byte alignment
                0x20, // 32 bytes
                &mut out,
                1 as _, // valid image handle
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::SUCCESS);
            assert!((0x3000..0x3300).contains(&out));
            assert_eq!(out & 0x7, 0, "alignment not respected");
        });
    }

    #[test]
    fn test_allocate_io_space_top_down_success() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(add_io_space(GcdIoType::Io, 0x4000, 0x400), efi::Status::SUCCESS);

            let mut out: efi::PhysicalAddress = 0;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::AnySearchTopDown,
                GcdIoType::Io,
                4,    // 16-byte alignment
                0x40, // 64 bytes
                &mut out,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::SUCCESS);
            assert!((0x4000..0x4400).contains(&out));
            assert_eq!(out & 0xF, 0);
        });
    }

    #[test]
    fn test_allocate_io_space_address_success() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(add_io_space(GcdIoType::Io, 0x5000, 0x200), efi::Status::SUCCESS);

            let mut desired: efi::PhysicalAddress = 0x5080;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::Address,
                GcdIoType::Io,
                0,    // no extra alignment
                0x20, // 32 bytes
                &mut desired,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::SUCCESS);
            assert_eq!(desired, 0x5080);
        });
    }

    #[test]
    fn test_allocate_io_space_address_unsupported_when_out_of_io_range() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let mut desired: efi::PhysicalAddress = 0xFF80;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::Address,
                GcdIoType::Io,
                0,
                0x200,
                &mut desired,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::UNSUPPORTED);
        });
    }

    #[test]
    fn test_allocate_io_space_max_address_bottom_up_respected() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(add_io_space(GcdIoType::Io, 0x6000, 0x400), efi::Status::SUCCESS);

            let mut limit: efi::PhysicalAddress = 0x6100;
            let s = allocate_io_space(
                dxe_services::GcdAllocateType::MaxAddressSearchBottomUp,
                GcdIoType::Io,
                3,
                0x20,
                &mut limit,
                1 as _,
                core::ptr::null_mut(),
            );
            assert_eq!(s, efi::Status::SUCCESS);
            assert!(limit <= 0x6100);
        });
    }

    #[test]
    fn test_free_io_space_success() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let base = 0x7000u64;
            let len = 0x80u64;
            assert_eq!(add_io_space(GcdIoType::Io, base, len), efi::Status::SUCCESS);

            let mut desired = base;
            assert_eq!(
                allocate_io_space(
                    dxe_services::GcdAllocateType::Address,
                    GcdIoType::Io,
                    0,
                    len,
                    &mut desired,
                    1 as _,
                    core::ptr::null_mut(),
                ),
                efi::Status::SUCCESS
            );

            let s = free_io_space(base, len);
            assert_eq!(s, efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_free_io_space_zero_length_invalid_parameter() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(free_io_space(0x1000, 0), efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_free_io_space_not_ready() {
        with_locked_state(|| {
            unsafe { GCD.reset() };
            let s = free_io_space(0x1000, 0x10);
            assert_eq!(s, efi::Status::NOT_READY);
        });
    }

    #[test]
    fn test_free_io_space_out_of_range_unsupported() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };

            let s = free_io_space(0xFF80, 0x200);
            assert_eq!(s, efi::Status::UNSUPPORTED);
        });
    }

    #[test]
    fn test_free_io_space_unallocated_not_found() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            // Add region but do not allocate
            assert_eq!(add_io_space(GcdIoType::Io, 0x8000, 0x100), efi::Status::SUCCESS);
            let s = free_io_space(0x8000, 0x20);
            assert_eq!(s, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_free_io_space_double_free() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(add_io_space(GcdIoType::Io, 0x9000, 0x100), efi::Status::SUCCESS);

            let mut desired: efi::PhysicalAddress = 0x9000;
            assert_eq!(
                allocate_io_space(
                    dxe_services::GcdAllocateType::Address,
                    GcdIoType::Io,
                    0,
                    0x40,
                    &mut desired,
                    1 as _,
                    core::ptr::null_mut(),
                ),
                efi::Status::SUCCESS
            );
            assert_eq!(free_io_space(0x9000, 0x40), efi::Status::SUCCESS);

            // second free should fail with NOT_FOUND
            assert_eq!(free_io_space(0x9000, 0x40), efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_free_io_space_partial_free() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(add_io_space(GcdIoType::Io, 0xA000, 0x100), efi::Status::SUCCESS);

            // allocate 0x80 bytes starting at 0xA000
            let mut desired: efi::PhysicalAddress = 0xA000;
            assert_eq!(
                allocate_io_space(
                    dxe_services::GcdAllocateType::Address,
                    GcdIoType::Io,
                    0,
                    0x80,
                    &mut desired,
                    1 as _,
                    core::ptr::null_mut(),
                ),
                efi::Status::SUCCESS
            );

            // Free first half
            assert_eq!(free_io_space(0xA000, 0x40), efi::Status::SUCCESS);
            // Free second half
            assert_eq!(free_io_space(0xA000 + 0x40, 0x40), efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_remove_io_space_success() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let base = 0xB000u64;
            let len = 0x80u64;
            assert_eq!(add_io_space(GcdIoType::Io, base, len), efi::Status::SUCCESS);
            assert_eq!(remove_io_space(base, len), efi::Status::SUCCESS);
        });
    }

    #[test]
    fn test_remove_io_space_zero_length_invalid_parameter() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(remove_io_space(0x1000, 0), efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_remove_io_space_not_ready() {
        with_locked_state(|| {
            unsafe { GCD.reset() };
            assert_eq!(remove_io_space(0x1000, 0x10), efi::Status::NOT_READY);
        });
    }

    #[test]
    fn test_remove_io_space_out_of_range_unsupported() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            // IO address space is 16 bits in tests => maximum address 0x10000
            assert_eq!(remove_io_space(0xFF80, 0x200), efi::Status::UNSUPPORTED);
        });
    }

    #[test]
    fn test_remove_io_space_not_found_when_never_added() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            assert_eq!(remove_io_space(0xC000, 0x40), efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_remove_io_space_double_remove_not_found() {
        with_locked_state(|| {
            unsafe { crate::test_support::init_test_gcd(None) };
            let base = 0xE000u64;
            let len = 0x40u64;
            assert_eq!(add_io_space(GcdIoType::Io, base, len), efi::Status::SUCCESS);
            assert_eq!(remove_io_space(base, len), efi::Status::SUCCESS);
            assert_eq!(remove_io_space(base, len), efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_dispatch_returns_not_found_when_nothing_to_do() {
        with_locked_state(|| {
            let s = dispatch();
            assert_eq!(s, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_dispatch_is_idempotent_and_consistently_not_found() {
        with_locked_state(|| {
            let s1 = dispatch();
            let s2 = dispatch();
            assert_eq!(s1, efi::Status::NOT_FOUND);
            assert_eq!(s2, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_dispatch_with_installed_fv_still_not_found() {
        use crate::test_collateral;
        use std::{fs::File, io::Read};

        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            unsafe { crate::test_support::init_test_protocol_db() };

            // Install the FV to obtain a real handle
            let _handle = unsafe { crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap() };

            // Wrapper should still surface NOT_FOUND (no pending drivers to dispatch in tests)
            let s = dispatch();
            assert_eq!(s, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_schedule_invalid_parameter_when_file_is_null() {
        with_locked_state(|| {
            // Passing a null file GUID pointer should return INVALID_PARAMETER
            let s = schedule(core::ptr::null_mut(), core::ptr::null());
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_schedule_not_found_without_pending_drivers() {
        with_locked_state(|| {
            // Any GUID is fine; there are no pending drivers in this test harness
            let guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
            let s = schedule(core::ptr::null_mut(), &guid);
            assert_eq!(s, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_schedule_with_installed_fv_returns_not_found() {
        use crate::test_collateral;
        use std::{fs::File, io::Read};
        use uuid::Uuid;

        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            unsafe { crate::test_support::init_test_protocol_db() };

            // Install the FV to obtain a real handle
            let handle = unsafe { crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap() };

            // Use the same GUID as the dispatcher tests; wrapper should map NotFound correctly
            let file_guid = efi::Guid::from_bytes(Uuid::from_u128(0x1fa1f39e_feff_4aae_bd7b_38a070a3b609).as_bytes());
            let s = schedule(handle, &file_guid);
            assert_eq!(s, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_trust_invalid_parameter_when_file_is_null() {
        with_locked_state(|| {
            let s = trust(core::ptr::null_mut(), core::ptr::null());
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_trust_not_found_without_pending_drivers() {
        with_locked_state(|| {
            // Any GUID and handle are fine; there are no pending drivers in this harness
            let guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[1, 2, 3, 4, 5, 6]);
            let s = trust(core::ptr::null_mut(), &guid);
            assert_eq!(s, efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn test_process_firmware_volume_invalid_parameters() {
        with_locked_state(|| {
            let mut out_handle: efi::Handle = core::ptr::null_mut();

            // Null header
            let s = process_firmware_volume(core::ptr::null(), 0, &mut out_handle);
            assert_eq!(s, efi::Status::INVALID_PARAMETER);

            // Null output handle pointer
            let bogus = 0xDEAD_BEEFu64 as *const core::ffi::c_void;
            let s = process_firmware_volume(bogus, 0, core::ptr::null_mut());
            assert_eq!(s, efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn test_process_firmware_volume_volume_corrupted_on_bad_input() {
        with_locked_state(|| {
            // Provide a tiny, obviously invalid buffer
            let bad_buf: [u8; 16] = [0u8; 16];
            let mut out_handle: efi::Handle = core::ptr::null_mut();
            let s =
                process_firmware_volume(bad_buf.as_ptr() as *const core::ffi::c_void, bad_buf.len(), &mut out_handle);
            assert_eq!(s, efi::Status::VOLUME_CORRUPTED);
        });
    }

    #[test]
    fn test_process_firmware_volume_success_with_real_fv() {
        use crate::test_collateral;
        use std::{fs::File, io::Read};

        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            // Ensure protocol DB is ready for installing the FV
            unsafe { crate::test_support::init_test_protocol_db() };

            let mut out_handle: efi::Handle = core::ptr::null_mut();
            let s = process_firmware_volume(fv.as_ptr() as *const core::ffi::c_void, fv.len(), &mut out_handle);

            assert_eq!(s, efi::Status::SUCCESS);
            assert!(!out_handle.is_null(), "process_firmware_volume should return a valid handle");
        });
    }

    #[test]
    fn test_init_dxe_services_installs_config_table_with_valid_crc_and_functions() {
        with_locked_state(|| {
            // Initialize a fresh System Table (requires GCD already initialized by with_locked_state)
            crate::systemtables::init_system_table();

            // Get a mutable reference to the system table
            let mut st_guard = crate::systemtables::SYSTEM_TABLE.lock();
            let st = st_guard.as_mut().expect("System Table not initialized");

            // Before: no configuration tables are expected on a fresh init
            assert_eq!(st.system_table().number_of_table_entries, 0);

            // Act: install the DXE Services table
            init_dxe_services(st);

            // After: one entry should exist and match DXE_SERVICES_TABLE_GUID
            let st_ref = st.system_table();
            assert_eq!(st_ref.number_of_table_entries, 1);
            assert!(!st_ref.configuration_table.is_null());

            let entries =
                unsafe { core::slice::from_raw_parts(st_ref.configuration_table, st_ref.number_of_table_entries) };

            let entry = entries
                .iter()
                .find(|e| e.vendor_guid == dxe_services::DXE_SERVICES_TABLE_GUID)
                .expect("DXE Services table entry not found in configuration table");
            assert!(!entry.vendor_table.is_null(), "DXE Services vendor_table pointer should be non-null");

            // Validate the contents of the installed DXE Services table
            let dxe_tbl = unsafe { &*(entry.vendor_table as *const dxe_services::DxeServicesTable) };

            // Header signature/revision should match what init_dxe_services sets
            assert_eq!(dxe_tbl.header.signature, efi::BOOT_SERVICES_SIGNATURE);
            assert_eq!(dxe_tbl.header.revision, efi::BOOT_SERVICES_REVISION);

            // Recompute CRC32 by zeroing the field in a local copy
            let mut copy = unsafe { core::ptr::read(dxe_tbl) };
            copy.header.crc32 = 0;
            let crc = crc32fast::hash(unsafe {
                core::slice::from_raw_parts(
                    (&copy as *const dxe_services::DxeServicesTable) as *const u8,
                    core::mem::size_of::<dxe_services::DxeServicesTable>(),
                )
            });
            assert_eq!(dxe_tbl.header.crc32, crc, "DXE Services table CRC32 should be valid");

            // Spot-check a few function pointers are correctly wired
            assert_eq!(dxe_tbl.add_memory_space as usize, add_memory_space as usize);
            assert_eq!(dxe_tbl.dispatch as usize, dispatch as usize);
            assert_eq!(dxe_tbl.process_firmware_volume as usize, process_firmware_volume as usize);
        });
    }
}
