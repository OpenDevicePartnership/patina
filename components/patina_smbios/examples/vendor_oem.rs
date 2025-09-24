use patina_smbios::smbios_derive::{SMBIOS_HANDLE_PI_RESERVED, SmbiosManager, SmbiosRecords, SmbiosTableHeader};
use patina_smbios::smbios_record::{FieldInfo, FieldLayout, FieldType, SmbiosFieldLayout, SmbiosRecordStructure};
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

    // Verify added
    let mut search = SMBIOS_HANDLE_PI_RESERVED;
    let (found, _) = manager.get_next(&mut search, Some(VendorOemRecord::RECORD_TYPE)).expect("get_next failed");
    assert_eq!(found.record_type, VendorOemRecord::RECORD_TYPE);

    println!("Added vendor record handle: {}", search);
}
