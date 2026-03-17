//! ACPI Service Platform Integration Tests.
//!
//! Defines basic integration tests for the ACPI service interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::{ffi::c_void, mem};

use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::service::Service,
    test::patina_test,
    u_assert, u_assert_eq,
};
use r_efi::efi;

use crate::{
    acpi_protocol::{AcpiGetProtocol, AcpiTableProtocol},
    acpi_table::AcpiTableHeader,
    service::AcpiTableManager,
    signature::{self, ACPI_VERSIONS_GTE_2},
};

#[repr(C)]
#[derive(Clone)]
struct MockSmallTable {
    _header: AcpiTableHeader,
}

#[repr(C)]
#[derive(Clone, Default)]
struct MockLargeTable {
    header: AcpiTableHeader,
    data: [u8; 32],
}

#[coverage(off)]
#[patina_test]
fn acpi_test(table_manager: Service<AcpiTableManager>) -> patina::test::Result {
    let original_length = table_manager.iter_tables().len();

    // Install a dummy ACPI table.
    let mock_table1 = MockSmallTable {
        _header: AcpiTableHeader {
            signature: 0x12341234,
            length: mem::size_of::<MockSmallTable>() as u32,
            ..Default::default()
        },
    };

    // SAFETY: The constructed table is a valid ACPI table.
    let key1 = unsafe { table_manager.install_acpi_table(mock_table1) }.expect("Should install table.");

    // Install another table.
    let mock_table2 = MockLargeTable {
        header: AcpiTableHeader {
            signature: 0x43214321,
            length: mem::size_of::<MockLargeTable>() as u32,
            ..Default::default()
        },
        data: [1; 32],
    };

    // SAFETY: The constructed table is a valid ACPI table.
    let key2 = unsafe { table_manager.install_acpi_table(mock_table2) }.expect("Should install table.");

    // Install an invalid ACPI table (too small).
    let invalid_table =
        AcpiTableHeader { signature: signature::MADT, length: (signature::MADT_SIZE - 2) as u32, ..Default::default() };
    // SAFETY: invalid_table has a valid layout, but an invalid length value, so this should return an error.
    u_assert!(unsafe { table_manager.install_acpi_table(invalid_table) }.is_err(), "Should not install invalid table.");

    // Verify only valid tables are in the iterator.
    let tables = table_manager.iter_tables();
    u_assert!(tables.len() == original_length + 2, "Should have two more tables than original.");
    u_assert!(tables.iter().any(|t| t.signature() == 0x12341234));
    u_assert!(tables.iter().any(|t| t.signature() == 0x43214321));

    // Get the complex table and verify its trailing contents are preserved.
    let retrieved_mocktable2 = table_manager.get_acpi_table::<MockLargeTable>(key2).expect("Should get mock table.");
    u_assert_eq!(retrieved_mocktable2.header.signature(), 0x43214321, "Signature should match mock table.");
    u_assert_eq!(retrieved_mocktable2.data, [1; 32], "Data should match mock table.");

    // Uninstall the tables for cleanup (and tests uninstall).
    table_manager.uninstall_acpi_table(key1).expect("Delete should succeed");
    table_manager.uninstall_acpi_table(key2).expect("Delete should succeed");

    // get() should now fail.
    u_assert!(table_manager.get_acpi_table::<MockSmallTable>(key1).is_err(), "Table should no longer be accessible");
    u_assert!(table_manager.get_acpi_table::<MockLargeTable>(key2).is_err(), "Table should no longer be accessible");

    Ok(())
}

#[coverage(off)]
#[patina_test]
fn acpi_protocol_test(bs: StandardBootServices) -> patina::test::Result {
    // SAFETY: there is only one reference to the `AcpiTableProtocol` during this test.
    let table_protocol =
        unsafe { bs.locate_protocol::<AcpiTableProtocol>(None) }.expect("Locate protocol should succeed.");
    // SAFETY: there is only one reference to the `AcpiGetProtocol` during this test.
    let acpi_get_protocol =
        unsafe { bs.locate_protocol::<AcpiGetProtocol>(None) }.expect("Locate protocol should succeed.");

    let mut table_key_buf: usize = 0;

    // Install a dummy table using the ACPI Table Protocol.
    (table_protocol.install_table)(
        table_protocol as *const AcpiTableProtocol,
        &MockLargeTable {
            header: AcpiTableHeader {
                signature: 0x12341234,
                length: mem::size_of::<MockLargeTable>() as u32,
                ..Default::default()
            },
            data: [2; 32],
        } as *const _ as *const c_void,
        mem::size_of::<MockLargeTable>(),
        &mut table_key_buf as *mut usize,
    );

    u_assert!(table_key_buf > 0, "Table key should be set after install");

    // Verify the table can be retrieved.
    let mut table_buf = MockLargeTable::default();
    let mut table_buf = &mut table_buf as *mut MockLargeTable as *mut AcpiTableHeader;
    let mut table_idx = 0;
    let mut get_supported_table_versions: u32 = 0;
    let mut get_table_key = 0;
    loop {
        let get_result = (acpi_get_protocol.get_table)(
            table_idx,
            &mut table_buf as *mut *mut AcpiTableHeader,
            &mut get_supported_table_versions,
            &mut get_table_key,
        );
        // We should be able to find our installed table.
        if get_result != efi::Status::SUCCESS {
            // If fails, either hit an error on a previous table or reached the end of the list without finding the installed table.
            // Both are error cases.
            u_assert!(false, "Get table should succeed for installed table");
        }

        // SAFETY: `table_buf` is valid and directly constructed from the dummy table.
        if unsafe { (*table_buf).signature } == 0x12341234 {
            break;
        }

        table_idx += 1;
    }

    // SAFETY: `table_buf` is valid and directly constructed from the dummy table.
    let retrieved_table = unsafe { &*table_buf };
    u_assert_eq!(retrieved_table.signature(), 0x12341234, "Signature should match installed table.");
    u_assert_eq!(get_supported_table_versions, ACPI_VERSIONS_GTE_2, "Should support ACPI version 2.0+");
    u_assert_eq!(get_table_key, table_key_buf, "Table key should match installed key");

    // We should be able to access the normal table fields.
    // SAFETY: We know that the table_buf points to an MockLargeTable (constructed above).
    let large_table = unsafe { &*(table_buf as *const MockLargeTable) };
    u_assert_eq!(large_table.data, [2; 32], "Data should match installed table.");

    // Verify the table can be uninstalled.
    let uninstall_result = (table_protocol.uninstall_table)(table_protocol as *const AcpiTableProtocol, get_table_key);
    u_assert_eq!(uninstall_result, efi::Status::SUCCESS, "Uninstall should succeed");

    Ok(())
}
