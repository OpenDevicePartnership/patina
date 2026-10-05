//! DXE Core implementation of [`ConfigurationTableServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec;
use alloc::vec::Vec;
use core::any::TypeId;

use patina::BinaryGuid;
use patina::component::service::{
    IntoService,
    uefi_services::config_table::{ConfigTableError, ConfigTablePtr, ConfigurationTableServices},
};

use crate::config_tables::{
    CONFIG_TABLE_TYPES, configuration_table_entry_ptr, core_install_configuration_table,
    core_install_typed_configuration_table, get_configuration_table,
};
use crate::systemtables::{EfiSystemTable, SYSTEM_TABLE};

/// Looks up the pointer for `guid`'s table given an already-locked system table reference,
/// and verifies it was recorded with `type_id` in [`CONFIG_TABLE_TYPES`]. Returns `None` if either
/// check fails.
fn typed_table_ptr(st: &EfiSystemTable, guid: BinaryGuid, type_id: TypeId) -> Option<ConfigTablePtr> {
    if CONFIG_TABLE_TYPES.lock().get(&guid) != Some(&type_id) {
        return None;
    }
    let ptr = configuration_table_entry_ptr(st, &guid.into_inner())?;
    ConfigTablePtr::from_raw(ptr.as_ptr())
}

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
        let mut st_guard = SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().ok_or(ConfigTableError::NotFound)?;
        // SAFETY: forwarding the precondition on `table` upheld by this function's own caller.
        core_install_typed_configuration_table(guid.into_inner(), table.as_raw(), type_id, true, st)
            .map(|_| ())
            .map_err(ConfigTableError::from)
    }

    fn get_typed_table(&self, guid: BinaryGuid, type_id: TypeId) -> Option<ConfigTablePtr> {
        let st_guard = SYSTEM_TABLE.lock();
        let st = st_guard.as_ref()?;
        typed_table_ptr(st, guid, type_id)
    }

    unsafe fn read_typed_table_bytes(&self, guid: BinaryGuid, type_id: TypeId, buf: &mut [u8]) -> bool {
        // Note: The `SYSTEM_TABLE` lock is held across the type check, lookup, and copy to keep all three
        // synchronized against a concurrent install, replace, or remove of the same entry.
        let st_guard = SYSTEM_TABLE.lock();
        let Some(st) = st_guard.as_ref() else { return false };
        let Some(ptr) = typed_table_ptr(st, guid, type_id) else { return false };
        let base = ptr.as_raw() as *const u8;
        for (index, byte) in buf.iter_mut().enumerate() {
            // SAFETY: the caller guarantees `buf.len()` bytes are valid to read starting at `base`.
            *byte = unsafe { core::ptr::read(base.add(index)) };
        }
        true
    }

    unsafe fn read_typed_table_bytes_sized(
        &self,
        guid: BinaryGuid,
        type_id: TypeId,
        header_len: usize,
        table_len: fn(&[u8]) -> usize,
    ) -> Option<Vec<u8>> {
        // Holding `SYSTEM_TABLE`'s lock across check and read operations is done to keep all of them
        // synchronized against a concurrent operations against the same entry.
        let st_guard = SYSTEM_TABLE.lock();
        let st = st_guard.as_ref()?;
        let ptr = typed_table_ptr(st, guid, type_id)?;
        let base = ptr.as_raw() as *const u8;

        let mut header = vec![0u8; header_len];
        for (index, byte) in header.iter_mut().enumerate() {
            // SAFETY: the caller guarantees `header_len` bytes are valid to read starting at `base`.
            *byte = unsafe { core::ptr::read(base.add(index)) };
        }

        let mut bytes = vec![0u8; table_len(&header)];
        for (index, byte) in bytes.iter_mut().enumerate() {
            // SAFETY: the caller guarantees `table_len`'s result is valid to read starting at
            // `base`, and the table has stayed synchronized since the header read above because
            // `SYSTEM_TABLE`'s lock has been held continuously.
            *byte = unsafe { core::ptr::read(base.add(index)) };
        }
        Some(bytes)
    }

    fn remove_typed_table(&self, guid: BinaryGuid) -> Result<(), ConfigTableError> {
        // Note: `core_install_configuration_table` clears any type recorded for `guid` as part of
        // removing its entry, so the map entry cannot outlive its table.
        self.remove_table(guid)
    }

    unsafe fn replace_typed_table(
        &self,
        guid: BinaryGuid,
        type_id: TypeId,
        table: ConfigTablePtr,
    ) -> Result<(), ConfigTableError> {
        let mut st_guard = SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().ok_or(ConfigTableError::NotFound)?;
        // SAFETY: forwarding the precondition on `table` upheld by this function's own caller.
        core_install_typed_configuration_table(guid.into_inner(), table.as_raw(), type_id, false, st)
            .map(|_| ())
            .map_err(ConfigTableError::from)
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
                init_system_table(patina::UefiSpecVersion::V2_11);
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
            let _guard = test_support::StateGuard::new(|| init_system_table(patina::UefiSpecVersion::V2_11));

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
    fn install_typed_table_succeeds_after_raw_remove_table() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("1e2f3a4b-5c6d-4e7f-8a8b-9c0d1e2f3a4b");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(0x9000usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_typed_table(guid, type_id, table) }.unwrap();
            // Remove the real table using the untyped API. This also clears the recorded type for
            // `guid`, so a stale entry is not left behind.
            svc.remove_table(guid).unwrap();
            assert_eq!(svc.get_typed_table(guid, type_id), None);

            // Re-installing under the same GUID must succeed against this clean state.
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

    #[test]
    fn get_typed_table_does_not_vouch_for_a_table_replaced_out_of_band() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("4b5c6d7e-8f9a-4b0c-9d0e-2f3a4b5c6d7e");
            let type_id = TypeId::of::<u32>();
            let typed_table = ConfigTablePtr::from_raw(0xd000usize as *mut c_void).unwrap();
            let replacement = 0xe000usize as *mut c_void;

            // SAFETY: `typed_table` is a dummy address that is not dereferenced in this test.
            unsafe { svc.install_typed_table(guid, type_id, typed_table) }.unwrap();

            // Something bypasses `ConfigurationTableServices` and replaces the entry directly
            // (like the install config table boot service).
            core_install_configuration_table(
                guid.into_inner(),
                replacement,
                &mut *SYSTEM_TABLE.lock().as_mut().unwrap(),
            )
            .unwrap();

            // The stale type record must not vouch for the replacement pointer.
            assert_eq!(svc.get_typed_table(guid, type_id), None);
            assert_eq!(svc.get_table(guid).unwrap().as_raw(), replacement);
        });
    }

    #[test]
    fn read_typed_table_bytes_copies_installed_table() {
        with_locked_state(|| {
            static VALUE: u32 = 0x1234_5678;

            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("4a5b6c7d-8e9f-4a0b-9c1d-2e3f4a5b6c7d");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(&raw const VALUE as *mut c_void).unwrap();

            // SAFETY: `table` points to `VALUE`, a `'static u32` (outlives this test).
            unsafe { svc.install_typed_table(guid, type_id, table) }.unwrap();

            let mut buf = [0u8; size_of::<u32>()];
            // SAFETY: `buf` is exactly `size_of::<u32>()` bytes, matching the installed `VALUE`.
            let found = unsafe { svc.read_typed_table_bytes(guid, type_id, &mut buf) };

            assert!(found);
            assert_eq!(u32::from_ne_bytes(buf), VALUE);
        });
    }

    #[test]
    fn read_typed_table_bytes_returns_false_on_type_mismatch() {
        with_locked_state(|| {
            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("5b6c7d8e-9f0a-4b1c-9d2e-3f4a5b6c7d8e");
            let table = ConfigTablePtr::from_raw(0xc100usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address. It is never dereferenced because the type
            // mismatch below makes `read_typed_table_bytes` return before reading it.
            unsafe { svc.install_typed_table(guid, TypeId::of::<u32>(), table) }.unwrap();

            let mut buf = [0u8; 8];
            // SAFETY: `read_typed_table_bytes` reports a type mismatch without reading `table`'s
            // memory, so `buf` is never actually filled from the dummy address.
            let found = unsafe { svc.read_typed_table_bytes(guid, TypeId::of::<u64>(), &mut buf) };

            assert!(!found);
        });
    }

    #[test]
    fn read_typed_table_bytes_sized_reads_header_then_full_length() {
        with_locked_state(|| {
            #[repr(C)]
            struct HeaderWithTrailingData {
                total_len: u32,
                trailing: [u8; 4],
            }

            static TABLE: HeaderWithTrailingData =
                HeaderWithTrailingData { total_len: 8, trailing: [0xaa, 0xbb, 0xcc, 0xdd] };

            fn table_len(header: &[u8]) -> usize {
                u32::from_ne_bytes(header.try_into().unwrap()) as usize
            }

            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("6c7d8e9f-0a1b-4c2d-9e3f-4a5b6c7d8e9f");
            let type_id = TypeId::of::<u32>();
            let table = ConfigTablePtr::from_raw(&raw const TABLE as *mut c_void).unwrap();

            // SAFETY: `table` points to `TABLE`, a `'static` value (outlives this test).
            unsafe { svc.install_typed_table(guid, type_id, table) }.unwrap();

            // SAFETY: 4 header bytes and `table_len`'s result (8) are both valid to read starting
            // at `TABLE`'s address.
            let bytes = unsafe { svc.read_typed_table_bytes_sized(guid, type_id, 4, table_len) }.unwrap();

            assert_eq!(bytes.len(), 8);
            assert_eq!(&bytes[4..], &[0xaa, 0xbb, 0xcc, 0xdd]);
        });
    }

    #[test]
    fn read_typed_table_bytes_sized_returns_none_on_type_mismatch() {
        with_locked_state(|| {
            fn table_len(_header: &[u8]) -> usize {
                8
            }

            let svc = CoreConfigurationTableServices;
            let guid: BinaryGuid = BinaryGuid::from_string("7d8e9f0a-1b2c-4d3e-8f4a-5b6c7d8e9f0a");
            let table = ConfigTablePtr::from_raw(0xd100usize as *mut c_void).unwrap();

            // SAFETY: `table` is a dummy address. It is not dereferenced because the type
            // mismatch below makes `read_typed_table_bytes_sized` return before reading it.
            unsafe { svc.install_typed_table(guid, TypeId::of::<u32>(), table) }.unwrap();

            // SAFETY: `read_typed_table_bytes_sized` reports a type mismatch without reading `table`'s memory.
            let result = unsafe { svc.read_typed_table_bytes_sized(guid, TypeId::of::<u64>(), 4, table_len) };

            assert!(result.is_none());
        });
    }
}
