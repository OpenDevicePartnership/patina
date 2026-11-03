//! Configuration Table Management for MM System Table
//!
//! Implements `MmInstallConfigurationTable` - the ability to add, modify, or
//! delete (GUID, pointer) pairs stored in the MM System Table's configuration
//! table array.
//!
//! ## Semantics
//!
//! The configuration table is an array of `EFI_CONFIGURATION_TABLE` entries
//! exposed through `EfiMmSystemTable.mm_configuration_table`. Drivers use
//! this to publish well-known data (e.g. the HOB list) that other drivers
//! can discover by iterating the table.
//!
//! Operations:
//! - **Add**: GUID not present, table pointer non-null.
//! - **Modify**: GUID already present, table pointer non-null → update pointer.
//! - **Delete**: GUID already present, table pointer null → remove entry.
//! - **Error**: GUID not present, table pointer null → `NOT_FOUND`.
//!
//! After every modification the MM System Table's `mm_configuration_table`
//! pointer and `number_of_table_entries` count are updated so the change is
//! immediately visible to all consumers.
//!
//! ## Thread Safety
//!
//! All access is serialized through a `spin::Mutex`. The configuration table
//! array pointer stored in the system table is replaced atomically (pointer-
//! sized write) so readers always see a consistent snapshot even without
//! holding the lock.
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
/// Maintains the canonical list of `(GUID, Pointer)` pairs. After each
/// mutation the MM System Table's pointer and count are updated so all
/// consumers see the change immediately.
pub struct MmConfigurationTableDb {
    inner: Mutex<ConfigTableInner>,
}

struct ConfigTableInner {
    /// The authoritative list of entries.
    entries: Vec<efi::ConfigurationTable>,
    /// Raw pointer to the most recently leaked boxed‐slice that the system
    /// table currently points to. `null` when no allocation exists.
    leaked_ptr: *mut efi::ConfigurationTable,
    /// Length of the leaked allocation (for reclaim).
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

    /// Install, modify, or delete a configuration table entry.
    ///
    /// Semantics match the PI Specification `EFI_MM_INSTALL_CONFIGURATION_TABLE`:
    ///
    /// | Existing? | `table` | Action   |
    /// |-----------|---------|----------|
    /// | Yes       | non-null| Modify   |
    /// | Yes       | null    | Delete   |
    /// | No        | non-null| Add      |
    /// | No        | null    | NOT_FOUND|
    pub fn install_configuration_table(
        &self,
        mmst: *mut EfiMmSystemTable,
        guid: &efi::Guid,
        table: *mut c_void,
    ) -> efi::Status {
        let mut inner = self.inner.lock();

        // Search for an existing entry with the same GUID.
        let existing_idx = inner.entries.iter().position(|e| e.vendor_guid == *guid);

        match (existing_idx, table.is_null()) {
            // Match found, table non-null → modify
            (Some(idx), false) => {
                // `idx` came from `position`, so the entry is always present.
                if let Some(entry) = inner.entries.get_mut(idx) {
                    entry.vendor_table = table;
                }
                log::debug!("MmInstallConfigurationTable: modified {guid:?}");
            }
            // Match found, table null → delete
            (Some(idx), true) => {
                inner.entries.remove(idx);
                log::debug!("MmInstallConfigurationTable: deleted {guid:?}");
            }
            // No match, table non-null → add
            (None, false) => {
                inner.entries.push(efi::ConfigurationTable { vendor_guid: *guid, vendor_table: table });
                log::debug!("MmInstallConfigurationTable: added {guid:?}");
            }
            // No match, table null → error
            (None, true) => {
                log::error!("MmInstallConfigurationTable: cannot delete {guid:?}, it is not installed");
                return efi::Status::NOT_FOUND;
            }
        }

        // Publish the updated table to the MM System Table.
        Self::publish_to_system_table(&mut inner, mmst);

        efi::Status::SUCCESS
    }

    /// Look up a configuration table entry by GUID.
    ///
    /// Returns the `vendor_table` pointer if found, or `None`.
    #[allow(dead_code)]
    pub fn get_configuration_table(&self, guid: &efi::Guid) -> Option<*mut c_void> {
        let inner = self.inner.lock();
        inner.entries.iter().find(|e| e.vendor_guid == *guid).map(|e| e.vendor_table)
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    /// Re‐publish the current entry list to the MM System Table.
    ///
    /// Allocates a new boxed slice, updates the system table pointer, then
    /// reclaims the previous allocation.
    fn publish_to_system_table(inner: &mut ConfigTableInner, mmst: *mut EfiMmSystemTable) {
        if mmst.is_null() {
            return;
        }

        // --- Reclaim the previous allocation ---------------------------------
        if !inner.leaked_ptr.is_null() {
            // SAFETY: `leaked_ptr` / `leaked_len` were produced by
            // `Box::into_raw` in a prior call to this function.
            unsafe {
                let _ = Box::from_raw(core::ptr::slice_from_raw_parts_mut(inner.leaked_ptr, inner.leaked_len));
            }
            inner.leaked_ptr = core::ptr::null_mut();
            inner.leaked_len = 0;
        }

        // --- Produce the new allocation and patch the MMST -------------------
        if inner.entries.is_empty() {
            // SAFETY: We hold the Mutex, and `mmst` was initialized by
            // `init_mm_system_table`.
            unsafe {
                (*mmst).number_of_table_entries = 0;
                (*mmst).mm_configuration_table = core::ptr::null_mut();
            }
        } else {
            let boxed: Box<[efi::ConfigurationTable]> = inner.entries.clone().into_boxed_slice();
            let len = boxed.len();
            let ptr = Box::into_raw(boxed).cast::<efi::ConfigurationTable>();

            inner.leaked_ptr = ptr;
            inner.leaked_len = len;

            // SAFETY: Same as above.
            unsafe {
                (*mmst).number_of_table_entries = len;
                (*mmst).mm_configuration_table = ptr;
            }
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
                core::slice::from_raw_parts(table, count).iter().map(|e| (e.vendor_guid, e.vendor_table)).collect()
            }
        }
    }

    #[test]
    fn test_config_table_starts_empty() {
        let db = MmConfigurationTableDb::default();

        assert!(db.get_configuration_table(&GUID_A).is_none());
    }

    #[test]
    fn test_install_adds_a_new_entry_and_publishes_it() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        assert_eq!(db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000)), efi::Status::SUCCESS);

        assert_eq!(db.get_configuration_table(&GUID_A), Some(table_ptr(0x1000)));
        assert_eq!(mmst.published(), [(GUID_A, table_ptr(0x1000))]);
    }

    #[test]
    fn test_install_appends_entries_in_registration_order() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        for (guid, addr) in [(GUID_A, 0x1000), (GUID_B, 0x2000), (GUID_C, 0x3000)] {
            assert_eq!(db.install_configuration_table(mmst.as_ptr(), &guid, table_ptr(addr)), efi::Status::SUCCESS);
        }

        assert_eq!(
            mmst.published(),
            [(GUID_A, table_ptr(0x1000)), (GUID_B, table_ptr(0x2000)), (GUID_C, table_ptr(0x3000))]
        );
    }

    #[test]
    fn test_install_with_a_known_guid_replaces_the_pointer_in_place() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));
        db.install_configuration_table(mmst.as_ptr(), &GUID_B, table_ptr(0x2000));

        assert_eq!(db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0xbeef)), efi::Status::SUCCESS);

        // A modify updates in place rather than appending a duplicate or reordering.
        assert_eq!(db.get_configuration_table(&GUID_A), Some(table_ptr(0xbeef)));
        assert_eq!(mmst.published(), [(GUID_A, table_ptr(0xbeef)), (GUID_B, table_ptr(0x2000))]);
    }

    #[test]
    fn test_install_with_a_null_table_deletes_a_known_guid() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));
        db.install_configuration_table(mmst.as_ptr(), &GUID_B, table_ptr(0x2000));

        assert_eq!(db.install_configuration_table(mmst.as_ptr(), &GUID_A, core::ptr::null_mut()), efi::Status::SUCCESS);

        assert!(db.get_configuration_table(&GUID_A).is_none());
        assert_eq!(mmst.published(), [(GUID_B, table_ptr(0x2000))]);
    }

    #[test]
    fn test_deleting_the_last_entry_clears_the_published_table() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));
        db.install_configuration_table(mmst.as_ptr(), &GUID_A, core::ptr::null_mut());

        // The system table must advertise a null pointer, not a dangling empty allocation.
        assert!(mmst.published().is_empty());
    }

    #[test]
    fn test_install_with_a_null_table_reports_not_found_for_an_unknown_guid() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));

        assert_eq!(
            db.install_configuration_table(mmst.as_ptr(), &GUID_B, core::ptr::null_mut()),
            efi::Status::NOT_FOUND
        );

        // The failed request must leave the existing table untouched.
        assert_eq!(mmst.published(), [(GUID_A, table_ptr(0x1000))]);
    }

    #[test]
    fn test_install_tracks_entries_even_without_a_system_table() {
        let db = MmConfigurationTableDb::new();

        assert_eq!(
            db.install_configuration_table(core::ptr::null_mut(), &GUID_A, table_ptr(0x1000)),
            efi::Status::SUCCESS
        );

        assert_eq!(db.get_configuration_table(&GUID_A), Some(table_ptr(0x1000)));
    }

    #[test]
    fn test_get_configuration_table_returns_none_for_an_unknown_guid() {
        let db = MmConfigurationTableDb::new();
        let mut mmst = FakeSystemTable::new();

        db.install_configuration_table(mmst.as_ptr(), &GUID_A, table_ptr(0x1000));

        assert!(db.get_configuration_table(&GUID_B).is_none());
    }
}
