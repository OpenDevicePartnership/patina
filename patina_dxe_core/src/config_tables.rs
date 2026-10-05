//! Core Provided Configuration Tables
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub(crate) mod memory_attributes_table;

use alloc::{boxed::Box, collections::btree_map::BTreeMap, vec};
use core::{
    any::TypeId,
    ffi::c_void,
    ptr::{NonNull, slice_from_raw_parts_mut},
    slice::{from_raw_parts, from_raw_parts_mut},
};
use patina::BinaryGuid;
use patina::error::EfiError;
use patina::standard::efi;

use crate::{
    allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
    events::EVENT_DB,
    systemtables::{EfiSystemTable, SYSTEM_TABLE},
    tpl_mutex::TplMutex,
};

/// Records the Rust type installed under each GUID by [`core_install_typed_configuration_table`], so
/// a typed lookup (`crate::uefi_services::config_table`) can verify a type before a caller casts the
/// pointer.
///
/// [`core_install_configuration_table`] and [`core_install_typed_configuration_table`] are the only
/// two functions that mutate the system table's configuration table list, all other callers like
/// the `extern "efiapi"` boot service, internal core callers, and typed Rust APIs funnel through one
/// or the other.
pub(crate) static CONFIG_TABLE_TYPES: TplMutex<BTreeMap<BinaryGuid, TypeId>> =
    TplMutex::new(efi::TPL_NOTIFY, BTreeMap::new(), "ConfigTableTypeLock");

/// Installs a configuration table entry identified by `table_guid` into the system table.
///
/// # Safety
///
/// The caller is responsible for ensuring that `table_guid` points to valid, readable memory
/// containing an `efi::Guid` and that `table` (if non-null) points to valid memory that remains
/// valid for the lifetime of the configuration table entry. `table_guid` is null-checked, but
/// validity of the referenced memory is the caller's responsibility.
unsafe extern "efiapi" fn install_configuration_table(table_guid: *mut efi::Guid, table: *mut c_void) -> efi::Status {
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

/// Shared low-level mutation of the system table's configuration table list. Does not touch
/// [`CONFIG_TABLE_TYPES`]. Callers decide how (if at all) the type-tracking map should be reconciled
/// with the result. This function is not marked as unsafe because the `vendor_table` parameter is not
/// dereferenced inside the function.
fn install_configuration_table_entry(
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
            }
            // adding a new table to an empty configuration table list
            (vec![efi::ConfigurationTable { vendor_guid, vendor_table }], None)
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
        // SAFETY: configuration_table points to number_of_table_entries elements from the runtime allocator.
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

/// Install a configuration table in the system table, replacing any existing table with the same
/// GUID. If a table is replaced or deleted, a pointer to the old table is returned.
///
/// This call does not assert a Rust type for `vendor_table`, so it clears any type previously
/// recorded for `vendor_guid` by [`core_install_typed_configuration_table`]. Whatever ends up
/// under `vendor_guid` after this call is no longer vouched for by the type-tracking map, since
/// neither the raw `extern "efiapi"` boot service nor this untyped API knows what Rust type (if any)
/// backs it.
pub fn core_install_configuration_table(
    vendor_guid: efi::Guid,
    vendor_table: *mut c_void,
    efi_system_table: &mut EfiSystemTable,
) -> Result<Option<NonNull<c_void>>, EfiError> {
    let old_vendor_table_ptr = install_configuration_table_entry(vendor_guid, vendor_table, efi_system_table)?;
    CONFIG_TABLE_TYPES.lock().remove(&BinaryGuid::from(vendor_guid));
    Ok(old_vendor_table_ptr)
}

/// Returns whether `efi_system_table`'s configuration table list currently has an active entry for
/// `vendor_guid`. Reads the system table directly instead of going through [`get_configuration_table`],
/// which independently locks [`SYSTEM_TABLE`]. This allows the caller to this function to already
/// hold the lock.
fn configuration_table_has_entry(efi_system_table: &mut EfiSystemTable, vendor_guid: efi::Guid) -> bool {
    let system_table = efi_system_table.get();
    if system_table.configuration_table.is_null() {
        return false;
    }
    // SAFETY: configuration_table is non-null, and number_of_table_entries describes its length.
    let ct_slice = unsafe { from_raw_parts(system_table.configuration_table, system_table.number_of_table_entries) };
    ct_slice.iter().any(|entry| entry.vendor_guid == vendor_guid)
}

/// Same as [`core_install_configuration_table`], but records `type_id` as the Rust type backing
/// `vendor_guid`'s table, atomically with the install, so a later typed lookup can verify it before
/// casting the pointer. If `fail_if_exists` is set, fails with `EfiError::AlreadyStarted` rather than
/// replacing a table (typed or not) that exists under `vendor_guid`.
pub(crate) fn core_install_typed_configuration_table(
    vendor_guid: efi::Guid,
    vendor_table: *mut c_void,
    type_id: TypeId,
    fail_if_exists: bool,
    efi_system_table: &mut EfiSystemTable,
) -> Result<Option<NonNull<c_void>>, EfiError> {
    if fail_if_exists && configuration_table_has_entry(efi_system_table, vendor_guid) {
        return Err(EfiError::AlreadyStarted);
    }
    let old_vendor_table_ptr = install_configuration_table_entry(vendor_guid, vendor_table, efi_system_table)?;
    CONFIG_TABLE_TYPES.lock().insert(BinaryGuid::from(vendor_guid), type_id);
    Ok(old_vendor_table_ptr)
}

/// Returns the vendor table pointer for `vendor_guid` from a system table reference. This allows
/// reuse of a [`SYSTEM_TABLE`] lock already acquired by the caller.
pub(crate) fn configuration_table_entry_ptr(st: &EfiSystemTable, vendor_guid: &efi::Guid) -> Option<NonNull<c_void>> {
    let system_table = st.get();
    if system_table.configuration_table.is_null() || system_table.number_of_table_entries == 0 {
        return None;
    }
    // SAFETY: configuration_table is non-null, and number_of_table_entries describes its length.
    let ct_slice = unsafe { from_raw_parts(system_table.configuration_table, system_table.number_of_table_entries) };
    ct_slice.iter().find(|entry| entry.vendor_guid == *vendor_guid).and_then(|entry| NonNull::new(entry.vendor_table))
}

/// Returns the pointer to a configuration table for the specified guid, if it exists.
pub fn get_configuration_table(table_guid: &efi::Guid) -> Option<NonNull<c_void>> {
    let st_guard = SYSTEM_TABLE.lock();
    let st = st_guard.as_ref()?;
    configuration_table_entry_ptr(st, table_guid)
}

pub fn init_config_tables_support(st: &mut EfiSystemTable) {
    let mut bs = st.boot_services().get();
    bs.install_configuration_table = install_configuration_table;
    st.boot_services().set(bs);
}

#[cfg(test)]
mod tests {
    use patina::guid;

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
                init_system_table(patina::UefiSpecVersion::V2_11);
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

            assert_eq!(
                // SAFETY: The passed in values are safe because they are constructed in this test case.
                unsafe { install_configuration_table((&raw const guid).cast_mut(), table) },
                efi::Status::SUCCESS
            );
            assert_eq!(get_configuration_table(&guid).unwrap().as_ptr(), table);
        });
    }

    #[test]
    fn delete_config_table_should_return_ptr() {
        with_locked_state(|| {
            let guid: efi::Guid = guid::Guid::from_string("78926ab0-af16-49e4-8e05-115aafbca1df").to_efi_guid();
            let table = 0x12345678u32 as *mut c_void;

            assert_eq!(
                // SAFETY: The passed in values are safe because they are constructed in this test case.
                unsafe { install_configuration_table((&raw const guid).cast_mut(), table) },
                efi::Status::SUCCESS
            );

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

    #[test]
    fn core_install_typed_configuration_table_records_type() {
        with_locked_state(|| {
            let guid: efi::Guid = guid::Guid::from_string("11111111-1111-1111-1111-111111111111").to_efi_guid();
            let table = 0x1000u32 as *mut c_void;

            assert_eq!(
                core_install_typed_configuration_table(
                    guid,
                    table,
                    TypeId::of::<u32>(),
                    true,
                    &mut *SYSTEM_TABLE.lock().as_mut().unwrap()
                ),
                Ok(None)
            );
            assert_eq!(CONFIG_TABLE_TYPES.lock().get(&BinaryGuid::from(guid)), Some(&TypeId::of::<u32>()));
        });
    }

    #[test]
    fn core_install_typed_configuration_table_rejects_existing_untyped_entry() {
        with_locked_state(|| {
            let guid: efi::Guid = guid::Guid::from_string("22222222-2222-2222-2222-222222222222").to_efi_guid();
            let table = 0x2000u32 as *mut c_void;

            // Installed with the untyped path first, so a type is not recorded but the entry is active.
            core_install_configuration_table(guid, table, &mut *SYSTEM_TABLE.lock().as_mut().unwrap()).unwrap();

            // A typed install should not clobber it.
            assert_eq!(
                core_install_typed_configuration_table(
                    guid,
                    table,
                    TypeId::of::<u32>(),
                    true,
                    &mut *SYSTEM_TABLE.lock().as_mut().unwrap()
                ),
                Err(EfiError::AlreadyStarted)
            );
        });
    }

    #[test]
    fn core_install_configuration_table_clears_recorded_type_when_replacing_typed_entry() {
        with_locked_state(|| {
            let guid: efi::Guid = guid::Guid::from_string("33333333-3333-3333-3333-333333333333").to_efi_guid();
            let typed_table = 0x3000u32 as *mut c_void;
            let raw_table = 0x4000u32 as *mut c_void;

            core_install_typed_configuration_table(
                guid,
                typed_table,
                TypeId::of::<u32>(),
                true,
                &mut *SYSTEM_TABLE.lock().as_mut().unwrap(),
            )
            .unwrap();

            // Something bypasses the typed API entirely (e.g. the raw `extern "efiapi"` boot service
            // and replaces the entry directly, without removing it first.
            core_install_configuration_table(guid, raw_table, &mut *SYSTEM_TABLE.lock().as_mut().unwrap()).unwrap();

            // The stale type record must not be allowed to vouch for the replacement pointer.
            assert_eq!(CONFIG_TABLE_TYPES.lock().get(&BinaryGuid::from(guid)), None);
            assert_eq!(get_configuration_table(&guid).unwrap().as_ptr(), raw_table);
        });
    }
}
