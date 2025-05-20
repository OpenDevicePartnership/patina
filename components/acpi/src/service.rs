use crate::acpi_table::AcpiInstallable;
use crate::acpi_table::AcpiTable;
use crate::error::AcpiError;

pub type TableKey = usize;

pub type AcpiNotifyFn = fn(&AcpiTable, u32, TableKey) -> Result<(), AcpiError>;

pub trait AcpiProvider {
    fn install_acpi_table(&self, acpi_table: &dyn AcpiInstallable) -> Result<TableKey, AcpiError>;
    fn uninstall_acpi_table(&self, table_key: TableKey) -> Result<(), AcpiError>;
    fn get_acpi_table(&self, index: usize) -> Result<&AcpiTable, AcpiError>;
    fn register_notify(&self, should_register: bool, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError>;

    fn iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a AcpiTable> + 'a>;
}
