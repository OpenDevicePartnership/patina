//! Configuration Table Sample Component
//!
//! This component demonstrates [`ConfigurationTableServices`] by installing a vendor configuration
//! table under a GUID and reading it back. Configuration tables are how firmware publishes system-wide,
//! pointer-addressable tables such as ACPI RSDP. An OS or later component finds them by GUID in the UEFI
//! system table.
//!
//! [`SampleVendorTable`] implements [`ConfigTable`], which binds it to a GUID at the type level. This
//! lets the component install and retrieve it through [`ConfigurationTableServicesExt::install`] and
//! [`ConfigurationTableServicesExt::get`]. Neither method takes a raw pointer or directly requires the
//! GUID, so they're relatively straightforward and simple to use. `get` is not `unsafe`, because the
//! service verifies the installed type before casting the pointer.
//!
//! [`ConfigTable`] only supports tables whose size is known at compile time, but that type can be a
//! self-describing header whose own field covers trailing data laid out after it in the same allocation.
//! See [`SampleDynamicHeader`] below, for an example. Tables whose header type isn't owned by the installing
//! code at all must fall back to using [`ConfigurationTableServices::install_table`] with a raw
//! [`ConfigTablePtr`](patina::component::service::uefi_services::config_table::ConfigTablePtr) instead.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    BinaryGuid,
    component::{
        component,
        service::{
            Service,
            uefi_services::config_table::{ConfigTable, ConfigurationTableServices, ConfigurationTableServicesExt},
        },
    },
    error::Result,
};

/// A sample vendor table laid out the way a typical firmware table would be as `#[repr(C)]`, with a
/// signature and version so consumers can validate it.
#[repr(C)]
pub struct SampleVendorTable {
    /// Four-character signature identifying the table (`b"PTNA"`).
    pub signature: [u8; 4],
    /// Table format version.
    pub version: u32,
    /// Number of vendor-defined entries that follow in the real table.
    pub entry_count: u32,
}

impl ConfigTable for SampleVendorTable {
    /// GUID under which the table is installed in the system table.
    const TABLE_GUID: BinaryGuid = BinaryGuid::from_string("0fedcba9-8765-4321-fedc-ba9876543210");
}

/// The table instance. It must be `'static` as the system table stores a pointer to it that outlives
/// this component's entry point.
static VENDOR_TABLE: SampleVendorTable = SampleVendorTable { signature: *b"PTNA", version: 1, entry_count: 0 };

/// Installs [`SampleVendorTable`] into the system configuration table, then reads it back to
/// confirm the table was installed correctly.
#[derive(Default)]
pub struct ConfigurationTableSample;

#[component]
impl ConfigurationTableSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, config: Service<dyn ConfigurationTableServices>) -> Result<()> {
        // Two methods are available to install configuration tables:
        //   1. `install` - As shown below. This will return `ConfigTableError::AlreadyExists` instead of
        //      silently replacing an existing table, if a table is already installed under
        //      `SampleVendorTable::TABLE_GUID`.
        //   2. `install_or_replace` - Replaces any existing table under the same GUID. This is useful
        //      for tables that may be updated or re-published during boot.
        config.install(&VENDOR_TABLE)?;
        log::info!("Installed SampleVendorTable v{}", VENDOR_TABLE.version);

        // Read it back. Note that `unsafe` is not needed as the service only returns a table here if it was
        // installed as `SampleVendorTable`, which is what `install` did above.
        if let Some(table) = config.get::<SampleVendorTable>() {
            log::info!(
                "Read back signature {:?}, {} entries",
                core::str::from_utf8(&table.signature).unwrap_or("????"),
                table.entry_count
            );
        }

        Ok(())
    }
}

const ENTRY_COUNT: u32 = 3;

/// A sample vendor table with a self-describing header where `total_len` covers this header plus the
/// `entries` that follow it in memory.
#[repr(C)]
pub struct SampleDynamicHeader {
    /// Four-character signature identifying the table (`b"PTNB"`).
    pub signature: [u8; 4],
    /// Total size of the table, in bytes, including this header and the trailing entries.
    pub total_len: u32,
    /// Number of trailing `u32` entries.
    pub entry_count: u32,
}

impl ConfigTable for SampleDynamicHeader {
    /// GUID under which the table is installed in the system table.
    const TABLE_GUID: BinaryGuid = BinaryGuid::from_string("1a2b3c4d-5e6f-4788-99aa-bbccddeeff00");

    fn table_len(&self) -> usize {
        self.total_len as usize
    }
}

/// The header and its trailing entries are laid out contiguously so `SampleDynamicHeader::table_len`
/// describes the whole allocation starting at `header`'s address.
#[repr(C)]
struct SampleDynamicTable {
    header: SampleDynamicHeader,
    entries: [u32; ENTRY_COUNT as usize],
}

static DYNAMIC_TABLE: SampleDynamicTable = SampleDynamicTable {
    header: SampleDynamicHeader {
        signature: *b"PTNB",
        total_len: core::mem::size_of::<SampleDynamicTable>() as u32,
        entry_count: ENTRY_COUNT,
    },
    entries: [10, 20, 30],
};

/// Installs [`SampleDynamicHeader`] into the system configuration table. This pattern can be used for a
/// self-describing header type, with trailing data laid out after it in the same allocation.
#[derive(Default)]
pub struct DynamicConfigurationTableSample;

#[component]
impl DynamicConfigurationTableSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, config: Service<dyn ConfigurationTableServices>) -> Result<()> {
        config.install(&DYNAMIC_TABLE.header)?;
        log::info!("Installed SampleDynamicHeader with {} entries", DYNAMIC_TABLE.header.entry_count);

        // SAFETY: `table_len` is implemented to report `DYNAMIC_TABLE`'s allocated size (header plus entries),
        // matching what was installed above.
        if let Some(bytes) = unsafe { config.get_bytes::<SampleDynamicHeader>() } {
            let entries = bytes.get(core::mem::size_of::<SampleDynamicHeader>()..).unwrap_or(&[]);
            log::info!("Read back {} bytes, including the trailing entries: {:?}", bytes.len(), entries);
        }

        Ok(())
    }
}
