//! Vendor OEM SMBIOS Record Tests
//!
//! Tests for custom vendor-specific SMBIOS record creation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina_smbios::manager::{SMBIOS_HANDLE_PI_RESERVED, SmbiosManager, SmbiosRecords, SmbiosTableHeader};
use patina_smbios::smbios_record::{FieldInfo, FieldLayout, FieldType, SmbiosFieldLayout, SmbiosRecordStructure};
use std::string::String;
use std::vec::Vec;

// Recreate example's minimal OEM record in test form
pub struct VendorOemRecord {
    pub header: SmbiosTableHeader,
    pub oem_field: u32,
    pub string_pool: Vec<String>,
}

impl SmbiosFieldLayout for VendorOemRecord {
    fn field_layout() -> FieldLayout {
        FieldLayout {
            fields: vec![FieldInfo {
                name: "oem_field",
                field_type: FieldType::U32(core::mem::size_of::<SmbiosTableHeader>()),
            }],
        }
    }
}

impl SmbiosRecordStructure for VendorOemRecord {
    const RECORD_TYPE: u8 = 0x80;
    fn validate(&self) -> Result<(), patina_smbios::manager::SmbiosError> {
        Ok(())
    }
    fn string_pool(&self) -> &[String] {
        &self.string_pool
    }
    fn string_pool_mut(&mut self) -> &mut Vec<String> {
        &mut self.string_pool
    }
}

#[test]
fn example_vendor_oem_adds_to_manager() {
    let manager = SmbiosManager::new(3, 8);

    let rec = VendorOemRecord {
        header: SmbiosTableHeader::new(VendorOemRecord::RECORD_TYPE, 0, SMBIOS_HANDLE_PI_RESERVED),
        oem_field: 0xDEADBEEF,
        string_pool: vec![String::from("Vendor Extra")],
    };

    let bytes = rec.to_bytes();
    let _handle = manager.add_from_bytes(None, &bytes).expect("add_from_bytes failed");

    let mut search = SMBIOS_HANDLE_PI_RESERVED;
    let (found, _) = manager.get_next(&mut search, Some(VendorOemRecord::RECORD_TYPE)).expect("get_next failed");
    assert_eq!(found.record_type, VendorOemRecord::RECORD_TYPE);
}
