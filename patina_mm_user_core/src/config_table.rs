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
