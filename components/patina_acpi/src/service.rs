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
use crate::acpi_table::{AcpiInstallable, AcpiTableWrapper};
use crate::alloc::boxed::Box;
use crate::error::AcpiError;

pub type TableKey = usize;

pub type AcpiNotifyFn = fn(&AcpiTableWrapper, u32, TableKey) -> Result<(), AcpiError>;

/// The `AcpiProvider` trait provides an interface for installing, uninstalling, and accessing ACPI tables.
/// This trait serves as the API by which both internal code and external components can access ACPI services.
pub trait AcpiProvider {
    /// Installs an ACPI table.
    ///
    /// The `acpi_table` must implement the `AcpiInstallable` trait.
    /// Of the existing table types, the generic `AcpiTable` type and `AcpiFacs` implement `AcpiInstallable`.
    /// If using another table type than the two aforementioned tables, the caller must implement `AcpiInstallable`.
    ///
    /// If the table is already present in memory (such as an FACS being installed from a PEI HOB),
    /// `install_acpi_table` will use the already-allocated ACPI memory.
    /// Otherwise, it will place the table in ACPI memory as appropriate.
    ///
    /// For all types other than the DSDT and FACS (which are pointed to by the FADT),
    /// the table will be added to the XSDT.
    ///
    /// The returned `TableKey` can be used to uninstall the table later.
    /// It is an opaque reference to the table and should not be manipulated directly.
    fn install_acpi_table(&self, acpi_table: &dyn AcpiInstallable) -> Result<TableKey, AcpiError>;

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
    fn get_acpi_table(&self, index: usize) -> Result<&AcpiTableWrapper, AcpiError>;

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
    fn iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a AcpiTableWrapper> + 'a>;
}
