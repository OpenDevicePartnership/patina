//! ACPI Service Definitions.
//!
//! Defines the ACPI Provider for use as a service.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::any::TypeId;

use alloc::vec::Vec;
use patina_sdk::component::service::{IntoService, Service};

use crate::acpi_protocol::CAcpiTable;
use crate::acpi_table::{AcpiFacs, AcpiTableHeader, MemoryAcpiTable, RawAcpiTable, StandardAcpiTable};
use crate::error::AcpiError;

pub type TableKey = usize;

pub type AcpiNotifyFn = fn(&AcpiTableHeader, u32, TableKey) -> Result<(), AcpiError>;

/// The `AcpiTableManager` provides an interface for installing, uninstalling, and accessing ACPI tables.
/// This struct serves as the API by which external components can access ACPI services.
#[derive(IntoService)]
#[service(AcpiTableManager)]
pub struct AcpiTableManager {
    provider_service: Service<dyn AcpiProvider>,
}

impl AcpiTableManager {
    /// Installs an ACPI table.
    ///
    /// The table can be installed in either NVS or ACPI reclaim memory, depending on platform settings.
    /// `acpi_table` should point to an ACPI table with a standard ACPI header, followed by any additional trailing bytes specific to the table.
    /// The `length` field of the `AcpiTableHeader` must be set to the total size of the table, including the header and any trailing bytes.
    ///
    /// The table will be added to the list of installed tables in the XSDT.
    ///
    /// The returned `TableKey` can be used to uninstall the table later.
    /// It is an opaque reference to the table and should not be manipulated directly.
    pub fn install_acpi_table<T: StandardAcpiTable>(&self, acpi_table: &T) -> Result<TableKey, AcpiError> {
        self.provider_service.install_acpi_table(acpi_table)
    }

    /// Installs the FACS.
    ///
    /// The FACS has a non-standard table format but can be dynamically installed during runtime,
    /// hence the need for a seperate installation function.
    ///
    /// If the table is already present in memory, `install_facs` will use the already-allocated ACPI memory.
    /// Otherwise, it will place the table in ACPI memory as appropriate.
    ///
    /// The FACS is pointed to by the FADT only, and is not present in the list of tables in the XSDT.
    ///
    /// Since the FACS is not directly accessible, it does not have an associated table key,
    /// and cannot be directly uninstalled using `uninstall_acpi_table`.
    pub fn install_facs(&self, acpi_table: &AcpiFacs) -> Result<(), AcpiError> {
        self.provider_service.install_facs(acpi_table)
    }

    /// Uninstalls an ACPI table.
    ///
    /// The `table_key` is the opaque reference returned by `install_acpi_table`.
    ///
    /// This function will remove the table from the XSDT and free the memory associated with it.
    pub fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError> {
        self.provider_service.uninstall_acpi_table(table_key)
    }

    /// Retrieves an ACPI table by its table key.
    ///
    /// The `table_key` is the opaque reference returned by `install_acpi_table`.
    /// The generic type `T` should be the expected type of the table.
    ///
    /// The RSDP and XSDT cannot be accessed through `get_acpi_table`.
    pub fn get_acpi_table<T: 'static>(&self, table_key: TableKey) -> Result<&T, AcpiError> {
        let memory_table = self.provider_service.get_acpi_table(table_key)?;

        // There may be ACPI tables whose type is unknown at installation, due to installation from the HOB or a C protocol.
        // In these cases, the `type_id` may not be valid, so we skip checking the type id.
        if memory_table.type_id != TypeId::of::<RawAcpiTable>()
            && memory_table.type_id != TypeId::of::<CAcpiTable>()
            && memory_table.type_id != TypeId::of::<T>()
        {
            return Err(AcpiError::InvalidTableType);
        }

        unsafe { Ok(memory_table.header.cast::<T>().as_ref()) }
    }

    /// Registers or unregisters a function which will be called whenever a new ACPI table is installed.
    ///
    /// If `should_register` is true, it will register the function.
    /// Otherwise, it will unregister the function if it exists in the current notify list.
    pub fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError> {
        self.provider_service.register_notify(should_register, notify_fn)
    }

    /// Returns an iterator over the installed ACPI tables.
    /// Each returned `AcpiTableHeader` points to the header of an ACPI table installed in ACPI memory.
    ///
    /// This can be used in place of `get_acpi_table`, or in conjunction with it to retrieve a specific table reference.
    ///
    /// The RSDP and XSDT are not included in the list of iterable ACPI tables.
    pub fn iter(&self) -> Vec<&AcpiTableHeader> {
        self.provider_service.iter()
    }
}

/// The `AcpiTableManager` provides functionality for installing, uninstalling, and accessing ACPI tables.
/// This struct serves as the API by which internal implementations can provide custom ACPI implementation.
pub trait AcpiProvider {
    /// Installs an ACPI table and returns an associated key which can be used to get or uninstall the table later.
    fn install_acpi_table(&self, acpi_table: &dyn StandardAcpiTable) -> Result<TableKey, AcpiError>;

    /// Installs the FACS.
    fn install_facs(&self, acpi_table: &AcpiFacs) -> Result<(), AcpiError>;

    /// Uninstalls an ACPI table using the same `table_key` returned at the time of installation.
    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError>;

    /// Retrieves an ACPI table by its table key. This must be the same key returned at the time of installation.
    fn get_acpi_table(&self, table_key: TableKey) -> Result<MemoryAcpiTable, AcpiError>;

    /// Registers or unregisters a function which will be called whenever a new ACPI table is installed.
    fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError>;

    /// Returns an iterator over the installed ACPI tables.
    fn iter(&self) -> Vec<&AcpiTableHeader>;
}
