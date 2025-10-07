use log::info;
use patina_smbios::smbios_derive::{SMBIOS_HANDLE_PI_RESERVED, SmbiosManager, SmbiosTableHeader};
use patina_smbios::smbios_record::{
    FieldInfo, FieldLayout, FieldType, SmbiosFieldLayout, SmbiosRecordStructure, Type0PlatformFirmwareInformation,
    Type1SystemInformation, Type2BaseboardInformation, Type3SystemEnclosure,
};
use std::string::String;
use std::vec::Vec;
use zerocopy::FromBytes;

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
    let record_header =
        SmbiosTableHeader::ref_from_bytes(&bytes[..header_size]).expect("Failed to parse SMBIOS header");

    // Bring trait into scope so `add` and `get_next` methods are available on the manager
    use patina_smbios::smbios_derive::SmbiosRecords;

    let _handle = unsafe { manager.add(None, record_header).expect("add failed") };

    // Example 2: Type 0 BIOS Information Record
    let bios_rec = Type0PlatformFirmwareInformation {
        header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
        vendor: 1,                             // String 1: "ACME BIOS Corp"
        firmware_version: 2,                   // String 2: "v2.4.1"
        bios_starting_address_segment: 0xE000, // Standard BIOS segment
        firmware_release_date: 3,              // String 3: "09/26/2025"
        firmware_rom_size: 0x0F,               // 1MB ROM size
        characteristics: 0x08,                 // PCI supported
        characteristics_ext1: 0x03,            // ACPI supported + USB legacy
        characteristics_ext2: 0x01,            // UEFI specification supported
        system_bios_major_release: 2,          // BIOS major version
        system_bios_minor_release: 4,          // BIOS minor version
        embedded_controller_major_release: 1,  // EC major version
        embedded_controller_minor_release: 0,  // EC minor version
        extended_bios_rom_size: 0x0000,        // No extended size needed
        string_pool: vec![String::from("ACME BIOS Corp"), String::from("v2.4.1"), String::from("09/26/2025")],
    };

    let bios_bytes = bios_rec.to_bytes();
    let bios_header =
        SmbiosTableHeader::ref_from_bytes(&bios_bytes[..header_size]).expect("Failed to parse BIOS SMBIOS header");

    let _bios_handle = unsafe { manager.add(None, bios_header).expect("bios add failed") };

    // Example 3: Type 1 System Information Record
    let system_rec = Type1SystemInformation {
        header: SmbiosTableHeader::new(1, 0, SMBIOS_HANDLE_PI_RESERVED),
        manufacturer: 1,  // String 1: "ACME Corporation"
        product_name: 2,  // String 2: "SuperServer 5000"
        version: 3,       // String 3: "Rev 2.0"
        serial_number: 4, // String 4: "SYS123456789"
        uuid: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0], // System UUID
        wake_up_type: 0x06, // Power switch
        sku_number: 5,      // String 5: "SKU-5000-01"
        family: 6,          // String 6: "SuperServer Family"
        string_pool: vec![
            String::from("ACME Corporation"),
            String::from("SuperServer 5000"),
            String::from("Rev 2.0"),
            String::from("SYS123456789"),
            String::from("SKU-5000-01"),
            String::from("SuperServer Family"),
        ],
    };

    let system_bytes = system_rec.to_bytes();
    let system_header =
        SmbiosTableHeader::ref_from_bytes(&system_bytes[..header_size]).expect("Failed to parse system SMBIOS header");

    let _system_handle = unsafe { manager.add(None, system_header).expect("system add failed") };

    // Example 4: Type 2 Baseboard Information Record
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
    let baseboard_header = SmbiosTableHeader::ref_from_bytes(&baseboard_bytes[..header_size])
        .expect("Failed to parse baseboard SMBIOS header");

    let _baseboard_handle = unsafe { manager.add(None, baseboard_header).expect("baseboard add failed") };

    // Example 5: Type 3 System Enclosure Record
    let enclosure_rec = Type3SystemEnclosure {
        header: SmbiosTableHeader::new(3, 0, SMBIOS_HANDLE_PI_RESERVED),
        manufacturer: 1,                       // String 1: "ACME Corporation"
        enclosure_type: 0x03,                  // Desktop
        version: 2,                            // String 2: "Chassis v2.1"
        serial_number: 3,                      // String 3: "CH987654321"
        asset_tag_number: 4,                   // String 4: "ChassisAsset001"
        bootup_state: 0x03,                    // Safe
        power_supply_state: 0x03,              // Safe
        thermal_state: 0x03,                   // Safe
        security_status: 0x02,                 // Unknown
        oem_defined: 0x12345678,               // OEM specific data
        height: 0x04,                          // 4 rack units
        number_of_power_cords: 0x01,           // Single power cord
        contained_element_count: 0x00,         // No contained elements for this example
        contained_element_record_length: 0x00, // No contained elements
        string_pool: vec![
            String::from("ACME Corporation"),
            String::from("Chassis v2.1"),
            String::from("CH987654321"),
            String::from("ChassisAsset001"),
        ],
    };

    let enclosure_bytes = enclosure_rec.to_bytes();
    let enclosure_header = SmbiosTableHeader::ref_from_bytes(&enclosure_bytes[..header_size])
        .expect("Failed to parse enclosure SMBIOS header");

    let _enclosure_handle = unsafe { manager.add(None, enclosure_header).expect("enclosure add failed") };

    // Verify all five records were added
    let mut search = SMBIOS_HANDLE_PI_RESERVED;
    let (found, _) = manager.get_next(&mut search, Some(VendorOemRecord::RECORD_TYPE)).expect("get_next failed");
    assert_eq!(found.record_type, VendorOemRecord::RECORD_TYPE);
    info!("Added vendor record handle: {}", search);

    search = SMBIOS_HANDLE_PI_RESERVED;
    let (found_bios, _) = manager.get_next(&mut search, Some(0)).expect("get_next failed for bios");
    assert_eq!(found_bios.record_type, 0);
    info!("Added Type 0 BIOS record handle: {}", search);

    search = SMBIOS_HANDLE_PI_RESERVED;
    let (found_system, _) = manager.get_next(&mut search, Some(1)).expect("get_next failed for system");
    assert_eq!(found_system.record_type, 1);
    info!("Added Type 1 system record handle: {}", search);

    search = SMBIOS_HANDLE_PI_RESERVED;
    let (found_baseboard, _) = manager.get_next(&mut search, Some(2)).expect("get_next failed for baseboard");
    assert_eq!(found_baseboard.record_type, 2);
    info!("Added Type 2 baseboard record handle: {}", search);

    search = SMBIOS_HANDLE_PI_RESERVED;
    let (found_enclosure, _) = manager.get_next(&mut search, Some(3)).expect("get_next failed for enclosure");
    assert_eq!(found_enclosure.record_type, 3);
    info!("Added Type 3 enclosure record handle: {}", search);
}
