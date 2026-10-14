//! Configuration Table Management for MM System Table
//!
//! Implements `MmInstallConfigurationTable`, which adds, modifies, or deletes
//! (GUID, pointer) pairs in the MM System Table's configuration table. Drivers use
//! the table to publish well-known data (for example the HOB list) for other drivers.
//!
//! After every change the MM System Table's `mm_configuration_table` pointer and
//! `number_of_table_entries` count are updated, so the change is visible at once.
//!
//! ## Thread Safety
//!
//! Access is serialized by a `spin::Mutex`. The system table is pointed at the new
//! array before the previous array is freed, so it never holds a freed pointer. A
//! reader that copied the old pointer before an install must not use it afterward.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::ffi::c_void;

use patina::standard::efi;
use spin::Mutex;

use patina::pi::mm_cis::EfiMmSystemTable;

/// Configuration table database for the MM System Table.
///
/// Holds the list of (GUID, pointer) pairs and republishes it to the MM System Table
/// after each change.
pub struct MmConfigurationTableDb {
    inner: Mutex<ConfigTableInner>,
}

struct ConfigTableInner {
    /// The authoritative list of entries.
    entries: Vec<efi::ConfigurationTable>,
    /// The leaked boxed slice the system table currently points to, or null if there is none.
    leaked_ptr: *mut efi::ConfigurationTable,
    /// Length of the leaked slice, needed to reclaim it.
    leaked_len: usize,
}

// SAFETY: All access is synchronized by the Mutex.
unsafe impl Send for MmConfigurationTableDb {}
// SAFETY: All access is synchronized by the Mutex.
unsafe impl Sync for MmConfigurationTableDb {}

impl Default for MmConfigurationTableDb {
    fn default() -> Self {
        Self::new()
    }
}

impl MmConfigurationTableDb {
    /// Create a new, empty configuration table database.
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(ConfigTableInner {
                entries: Vec::new(),
                leaked_ptr: core::ptr::null_mut(),
                leaked_len: 0,
            }),
        }
    }

    /// Installs, modifies, or deletes a configuration table entry.
    ///
    /// Follows the PI Specification `EFI_MM_INSTALL_CONFIGURATION_TABLE`:
    ///
    /// | Existing? | `table`  | Action        |
    /// |-----------|----------|---------------|
    /// | Yes       | non-null | Modify        |
    /// | Yes       | null     | Delete        |
    /// | No        | non-null | Add           |
    /// | No        | null     | `NOT_FOUND`   |
    ///
    /// A null `mmst` updates only the database. A non-null `mmst` must point to the table built
    /// by `init_mm_system_table`, so this is only callable inside the crate.
    pub(crate) fn install_configuration_table(
        &self,
        mmst: *mut EfiMmSystemTable,
        guid: &efi::Guid,
        table: *mut c_void,
    ) -> efi::Status {
        let mut inner = self.inner.lock();

        // Search for an existing entry with the same GUID.
        let existing_idx = inner.entries.iter().position(|entry| entry.vendor_guid == *guid);

        match (existing_idx, table.is_null()) {
            (Some(index), false) => {
                // `idx` came from `position`, so the entry is always present.
                if let Some(entry) = inner.entries.get_mut(index) {
                    entry.vendor_table = table;
                }
                log::debug!("MmInstallConfigurationTable: modified {guid:?}");
            }
            (Some(index), true) => {
                inner.entries.remove(index);
                log::debug!("MmInstallConfigurationTable: deleted {guid:?}");
            }
            (None, false) => {
                inner.entries.push(efi::ConfigurationTable { vendor_guid: *guid, vendor_table: table });
                log::debug!("MmInstallConfigurationTable: added {guid:?}");
            }
            (None, true) => {
                log::debug!("MmInstallConfigurationTable: cannot delete {guid:?}, it is not installed");
                return efi::Status::NOT_FOUND;
            }
        }

        Self::publish_to_system_table(&mut inner, mmst);

        efi::Status::SUCCESS
    }

    /// Returns the `vendor_table` pointer installed for `guid`, if any.
    pub fn get_configuration_table(&self, guid: &efi::Guid) -> Option<*mut c_void> {
        let inner = self.inner.lock();
        inner.entries.iter().find(|entry| entry.vendor_guid == *guid).map(|entry| entry.vendor_table)
    }

    /// Republishes the entry list to the MM System Table.
    ///
    /// Points the system table at a newly leaked boxed slice (or at nothing when the list is
    /// empty), then frees the previous array. Does nothing if `mmst` is null.
    fn publish_to_system_table(inner: &mut ConfigTableInner, mmst: *mut EfiMmSystemTable) {
        if mmst.is_null() {
            return;
        }

        let (new_ptr, new_len) = if inner.entries.is_empty() {
            (core::ptr::null_mut(), 0)
        } else {
            let boxed: Box<[efi::ConfigurationTable]> = inner.entries.clone().into_boxed_slice();
            let len = boxed.len();
            (Box::into_raw(boxed).cast::<efi::ConfigurationTable>(), len)
        };

        // SAFETY: The mutex is held, and `mmst` points to the table built by
        // `init_mm_system_table`.
        unsafe {
            (*mmst).number_of_table_entries = new_len;
            (*mmst).mm_configuration_table = new_ptr;
        }

        let old_ptr = core::mem::replace(&mut inner.leaked_ptr, new_ptr);
        let old_len = core::mem::replace(&mut inner.leaked_len, new_len);
        if !old_ptr.is_null() {
            // SAFETY: `old_ptr` / `old_len` were produced by `Box::into_raw` in a prior call to
            // this function, and the system table no longer points to them.
            drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(old_ptr, old_len)) });
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use core::mem::MaybeUninit;

    const GUID_A: efi::Guid = efi::Guid::from_fields(0xa000_0000, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);
    const GUID_B: efi::Guid = efi::Guid::from_fields(0xb000_0000, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 2]);
    const GUID_C: efi::Guid = efi::Guid::from_fields(0xc000_0000, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 3]);

    /// An arbitrary non-null address. The table pointers are only ever stored and compared,
    /// never dereferenced.
    fn table_ptr(addr: usize) -> *mut c_void {
        addr as *mut c_void
    }

    /// Heap-backed stand-in for the MM System Table.
    ///
    /// Only the two extensibility fields are ever touched by this module, so the rest of the
    /// struct - which includes non-nullable function pointers and therefore cannot legally be
    /// zeroed - is left uninitialized and never read.
    struct FakeSystemTable {
        storage: Box<MaybeUninit<EfiMmSystemTable>>,
    }

    impl FakeSystemTable {
        fn new() -> Self {
            let mut storage = Box::new(MaybeUninit::<EfiMmSystemTable>::uninit());
            // SAFETY: This initializes exactly the two fields the module reads back. No
            // reference to the struct as a whole is ever created.
            unsafe {
                let ptr = storage.as_mut_ptr();
                (&raw mut (*ptr).number_of_table_entries).write(0);
                (&raw mut (*ptr).mm_configuration_table).write(core::ptr::null_mut());
            }
            Self { storage }
        }

        fn as_ptr(&mut self) -> *mut EfiMmSystemTable {
            self.storage.as_mut_ptr()
        }

        /// Returns the entries currently published to the system table.
        fn published(&self) -> Vec<(efi::Guid, *mut c_void)> {
            // SAFETY: both fields were initialized in `new` and are only ever updated together
            // by `publish_to_system_table`.
            unsafe {
                let ptr = self.storage.as_ptr();
                let count = (&raw const (*ptr).number_of_table_entries).read();
                let table = (&raw const (*ptr).mm_configuration_table).read();
                if table.is_null() {
                    return Vec::new();
                }
                core::slice::from_raw_parts(table, count)
                    .iter()
                    .map(|entry| (entry.vendor_guid, entry.vendor_table))
                    .collect()
            }
        }
    }

    #[test]
    fn test_config_table_starts_empty() {
        let database = MmConfigurationTableDb::default();

        assert!(database.get_configuration_table(&GUID_A).is_none());
    }

    #[test]
    fn test_install_adds_a_new_entry_and_publishes_it() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        assert_eq!(
            database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000)),
            efi::Status::SUCCESS
        );

        assert_eq!(database.get_configuration_table(&GUID_A), Some(table_ptr(0x1000)));
        assert_eq!(mmst.published(), [(GUID_A, table_ptr(0x1000))]);
    }

    #[test]
    fn test_install_appends_entries_in_registration_order() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        for (guid, addr) in [(GUID_A, 0x1000), (GUID_B, 0x2000), (GUID_C, 0x3000)] {
            assert_eq!(
                database.install_configuration_table(mmst.as_ptr(), &guid, table_ptr(addr)),
                efi::Status::SUCCESS
            );
        }

        assert_eq!(
            mmst.published(),
            [(GUID_A, table_ptr(0x1000)), (GUID_B, table_ptr(0x2000)), (GUID_C, table_ptr(0x3000))]
        );
    }

    #[test]
    fn test_install_with_a_known_guid_replaces_the_pointer_in_place() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));
        database.install_configuration_table(mmst.as_ptr(), &GUID_B, table_ptr(0x2000));

        assert_eq!(
            database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0xbeef)),
            efi::Status::SUCCESS
        );

        // A modify updates in place rather than appending a duplicate or reordering.
        assert_eq!(database.get_configuration_table(&GUID_A), Some(table_ptr(0xbeef)));
        assert_eq!(mmst.published(), [(GUID_A, table_ptr(0xbeef)), (GUID_B, table_ptr(0x2000))]);
    }

    #[test]
    fn test_install_with_a_null_table_deletes_a_known_guid() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));
        database.install_configuration_table(mmst.as_ptr(), &GUID_B, table_ptr(0x2000));

        assert_eq!(
            database.install_configuration_table(mmst.as_ptr(), &GUID_A, core::ptr::null_mut()),
            efi::Status::SUCCESS
        );

        assert!(database.get_configuration_table(&GUID_A).is_none());
        assert_eq!(mmst.published(), [(GUID_B, table_ptr(0x2000))]);
    }

    #[test]
    fn test_deleting_the_last_entry_clears_the_published_table() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));
        database.install_configuration_table(mmst.as_ptr(), &GUID_A, core::ptr::null_mut());

        // The system table must advertise a null pointer, not a dangling empty allocation.
        assert!(mmst.published().is_empty());
    }

    #[test]
    fn test_install_with_a_null_table_reports_not_found_for_an_unknown_guid() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));

        assert_eq!(
            database.install_configuration_table(mmst.as_ptr(), &GUID_B, core::ptr::null_mut()),
            efi::Status::NOT_FOUND
        );

        // The failed request must leave the existing table untouched.
        assert_eq!(mmst.published(), [(GUID_A, table_ptr(0x1000))]);
    }

    #[test]
    fn test_install_tracks_entries_even_without_a_system_table() {
        let database = MmConfigurationTableDb::new();

        assert_eq!(
            database.install_configuration_table(core::ptr::null_mut(), &GUID_A, table_ptr(0x1000)),
            efi::Status::SUCCESS
        );

        assert_eq!(database.get_configuration_table(&GUID_A), Some(table_ptr(0x1000)));
    }

    #[test]
    fn test_get_configuration_table_returns_none_for_an_unknown_guid() {
        let database = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        database.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));

        assert!(database.get_configuration_table(&GUID_B).is_none());
    }
}
