//! Core Provided Configuration Tables
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub(crate) mod memory_attributes_table;

use alloc::{boxed::Box, vec};
use core::{
    ffi::c_void,
    ptr::{NonNull, slice_from_raw_parts_mut},
    slice::{from_raw_parts, from_raw_parts_mut},
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

    // SAFETY: caller must ensure that table_guid is a valid pointer. It is null-checked above.
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
    let mut system_table = efi_system_table.get();

    let (updated_table, old_vendor_table_ptr) = match system_table.configuration_table {
        existing_tbl_ptr if existing_tbl_ptr.is_null() => {
            // existing table is empty.
            if vendor_table.is_null() {
                // trying to delete a non-existing table
                return Err(EfiError::NotFound);
            } else {
                // adding a new table to an empty configuration table list
                (vec![efi::ConfigurationTable { vendor_guid, vendor_table }], None)
            }
        }
        existing_table_ptr => {
            // existing table is present. Make a copy of it as a Vec to process the updates.
            // SAFETY: existing_table_ptr is non-null, and number_of_table_entries is valid.
            let mut updated_table =
                unsafe { from_raw_parts_mut(existing_table_ptr, system_table.number_of_table_entries).to_vec() };
            let existing_entry = updated_table.iter_mut().find(|x| x.vendor_guid == vendor_guid);
            if vendor_table.is_null() {
                // deleting an entry.
                if let Some(entry) = existing_entry {
                    //entry exists, we can delete it
                    let old_vendor_table_ptr = NonNull::new(entry.vendor_table);
                    updated_table.retain(|x| x.vendor_guid != vendor_guid);
                    (updated_table, old_vendor_table_ptr)
                } else {
                    //entry doesn't exist, can't delete it.
                    return Err(EfiError::NotFound);
                }
            } else {
                // adding or modifying an entry.
                if let Some(entry) = existing_entry {
                    //entry exists, modify it.
                    let old_vendor_table_ptr = NonNull::new(entry.vendor_table);
                    entry.vendor_table = vendor_table;
                    (updated_table, old_vendor_table_ptr)
                } else {
                    //entry doesn't exist, add it.
                    updated_table.push(efi::ConfigurationTable { vendor_guid, vendor_table });
                    (updated_table, None)
                }
            }
        }
    };

    // Updating the table. Reclaim the old table (if present) so it'll get dropped by the runtime allocator.
    if !system_table.configuration_table.is_null() {
        unsafe {
            let _old_boxed_table = Box::from_raw_in(
                slice_from_raw_parts_mut(system_table.configuration_table, system_table.number_of_table_entries),
                &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
            );
        }
    }

    if updated_table.is_empty() {
        system_table.number_of_table_entries = 0;
        system_table.configuration_table = core::ptr::null_mut();
    } else {
        // Move the updated table into an allocation in the runtime services data allocator.
        system_table.number_of_table_entries = updated_table.len();
        let updated_table = updated_table.to_vec_in(&EFI_RUNTIME_SERVICES_DATA_ALLOCATOR).into_boxed_slice();
        system_table.configuration_table =
            Box::into_raw_with_allocator(updated_table).0 as *mut efi::ConfigurationTable;
    }

    efi_system_table.set(system_table);

    //signal the table guid as an event group
    EVENT_DB.signal_group(vendor_guid);

    Ok(old_vendor_table_ptr)
}

/// Returns the pointer to a configuration table for the specified guid, if it exists.
pub fn get_configuration_table(table_guid: &efi::Guid) -> Option<NonNull<c_void>> {
    let st_guard = SYSTEM_TABLE.lock();
    let st = st_guard.as_ref()?;

    let system_table = st.get();
    if system_table.configuration_table.is_null() || system_table.number_of_table_entries == 0 {
        return None;
    }

    // SAFETY: system table exists, and configuration is non-null, and number_of_table_entries is non-zero.
    let ct_slice = unsafe { from_raw_parts(system_table.configuration_table, system_table.number_of_table_entries) };

    for entry in ct_slice {
        if entry.vendor_guid == *table_guid {
            return NonNull::new(entry.vendor_table);
        }
    }
    None
}

pub fn init_config_tables_support(st: &mut EfiSystemTable) {
    let mut bs = st.boot_services().get();
    bs.install_configuration_table = install_configuration_table;
    st.boot_services().set(bs);
}

#[cfg(test)]
mod tests {
    use patina::base::guid;

    use crate::{systemtables::init_system_table, test_support};

    use super::*;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            // SAFETY: multiple functions modify global state. Functions are
            // called within a global lock to ensure exclusive access during
            // initialization.
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
