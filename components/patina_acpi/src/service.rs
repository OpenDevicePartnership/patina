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
use alloc::vec::Vec;

use crate::acpi_table::{AcpiFacs, AcpiTableHeader, MemoryAcpiTable};
use crate::error::AcpiError;

pub type TableKey = usize;

pub type AcpiNotifyFn = fn(&AcpiTableHeader, u32, TableKey) -> Result<(), AcpiError>;

/// The `AcpiProvider` trait provides an interface for installing, uninstalling, and accessing ACPI tables.
/// This trait serves as the API by which both internal code and external components can access ACPI services.
pub trait AcpiProvider {
    /// Installs an ACPI table.
    ///
    /// The table can be installed in either NVS or ACPI reclaim memory, depending on platform settings.
    /// `acpi_table` should point to a valid ACPI table header, followed by any additional trailing bytes specific to the table.
    /// The `length` field of the `AcpiTableHeader` must be set to the total size of the table, including the header and any trailing bytes.
    ///
    /// The table will be added to the list of installed tables in the XSDT.
    ///
    /// The returned `TableKey` can be used to uninstall the table later.
    /// It is an opaque reference to the table and should not be manipulated directly.
    fn install_acpi_table(&self, acpi_table: &AcpiTableHeader) -> Result<TableKey, AcpiError>;

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
    fn install_facs(&self, acpi_table: &AcpiFacs) -> Result<(), AcpiError>;

    /// Uninstalls an ACPI table.
    ///
    /// The `table_key` is the opaque reference returned by `install_acpi_table`.
    ///
    /// This function will remove the table from the XSDT and free the memory associated with it.
    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError>;

    /// Retrieves an ACPI table by its index.
    ///
    /// `index` is zero-based index of the table in the XSDT.
    /// The correct `index` value can be discovered by using the `iter` method, along with appropriate filters.
    ///
    /// For example, to retrieve a table by tablekey:
    /// let idx = acpi_tables.iter().position(|&table| unsafe { table.as_ref().table_key } == table_key);
    /// acpi_provider.get_acpi_table(idx.unwrap());
    ///
    /// The returned `&AcpiTable` is reference to the table in ACPI memory.
    ///
    /// The RSDP and XSDT cannot be accessed through `get_acpi_table`.
    fn get_acpi_table(&self, index: usize) -> Result<MemoryAcpiTable, AcpiError>;

    /// Registers or unregisters a function which will be called whenever a new ACPI table is installed.
    ///
    /// If `should_register` is true, it will register the function.
    /// Otherwise, it will unregister the function if it exists in the current notify list.
    fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError>;

    /// Returns an iterator over the installed ACPI tables.
    ///
    /// This can be used in place of `get_acpi_table`, or in conjunction with it to retrieve a specific table reference.
    ///
    /// The RSDP and XSDT are not included in the list of iterable ACPI tables.
    fn iter(&self) -> Vec<MemoryAcpiTable>;
}
