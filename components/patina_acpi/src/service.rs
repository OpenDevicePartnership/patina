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
use core::any::{Any, TypeId};

use alloc::vec::Vec;
use patina_sdk::component::service::memory::MemoryManager;
use patina_sdk::component::service::{IntoService, Service};

use crate::acpi_table::{AcpiFacs, AcpiTable, AcpiTableHeader, StandardAcpiTable};
use crate::error::AcpiError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableKey(pub(crate) usize);

pub type AcpiNotifyFn = fn(&AcpiTableHeader, u32, TableKey) -> Result<(), AcpiError>;

/// The `AcpiTableManager` provides an interface for installing, uninstalling, and accessing ACPI tables.
/// This struct serves as the API by which external components can access ACPI services.
#[derive(IntoService)]
#[service(AcpiTableManager)]
pub struct AcpiTableManager {
    provider_service: Service<dyn AcpiProvider>,
    memory_manager: Service<dyn MemoryManager>,
}

impl AcpiTableManager {
    /// Installs an ACPI table.
    ///
    /// `table` should point to an ACPI table with a standard ACPI header, followed by any additional trailing bytes specific to the table.
    /// The `length` field of the `AcpiTableHeader` must be set to the total size of the table, including the header and any trailing bytes.
    ///
    /// The table, unless it is the FACS or DSDT, will be added to the list of installed tables in the XSDT.
    /// (The FACS and DSDT are accessible only through fields in the FADT.)
    ///
    /// CAUTION: This implementation of ACPI prevents duplicate installations of the XSDT, FADT, FACS, and DSDT.
    /// Attempts to install a duplicate of the listed tables will result in a failed installation.
    ///
    /// The returned `TableKey` can be used to uninstall the table later.
    /// It is an opaque reference to the table and should not be manipulated directly.
    ///
    /// ## SAFETY
    /// - Caller must ensure the provided table, `T`, has a C compatible layout (typically using `#[repr(C)]`).
    /// - Caller must ensure that the table's first field is [AcpiTableHeader].
    pub unsafe fn install_acpi_table<T>(&self, table: &T) -> Result<TableKey, AcpiError> {
        let acpi_table = unsafe { AcpiTable::new(table, &self.memory_manager) };
        self.provider_service.install_acpi_table(acpi_table)
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
    pub fn get_acpi_table<T: 'static + Clone>(&self, table_key: TableKey) -> Result<&T, AcpiError> {
        let acpi_table = self.provider_service.get_acpi_table(table_key)?;

        // There may be ACPI tables whose type is unknown at installation, due to installation from the HOB or a C protocol.
        // In these cases, the type is is unspecified (AcpiTableHeader instead of a specific table type), so we skip type checking.
        // In all other cases, verify the type provided by the user is valid.
        if acpi_table.type_id() != TypeId::of::<AcpiTableHeader>() && acpi_table.type_id() != TypeId::of::<T>() {
            return Err(AcpiError::InvalidTableType);
        }

        // SAFETY: The type id of the returned table has been verified.
        // SAFETY: The installed tables are stored in the provider and live at least as long as `self`,
        let raw_table_ptr: *const T = acpi_table
            .table // this is your NonNull<Table>
            .cast::<T>() // reinterpret it as NonNull<T>
            .as_ptr(); // get a *const T

        Ok(unsafe { &*raw_table_ptr })
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
    pub fn iter(&self) -> impl Iterator<Item = &AcpiTableHeader> {
        let wrapper_vec = self.provider_service.iter_tables().collect();
        wrapper_vec.iter().map(|tbl| tbl.header())
    }
}

/// The `AcpiTableManager` provides functionality for installing, uninstalling, and accessing ACPI tables.
/// This struct serves as the API by which internal implementations can provide custom ACPI implementation.
pub trait AcpiProvider {
    /// Installs an ACPI table and returns an associated key which can be used to get or uninstall the table later.
    fn install_acpi_table(&self, acpi_table: AcpiTable) -> Result<TableKey, AcpiError>;

    /// Uninstalls an ACPI table using the same `table_key` returned at the time of installation.
    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError>;

    /// Retrieves an ACPI table by its table key. This must be the same key returned at the time of installation.
    fn get_acpi_table(&self, table_key: TableKey) -> Result<AcpiTable, AcpiError>;

    /// Registers or unregisters a function which will be called whenever a new ACPI table is installed.
    fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError>;

    /// Returns all currently installed tables in an iterable format.
    fn iter_tables(&self) -> Vec<AcpiTable>;
}
