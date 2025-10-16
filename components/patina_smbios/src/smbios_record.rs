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
use crate::manager::{SMBIOS_HANDLE_PI_RESERVED, SmbiosError, SmbiosTableHeader};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use zerocopy::IntoBytes;

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

/// Generic SMBIOS record serializer using compile-time field introspection
///
/// This serializer uses a reflection-like pattern implemented via traits and macros
/// to introspect struct fields at compile-time. Each SMBIOS record type implements
/// `SmbiosFieldLayout` which provides metadata about field types and offsets, allowing
/// the serializer to generically extract and serialize primitive fields while handling
/// the `Vec<String>` string pool separately.
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

    fn serialize_header(header: &SmbiosTableHeader) -> Vec<u8> {
        header.as_bytes().to_vec()
    }

    fn serialize_fields<T: SmbiosRecordStructure + SmbiosFieldLayout>(record: &T, layout: &FieldLayout) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Use the field layout to serialize each field generically.
        //
        // IMPORTANT: The SMBIOS record structs (Type0, Type1, etc.) contain a `string_pool: Vec<String>`
        // field which is Rust metadata (fat pointers) and NOT part of the SMBIOS binary format.
        // This field layout mechanism only extracts the primitive fields (u8, u16, u32, u64, uuid)
        // that form the structured portion of the SMBIOS table. The string pool is handled separately
        // by serialize_string_pool().
        for field_info in &layout.fields {
            match field_info.field_type {
                FieldType::U8(offset) => {
                    let ptr = (record as *const T as *const u8).wrapping_add(offset);
                    let value = unsafe { core::ptr::read_unaligned(ptr) };
                    bytes.push(value);
                }
                FieldType::U16(offset) => {
                    let ptr = (record as *const T as *const u8).wrapping_add(offset);
                    let value = unsafe { core::ptr::read_unaligned(ptr as *const u16) };
                    bytes.extend_from_slice(value.as_bytes());
                }
                FieldType::U32(offset) => {
                    let ptr = (record as *const T as *const u8).wrapping_add(offset);
                    let value = unsafe { core::ptr::read_unaligned(ptr as *const u32) };
                    bytes.extend_from_slice(value.as_bytes());
                }
                FieldType::U64(offset) => {
                    let ptr = (record as *const T as *const u8).wrapping_add(offset);
                    let value = unsafe { core::ptr::read_unaligned(ptr as *const u64) };
                    bytes.extend_from_slice(value.as_bytes());
                }
                FieldType::ByteArray { offset, len } => {
                    let ptr = (record as *const T as *const u8).wrapping_add(offset);
                    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
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

/// Trait for types that can provide field layout information
///
/// Used for generic serialization of SMBIOS records.
pub trait SmbiosFieldLayout {
    /// Returns the field layout description for this type
    fn field_layout() -> FieldLayout;
}

/// Description of SMBIOS record field layout
#[derive(Debug, Clone)]
pub struct FieldLayout {
    /// List of fields in the record
    pub fields: Vec<FieldInfo>,
}

impl FieldLayout {
    /// Calculates the total size of all fields
    pub fn total_size(&self) -> usize {
        self.fields.iter().map(|f| f.size()).sum()
    }
}

/// Information about a single field in an SMBIOS record
#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// Field name
    pub name: &'static str,
    /// Field type and offset
    pub field_type: FieldType,
}

/// Field type enumeration for SMBIOS records
#[derive(Debug, Clone)]
pub enum FieldType {
    /// 8-bit unsigned integer at offset
    U8(usize),
    /// 16-bit unsigned integer at offset
    U16(usize),
    /// 32-bit unsigned integer at offset
    U32(usize),
    /// 64-bit unsigned integer at offset
    U64(usize),
    /// Byte array at offset with length
    ByteArray {
        /// Offset of the byte array
        offset: usize,
        /// Length of the byte array
        len: usize,
    },
}

impl FieldInfo {
    /// Returns the size in bytes of this field
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
                    if string.len() > crate::manager::SMBIOS_STRING_MAX_LENGTH {
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
///
/// # Important: Not C-Compatible
///
/// This struct is **NOT** `#[repr(C)]` and should **NEVER** be directly cast to bytes
/// or used in FFI contexts. The `string_pool` field contains Rust-native `String` types
/// (which are fat pointers) and is **NOT** part of the SMBIOS table binary format.
///
/// ## Proper Usage
///
/// Always use [`SmbiosSerializer::serialize()`] to convert this struct to bytes for the
/// SMBIOS table. The serializer:
/// - Extracts only the primitive fields (u8, u16, u64) for the structured portion
/// - Converts the `string_pool` to null-terminated byte sequences in the SMBIOS format
/// - Properly handles all alignment and padding requirements
///
/// ## String Pool
///
/// The `string_pool` field is metadata that holds the actual string content. The primitive
/// string fields (e.g., `vendor`, `firmware_version`) contain 1-based indices into this pool.
/// During serialization, the string pool is converted to the SMBIOS null-terminated string
/// format and appended after the structured data.
pub struct Type0PlatformFirmwareInformation {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Vendor string index
    pub vendor: u8,
    /// Firmware version string index
    pub firmware_version: u8,
    /// BIOS starting address segment
    pub bios_starting_address_segment: u16,
    /// Firmware release date string index
    pub firmware_release_date: u8,
    /// Firmware ROM size
    pub firmware_rom_size: u8,
    /// BIOS characteristics
    pub characteristics: u64,
    /// BIOS characteristics extension byte 1
    pub characteristics_ext1: u8,
    /// BIOS characteristics extension byte 2
    pub characteristics_ext2: u8,
    /// System BIOS major release
    pub system_bios_major_release: u8,
    /// System BIOS minor release
    pub system_bios_minor_release: u8,
    /// Embedded controller firmware major release
    pub embedded_controller_major_release: u8,
    /// Embedded controller firmware minor release
    pub embedded_controller_minor_release: u8,
    /// Extended BIOS ROM size
    pub extended_bios_rom_size: u16,

    /// String pool containing the actual string content.
    ///
    /// **IMPORTANT**: This field is NOT part of the SMBIOS table binary layout.
    /// It is Rust metadata that gets converted to null-terminated bytes during serialization.
    /// Never attempt to directly cast this struct to bytes or use it in FFI - always use
    /// `SmbiosSerializer::serialize()`.
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
///
/// # Important: Not C-Compatible
///
/// This struct contains a `string_pool: Vec<String>` field which is Rust metadata and
/// **NOT** part of the SMBIOS table binary format. Never cast this struct to bytes directly.
/// Always use [`SmbiosSerializer::serialize()`] to convert to proper SMBIOS format.
///
/// See [`Type0PlatformFirmwareInformation`] for detailed documentation on proper usage.
pub struct Type1SystemInformation {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Manufacturer string index
    pub manufacturer: u8,
    /// Product name string index
    pub product_name: u8,
    /// Version string index
    pub version: u8,
    /// Serial number string index
    pub serial_number: u8,
    /// UUID bytes
    pub uuid: [u8; 16],
    /// Wake-up type
    pub wake_up_type: u8,
    /// SKU number string index
    pub sku_number: u8,
    /// Family string index
    pub family: u8,

    /// String pool (NOT part of binary SMBIOS format - see struct documentation)
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
///
/// # Important: Not C-Compatible
///
/// This struct contains a `string_pool: Vec<String>` field which is Rust metadata and
/// **NOT** part of the SMBIOS table binary format. Never cast this struct to bytes directly.
/// Always use [`SmbiosSerializer::serialize()`] to convert to proper SMBIOS format.
///
/// See [`Type0PlatformFirmwareInformation`] for detailed documentation on proper usage.
pub struct Type2BaseboardInformation {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Manufacturer string index
    pub manufacturer: u8,
    /// Product string index
    pub product: u8,
    /// Version string index
    pub version: u8,
    /// Serial number string index
    pub serial_number: u8,
    /// Asset tag string index
    pub asset_tag: u8,
    /// Feature flags
    pub feature_flags: u8,
    /// Location in chassis string index
    pub location_in_chassis: u8,
    /// Chassis handle
    pub chassis_handle: u16,
    /// Board type
    pub board_type: u8,
    /// Number of contained object handles
    pub contained_object_handles: u8,

    /// String pool (NOT part of binary SMBIOS format - see struct documentation)
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

/// Type 3: System Enclosure
///
/// # Important: Not C-Compatible
///
/// This struct contains a `string_pool: Vec<String>` field which is Rust metadata and
/// **NOT** part of the SMBIOS table binary format. Never cast this struct to bytes directly.
/// Always use [`SmbiosSerializer::serialize()`] to convert to proper SMBIOS format.
///
/// See [`Type0PlatformFirmwareInformation`] for detailed documentation on proper usage.
pub struct Type3SystemEnclosure {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Manufacturer string index
    pub manufacturer: u8,
    /// Enclosure type
    pub enclosure_type: u8,
    /// Version string index
    pub version: u8,
    /// Serial number string index
    pub serial_number: u8,
    /// Asset tag number string index
    pub asset_tag_number: u8,
    /// Boot-up state
    pub bootup_state: u8,
    /// Power supply state
    pub power_supply_state: u8,
    /// Thermal state
    pub thermal_state: u8,
    /// Security status
    pub security_status: u8,
    /// OEM-defined
    pub oem_defined: u32,
    /// Height
    pub height: u8,
    /// Number of power cords
    pub number_of_power_cords: u8,
    /// Contained element count
    pub contained_element_count: u8,
    /// Contained element record length
    pub contained_element_record_length: u8,

    /// String pool (NOT part of binary SMBIOS format - see struct documentation)
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
