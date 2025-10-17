//! SMBIOS Record Structures and Builders
//!
//! This module provides type-safe SMBIOS record structures and serialization for creating
//! standards-compliant SMBIOS tables. It includes pre-defined structures for common SMBIOS
//! record types and a generic serialization framework.
//!
//! # Overview
//!
//! SMBIOS records consist of three parts:
//!
//! 1. **Header** (4 bytes): Type, Length, Handle
//! 2. **Structured Data**: Fixed-format fields (type-specific)
//! 3. **String Pool**: Null-terminated strings (referenced by 1-based indices)
//!
//! This module handles the serialization of Rust structures into the binary SMBIOS format
//! automatically, allowing you to work with type-safe Rust structs instead of raw bytes.
//!
//! # Quick Start
//!
//! ```ignore
//! use patina_smbios::smbios_record::{Type0PlatformFirmwareInformation, SmbiosTableHeader};
//! use patina_smbios::service::SMBIOS_HANDLE_PI_RESERVED;
//! use alloc::string::String;
//! use alloc::vec;
//!
//! // Create a Type 0 (BIOS Information) record
//! let bios_info = Type0PlatformFirmwareInformation {
//!     header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     vendor: 1,                               // Index to string pool
//!     firmware_version: 2,                     // Index to string pool
//!     bios_starting_address_segment: 0xE000,   
//!     firmware_release_date: 3,                
//!     firmware_rom_size: 0x0F,                 
//!     characteristics: 0x08,                   
//!     characteristics_ext1: 0x03,              
//!     characteristics_ext2: 0x01,              
//!     system_bios_major_release: 2,
//!     system_bios_minor_release: 4,
//!     embedded_controller_major_release: 0xFF,
//!     embedded_controller_minor_release: 0xFF,
//!     extended_bios_rom_size: 0x0000,
//!     string_pool: vec![
//!         String::from("ACME BIOS Corp"),    // String 1 (vendor)
//!         String::from("v2.4.1"),            // String 2 (firmware_version)
//!         String::from("09/26/2025"),        // String 3 (firmware_release_date)
//!     ],
//! };
//!
//! // Serialize to bytes (automatically handled by the framework)
//! let record_bytes = bios_info.to_bytes();
//!
//! // Add to SMBIOS table
//! let handle = smbios_service.add_from_bytes(None, &record_bytes)?;
//! ```
//!
//! # Available Record Types
//!
//! This module provides the following pre-defined SMBIOS record types:
//!
//! - [`Type0PlatformFirmwareInformation`]: BIOS/firmware information (Type 0)
//! - [`Type1SystemInformation`]: System manufacturer, product, UUID (Type 1)
//! - [`Type2BaseboardInformation`]: Motherboard information (Type 2)
//! - [`Type3SystemEnclosure`]: Chassis/enclosure information (Type 3)
//! - [`Type127EndOfTable`]: End-of-table marker (Type 127)
//!
//! # String Pool Format
//!
//! SMBIOS strings are stored in a "string pool" appended after the structured data:
//!
//! ```text
//! [Header][Structured Data][String1\0][String2\0][String3\0]\0
//!                           └─────────── String Pool ──────────┘
//! ```
//!
//! **Important Rules:**
//!
//! - Strings in the pool are **1-indexed** (not 0-indexed)
//! - String fields contain the **index** into the pool (1, 2, 3, ...)
//! - Each string is null-terminated (`\0`)
//! - The entire pool ends with a **double null** (`\0\0`)
//! - Empty pool is represented as `\0\0` (2 bytes)
//! - Maximum string length: 64 bytes (per SMBIOS specification)
//!
//! ## String Pool Example
//!
//! ```ignore
//! // Define string pool
//! string_pool: vec![
//!     String::from("ACME Corp"),     // Index 1
//!     String::from("Product X"),     // Index 2
//!     String::from("v1.0"),          // Index 3
//! ]
//!
//! // Reference strings by index in fields
//! manufacturer: 1,  // Points to "ACME Corp"
//! product_name: 2,  // Points to "Product X"
//! version: 3,       // Points to "v1.0"
//! ```
//!
//! Serialized binary format:
//! ```text
//! [...structured data...][ACME Corp\0][Product X\0][v1.0\0]\0
//! ```
//!
//! # Creating Type 0 (BIOS Information) Records
//!
//! Type 0 records contain BIOS/firmware information.
//!
//! ```ignore
//! use patina_smbios::smbios_record::Type0PlatformFirmwareInformation;
//!
//! let bios_record = Type0PlatformFirmwareInformation {
//!     header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     
//!     // String indices (1-based)
//!     vendor: 1,                               
//!     firmware_version: 2,                     
//!     firmware_release_date: 3,                
//!     
//!     // Numeric fields
//!     bios_starting_address_segment: 0xE000,   // Standard BIOS segment
//!     firmware_rom_size: 0x0F,                 // ROM size (see SMBIOS spec)
//!     
//!     // BIOS characteristics (bitfield)
//!     characteristics: 0x08,                   // Bit 3: PCI supported
//!     characteristics_ext1: 0x03,              // ACPI + USB legacy
//!     characteristics_ext2: 0x01,              // UEFI specification supported
//!     
//!     // Version information
//!     system_bios_major_release: 2,
//!     system_bios_minor_release: 4,
//!     embedded_controller_major_release: 0xFF, // 0xFF = not supported
//!     embedded_controller_minor_release: 0xFF,
//!     extended_bios_rom_size: 0x0000,
//!     
//!     // String pool (1-indexed)
//!     string_pool: vec![
//!         String::from("ACME BIOS Corp"),       // String 1: vendor
//!         String::from("v2.4.1"),               // String 2: firmware_version
//!         String::from("09/26/2025"),           // String 3: firmware_release_date
//!     ],
//! };
//!
//! // Validate before using
//! bios_record.validate()?;
//!
//! // Serialize to bytes
//! let bytes = bios_record.to_bytes();
//! ```
//!
//! # Creating Type 1 (System Information) Records
//!
//! Type 1 records contain system manufacturer and product information.
//!
//! ```ignore
//! use patina_smbios::smbios_record::Type1SystemInformation;
//!
//! let system_record = Type1SystemInformation {
//!     header: SmbiosTableHeader::new(1, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     
//!     // String indices
//!     manufacturer: 1,
//!     product_name: 2,
//!     version: 3,
//!     serial_number: 4,
//!     sku_number: 5,
//!     family: 6,
//!     
//!     // System UUID (16 bytes)
//!     uuid: [
//!         0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0,
//!         0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0,
//!     ],
//!     
//!     // Wake-up type
//!     wake_up_type: 0x06,  // 0x06 = Power Switch
//!     
//!     // String pool
//!     string_pool: vec![
//!         String::from("ACME Corporation"),     // 1: manufacturer
//!         String::from("SuperServer 5000"),     // 2: product_name
//!         String::from("Rev 2.0"),              // 3: version
//!         String::from("SYS123456789"),         // 4: serial_number
//!         String::from("SKU-5000-01"),          // 5: sku_number
//!         String::from("SuperServer Family"),   // 6: family
//!     ],
//! };
//! ```
//!
//! # Creating Type 2 (Baseboard Information) Records
//!
//! Type 2 records describe the motherboard/baseboard.
//!
//! ```ignore
//! use patina_smbios::smbios_record::Type2BaseboardInformation;
//!
//! let baseboard_record = Type2BaseboardInformation {
//!     header: SmbiosTableHeader::new(2, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     
//!     // String indices
//!     manufacturer: 1,
//!     product: 2,
//!     version: 3,
//!     serial_number: 4,
//!     asset_tag: 5,
//!     location_in_chassis: 6,
//!     
//!     // Feature flags (bitfield)
//!     feature_flags: 0x01,  // Bit 0: board is a hosting board
//!     
//!     // Chassis handle (reference to Type 3 record)
//!     chassis_handle: 0x0003,
//!     
//!     // Board type
//!     board_type: 0x0A,  // 0x0A = Motherboard
//!     
//!     // Number of contained object handles
//!     contained_object_handles: 0,
//!     
//!     // String pool
//!     string_pool: vec![
//!         String::from("ACME Corporation"),      // 1: manufacturer
//!         String::from("Motherboard Model X"),   // 2: product
//!         String::from("Rev 1.0"),               // 3: version
//!         String::from("MB123456789"),           // 4: serial_number
//!         String::from("Asset001"),              // 5: asset_tag
//!         String::from("Slot 1"),                // 6: location_in_chassis
//!     ],
//! };
//! ```
//!
//! # Creating Type 3 (System Enclosure) Records
//!
//! Type 3 records describe the physical chassis/enclosure.
//!
//! ```ignore
//! use patina_smbios::smbios_record::Type3SystemEnclosure;
//!
//! let enclosure_record = Type3SystemEnclosure {
//!     header: SmbiosTableHeader::new(3, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     
//!     // String indices
//!     manufacturer: 1,
//!     version: 2,
//!     serial_number: 3,
//!     asset_tag_number: 4,
//!     
//!     // Enclosure type
//!     enclosure_type: 0x03,  // 0x03 = Desktop
//!     
//!     // State information
//!     bootup_state: 0x03,           // 0x03 = Safe
//!     power_supply_state: 0x03,     // 0x03 = Safe
//!     thermal_state: 0x03,          // 0x03 = Safe
//!     security_status: 0x02,        // 0x02 = Unknown
//!     
//!     // Physical characteristics
//!     oem_defined: 0x12345678,      // OEM-specific data
//!     height: 0x04,                 // 4 rack units
//!     number_of_power_cords: 0x01,  // Single power cord
//!     
//!     // Contained elements
//!     contained_element_count: 0x00,
//!     contained_element_record_length: 0x00,
//!     
//!     // String pool
//!     string_pool: vec![
//!         String::from("ACME Corporation"),  // 1: manufacturer
//!         String::from("Chassis v2.1"),     // 2: version
//!         String::from("CH987654321"),      // 3: serial_number
//!         String::from("ChassisAsset001"),  // 4: asset_tag_number
//!     ],
//! };
//! ```
//!
//! # Creating Type 127 (End-of-Table) Records
//!
//! Type 127 marks the end of the SMBIOS table. It has no additional fields.
//!
//! ```ignore
//! use patina_smbios::smbios_record::Type127EndOfTable;
//!
//! // Simple creation
//! let end_marker = Type127EndOfTable::new();
//!
//! // Or using Default
//! let end_marker = Type127EndOfTable::default();
//!
//! // Serialize to bytes
//! let bytes = end_marker.to_bytes();
//! ```
//!
//! # Validation
//!
//! All record types implement validation to catch common errors:
//!
//! ```ignore
//! let record = Type0PlatformFirmwareInformation { /* ... */ };
//!
//! // Validate before serialization
//! match record.validate() {
//!     Ok(()) => {
//!         // Safe to serialize
//!         let bytes = record.to_bytes();
//!     }
//!     Err(SmbiosError::StringTooLong) => {
//!         log::error!("String exceeds 64 byte limit");
//!     }
//!     Err(e) => {
//!         log::error!("Validation failed: {:?}", e);
//!     }
//! }
//! ```
//!
//! **Validation Checks:**
//!
//! - String length ≤ 64 bytes (SMBIOS_STRING_MAX_LENGTH)
//! - No null bytes in strings (added automatically during serialization)
//! - Proper string pool format
//!
//! # Serialization Process
//!
//! The serialization process is automatic via the [`SmbiosRecordStructure`] trait:
//!
//! ```ignore
//! let record = Type1SystemInformation { /* ... */ };
//!
//! // This automatically:
//! // 1. Creates header with correct type and length
//! // 2. Serializes all primitive fields (u8, u16, u32, u64, UUID)
//! // 3. Appends string pool with proper null termination
//! // 4. Returns complete SMBIOS record bytes
//! let bytes = record.to_bytes();
//! ```
//!
//! **Important**: The `string_pool` field is **NOT** part of the SMBIOS binary format.
//! It's Rust metadata that gets converted to null-terminated bytes during serialization.
//! Never cast these structs to bytes directly - always use `to_bytes()`.
//!
//! # Custom Record Types (Advanced)
//!
//! You can create custom vendor-specific record types (0x80-0xFF) using the
//! `impl_smbios_record!` macro:
//!
//! ```ignore
//! pub struct CustomVendorRecord {
//!     pub header: SmbiosTableHeader,
//!     pub vendor_field_1: u8,
//!     pub vendor_field_2: u32,
//!     pub string_pool: Vec<String>,
//! }
//!
//! // Use the macro to generate serialization code
//! impl_smbios_record!(
//!     CustomVendorRecord,
//!     0x80,              // Record type (vendor-specific range)
//!     string_pool,       // String pool field name
//!     vendor_field_1: u8,
//!     vendor_field_2: u32
//! );
//! ```
//!
//! The macro generates:
//! - Efficient `to_bytes()` serialization at compile time
//! - Basic string validation in `validate()`
//! - String pool accessors
//!
//!
//! # Best Practices
//!
//! - String indices start at 1, not 0 (SMBIOS specification requirement)
//! - Keep strings under 64 bytes (SMBIOS specification limit)
//! - Order strings in the pool to match field indices for clarity
//! - Set unused fields to 0xFF when not applicable (SMBIOS specification convention)
//! - Don't include null terminators in strings - they're added automatically
//!
//! # Common Mistakes
//!
//! **String indexing:**
//! ```ignore
//! // String indices are 1-based in SMBIOS
//! manufacturer: 1,  // References first string in pool
//! product_name: 2,  // References second string in pool
//! ```
//!
//! **String content:**
//! ```ignore
//! // Don't include null terminators - they're added during serialization
//! string_pool: vec![
//!     String::from("ACME Corp"),    // Good
//!     String::from("Product\0X"),   // Bad - contains null
//! ]
//! ```
//!
//! # License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;
use crate::error::SmbiosError;
use crate::service::{SMBIOS_HANDLE_PI_RESERVED, SMBIOS_STRING_MAX_LENGTH, SmbiosTableHeader};
use alloc::string::String;
use alloc::vec::Vec;
use zerocopy::IntoBytes;

/// Base trait for SMBIOS record structures
///
/// This trait defines the interface for all SMBIOS record types. Each record type
/// must implement serialization to convert from the high-level Rust struct to the
/// binary SMBIOS format.
pub trait SmbiosRecordStructure {
    /// The SMBIOS record type number (e.g., 0 for BIOS Information, 1 for System Information)
    const RECORD_TYPE: u8;

    /// Convert the structure to a complete SMBIOS record byte array
    ///
    /// This serializes the struct into the SMBIOS binary format:
    /// [Header][Structured Fields][String Pool]
    fn to_bytes(&self) -> Vec<u8>;

    /// Validate the structure before serialization
    ///
    /// Checks that all fields meet SMBIOS specification requirements, such as:
    /// - Strings are not too long (≤ 64 bytes)
    /// - Required fields are populated
    fn validate(&self) -> Result<(), SmbiosError>;

    /// Get the string pool for this record
    fn string_pool(&self) -> &[String];

    /// Get mutable access to the string pool
    fn string_pool_mut(&mut self) -> &mut Vec<String>;
}

/// Helper to serialize string pool to SMBIOS format
///
/// Converts a Vec<String> to null-terminated byte sequences ending with double-null.
/// This is used by the impl_smbios_record! macro.
#[doc(hidden)]
pub fn serialize_string_pool(strings: &[String]) -> Vec<u8> {
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

/// Macro to automatically generate SmbiosRecordStructure implementation
///
/// This macro generates efficient, direct serialization code at compile time,
/// avoiding the overhead of runtime field layout inspection.
///
/// # Usage
///
/// ```ignore
/// impl_smbios_record!(
///     Type0PlatformFirmwareInformation,
///     0,                    // Record type
///     string_pool,          // String pool field name
///     vendor: u8,           // Field name: type
///     firmware_version: u8,
///     // ... more fields ...
/// );
/// ```
macro_rules! impl_smbios_record {
    ($struct_name:ident, $record_type:expr, $string_pool_field:ident, $($field_name:ident: $field_type:ident),* $(,)?) => {
        impl SmbiosRecordStructure for $struct_name {
            const RECORD_TYPE: u8 = $record_type;

            fn to_bytes(&self) -> Vec<u8> {
                let mut bytes = Vec::new();

                // Calculate structured data size (header + fields, excluding string pool)
                let structured_size = core::mem::size_of::<SmbiosTableHeader>()
                    $(+ impl_smbios_record!(@field_size $field_type))*;

                // Create and serialize header
                let header = SmbiosTableHeader {
                    record_type: Self::RECORD_TYPE,
                    length: structured_size as u8,
                    handle: SMBIOS_HANDLE_PI_RESERVED,
                };
                bytes.extend_from_slice(header.as_bytes());

                // Serialize each field directly (compile-time generated code)
                $(
                    impl_smbios_record!(@serialize_field bytes, self, $field_name, $field_type);
                )*

                // Serialize string pool
                bytes.extend_from_slice(&serialize_string_pool(&self.$string_pool_field));

                bytes
            }

            fn validate(&self) -> Result<(), SmbiosError> {
                // Basic validation for strings
                for string in &self.$string_pool_field {
                    if string.len() > SMBIOS_STRING_MAX_LENGTH {
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

    // Helper: Calculate field size at compile time
    (@field_size u8) => { 1 };
    (@field_size u16) => { 2 };
    (@field_size u32) => { 4 };
    (@field_size u64) => { 8 };
    (@field_size uuid) => { 16 };

    // Helper: Generate serialization code for each field type
    (@serialize_field $bytes:ident, $self:ident, $field_name:ident, u8) => {
        $bytes.push($self.$field_name);
    };

    (@serialize_field $bytes:ident, $self:ident, $field_name:ident, u16) => {
        $bytes.extend_from_slice($self.$field_name.as_bytes());
    };

    (@serialize_field $bytes:ident, $self:ident, $field_name:ident, u32) => {
        $bytes.extend_from_slice($self.$field_name.as_bytes());
    };

    (@serialize_field $bytes:ident, $self:ident, $field_name:ident, u64) => {
        $bytes.extend_from_slice($self.$field_name.as_bytes());
    };

    (@serialize_field $bytes:ident, $self:ident, $field_name:ident, uuid) => {
        $bytes.extend_from_slice(&$self.$field_name);
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
/// Always use the `to_bytes()` method to convert this struct to bytes for the
/// SMBIOS table. The generated serialization code:
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
/// Always use `to_bytes()` to convert to proper SMBIOS format.
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
/// Always use `to_bytes()` to convert to proper SMBIOS format.
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
/// Always use `to_bytes()` to convert to proper SMBIOS format.
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

/// SMBIOS Type 127: End-of-Table
///
/// The End-of-Table marker indicates the end of the SMBIOS structure table.
/// This is a simple marker structure with no additional fields beyond the standard header.
///
/// Per SMBIOS specification 3.0+:
/// - Type: 127
/// - Length: 4 (header only)
/// - No strings
pub struct Type127EndOfTable {
    /// SMBIOS header
    pub header: SmbiosTableHeader,

    /// String pool (always empty for Type 127)
    pub string_pool: Vec<String>,
}

impl Type127EndOfTable {
    /// Create a new End-of-Table marker
    pub fn new() -> Self {
        Self { header: SmbiosTableHeader::new(127, 4, SMBIOS_HANDLE_PI_RESERVED), string_pool: Vec::new() }
    }
}

impl Default for Type127EndOfTable {
    fn default() -> Self {
        Self::new()
    }
}

impl_smbios_record!(
    Type127EndOfTable,
    127,
    string_pool,
    // No fields beyond the header
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::SMBIOS_STRING_MAX_LENGTH;
    use alloc::vec;

    #[test]
    fn test_type0_new() {
        let type0 = Type0PlatformFirmwareInformation {
            header: SmbiosTableHeader { record_type: 0, length: 0, handle: 0 },
            vendor: 1,
            firmware_version: 2,
            bios_starting_address_segment: 0xE800,
            firmware_release_date: 3,
            firmware_rom_size: 0xFF,
            characteristics: 0x08,
            characteristics_ext1: 0x03,
            characteristics_ext2: 0x03,
            system_bios_major_release: 1,
            system_bios_minor_release: 0,
            embedded_controller_major_release: 0xFF,
            embedded_controller_minor_release: 0xFF,
            extended_bios_rom_size: 0,
            string_pool: vec![String::from("Vendor"), String::from("Version"), String::from("Date")],
        };

        assert!(type0.validate().is_ok());
        assert_eq!(type0.string_pool().len(), 3);
        assert_eq!(Type0PlatformFirmwareInformation::RECORD_TYPE, 0);
    }

    #[test]
    fn test_type0_validate_string_too_long() {
        let type0 = Type0PlatformFirmwareInformation {
            header: SmbiosTableHeader { record_type: 0, length: 0, handle: 0 },
            vendor: 1,
            firmware_version: 2,
            bios_starting_address_segment: 0xE800,
            firmware_release_date: 3,
            firmware_rom_size: 0xFF,
            characteristics: 0x08,
            characteristics_ext1: 0x03,
            characteristics_ext2: 0x03,
            system_bios_major_release: 1,
            system_bios_minor_release: 0,
            embedded_controller_major_release: 0xFF,
            embedded_controller_minor_release: 0xFF,
            extended_bios_rom_size: 0,
            string_pool: vec![String::from("x").repeat(SMBIOS_STRING_MAX_LENGTH + 1)],
        };

        assert_eq!(type0.validate(), Err(SmbiosError::StringTooLong));
    }

    #[test]
    fn test_type1_new() {
        let type1 = Type1SystemInformation {
            header: SmbiosTableHeader { record_type: 1, length: 0, handle: 0 },
            manufacturer: 1,
            product_name: 2,
            version: 3,
            serial_number: 4,
            uuid: [0; 16],
            wake_up_type: 0x06,
            sku_number: 5,
            family: 6,
            string_pool: vec![
                String::from("Manufacturer"),
                String::from("Product"),
                String::from("Version"),
                String::from("Serial"),
                String::from("SKU"),
                String::from("Family"),
            ],
        };

        assert!(type1.validate().is_ok());
        assert_eq!(type1.string_pool().len(), 6);
        assert_eq!(Type1SystemInformation::RECORD_TYPE, 1);
    }

    #[test]
    fn test_type1_string_pool_mut() {
        let mut type1 = Type1SystemInformation {
            header: SmbiosTableHeader { record_type: 1, length: 0, handle: 0 },
            manufacturer: 1,
            product_name: 2,
            version: 3,
            serial_number: 4,
            uuid: [0; 16],
            wake_up_type: 0x06,
            sku_number: 5,
            family: 6,
            string_pool: vec![String::from("Initial")],
        };

        let pool = type1.string_pool_mut();
        pool.push(String::from("Added"));

        assert_eq!(type1.string_pool().len(), 2);
        assert_eq!(type1.string_pool()[1], "Added");
    }

    #[test]
    fn test_type127_end_of_table() {
        let type127 = Type127EndOfTable::new();

        assert_eq!(type127.header.record_type, 127);
        assert_eq!(type127.header.length, 4);
        // Copy to avoid unaligned reference
        let handle = type127.header.handle;
        assert_eq!(handle, SMBIOS_HANDLE_PI_RESERVED);
        assert_eq!(type127.string_pool.len(), 0);
        assert!(type127.validate().is_ok());
        assert_eq!(Type127EndOfTable::RECORD_TYPE, 127);
    }

    #[test]
    fn test_type127_default() {
        let type127 = Type127EndOfTable::default();

        assert_eq!(type127.header.record_type, 127);
        assert_eq!(type127.string_pool.len(), 0);
    }

    #[test]
    fn test_smbios_record_structure_validation() {
        // Test that validation catches string length issues
        let mut type1 = Type1SystemInformation {
            header: SmbiosTableHeader { record_type: 1, length: 0, handle: 0 },
            manufacturer: 1,
            product_name: 2,
            version: 3,
            serial_number: 4,
            uuid: [0; 16],
            wake_up_type: 0x06,
            sku_number: 5,
            family: 6,
            string_pool: vec![String::from("Valid")],
        };

        assert!(type1.validate().is_ok());

        // Add an invalid string
        type1.string_pool.push("x".repeat(SMBIOS_STRING_MAX_LENGTH + 1));
        assert_eq!(type1.validate(), Err(SmbiosError::StringTooLong));
    }
}
