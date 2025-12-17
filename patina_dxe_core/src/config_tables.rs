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
use core::{
    ffi::c_void,
    ptr::{NonNull, slice_from_raw_parts_mut},
    slice::from_raw_parts,
};
use patina::error::EfiError;
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

    // Safety: caller must ensure that table_guid is a valid pointer. It is null-checked above.
    let table_guid = unsafe { table_guid.read_unaligned() };

    let mut st_guard = SYSTEM_TABLE.lock();
    let st = match st_guard.as_mut() {
        Some(st) => st,
        None => return efi::Status::NOT_FOUND,
    };

    match core_install_configuration_table(table_guid, table, st) {
        Err(err) => err.into(),
        Ok(_) => efi::Status::SUCCESS,
    }
}

/// Install a configuration table in the system table, replacing any existing table with the same GUID.
/// If a table is replaced or deleted, a pointer to the old table is returned.
pub fn core_install_configuration_table(
    vendor_guid: efi::Guid,
    vendor_table: *mut c_void,
    efi_system_table: &mut EfiSystemTable,
) -> Result<Option<NonNull<c_void>>, EfiError> {
    let system_table = efi_system_table.as_mut();
    //if a table is already present, reconstruct it from the pointer and length in the st.
    let old_cfg_table = if system_table.configuration_table.is_null() {
        assert_eq!(system_table.number_of_table_entries, 0);
        None
    } else {
        let ct_slice_box = unsafe {
            Box::from_raw_in(
                slice_from_raw_parts_mut(system_table.configuration_table, system_table.number_of_table_entries),
                &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
            )
        };
        Some(ct_slice_box)
    };

    let mut old_vendor_table_ptr = None;
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
                    old_vendor_table_ptr = NonNull::new(entry.vendor_table);
                    entry.vendor_table = vendor_table;
                } else {
                    //entry doesn't exist, add it.
                    current_table.push(efi::ConfigurationTable { vendor_guid, vendor_table });
                }
            } else {
                //vendor_table is none; we are deleting an entry.
                if let Some(entry) = existing_entry {
                    //entry exists, we can delete it
                    old_vendor_table_ptr = NonNull::new(entry.vendor_table);
                    current_table.retain(|x| x.vendor_guid != vendor_guid);
                } else {
                    //entry does not exist, we can't delete it. We have to put the original box back
                    //in the config table so it doesn't get dropped though. Pointer should be the same
                    //so we should not need to recompute CRC.
                    system_table.configuration_table =
                        Box::into_raw_with_allocator(cfg_table).0 as *mut efi::ConfigurationTable;
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
        system_table.configuration_table = Box::into_raw_with_allocator(new_table).0 as *mut efi::ConfigurationTable;
    }
    //since we modified the system table, re-calculate CRC.
    efi_system_table.checksum();

    //signal the table guid as an event group
    EVENT_DB.signal_group(vendor_guid);

    Ok(old_vendor_table_ptr)
}

/// Returns the pointer to a configuration table for the specified guid, if it exists.
pub fn get_configuration_table(table_guid: &efi::Guid) -> Option<NonNull<c_void>> {
    let st_guard = SYSTEM_TABLE.lock();
    let st = st_guard.as_ref()?;

    let system_table = st.as_ref();
    if system_table.configuration_table.is_null() || system_table.number_of_table_entries == 0 {
        return None;
    }

    // Safety: system table exists, and configuration is non-null, and number_of_table_entries is non-zero.
    let ct_slice = unsafe { from_raw_parts(system_table.configuration_table, system_table.number_of_table_entries) };

    for entry in ct_slice {
        if entry.vendor_guid == *table_guid {
            return NonNull::new(entry.vendor_table);
        }
    }
    None
}

pub fn init_config_tables_support(bs: &mut efi::BootServices) {
    bs.install_configuration_table = install_configuration_table;
}

#[cfg(test)]
mod tests {
    use patina::base::guid;

    use crate::{systemtables::init_system_table, test_support};

    use super::*;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                test_support::init_test_gcd(None);
                test_support::reset_allocators();
                init_system_table();
            }
            f();
        })
        .unwrap();
    }

    #[test]
    fn install_configuration_table_should_install_table() {
        with_locked_state(|| {
            let guid: efi::Guid = guid::Guid::from_string("78926ab0-af16-49e4-8e05-115aafbca1df").to_efi_guid();
            let table = 0x12345678u32 as *mut c_void;

            assert!(get_configuration_table(&guid).is_none());

            assert_eq!(install_configuration_table(&guid as *const _ as *mut _, table), efi::Status::SUCCESS);
            assert_eq!(get_configuration_table(&guid).unwrap().as_ptr(), table);
        });
    }

    #[test]
    fn delete_config_table_should_return_ptr() {
        with_locked_state(|| {
            let guid: efi::Guid = guid::Guid::from_string("78926ab0-af16-49e4-8e05-115aafbca1df").to_efi_guid();
            let table = 0x12345678u32 as *mut c_void;

            assert_eq!(install_configuration_table(&guid as *const _ as *mut _, table), efi::Status::SUCCESS);

            assert_eq!(get_configuration_table(&guid).unwrap().as_ptr(), table);

            assert_eq!(
                core_install_configuration_table(
                    guid,
                    core::ptr::null_mut(),
                    &mut *SYSTEM_TABLE.lock().as_mut().unwrap()
                ),
                Ok(Some(NonNull::new(table).unwrap()))
            );

            assert!(get_configuration_table(&guid).is_none());
        });
    }
}
