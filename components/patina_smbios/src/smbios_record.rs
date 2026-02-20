//! SMBIOS Record Types and Serialization
//!
//! This module provides type-safe structures for creating SMBIOS (System Management BIOS)
//! records with automatic serialization to the standard binary format. It includes built-in
//! types for common SMBIOS tables and a powerful derive macro for creating custom records.
//!
//! # SMBIOS Record Structure
//!
//! Every SMBIOS record consists of three parts in the binary format:
//!
//! ```text
//! ┌─────────────┬──────────────────────┬────────────────────────────┐
//! │   Header    │   Structured Data    │       String Pool          │
//! │   (4 bytes) │   (varies by type)   │  (null-terminated strings) │
//! └─────────────┴──────────────────────┴────────────────────────────┘
//! ```
//!
//! 1. **Header** (4 bytes): Record type, length, and handle
//! 2. **Structured Data**: Fixed-size fields specific to the record type
//! 3. **String Pool**: Variable-length null-terminated strings referenced by indices
//!
//! # Quick Start
//!
//! Create and serialize a BIOS Information record (Type 0):
//!
//! ```ignore
//! use patina_smbios::smbios_record::{Type0PlatformFirmwareInformation, SmbiosTableHeader};
//! use patina_smbios::service::SMBIOS_HANDLE_PI_RESERVED;
//! use alloc::vec;
//!
//! // Create the record
//! let bios_info = Type0PlatformFirmwareInformation {
//!     header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     
//!     // String indices (reference the string pool below)
//!     vendor: 1,
//!     firmware_version: 2,
//!     firmware_release_date: 3,
//!     
//!     // Binary fields
//!     bios_starting_address_segment: 0xE000,
//!     firmware_rom_size: 0x0F,
//!     characteristics: 0x08,
//!     characteristics_ext1: 0x03,
//!     characteristics_ext2: 0x01,
//!     system_bios_major_release: 2,
//!     system_bios_minor_release: 4,
//!     embedded_controller_major_release: 0xFF,
//!     embedded_controller_minor_release: 0xFF,
//!     extended_bios_rom_size: 0x0000,
//!     
//!     // String pool (1-indexed)
//!     string_pool: vec![
//!         String::from("Patina BIOS"),    // Index 1
//!         String::from("v2.4.0"),         // Index 2
//!         String::from("10/28/2025"),     // Index 3
//!     ],
//! };
//! ```
//!
//! # Understanding String Pools
//!
//! SMBIOS uses an efficient string storage format where text strings are stored in a
//! "string pool" at the end of each record, referenced by 1-based indices.
//!
//! ## String Pool Format
//!
//! ```text
//! [Header + Structured Data][String 1\0][String 2\0][String 3\0]\0
//!                            └────────── String Pool ──────────┘
//! ```
//!
//! ## Key Rules
//!
//! - **1-based indexing**: Strings are numbered 1, 2, 3, ... (not 0, 1, 2)
//! - **Index 0 means "no string"**: Use when a string field is not applicable
//! - **Null termination**: Each string ends with `\0` (added automatically)
//! - **Double null terminator**: Pool ends with `\0\0`
//! - **Empty pool**: Represented as just `\0\0` (2 bytes)
//! - **Max length**: 64 bytes per string (SMBIOS specification)
//!
//! ## Example
//!
//! ```ignore
//! // Define strings
//! string_pool: vec![
//!     String::from("ACME Corp"),     // Index 1
//!     String::from("Model X100"),    // Index 2
//!     String::from("v1.0"),          // Index 3
//! ]
//!
//! // Reference strings in fields
//! manufacturer: 1,     // "ACME Corp"
//! product_name: 2,     // "Model X100"
//! version: 3,          // "v1.0"
//! unused_field: 0,     // No string
//! ```
//!
//! Binary output:
//! ```text
//! [... structured data ...][ACME Corp\0][Model X100\0][v1.0\0]\0
//! ```
//!
//! # Validation
//!
//! All record types implement the `validate()` method through the [`SmbiosRecordStructure`] trait:
//!
//! ```ignore
//! let record = Type1SystemInformation { /* ... */ };
//!
//! match record.validate() {
//!     Ok(()) => {
//!         // Record is valid
//!     }
//!     Err(SmbiosError::StringTooLong) => {
//!         log::error!("String exceeds 64-byte limit");
//!     }
//!     Err(e) => {
//!         log::error!("Validation error: {:?}", e);
//!     }
//! }
//! ```
//!
//! Validation checks:
//! - All strings ≤ 64 bytes
//! - No embedded null bytes in strings
//! - String pool is well-formed
//!
//! # Creating Custom Record Types
//!
//! Define vendor-specific SMBIOS records (types 128-255) using the derive macro:
//!
//! ```ignore
//! use patina_macro::SmbiosRecord;
//! use patina_smbios::service::SmbiosTableHeader;
//!
//! #[derive(SmbiosRecord)]
//! #[smbios(record_type = 0x80)]  // Vendor-specific (128-255)
//! pub struct CustomRecord {
//!     pub header: SmbiosTableHeader,     // Required
//!     pub vendor_id: u32,                // Custom field
//!     pub revision: u16,                 // Custom field
//!     pub flags: u8,                     // Custom field
//!     pub name: u8,                      // String index
//!     #[string_pool]
//!     pub string_pool: Vec<String>,      // Required for strings
//! }
//! ```
//!
//! ## Derive Macro Requirements
//!
//! 1. **`#[smbios(record_type = N)]`**: Specify the SMBIOS type number (0-255)
//! 2. **`header` field**: Must be of type `SmbiosTableHeader`
//! 3. **`#[string_pool]`**: Mark the `Vec<String>` field (if using strings)
//!
//! ## Supported Field Types
//!
//! - **Primitives**: `u8`, `u16`, `u32`, `u64` (little-endian serialization)
//! - **Arrays**: `[u8; N]` (direct memory copy, e.g., UUIDs, MACs)
//! - **String pool**: `Vec<String>` (converted to SMBIOS format)
//!
//! ## Advanced Example
//!
//! ```ignore
//! #[derive(SmbiosRecord)]
//! #[smbios(record_type = 0x81)]
//! pub struct ExtendedDeviceInfo {
//!     pub header: SmbiosTableHeader,
//!     pub device_id: u64,
//!     pub capabilities: u32,
//!     pub mac_address: [u8; 6],          // 6-byte MAC
//!     pub serial: u8,                    // String index
//!     pub model: u8,                     // String index
//!     #[string_pool]
//!     pub string_pool: Vec<String>,
//! }
//!
//! // Usage
//! let device = ExtendedDeviceInfo {
//!     header: SmbiosTableHeader::new(0x81, 0, SMBIOS_HANDLE_PI_RESERVED),
//!     device_id: 0x1234567890ABCDEF,
//!     capabilities: 0x000000FF,
//!     mac_address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
//!     serial: 1,
//!     model: 2,
//!     string_pool: vec![
//!         String::from("SN987654321"),
//!         String::from("Device Model X"),
//!     ],
//! };
//! ```
//!
//! # Serialization Details
//!
//! The `#[derive(SmbiosRecord)]` macro generates a `to_bytes()` implementation that:
//!
//! 1. Calculates total structure size (header + fields)
//! 2. Creates the SMBIOS header with correct type and length
//! 3. Serializes primitive fields in little-endian byte order
//! 4. Copies array fields directly (e.g., UUIDs)
//! 5. Appends string pool with proper null termination
//! 6. Returns complete SMBIOS record as `Vec<u8>`
//!
//! **Critical**: The `string_pool` field is **Rust metadata only** and is NOT part of
//! the binary structure. Never use `#[repr(C)]` or cast these structs to bytes directly.
//! Always use `to_bytes()`.
//!
//! # Best Practices
//!
//! ## String Management
//! - Use 1-based indices (1, 2, 3, ...)
//! - Use index 0 for "no string"
//! - Keep strings under 64 bytes
//! - Order string pool to match field indices
//! - Don't add null terminators (automatic)
//!
//! ## Field Values
//! - Use 0xFF for "not present" (SMBIOS convention)
//! - Use 0x00 for "unknown" or disabled
//!
//! ## Error Handling
//! - Handle `SmbiosError::StringTooLong` gracefully
//! - Test with various string lengths
//!
//! # See Also
//!
//! - [`SmbiosRecordStructure`] - Base trait for all SMBIOS records
//! - [`SmbiosTableHeader`] - SMBIOS record header structure
//! - [SMBIOS Specification](https://www.dmtf.org/standards/smbios) - Official DMTF spec
//!
//! # License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;
use crate::{
    error::SmbiosError,
    service::{SMBIOS_HANDLE_PI_RESERVED, SmbiosTableHeader},
};
use alloc::{string::String, vec::Vec};

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
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 0)]
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
    #[string_pool]
    pub string_pool: Vec<String>,
}

/// Type 1: System Information
///
/// # Important: Not C-Compatible
///
/// This struct contains a `string_pool: Vec<String>` field which is Rust metadata and
/// **NOT** part of the SMBIOS table binary format. Never cast this struct to bytes directly.
/// Always use `to_bytes()` to convert to proper SMBIOS format.
///
/// See [`Type0PlatformFirmwareInformation`] for detailed documentation on proper usage.
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 1)]
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
    #[string_pool]
    pub string_pool: Vec<String>,
}

/// Type 2: Baseboard Information
///
/// # Important: Not C-Compatible
///
/// This struct contains a `string_pool: Vec<String>` field which is Rust metadata and
/// **NOT** part of the SMBIOS table binary format. Never cast this struct to bytes directly.
/// Always use `to_bytes()` to convert to proper SMBIOS format.
///
/// See [`Type0PlatformFirmwareInformation`] for detailed documentation on proper usage.
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 2)]
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
    #[string_pool]
    pub string_pool: Vec<String>,
}

/// Type 3: System Enclosure
///
/// # Important: Not C-Compatible
///
/// This struct contains a `string_pool: Vec<String>` field which is Rust metadata and
/// **NOT** part of the SMBIOS table binary format. Never cast this struct to bytes directly.
/// Always use `to_bytes()` to convert to proper SMBIOS format.
///
/// See [`Type0PlatformFirmwareInformation`] for detailed documentation on proper usage.
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 3)]
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
    #[string_pool]
    pub string_pool: Vec<String>,
}

/// Type 4: Processor Information
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 4)]
pub struct Type4ProcessorInformation {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Socket designation string index
    pub socket_designation: u8,
    /// Processor type
    pub processor_type: u8,
    /// Processor family
    pub processor_family: u8,
    /// Processor manufacturer string index
    pub processor_manufacturer: u8,
    /// Processor ID (PROCESSOR_ID_DATA: 2x u32 = 8 bytes)
    pub processor_id: [u8; 8],
    /// Processor version string index
    pub processor_version: u8,
    /// Voltage
    pub voltage: u8,
    /// External clock frequency in MHz
    pub external_clock: u16,
    /// Max speed in MHz
    pub max_speed: u16,
    /// Current speed in MHz
    pub current_speed: u16,
    /// Status
    pub status: u8,
    /// Processor upgrade
    pub processor_upgrade: u8,
    /// L1 cache handle
    pub l1_cache_handle: u16,
    /// L2 cache handle
    pub l2_cache_handle: u16,
    /// L3 cache handle
    pub l3_cache_handle: u16,
    /// Serial number string index
    pub serial_number: u8,
    /// Asset tag string index
    pub asset_tag: u8,
    /// Part number string index
    pub part_number: u8,
    /// Core count
    pub core_count: u8,
    /// Core enabled
    pub core_enabled: u8,
    /// Thread count
    pub thread_count: u8,
    /// Processor characteristics
    pub processor_characteristics: u16,
    /// Processor family 2
    pub processor_family2: u16,
    /// Core count 2 (SMBIOS 3.0+)
    pub core_count2: u16,
    /// Core enabled 2 (SMBIOS 3.0+)
    pub core_enabled2: u16,
    /// Thread count 2 (SMBIOS 3.0+)
    pub thread_count2: u16,

    /// String pool
    #[string_pool]
    pub string_pool: Vec<String>,
}

/// Type 7: Cache Information
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 7)]
pub struct Type7CacheInformation {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Socket designation string index
    pub socket_designation: u8,
    /// Cache configuration
    pub cache_configuration: u16,
    /// Maximum cache size (in KB)
    pub maximum_cache_size: u16,
    /// Installed size (in KB)
    pub installed_size: u16,
    /// Supported SRAM type (bitfield)
    pub supported_sram_type: u16,
    /// Current SRAM type (bitfield)
    pub current_sram_type: u16,
    /// Cache speed in nanoseconds
    pub cache_speed: u8,
    /// Error correction type
    pub error_correction_type: u8,
    /// System cache type
    pub system_cache_type: u8,
    /// Associativity
    pub associativity: u8,
    /// Maximum cache size 2 (SMBIOS 3.1+)
    pub maximum_cache_size2: u32,
    /// Installed cache size 2 (SMBIOS 3.1+)
    pub installed_cache_size2: u32,

    /// String pool
    #[string_pool]
    pub string_pool: Vec<String>,
}

/// SMBIOS Type 127: End-of-Table
///
/// The End-of-Table marker indicates the end of the SMBIOS structure table.
/// This is a simple marker structure with no additional fields beyond the standard header.
///
/// Per SMBIOS specification 3.0+:
/// - Type: 127
/// - Length: 4 (header only)
/// - No strings
#[derive(patina_macro::SmbiosRecord)]
#[smbios(record_type = 127)]
pub(crate) struct Type127EndOfTable {
    /// SMBIOS header
    pub header: SmbiosTableHeader,

    /// String pool (always empty for Type 127)
    #[string_pool]
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
    fn test_type2_new() {
        let type2 = Type2BaseboardInformation {
            header: SmbiosTableHeader { record_type: 2, length: 0, handle: 0 },
            manufacturer: 1,
            product: 2,
            version: 3,
            serial_number: 4,
            asset_tag: 5,
            feature_flags: 0x01,
            location_in_chassis: 6,
            chassis_handle: 0x0003,
            board_type: 0x0A,
            contained_object_handles: 0,
            string_pool: vec![
                String::from("Manufacturer"),
                String::from("Product"),
                String::from("Version"),
                String::from("Serial"),
                String::from("Asset Tag"),
                String::from("Location"),
            ],
        };

        assert!(type2.validate().is_ok());
        assert_eq!(type2.string_pool().len(), 6);
        assert_eq!(Type2BaseboardInformation::RECORD_TYPE, 2);
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

    #[test]
    fn test_type3_new() {
        let type3 = Type3SystemEnclosure {
            header: SmbiosTableHeader { record_type: 3, length: 21, handle: 0x0300 },
            manufacturer: 1,
            enclosure_type: 0x01, // Other
            version: 2,
            serial_number: 3,
            asset_tag_number: 4,
            bootup_state: 0x03,       // Safe
            power_supply_state: 0x03, // Safe
            thermal_state: 0x03,      // Safe
            security_status: 0x03,    // None
            oem_defined: 0,
            height: 0, // Unspecified
            number_of_power_cords: 1,
            contained_element_count: 0,
            contained_element_record_length: 0,
            string_pool: vec![
                String::from("Test Manufacturer"),
                String::from("1.0"),
                String::from("SN123456"),
                String::from("Asset-001"),
            ],
        };

        assert_eq!(type3.header.record_type, 3);
        assert_eq!(Type3SystemEnclosure::RECORD_TYPE, 3);
        assert_eq!(type3.manufacturer, 1);
        assert_eq!(type3.string_pool.len(), 4);
        assert!(type3.validate().is_ok());
    }

    #[test]
    fn test_type3_to_bytes() {
        let type3 = Type3SystemEnclosure {
            header: SmbiosTableHeader { record_type: 3, length: 21, handle: 0x0300 },
            manufacturer: 1,
            enclosure_type: 0x01,
            version: 2,
            serial_number: 3,
            asset_tag_number: 4,
            bootup_state: 0x03,
            power_supply_state: 0x03,
            thermal_state: 0x03,
            security_status: 0x03,
            oem_defined: 0,
            height: 0,
            number_of_power_cords: 1,
            contained_element_count: 0,
            contained_element_record_length: 0,
            string_pool: vec![
                String::from("Manufacturer"),
                String::from("Version"),
                String::from("Serial"),
                String::from("Asset"),
            ],
        };

        let bytes = type3.to_bytes();
        // Verify header
        assert_eq!(bytes[0], 3); // Type
        assert_eq!(bytes[1], 21); // Length
        // Verify some fields
        assert_eq!(bytes[4], 1); // manufacturer
        assert_eq!(bytes[5], 0x01); // enclosure_type
        // Verify strings are present
        assert!(bytes.len() > 21);
    }

    #[test]
    fn test_type2_validation() {
        let type2 = Type2BaseboardInformation {
            header: SmbiosTableHeader { record_type: 2, length: 0, handle: 0x0200 },
            manufacturer: 1,
            product: 2,
            version: 3,
            serial_number: 4,
            asset_tag: 5,
            feature_flags: 0,
            location_in_chassis: 6,
            chassis_handle: 0,
            board_type: 0,
            contained_object_handles: 0,
            string_pool: vec![
                String::from("Board Manufacturer"),
                String::from("Board Product"),
                String::from("1.0"),
                String::from("SN123"),
                String::from("Asset-123"),
                String::from("Location"),
            ],
        };

        assert_eq!(Type2BaseboardInformation::RECORD_TYPE, 2);
        assert!(type2.validate().is_ok());
    }

    #[test]
    fn test_type4_new() {
        let type4 = Type4ProcessorInformation {
            header: SmbiosTableHeader { record_type: 4, length: 0, handle: 0x0400 },
            socket_designation: 1,
            processor_type: 0x03,   // Central Processor
            processor_family: 0xFE, // Use Family2
            processor_manufacturer: 2,
            processor_id: [0u8; 8],
            processor_version: 3,
            voltage: 0x80,
            external_clock: 100,
            max_speed: 3000,
            current_speed: 2400,
            status: 0x41,
            processor_upgrade: 0x02,
            l1_cache_handle: 0xFFFF,
            l2_cache_handle: 0xFFFF,
            l3_cache_handle: 0xFFFF,
            serial_number: 4,
            asset_tag: 5,
            part_number: 6,
            core_count: 4,
            core_enabled: 4,
            thread_count: 8,
            processor_characteristics: 0x04,
            processor_family2: 0x0101,
            core_count2: 4,
            core_enabled2: 4,
            thread_count2: 8,
            string_pool: vec![
                String::from("CPU0"),
                String::from("Test Manufacturer"),
                String::from("ARMv8"),
                String::from("SN-CPU-001"),
                String::from("ASSET-CPU-001"),
                String::from("PN-CPU-001"),
            ],
        };

        assert_eq!(type4.header.record_type, 4);
        assert_eq!(Type4ProcessorInformation::RECORD_TYPE, 4);
        assert_eq!(type4.string_pool.len(), 6);
        assert!(type4.validate().is_ok());
    }

    #[test]
    fn test_type4_to_bytes() {
        let type4 = Type4ProcessorInformation {
            header: SmbiosTableHeader { record_type: 4, length: 0, handle: 0x0400 },
            socket_designation: 1,
            processor_type: 0x03,
            processor_family: 0xFE,
            processor_manufacturer: 2,
            processor_id: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            processor_version: 3,
            voltage: 0x80,
            external_clock: 100,
            max_speed: 3000,
            current_speed: 2400,
            status: 0x41,
            processor_upgrade: 0x02,
            l1_cache_handle: 0x0001,
            l2_cache_handle: 0x0002,
            l3_cache_handle: 0xFFFF,
            serial_number: 0,
            asset_tag: 0,
            part_number: 0,
            core_count: 4,
            core_enabled: 4,
            thread_count: 8,
            processor_characteristics: 0x04,
            processor_family2: 0x0101,
            core_count2: 4,
            core_enabled2: 4,
            thread_count2: 8,
            string_pool: vec![String::from("CPU0"), String::from("Manufacturer"), String::from("Version")],
        };

        let bytes = type4.to_bytes();
        // Verify header
        assert_eq!(bytes[0], 4); // Type
        // Verify some fields after header (4 bytes)
        assert_eq!(bytes[4], 1); // socket_designation
        assert_eq!(bytes[5], 0x03); // processor_type
        assert_eq!(bytes[6], 0xFE); // processor_family
        assert_eq!(bytes[7], 2); // processor_manufacturer
        // processor_id at offset 8..16
        assert_eq!(bytes[8], 0x01);
        assert_eq!(bytes[15], 0x08);
        // Verify strings are present
        assert!(bytes.len() > bytes[1] as usize);
    }

    #[test]
    fn test_type7_new() {
        let type7 = Type7CacheInformation {
            header: SmbiosTableHeader { record_type: 7, length: 0, handle: 0x0700 },
            socket_designation: 1,
            cache_configuration: 0x0180, // L1, enabled, write-back
            maximum_cache_size: 64,
            installed_size: 64,
            supported_sram_type: 0x0002,
            current_sram_type: 0x0002,
            cache_speed: 0,
            error_correction_type: 0x02,
            system_cache_type: 0x04, // Data
            associativity: 0x05,     // 4-way
            maximum_cache_size2: 64,
            installed_cache_size2: 64,
            string_pool: vec![String::from("L1 Data Cache")],
        };

        assert_eq!(type7.header.record_type, 7);
        assert_eq!(Type7CacheInformation::RECORD_TYPE, 7);
        assert_eq!(type7.string_pool.len(), 1);
        assert!(type7.validate().is_ok());
    }

    #[test]
    fn test_type7_to_bytes() {
        let type7 = Type7CacheInformation {
            header: SmbiosTableHeader { record_type: 7, length: 0, handle: 0x0700 },
            socket_designation: 1,
            cache_configuration: 0x0180,
            maximum_cache_size: 64,
            installed_size: 64,
            supported_sram_type: 0x0002,
            current_sram_type: 0x0002,
            cache_speed: 0,
            error_correction_type: 0x02,
            system_cache_type: 0x04,
            associativity: 0x05,
            maximum_cache_size2: 64,
            installed_cache_size2: 64,
            string_pool: vec![String::from("L1 Data Cache")],
        };

        let bytes = type7.to_bytes();
        // Verify header
        assert_eq!(bytes[0], 7); // Type
        // Verify some fields after header (4 bytes)
        assert_eq!(bytes[4], 1); // socket_designation
        // cache_configuration at offset 5-6 (little-endian)
        assert_eq!(bytes[5], 0x80);
        assert_eq!(bytes[6], 0x01);
        // Verify strings are present
        assert!(bytes.len() > bytes[1] as usize);
    }

    #[test]
    fn test_string_pool_empty() {
        let type1 = Type1SystemInformation {
            header: SmbiosTableHeader { record_type: 1, length: 0, handle: 0 },
            manufacturer: 0,
            product_name: 0,
            version: 0,
            serial_number: 0,
            uuid: [0; 16],
            wake_up_type: 0x06,
            sku_number: 0,
            family: 0,
            string_pool: vec![],
        };

        let bytes = type1.to_bytes();
        // Should have double null terminator even with no strings
        assert!(bytes.ends_with(&[0, 0]));
    }
}
