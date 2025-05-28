use patina_sdk::boot_services::StandardBootServices;
use patina_sdk::component::service::{memory::MemoryManager, Service};
use patina_sdk::test::patina_test;

use crate::{
    acpi::StandardAcpiProvider,
    acpi_table::{AcpiFacs, AcpiTable},
    service::AcpiProvider,
    signature::{self, ACPI_HEADER_LEN},
};

#[patina_test]
fn acpi_test(
    // _provider_service: Service<dyn AcpiProvider>,
    memory_manager: Service<dyn MemoryManager>,
    bs: StandardBootServices,
) -> patina_sdk::test::Result {
    let provider = StandardAcpiProvider::new_uninit();
    provider.initialize(2, true, bs, memory_manager);

    println!("ACPI TEST");

    // Install a regular dummy ACPI table
    let dummy_signature = u32::from_le_bytes(*b"DEMO");
    let mut dummy_table = AcpiTable {
        signature: dummy_signature,
        length: ACPI_HEADER_LEN as u32,
        revision: 1,
        checksum: 0,
        oem_id: *b"OEMID ",
        oem_table_id: *b"TABLID  ",
        oem_revision: 1,
        creator_id: 0,
        creator_revision: 0,
        ..Default::default()
    };

    let key = provider.install_acpi_table(&mut dummy_table).expect("Should install dummy table");
    assert!(key > 0, "Table key should be greater than zero");

    // Install a FACS table (special case — not iterated over)
    let mut facs = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
    let key = provider.install_acpi_table(&mut facs).expect("Should install FACS");
    assert_eq!(key, 0, "Table key should be zero for FACS");

    // Verify only the dummy table is in the iterator
    let tables: Vec<&AcpiTable> = provider.iter().collect();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].signature, dummy_signature);

    // Uninstall the dummy table
    provider.uninstall_acpi_table(dummy_table.table_key).expect("Delete should succeed");

    // get(0) should now fail
    assert!(provider.get_acpi_table(0).is_err(), "Table should no longer be accessible");

    Ok(())
}
