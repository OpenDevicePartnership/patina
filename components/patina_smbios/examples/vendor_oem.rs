use patina_smbios::smbios_derive::{SMBIOS_HANDLE_PI_RESERVED, SmbiosManager, SmbiosRecords, SmbiosTableHeader};
use patina_smbios::smbios_record::{
    FieldInfo, FieldLayout, FieldType, SmbiosFieldLayout, SmbiosRecordStructure, Type2BaseboardInformation,
};
use std::string::String;
use std::vec::Vec;

// Minimal OEM record example (record types 0x80-0xFF reserved for vendor specific records)
pub struct VendorOemRecord {
    pub header: SmbiosTableHeader,
    pub oem_field: u32,
    pub string_pool: Vec<String>,
}

impl SmbiosFieldLayout for VendorOemRecord {
    fn field_layout() -> FieldLayout {
        // We place oem_field immediately after the header
        FieldLayout {
            fields: vec![FieldInfo {
                name: "oem_field",
                field_type: FieldType::U32(core::mem::size_of::<SmbiosTableHeader>()),
            }],
        }
    }
}

impl SmbiosRecordStructure for VendorOemRecord {
    const RECORD_TYPE: u8 = 0x80; // vendor-specific type

    fn validate(&self) -> Result<(), patina_smbios::smbios_derive::SmbiosError> {
        // basic validation
        Ok(())
    }

    fn string_pool(&self) -> &[String] {
        &self.string_pool
    }
    fn string_pool_mut(&mut self) -> &mut Vec<String> {
        &mut self.string_pool
    }
}

fn main() {
    // Build manager
    let mut manager = SmbiosManager::new(3, 8);

    // Example 1: Vendor OEM Record
    let rec = VendorOemRecord {
        header: SmbiosTableHeader::new(VendorOemRecord::RECORD_TYPE, 0, SMBIOS_HANDLE_PI_RESERVED),
        oem_field: 0xDEADBEEF,
        string_pool: vec![String::from("Vendor Extra")],
    };

    // Serialize and add (pattern same as unit test)
    let bytes = rec.to_bytes();
    let header_size = core::mem::size_of::<SmbiosTableHeader>();
    let record_header: SmbiosTableHeader =
        unsafe { core::ptr::read_unaligned(bytes[..header_size].as_ptr() as *const SmbiosTableHeader) };

    let mut handle = SMBIOS_HANDLE_PI_RESERVED;
    // Bring trait into scope so `add` and `get_next` methods are available on the manager
    use patina_smbios::smbios_derive::SmbiosRecords;

    manager.add(None, &mut handle, &record_header).expect("add failed");

    // Example 2: Type 2 Baseboard Information Record
    let baseboard_rec = Type2BaseboardInformation {
        header: SmbiosTableHeader::new(2, 0, SMBIOS_HANDLE_PI_RESERVED),
        manufacturer: 1,             // String 1: "ACME Corporation"
        product: 2,                  // String 2: "Motherboard Model X"
        version: 3,                  // String 3: "Rev 1.0"
        serial_number: 4,            // String 4: "MB123456789"
        asset_tag: 5,                // String 5: "Asset001"
        feature_flags: 0x01,         // Feature flags (bit 0 = board is a hosting board)
        location_in_chassis: 6,      // String 6: "Slot 1"
        chassis_handle: 0x0003,      // Handle of containing chassis
        board_type: 0x0A,            // Motherboard type
        contained_object_handles: 0, // No contained object handles for this example
        string_pool: vec![
            String::from("ACME Corporation"),
            String::from("Motherboard Model X"),
            String::from("Rev 1.0"),
            String::from("MB123456789"),
            String::from("Asset001"),
            String::from("Slot 1"),
        ],
    };

    let baseboard_bytes = baseboard_rec.to_bytes();
    let baseboard_header: SmbiosTableHeader =
        unsafe { core::ptr::read_unaligned(baseboard_bytes[..header_size].as_ptr() as *const SmbiosTableHeader) };

    let mut baseboard_handle = SMBIOS_HANDLE_PI_RESERVED;
    manager.add(None, &mut baseboard_handle, &baseboard_header).expect("baseboard add failed");

    // Verify both records were added
    let mut search = SMBIOS_HANDLE_PI_RESERVED;
    let (found, _) = manager.get_next(&mut search, Some(VendorOemRecord::RECORD_TYPE)).expect("get_next failed");
    assert_eq!(found.record_type, VendorOemRecord::RECORD_TYPE);
    println!("Added vendor record handle: {}", search);

    search = SMBIOS_HANDLE_PI_RESERVED;
    let (found_baseboard, _) = manager.get_next(&mut search, Some(2)).expect("get_next failed for baseboard");
    assert_eq!(found_baseboard.record_type, 2);
    println!("Added Type 2 baseboard record handle: {}", search);
}
