//! ACPI Service Q35 Integration Test.
//!
//! Defines basic integration tests for the ACPI service interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent

use patina_sdk::component::service::Service;
use patina_sdk::test::patina_test;

use crate::{
    acpi_table::{AcpiDsdt, AcpiFacs, AcpiFadt, AcpiTableHeader},
    service::AcpiTableManager,
    signature::{self, ACPI_HEADER_LEN},
};

#[patina_test]
fn acpi_test(table_manager: Service<AcpiTableManager>) -> patina_sdk::test::Result {
    // Install a dummy FADT.
    // The FADT is treated as a normal ACPI table and should be added to the list of installed tables.
    let dummy_header =
        AcpiTableHeader { signature: signature::FADT, length: ACPI_HEADER_LEN as u32, ..Default::default() };
    let dummy_fadt = AcpiFadt { header: dummy_header, ..Default::default() };

    let table_key = unsafe { table_manager.install_acpi_table(&dummy_fadt) }.expect("Should install dummy table");

    // Install a FACS table (special case — not iterated over).
    let facs = AcpiFacs { signature: signature::FACS, length: 64, ..Default::default() };
    assert!(unsafe { table_manager.install_acpi_table(&facs) }.is_ok());

    // Verify only the FADT is in the iterator.
    let tables = table_manager.iter_tables();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].signature(), signature::FADT);

    // Get the dummy FADT and verify its contents.
    let fadt = table_manager.get_acpi_table::<AcpiFadt>(table_key).expect("Should get dummy FADT");
    assert_eq!(fadt.header.signature, signature::FADT, "Signature should match dummy FADT");
    assert!(fadt.x_firmware_ctrl() > 0, "Should have installed FACS");

    // Attempt to get the FADT with the wrong table type (should fail).
    let bad_fadt = table_manager.get_acpi_table::<AcpiDsdt>(table_key);
    assert!(bad_fadt.is_err(), "Incorrect type provided. Should fail.");

    // Uninstall the dummy table.
    table_manager.uninstall_acpi_table(table_key).expect("Delete should succeed");

    // get(0) should now fail.
    assert!(table_manager.get_acpi_table::<AcpiFadt>(table_key).is_err(), "Table should no longer be accessible");

    Ok(())
}
