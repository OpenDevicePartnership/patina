use patina_sdk::component::service::Service;
use patina_sdk::test::patina_test;

use crate::{
    acpi_table::{AcpiFacs, AcpiInstallable, AcpiTable, AcpiTableWrapper},
    alloc::vec::Vec,
    service::AcpiProvider,
    signature::{self, ACPI_HEADER_LEN},
};

#[patina_test]
fn acpi_test(provider: Service<dyn AcpiProvider>) -> patina_sdk::test::Result {
    // Install a regular dummy ACPI table
    let dummy_signature = u32::from_le_bytes(*b"DEMO");
    let dummy_header = AcpiTable {
        signature: dummy_signature,
        length: ACPI_HEADER_LEN as u32,
        revision: 1,
        checksum: 0,
        oem_id: *b"123456",
        oem_table_id: *b"12345678",
        oem_revision: 1,
        creator_id: 0,
        creator_revision: 0,
    };
    let dummy_table = AcpiTableWrapper { header: dummy_header, ..Default::default() };

    let key = provider.install_acpi_table(&dummy_table).expect("Should install dummy table");
    assert!(key > 0, "Table key should be greater than zero");

    // Install a FACS table (special case — not iterated over)
    let facs = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
    let facs_key = provider.install_acpi_table(&facs).expect("Should install FACS");
    assert_eq!(facs_key, 0, "Table key should be zero for FACS");

    // Verify only the dummy table is in the iterator
    let tables: Vec<&AcpiTableWrapper> = provider.iter().collect();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].signature(), dummy_signature);

    // Uninstall the dummy table
    provider.uninstall_acpi_table(key).expect("Delete should succeed");

    // get(0) should now fail
    assert!(provider.get_acpi_table(0).is_err(), "Table should no longer be accessible");

    Ok(())
}
