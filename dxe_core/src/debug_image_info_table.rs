//! EFI_DEBUG_IMAGE_INFO_TABLE Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;
use alloc::boxed::Box;
use uefi_sdk::{base::UEFI_PAGE_SIZE, error::EfiError};

use core::{
    ffi::c_void,
    fmt::Debug,
    mem::size_of,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use crate::{
    allocator::{core_allocate_pool, core_free_pool},
    misc_boot_services::core_install_configuration_table,
    systemtables::EfiSystemTable,
};
use r_efi::efi;

pub const EFI_DEBUG_IMAGE_INFO_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x49152e77, 0x1ada, 0x4764, 0xb7, 0xa2, &[0x7a, 0xfe, 0xfe, 0xd9, 0x5e, 0x8b]);

#[repr(C)]
#[derive(Debug)]
pub struct DebugImageInfoTableHeader {
    // This is made not pub to force volatile access to the field, per UEFI spec
    update_status: u32,
    pub table_size: u32,
    pub efi_debug_image_info_table: *mut u32,
}

impl DebugImageInfoTableHeader {
    pub fn new(initial_table_size: u32) -> Result<Self, EfiError> {
        Ok(Self {
            update_status: 0,
            table_size: 0,
            efi_debug_image_info_table: core_allocate_pool(
                efi::BOOT_SERVICES_DATA,
                size_of::<EfiDebugImageInfo>() * initial_table_size as usize,
            )
            .map_err(|_| EfiError::OutOfResources)? as *mut u32,
        })
    }

    fn get_entry(&mut self, index: u32) -> &mut EfiDebugImageInfo {
        unsafe {
            let table_ptr = self.efi_debug_image_info_table as *mut EfiDebugImageInfo;
            &mut *table_ptr.add(index as usize)
        }
    }

    pub unsafe fn get_update_status(&self) -> u32 {
        ptr::read_volatile(&self.update_status)
    }

    pub unsafe fn set_update_status(&mut self, status: u32) {
        ptr::write_volatile(&mut self.update_status as *mut u32, status)
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct EfiDebugImageInfoNormal {
    pub image_info_type: u32,
    pub loaded_image_protocol_instance: *const efi::protocols::loaded_image::Protocol,
    pub image_handle: efi::Handle,
}

#[repr(C)]
union EfiDebugImageInfo {
    image_info_type: *const u32,
    normal_image: *mut EfiDebugImageInfoNormal,
}

struct DebugImageInfoTableMetadata<'a> {
    actual_table_size: u32,
    table: &'a mut DebugImageInfoTableHeader,
}

pub const EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS: u32 = 0x1;
pub const EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED: u32 = 0x2;

pub const EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL: u32 = 0x1;

static METADATA_TABLE: AtomicPtr<DebugImageInfoTableMetadata> = AtomicPtr::new(core::ptr::null_mut());

pub(crate) fn initialize_debug_image_info_table(system_table: &mut EfiSystemTable) {
    let debug_image_info_table_header = match DebugImageInfoTableHeader::new(128) {
        Ok(table) => Box::new(table),
        Err(_) => return,
    };

    let table_ptr = Box::into_raw(debug_image_info_table_header) as *mut c_void;
    unsafe {
        match core_install_configuration_table(EFI_DEBUG_IMAGE_INFO_TABLE_GUID, table_ptr.as_mut(), system_table) {
            Ok(_) => {
                // Successfully installed the configuration table
            }
            Err(_) => {
                log::error!("Failed to install configuration table for EFI_DEBUG_IMAGE_INFO_TABLE_GUID");
                return;
            }
        };
    }

    let table = Box::new(DebugImageInfoTableMetadata {
        actual_table_size: 128,
        table: unsafe { &mut *table_ptr.cast::<DebugImageInfoTableHeader>() },
    });
    METADATA_TABLE.store(Box::into_raw(table), Ordering::SeqCst);
}

pub(crate) fn core_new_debug_image_info_entry(
    image_info_type: u32,
    loaded_image_protocol_instance: *const efi::protocols::loaded_image::Protocol,
    image_handle: efi::Handle,
) {
    // This is a very funny check for null because it is working around an LLVM bug where checking is_null() or variations
    // of that on a load of an atomic pointer causes improper code generation and LLVM to crash. So, this check is a workaround
    // to check if the pointer is in the first page of memory, which is a valid check for null in this case, as we mark
    // that entire page as invalid.
    let metadata_table = METADATA_TABLE.load(Ordering::SeqCst) as *mut DebugImageInfoTableMetadata;
    if metadata_table < UEFI_PAGE_SIZE as *mut DebugImageInfoTableMetadata {
        log::error!("EFI_DEBUG_IMAGE_INFO_TABLE_GUID table not initialized");
        return;
    }

    // SAFETY: This is safe because we check that the table is initialized above
    let metadata_table = unsafe { &mut *(metadata_table) };

    // per UEFI spec, need to mark the table is being updated and preserve the modified bit if set
    let update_status = unsafe { metadata_table.table.get_update_status() };
    unsafe { metadata_table.table.set_update_status(update_status | EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS) };

    // create our new table
    if metadata_table.table.table_size >= metadata_table.actual_table_size {
        // We need to allocate more space for the table
        let new_table_size = metadata_table.table.table_size + 128;
        let old_table_size = metadata_table.table.table_size;
        let new_table =
            match core_allocate_pool(efi::BOOT_SERVICES_DATA, size_of::<EfiDebugImageInfo>() * new_table_size as usize)
            {
                Ok(table) => table as *mut u32,
                Err(_) => return,
            };

        // Copy the old table to the new one
        unsafe {
            ptr::copy_nonoverlapping(
                metadata_table.table.efi_debug_image_info_table,
                new_table,
                size_of::<EfiDebugImageInfo>() * metadata_table.table.table_size as usize,
            );
            match core_free_pool(metadata_table.table.efi_debug_image_info_table as *mut c_void) {
                Ok(_) => {
                    // Successfully freed the old table
                }
                Err(_) => {
                    log::error!("Failed to free old EFI_DEBUG_IMAGE_INFO_TABLE_GUID table memory");
                    debug_assert!(false);
                    // continue even if we fail to free the old table
                }
            }
        }

        // Update the table pointer and size, using the old size, as table_size is the number of entries
        metadata_table.table.efi_debug_image_info_table = new_table;
        metadata_table.table.table_size = old_table_size;
    }

    // size here is last_index + 1
    let debug_image_info = metadata_table.table.get_entry(metadata_table.table.table_size);
    debug_image_info.normal_image =
        match core_allocate_pool(efi::BOOT_SERVICES_DATA, size_of::<EfiDebugImageInfoNormal>()) {
            Ok(image_info) => image_info as *mut EfiDebugImageInfoNormal,
            Err(_) => return,
        };
    let debug_image_info_table = unsafe { &mut *debug_image_info.normal_image };
    debug_image_info_table.image_info_type = image_info_type;
    debug_image_info_table.loaded_image_protocol_instance = loaded_image_protocol_instance;
    debug_image_info_table.image_handle = image_handle;
    metadata_table.table.table_size += 1;

    log::error!("EFI_DEBUG_IMAGE_INFO_TABLE_GUID table size: {}", metadata_table.table.table_size);

    unsafe {
        let update_status = metadata_table.table.get_update_status();
        metadata_table.table.set_update_status(
            (update_status & !EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS) | EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
        )
    };
}

pub(crate) fn core_remove_debug_image_info_entry(image_handle: efi::Handle) {
    let metadata_table = METADATA_TABLE.load(Ordering::SeqCst) as *mut DebugImageInfoTableMetadata;
    if metadata_table < UEFI_PAGE_SIZE as *mut DebugImageInfoTableMetadata {
        log::error!("EFI_DEBUG_IMAGE_INFO_TABLE_GUID table not initialized");
        return;
    }

    // SAFETY: This is safe because we check that the table is initialized above
    let metadata_table = unsafe { &mut *(metadata_table) };

    // per UEFI spec, need to mark the table is being updated and preserve the modified bit if set
    let update_status = unsafe { metadata_table.table.get_update_status() };
    unsafe { metadata_table.table.set_update_status(update_status | EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS) };

    let table_size = metadata_table.table.table_size;

    let last_debug_image_info_table;
    {
        let last_debug_image_info = metadata_table.table.get_entry(table_size - 1);
        last_debug_image_info_table = unsafe { last_debug_image_info.normal_image };
    }

    // find the entry to remove
    for i in 0..table_size {
        let debug_image_info_table = {
            let debug_image_info = metadata_table.table.get_entry(i);
            Some(unsafe { &*debug_image_info.normal_image })
        };
        if let Some(debug_image_info_table) = debug_image_info_table {
            if debug_image_info_table.image_handle == image_handle {
                // free the entry
                match core_free_pool(debug_image_info_table as *const _ as *mut c_void) {
                    Ok(_) => {
                        // Successfully freed the old table
                    }
                    Err(_) => {
                        log::error!("Failed to free old EFI_DEBUG_IMAGE_INFO_TABLE_GUID table memory");
                        debug_assert!(false);
                        // continue even if we fail to free the old table
                    }
                }

                // we don't care about the order of the table, so just move the last entry to this one
                // and decrement the size of the table
                if i != table_size - 1 {
                    let debug_image_info = metadata_table.table.get_entry(i);
                    debug_image_info.normal_image = last_debug_image_info_table;
                    let last_debug_image_info = metadata_table.table.get_entry(table_size - 1);
                    last_debug_image_info.normal_image = core::ptr::null_mut();
                }
                metadata_table.table.table_size -= 1;
                break;
            }
        }
    }

    log::error!("EFI_DEBUG_IMAGE_INFO_TABLE_GUID table size: {}", metadata_table.table.table_size);

    unsafe {
        let update_status = metadata_table.table.get_update_status();
        metadata_table.table.set_update_status(
            (update_status & !EFI_DEBUG_IMAGE_INFO_UPDATE_IN_PROGRESS) | EFI_DEBUG_IMAGE_INFO_TABLE_MODIFIED,
        )
    };
}
