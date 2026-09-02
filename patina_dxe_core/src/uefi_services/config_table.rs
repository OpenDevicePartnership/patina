//! DXE Core implementation of [`ConfigurationTableServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::collections::btree_map::BTreeMap;
use core::any::TypeId;

use patina::BinaryGuid;
use patina::component::service::{
    IntoService,
    uefi_services::config_table::{ConfigTableError, ConfigTablePtr, ConfigurationTableServices},
};
use patina::standard::efi;

use crate::config_tables::{core_install_configuration_table, get_configuration_table};
use crate::systemtables::SYSTEM_TABLE;
use crate::tpl_mutex::TplMutex;

/// Records the Rust type installed under each GUID using [`ConfigurationTableServices::install_typed_table`], so
/// [`ConfigurationTableServices::get_typed_table`] can verify a lookup's type before a caller casts the pointer.
///
/// This is separate from the real configuration table entries in [`SYSTEM_TABLE`] and is only used internally.
static CONFIG_TABLE_TYPES: TplMutex<BTreeMap<BinaryGuid, TypeId>> =
    TplMutex::new(efi::TPL_NOTIFY, BTreeMap::new(), "ConfigTableTypeLock");

/// Core implementation of [`ConfigurationTableServices`], operating on the global system table via
/// the core's internal Rust APIs.
#[derive(IntoService)]
#[service(dyn ConfigurationTableServices)]
pub(crate) struct CoreConfigurationTableServices;

impl ConfigurationTableServices for CoreConfigurationTableServices {
    unsafe fn install_table(&self, guid: BinaryGuid, table: ConfigTablePtr) -> Result<(), ConfigTableError> {
        let mut st_guard = SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().ok_or(ConfigTableError::NotFound)?;
        core_install_configuration_table(guid.into_inner(), table.as_raw(), st)
            .map(|_| ())
            .map_err(ConfigTableError::from)
    }

    fn remove_table(&self, guid: BinaryGuid) -> Result<(), ConfigTableError> {
        let mut st_guard = SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().ok_or(ConfigTableError::NotFound)?;
        // Installing a null table removes the entry for the GUID.
        core_install_configuration_table(guid.into_inner(), core::ptr::null_mut(), st)
            .map(|_| ())
            .map_err(ConfigTableError::from)
    }

    fn get_table(&self, guid: BinaryGuid) -> Option<ConfigTablePtr> {
        get_configuration_table(&guid.into_inner()).and_then(|table| ConfigTablePtr::from_raw(table.as_ptr()))
    }

    unsafe fn install_typed_table(
        &self,
        guid: BinaryGuid,
        type_id: TypeId,
        table: ConfigTablePtr,
    ) -> Result<(), ConfigTableError> {
        let mut types = CONFIG_TABLE_TYPES.lock();
        // A stale type entry (in the `BTreeMap`) can outlive its table if something removed it using
        // `remove_table` (untyped) directly, so only reject the install if the table is still present
        // in the actual system table.
        if types.contains_key(&guid) && self.get_table(guid).is_some() {
            return Err(ConfigTableError::AlreadyExists);
        }
        // SAFETY: forwarding the precondition on `table` upheld by this function's own caller.
        unsafe { self.install_table(guid, table) }?;
        types.insert(guid, type_id);
        Ok(())
    }

    fn get_typed_table(&self, guid: BinaryGuid, type_id: TypeId) -> Option<ConfigTablePtr> {
        if CONFIG_TABLE_TYPES.lock().get(&guid) != Some(&type_id) {
            return None;
        }
        self.get_table(guid)
    }

    fn remove_typed_table(&self, guid: BinaryGuid) -> Result<(), ConfigTableError> {
        self.remove_table(guid)?;
        CONFIG_TABLE_TYPES.lock().remove(&guid);
        Ok(())
    }

    unsafe fn replace_typed_table(
        &self,
        guid: BinaryGuid,
        type_id: TypeId,
        table: ConfigTablePtr,
    ) -> Result<(), ConfigTableError> {
        let mut types = CONFIG_TABLE_TYPES.lock();
        // SAFETY: forwarding the precondition on `table` upheld by this function's own caller.
        unsafe { self.install_table(guid, table) }?;
        types.insert(guid, type_id);
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::ffi::c_void;

    use crate::{systemtables::init_system_table, test_support};

    use super::*;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            // SAFETY: functions modify global state; called within the global test lock.
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
    fn install_table_then_get_table_returns_same_pointer() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("1a2b3c4d-5e6f-4a1b-9c2d-3e4f5a6b7c8d");
            let table = ConfigTablePtr::from_raw(0x1000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is never dereferenced. This test only checks
            // that the opaque pointer value carries through installation and lookup.
            assert_eq!(unsafe { svc.install_table(guid, table) }, Ok(()));
            assert_eq!(svc.get_table(guid), Some(table));
        });
    }

    #[test]
    fn remove_table_removes_installed_table() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("2b3c4d5e-6f7a-4b2c-8d3e-4f5a6b7c8d9e");
            let table = ConfigTablePtr::from_raw(0x2000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_table(guid, table) }.unwrap();
            assert_eq!(svc.get_table(guid), Some(table));

            assert_eq!(svc.remove_table(guid), Ok(()));
            assert_eq!(svc.get_table(guid), None);
        });
    }

    #[test]
    fn remove_table_for_unknown_guid_returns_not_found() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("3c4d5e6f-7a8b-4c3d-9e4f-5a6b7c8d9e0f");

            assert_eq!(svc.remove_table(guid), Err(ConfigTableError::NotFound));
        });
    }

    #[test]
    fn get_table_for_unknown_guid_returns_none() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("4d5e6f7a-8b9c-4d4e-8f5a-6b7c8d9e0f1a");

            assert_eq!(svc.get_table(guid), None);
        });
    }

    #[test]
    fn install_table_and_remove_table_return_not_found_when_system_table_uninitialized() {
        with_locked_state(|| {
            // Simulate an uninitialized system table. Restore it afterward (even on panic) so
            // later tests relying on `with_locked_state`'s invariant are unaffected.
            *SYSTEM_TABLE.lock() = None;
            let _guard = test_support::StateGuard::new(init_system_table);

            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("5e6f7a8b-9c0d-4e5f-9a6b-7c8d9e0f1a2b");
            let table = ConfigTablePtr::from_raw(0x3000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            assert_eq!(unsafe { svc.install_table(guid, table) }, Err(ConfigTableError::NotFound));
            assert_eq!(svc.remove_table(guid), Err(ConfigTableError::NotFound));
        });
    }

    #[test]
    fn install_typed_table_then_get_typed_table_returns_same_pointer() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("6f7a8b9c-0d1e-4f5a-8b6c-7d8e9f0a1b2c");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(0x4000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            assert_eq!(unsafe { svc.install_typed_table(guid, type_id, table) }, Ok(()));
            assert_eq!(svc.get_typed_table(guid, type_id), Some(table));
        });
    }

    #[test]
    fn install_typed_table_rejects_duplicate_guid() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("7a8b9c0d-1e2f-4a5b-9c6d-7e8f9a0b1c2d");
            let table = ConfigTablePtr::from_raw(0x5000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            assert_eq!(unsafe { svc.install_typed_table(guid, TypeId::of::<u32>(), table) }, Ok(()));
            // A second install under the same GUID must fail, even with a different recorded type.
            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            let second_install = unsafe { svc.install_typed_table(guid, TypeId::of::<u64>(), table) };
            assert_eq!(second_install, Err(ConfigTableError::AlreadyExists));
        });
    }

    #[test]
    fn get_typed_table_returns_none_for_type_mismatch() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("8b9c0d1e-2f3a-4b6c-8d7e-8f9a0b1c2d3e");
            let table = ConfigTablePtr::from_raw(0x6000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_typed_table(guid, TypeId::of::<u32>(), table) }.unwrap();

            assert_eq!(svc.get_typed_table(guid, TypeId::of::<u64>()), None);
        });
    }

    #[test]
    fn get_typed_table_returns_none_for_untyped_install() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("9c0d1e2f-3a4b-4c6d-8e7f-8a9b0c1d2e3f");
            let table = ConfigTablePtr::from_raw(0x7000usize as *mut c_void).unwrap();

            // Installed using the untyped, raw API - no type is on record for `guid`.
            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_table(guid, table) }.unwrap();

            assert_eq!(svc.get_typed_table(guid, TypeId::of::<u32>()), None);
        });
    }

    #[test]
    fn remove_typed_table_removes_installed_table_and_type() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("0d1e2f3a-4b5c-4d6e-8f7a-8b9c0d1e2f3a");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(0x8000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_typed_table(guid, type_id, table) }.unwrap();
            assert_eq!(svc.remove_typed_table(guid), Ok(()));

            assert_eq!(svc.get_table(guid), None);
            assert_eq!(svc.get_typed_table(guid, type_id), None);
        });
    }

    #[test]
    fn install_typed_table_self_gracefully_handles_raw_remove_table() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("1e2f3a4b-5c6d-4e7f-8a8b-9c0d1e2f3a4b");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(0x9000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_typed_table(guid, type_id, table) }.unwrap();
            // Remove the real table using the untyped API, bypassing type-registry cleanup. A stale
            // type entry for `guid` is left behind.
            svc.remove_table(guid).unwrap();

            // Re-installing under the same GUID must succeed since the table itself is gone, even
            // though the (now stale) type entry was never cleared.
            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            assert_eq!(unsafe { svc.install_typed_table(guid, type_id, table) }, Ok(()));
            assert_eq!(svc.get_typed_table(guid, type_id), Some(table));
        });
    }

    #[test]
    fn replace_typed_table_installs_when_nothing_exists() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("2f3a4b5c-6d7e-4f8a-9b8c-0d1e2f3a4b5c");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(0xa000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            assert_eq!(unsafe { svc.replace_typed_table(guid, type_id, table) }, Ok(()));
            assert_eq!(svc.get_typed_table(guid, type_id), Some(table));
        });
    }

    #[test]
    fn replace_typed_table_replaces_without_error_when_already_installed() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("3a4b5c6d-7e8f-4a9b-8c9d-1e2f3a4b5c6d");
            let type_id = TypeId::of::<u32>();
            let first_table = ConfigTablePtr::from_raw(0xb000usize as *mut c_void).unwrap();
            let second_table = ConfigTablePtr::from_raw(0xc000usize as *mut c_void).unwrap();

            // SAFETY: `first_table`/`second_table` are dummy addresses that are not dereferenced in
            // this test.
            unsafe { svc.replace_typed_table(guid, type_id, first_table) }.unwrap();
            // Republishing under the same GUID (e.g. after mutating the table's contents) must
            // succeed rather than fail with `AlreadyExists`, and reflect the newest pointer.
            // SAFETY: `second_table` is a dummy address that is not dereferenced in this test.
            assert_eq!(unsafe { svc.replace_typed_table(guid, type_id, second_table) }, Ok(()));
            assert_eq!(svc.get_typed_table(guid, type_id), Some(second_table));
        });
    }
}
