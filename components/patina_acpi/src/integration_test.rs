//! ACPI Service Q35 Integration Test.
//!
//! Defines basic integration tests for the ACPI service interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent

use core::ffi::c_void;
use core::mem;

use patina_sdk::boot_services::BootServices;
use patina_sdk::test::patina_test;
use patina_sdk::{boot_services::StandardBootServices, component::service::Service};
use r_efi::efi;

use crate::acpi::ACPI_TABLE_INFO;
use crate::acpi_protocol::{AcpiSdtProtocol, AcpiTableProtocol};
use crate::signature::ACPI_VERSIONS_GTE_2;
use crate::{
    acpi_table::{AcpiFacs, AcpiFadt, AcpiTableHeader},
    service::AcpiTableManager,
    signature::{self},
};

#[patina_test]
fn acpi_test(table_manager: Service<AcpiTableManager>) -> patina_sdk::test::Result {
    // Install a dummy FADT.
    // The FADT is treated as a normal ACPI table and should be added to the list of installed tables.
    let dummy_header =
        AcpiTableHeader { signature: signature::FADT, length: mem::size_of::<AcpiFadt>() as u32, ..Default::default() };
    let dummy_fadt = AcpiFadt { header: dummy_header, ..Default::default() };

    let table_key = unsafe { table_manager.install_acpi_table(dummy_fadt) }.expect("Should install dummy FADT.");

    // Install a FACS table (special case — not iterated over).
    let facs = AcpiFacs { signature: signature::FACS, length: mem::size_of::<AcpiFacs>() as u32, ..Default::default() };
    assert!(unsafe { table_manager.install_acpi_table(facs) }.is_ok(), "Should install FACS table.");

    // Verify only the FADT is in the iterator.
    let tables = table_manager.iter_tables();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].signature(), signature::FADT);

    // Get the dummy FADT and verify its contents.
    let fadt = table_manager.get_acpi_table::<AcpiFadt>(table_key).expect("Should get dummy FADT");
    assert_eq!(fadt.header.signature, signature::FADT, "Signature should match dummy FADT");
    assert!(fadt.x_firmware_ctrl() > 0, "Should have installed FACS");

    // Uninstall the dummy table.
    table_manager.uninstall_acpi_table(table_key).expect("Delete should succeed");

    // get(0) should now fail.
    assert!(table_manager.get_acpi_table::<AcpiFadt>(table_key).is_err(), "Table should no longer be accessible");

    Ok(())
}

#[patina_test]
fn acpi_protocol_test(bs: StandardBootServices) -> patina_sdk::test::Result {
    // Hack that is necessary since all tests share a global `ACPI_TABLE_INFO`.
    ACPI_TABLE_INFO.acpi_tables.write().clear();

    let table_protocol =
        unsafe { bs.locate_protocol::<AcpiTableProtocol>(None) }.expect("Locate protocol should succeed.");
    let sdt_protocol = unsafe { bs.locate_protocol::<AcpiSdtProtocol>(None) }.expect("Locate protocol should succeed.");

    let mut table_key_buf: usize = 0;

    // Install a dummy FADT using the ACPI Table Protocol.
    (table_protocol.install_table)(
        table_protocol as *const AcpiTableProtocol,
        &AcpiFadt {
            header: AcpiTableHeader {
                signature: signature::FADT,
                length: mem::size_of::<AcpiFadt>() as u32,
                ..Default::default()
            },
            ..Default::default()
        } as *const _ as *const c_void,
        mem::size_of::<AcpiFadt>(),
        &mut table_key_buf as *mut usize,
    );

    assert!(table_key_buf > 0, "Table key should be set after install");

    // Verify the table can be retrieved.
    let mut fadt_buf = AcpiFadt::default();
    let mut table_buf = &mut fadt_buf as *mut AcpiFadt as *mut AcpiTableHeader;
    let table_idx = 0; // We only installed one table, so index 0 should work.
    let mut get_supported_table_versions: u32 = 0;
    let mut get_table_key = 0;
    let get_result = (sdt_protocol.get_table)(
        table_idx,
        &mut table_buf as *mut *mut AcpiTableHeader,
        &mut get_supported_table_versions,
        &mut get_table_key,
    );
    assert_eq!(get_result, efi::Status::SUCCESS, "Get table should succeed");
    let retrieved_table = unsafe { &*table_buf };
    assert_eq!(retrieved_table.signature, signature::FADT, "Signature should match installed FADT");
    assert_eq!(get_supported_table_versions, ACPI_VERSIONS_GTE_2, "Should support ACPI version 2.0+");
    assert_eq!(get_table_key, table_key_buf, "Table key should match installed key");

    // We should be able to access the normal FADT fields.
    #[allow(invalid_reference_casting)]
    let retrieved_fadt = unsafe { &*(table_buf as *const AcpiFadt) };
    // We haven't installed a FACS, so this should be zero, but still accessible.
    assert_eq!(retrieved_fadt.x_firmware_ctrl(), 0);

    // Verify the table can be uninstalled.
    let uninstall_result = (table_protocol.uninstall_table)(table_protocol as *const AcpiTableProtocol, get_table_key);
    assert_eq!(uninstall_result, efi::Status::SUCCESS, "Uninstall should succeed");

    Ok(())
}
