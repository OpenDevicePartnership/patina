//! SMBIOS Record Structures and Builders
//!
//! Provides type-safe SMBIOS record structures and builder patterns.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;
use crate::smbios_derive::{SMBIOS_HANDLE_PI_RESERVED, SmbiosError, SmbiosTableHeader};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Base trait for SMBIOS record structures with generic serialization
pub trait SmbiosRecordStructure {
    /// The SMBIOS record type number
    const RECORD_TYPE: u8;

    /// Convert the structure to a complete SMBIOS record byte array
    // fn to_bytes(&self) -> Vec<u8>;
    fn to_bytes(&self) -> Vec<u8>
    // where Self: SmbiosFieldLayout
    where
        Self: SmbiosFieldLayout,
        Self: Sized,
    {
        SmbiosSerializer::serialize(self)
    }

    /// Validate the structure before serialization
    fn validate(&self) -> Result<(), SmbiosError>;

    /// Get the string pool for this record
    fn string_pool(&self) -> &[String];

    /// Get mutable access to the string pool
    fn string_pool_mut(&mut self) -> &mut Vec<String>;
}

/// Generic SMBIOS record serializer using reflection-like techniques
pub struct SmbiosSerializer;

impl SmbiosSerializer {
    /// Serialize any SMBIOS record structure to bytes
    pub fn serialize<T: SmbiosRecordStructure + SmbiosFieldLayout>(record: &T) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Step 1: Calculate structured data size using field layout
        let field_layout = T::field_layout();
        let structured_size = core::mem::size_of::<SmbiosTableHeader>() + field_layout.total_size();

        // Step 2: Create header
        let header = SmbiosTableHeader {
            record_type: T::RECORD_TYPE,
            length: structured_size as u8,
            handle: SMBIOS_HANDLE_PI_RESERVED,
        };

        // Step 3: Serialize header
        bytes.extend_from_slice(&Self::serialize_header(&header));

        // Step 4: Serialize structured fields using generic field serialization
        bytes.extend_from_slice(&Self::serialize_fields(record, &field_layout));

        // Step 5: Serialize string pool
        bytes.extend_from_slice(&Self::serialize_string_pool(record.string_pool()));

        bytes
    }

    fn serialize_header(header: &SmbiosTableHeader) -> [u8; 4] {
        [header.record_type, header.length, (header.handle & 0xFF) as u8, ((header.handle >> 8) & 0xFF) as u8]
    }

    fn serialize_fields<T: SmbiosRecordStructure + SmbiosFieldLayout>(record: &T, layout: &FieldLayout) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Use the field layout to serialize each field generically
        for field_info in &layout.fields {
            match field_info.field_type {
                FieldType::U8(offset) => {
                    let value = unsafe { *(record as *const T as *const u8).add(offset) };
                    bytes.push(value);
                }
                FieldType::U16(offset) => {
                    let value = unsafe { *((record as *const T as *const u8).add(offset) as *const u16) };
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                FieldType::U32(offset) => {
                    let value = unsafe { *((record as *const T as *const u8).add(offset) as *const u32) };
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                FieldType::U64(offset) => {
                    let value = unsafe { *((record as *const T as *const u8).add(offset) as *const u64) };
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                FieldType::ByteArray { offset, len } => {
                    let slice =
                        unsafe { core::slice::from_raw_parts((record as *const T as *const u8).add(offset), len) };
                    bytes.extend_from_slice(slice);
                }
            }
        }

        bytes
    }

    fn serialize_string_pool(strings: &[String]) -> Vec<u8> {
        let mut bytes = Vec::new();

        if strings.is_empty() {
            bytes.extend_from_slice(&[0, 0]);
        } else {
            for string in strings {
                if !string.is_empty() {
                    bytes.extend_from_slice(string.as_bytes());
                }
                bytes.push(0);
            }
            bytes.push(0); // Double null terminator
        }

        bytes
    }
}

/// Field layout description for generic serialization
pub trait SmbiosFieldLayout {
    fn field_layout() -> FieldLayout;
}

#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub fields: Vec<FieldInfo>,
}

impl FieldLayout {
    pub fn total_size(&self) -> usize {
        self.fields.iter().map(|f| f.size()).sum()
    }
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: &'static str,
    pub field_type: FieldType,
}

#[derive(Debug, Clone)]
pub enum FieldType {
    U8(usize),                               // offset
    U16(usize),                              // offset
    U32(usize),                              // offset
    U64(usize),                              // offset
    ByteArray { offset: usize, len: usize }, // offset, length
}

impl FieldInfo {
    pub fn size(&self) -> usize {
        match self.field_type {
            FieldType::U8(_) => 1,
            FieldType::U16(_) => 2,
            FieldType::U32(_) => 4,
            FieldType::U64(_) => 8,
            FieldType::ByteArray { len, .. } => len,
        }
    }
}

/// Macro to automatically generate both field layout and SmbiosRecordStructure implementation
macro_rules! impl_smbios_record {
    ($struct_name:ident, $record_type:expr, $string_pool_field:ident, $($field_name:ident: $field_type:ident),* $(,)?) => {
        impl SmbiosFieldLayout for $struct_name {
            fn field_layout() -> FieldLayout {
                use core::mem::{offset_of};

                FieldLayout {
                    fields: vec![
                        $(impl_smbios_record!(@field_info $struct_name, $field_name, $field_type),)*
                    ],
                }
            }
        }

        impl SmbiosRecordStructure for $struct_name {
            const RECORD_TYPE: u8 = $record_type;

            fn validate(&self) -> Result<(), SmbiosError> {
                // Basic validation for strings
                for string in &self.$string_pool_field {
                    if string.len() > crate::smbios_derive::SMBIOS_STRING_MAX_LENGTH {
                        return Err(SmbiosError::StringTooLong);
                    }
                }
                Ok(())
            }

            fn string_pool(&self) -> &[String] {
                &self.$string_pool_field
            }

            fn string_pool_mut(&mut self) -> &mut Vec<String> {
                &mut self.$string_pool_field
            }
        }
    };

    (@field_info $struct_name:ident, $field_name:ident, u8) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U8(offset_of!($struct_name, $field_name)),
        }
    };

    (@field_info $struct_name:ident, $field_name:ident, u16) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U16(offset_of!($struct_name, $field_name)),
        }
    };

    (@field_info $struct_name:ident, $field_name:ident, u32) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U32(offset_of!($struct_name, $field_name)),
        }
    };

    (@field_info $struct_name:ident, $field_name:ident, u64) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::U64(offset_of!($struct_name, $field_name)),
        }
    };

    (@field_info $struct_name:ident, $field_name:ident, uuid) => {
        FieldInfo {
            name: stringify!($field_name),
            field_type: FieldType::ByteArray {
                offset: offset_of!($struct_name, $field_name),
                len: 16
            },
        }
    };
}

/// Type 0: Platform Firmware Information (BIOS Information)
pub struct Type0PlatformFirmwareInformation {
    pub header: SmbiosTableHeader,
    pub vendor: u8,           // String index
    pub firmware_version: u8, // String index
    pub bios_starting_address_segment: u16,
    pub firmware_release_date: u8, // String index
    pub firmware_rom_size: u8,
    pub characteristics: u64,
    pub characteristics_ext1: u8,
    pub characteristics_ext2: u8,
    pub system_bios_major_release: u8,
    pub system_bios_minor_release: u8,
    pub embedded_controller_major_release: u8,
    pub embedded_controller_minor_release: u8,
    pub extended_bios_rom_size: u16,

    // Integrated string pool
    pub string_pool: Vec<String>,
}

impl_smbios_record!(
    Type0PlatformFirmwareInformation,
    0,
    string_pool,
    vendor: u8,
    firmware_version: u8,
    bios_starting_address_segment: u16,
    firmware_release_date: u8,
    firmware_rom_size: u8,
    characteristics: u64,
    characteristics_ext1: u8,
    characteristics_ext2: u8,
    system_bios_major_release: u8,
    system_bios_minor_release: u8,
    embedded_controller_major_release: u8,
    embedded_controller_minor_release: u8,
    extended_bios_rom_size: u16
);

/// Type 1: System Information
pub struct Type1SystemInformation {
    pub header: SmbiosTableHeader,
    pub manufacturer: u8,  // String index
    pub product_name: u8,  // String index
    pub version: u8,       // String index
    pub serial_number: u8, // String index
    pub uuid: [u8; 16],
    pub wake_up_type: u8,
    pub sku_number: u8, // String index
    pub family: u8,     // String index

    // Integrated string pool
    pub string_pool: Vec<String>,
}

impl_smbios_record!(
    Type1SystemInformation,
    1,
    string_pool,
    manufacturer: u8,
    product_name: u8,
    version: u8,
    serial_number: u8,
    uuid: uuid,
    wake_up_type: u8,
    sku_number: u8,
    family: u8
);

/// Type 2: Baseboard Information
pub struct Type2BaseboardInformation {
    pub header: SmbiosTableHeader,
    pub manufacturer: u8,  // String index
    pub product: u8,       // String index
    pub version: u8,       // String index
    pub serial_number: u8, // String index
    pub asset_tag: u8,     // String index
    pub feature_flags: u8,
    pub location_in_chassis: u8, // String index
    pub chassis_handle: u16,
    pub board_type: u8,
    pub contained_object_handles: u8,

    // Integrated string pool
    pub string_pool: Vec<String>,
}

impl_smbios_record!(
    Type2BaseboardInformation,
    2,
    string_pool,
    manufacturer: u8,
    product: u8,
    version: u8,
    serial_number: u8,
    asset_tag: u8,
    feature_flags: u8,
    location_in_chassis: u8,
    chassis_handle: u16,
    board_type: u8,
    contained_object_handles: u8
);

/// Type 3: System Enclosure - another example showing how simple it becomes
pub struct Type3SystemEnclosure {
    pub header: SmbiosTableHeader,
    pub manufacturer: u8, // String index
    pub enclosure_type: u8,
    pub version: u8,          // String index
    pub serial_number: u8,    // String index
    pub asset_tag_number: u8, // String index
    pub bootup_state: u8,
    pub power_supply_state: u8,
    pub thermal_state: u8,
    pub security_status: u8,
    pub oem_defined: u32,
    pub height: u8,
    pub number_of_power_cords: u8,
    pub contained_element_count: u8,
    pub contained_element_record_length: u8,

    // Integrated string pool
    pub string_pool: Vec<String>,
}

impl_smbios_record!(
    Type3SystemEnclosure,
    3,
    string_pool,
    manufacturer: u8,
    enclosure_type: u8,
    version: u8,
    serial_number: u8,
    asset_tag_number: u8,
    bootup_state: u8,
    power_supply_state: u8,
    thermal_state: u8,
    security_status: u8,
    oem_defined: u32,
    height: u8,
    number_of_power_cords: u8,
    contained_element_count: u8,
    contained_element_record_length: u8
);
