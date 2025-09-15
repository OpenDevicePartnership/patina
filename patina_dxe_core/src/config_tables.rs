//! Core Provided Configuration Tables
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub(crate) mod debug_image_info_table;
pub(crate) mod memory_attributes_table;

use alloc::{boxed::Box, vec};
use core::{ffi::c_void, slice::from_raw_parts_mut};
use patina_sdk::error::EfiError;
use r_efi::efi;

use crate::{
    allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
    events::EVENT_DB,
    systemtables::{EfiSystemTable, SYSTEM_TABLE},
};

extern "efiapi" fn install_configuration_table(table_guid: *mut efi::Guid, table: *mut c_void) -> efi::Status {
    if table_guid.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let table_guid = unsafe { *table_guid };

    let mut st_guard = SYSTEM_TABLE.lock();
    let st = match st_guard.as_mut() {
        Some(st) => st,
        None => return efi::Status::NOT_FOUND,
    };

    match core_install_configuration_table(table_guid, table, st) {
        Err(err) => err.into(),
        Ok(()) => efi::Status::SUCCESS,
    }
}

pub fn core_install_configuration_table(
    vendor_guid: efi::Guid,
    vendor_table: *mut c_void,
    efi_system_table: &mut EfiSystemTable,
) -> Result<(), EfiError> {
    let system_table = efi_system_table.as_mut();
    //if a table is already present, reconstruct it from the pointer and length in the st.
    let old_cfg_table = if system_table.configuration_table.is_null() {
        assert_eq!(system_table.number_of_table_entries, 0);
        None
    } else {
        let ct_slice_box = unsafe {
            Box::from_raw_in(
                from_raw_parts_mut(system_table.configuration_table, system_table.number_of_table_entries),
                &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
            )
        };
        Some(ct_slice_box)
    };

    // construct the new table contents as a vector.
    let new_table = match old_cfg_table {
        Some(cfg_table) => {
            // a configuration table list is already present.
            let mut current_table = cfg_table.to_vec();
            let existing_entry = current_table.iter_mut().find(|x| x.vendor_guid == vendor_guid);
            if !vendor_table.is_null() {
                // vendor_table is not null; we are adding or modifying an entry.
                if let Some(entry) = existing_entry {
                    //entry exists, modify it.
                    entry.vendor_table = vendor_table;
                } else {
                    //entry doesn't exist, add it.
                    current_table.push(efi::ConfigurationTable { vendor_guid, vendor_table });
                }
            } else {
                //vendor_table is none; we are deleting an entry.
                if let Some(_entry) = existing_entry {
                    //entry exists, we can delete it
                    current_table.retain(|x| x.vendor_guid != vendor_guid);
                } else {
                    //entry does not exist, we can't delete it. We have to put the original box back
                    //in the config table so it doesn't get dropped though. Pointer should be the same
                    //so we should not need to recompute CRC.
                    system_table.configuration_table = Box::into_raw(cfg_table) as *mut efi::ConfigurationTable;
                    return Err(EfiError::NotFound);
                }
            }
            current_table
        }
        None => {
            // config table list doesn't exist.
            if !vendor_table.is_null() {
                // table is some, meaning we should create the list and add this as the new entry.
                vec![efi::ConfigurationTable { vendor_guid, vendor_table }]
            } else {
                // table is null, but can't delete a table entry in a list that doesn't exist.
                //since the list doesn't exist, we can leave the (null) pointer in the st alone.
                return Err(EfiError::NotFound);
            }
        }
    };

    if new_table.is_empty() {
        // if empty, just set config table ptr to null
        system_table.number_of_table_entries = 0;
        system_table.configuration_table = core::ptr::null_mut();
    } else {
        //Box up the new table and put it in the system table. The old table (if any) will be dropped
        //when old_cfg_table goes out of scope at the end of the function.
        system_table.number_of_table_entries = new_table.len();
        let new_table = new_table.to_vec_in(&EFI_RUNTIME_SERVICES_DATA_ALLOCATOR).into_boxed_slice();
        system_table.configuration_table = Box::into_raw(new_table) as *mut efi::ConfigurationTable;
    }
    //since we modified the system table, re-calculate CRC.
    efi_system_table.checksum();

    //signal the table guid as an event group
    EVENT_DB.signal_group(vendor_guid);

    Ok(())
}

pub fn init_config_tables_support(bs: &mut efi::BootServices) {
    bs.install_configuration_table = install_configuration_table;
}
