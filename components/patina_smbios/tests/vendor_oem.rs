use patina_smbios::smbios_derive::{SMBIOS_HANDLE_PI_RESERVED, SmbiosManager, SmbiosRecords, SmbiosTableHeader};
use patina_smbios::smbios_record::{FieldInfo, FieldLayout, FieldType, SmbiosFieldLayout, SmbiosRecordStructure};
use std::string::String;
use std::vec::Vec;
use zerocopy::FromBytes;

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
    fn validate(&self) -> Result<(), patina_smbios::smbios_derive::SmbiosError> {
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
    let mut manager = SmbiosManager::new(3, 8);

    let rec = VendorOemRecord {
        header: SmbiosTableHeader::new(VendorOemRecord::RECORD_TYPE, 0, SMBIOS_HANDLE_PI_RESERVED),
        oem_field: 0xDEADBEEF,
        string_pool: vec![String::from("Vendor Extra")],
    };

    let bytes = rec.to_bytes();
    let header_size = core::mem::size_of::<SmbiosTableHeader>();
    let record_header =
        SmbiosTableHeader::ref_from_bytes(&bytes[..header_size]).expect("Failed to parse SMBIOS header");

    let _handle = unsafe { manager.add(None, record_header).expect("add failed") };

    let mut search = SMBIOS_HANDLE_PI_RESERVED;
    let (found, _) = manager.get_next(&mut search, Some(VendorOemRecord::RECORD_TYPE)).expect("get_next failed");
    assert_eq!(found.record_type, VendorOemRecord::RECORD_TYPE);
}
