//! SMBIOS Platform Example Component
//!
//! This component demonstrates the recommended SMBIOS integration pattern:
//! 1. Uses the type-safe `add_record<T>()` API for adding SMBIOS records
//! 2. Platform component publishes the table after all records are added
//! 3. Uses structured record types
//! 4. Shows how to create custom vendor-specific SMBIOS records (Type 0x80-0xFF)
//!
//! ## Usage
//!
//! In your platform's DXE core initialization:
//!
//! ```ignore
//! use patina_smbios::component::SmbiosProvider;
//! use patina_samples::SmbiosExampleComponent;
//!
//! Core::default()
//!     // Add SMBIOS provider component with version
//!     .with_component(SmbiosProvider::new(3, 9))
//!     // Add your platform SMBIOS component
//!     .with_component(SmbiosExampleComponent::new())
//!     // ... rest of your components
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;

use alloc::{string::String, vec};
use patina::{
    component::{component, service::Service},
    error::Result,
};
use patina_macro::SmbiosRecord;
use patina_smbios::{
    service::{SMBIOS_HANDLE_PI_RESERVED, Smbios, SmbiosExt, SmbiosTableHeader},
    smbios_record::{
        SmbiosRecordStructure, Type0PlatformFirmwareInformation, Type1SystemInformation, Type2BaseboardInformation,
        Type3SystemEnclosure,
    },
};

/// Example custom vendor-specific OEM record (Type 0x80)
///
/// This struct contains a `string_pool: Vec<String>` which is Rust metadata and NOT
/// part of the SMBIOS binary format. It should NOT have `#[repr(C, packed)]` because
/// it's never directly cast to bytes - always use `to_bytes()` for serialization.
#[derive(SmbiosRecord)]
#[smbios(record_type = 0x80)]
pub struct VendorOemRecord {
    /// SMBIOS table header
    pub header: SmbiosTableHeader,
    /// Example 32-bit vendor-specific OEM field
    pub oem_field: u32,
    /// String pool containing vendor and platform data strings
    #[string_pool]
    pub string_pool: alloc::vec::Vec<String>,
}

/// Example Platform SMBIOS Component
///
/// Demonstrates a real-world platform component that adds SMBIOS records
/// using the type-safe `add_record<T>()` API.
///
/// This example shows:
/// - Type 0: BIOS/Firmware information
/// - Type 1: System information with UUID and serial numbers
/// - Type 2: Baseboard/motherboard information
/// - Type 3: System chassis/enclosure information
/// - Type 0x80: Custom vendor-specific record (OEM range)
/// - Type 127: End-of-table marker
/// - Publishing the complete table to UEFI Configuration Table
#[derive(Default)]
pub struct SmbiosExampleComponent;

#[component]
impl SmbiosExampleComponent {
    /// Create a new SMBIOS platform component
    pub fn new() -> Self {
        Self
    }

    /// Platform SMBIOS initialization entry point
    ///
    /// Called by the component framework during DXE phase initialization.
    /// The framework automatically provides the SMBIOS service as a dependency.
    fn entry_point(self, smbios: Service<dyn Smbios>) -> Result<()> {
        log::info!("=== Example Platform SMBIOS Component ===");

        // Verify SMBIOS version
        let (major, minor) = smbios.version();
        log::info!("SMBIOS Version: {}.{}", major, minor);

        // Add platform-specific SMBIOS records
        log::info!("Creating platform SMBIOS records...");

        // Type 0: Platform firmware/BIOS information
        if let Err(e) = Self::add_bios_information(&smbios) {
            log::error!("Failed to add BIOS information: {:?}", e);
            return Err(e);
        }

        // Type 1: System information
        if let Err(e) = Self::add_system_information(&smbios) {
            log::error!("Failed to add system information: {:?}", e);
            return Err(e);
        }

        // Type 2: Baseboard/motherboard information
        if let Err(e) = Self::add_baseboard_information(&smbios) {
            log::error!("Failed to add baseboard information: {:?}", e);
            return Err(e);
        }

        // Type 3: System chassis/enclosure information
        if let Err(e) = Self::add_system_enclosure(&smbios) {
            log::error!("Failed to add system enclosure: {:?}", e);
            return Err(e);
        }

        // Type 0x80: Platform-specific vendor record
        if let Err(e) = Self::add_vendor_oem_record(&smbios) {
            log::error!("Failed to add vendor OEM record: {:?}", e);
            return Err(e);
        }

        // Type 127 End-of-Table marker is automatically added by the manager during initialization
        log::info!("Platform SMBIOS records created successfully");

        // Publish the SMBIOS table to the UEFI Configuration Table
        log::info!("Publishing SMBIOS table to Configuration Table...");
        match smbios.publish_table() {
            Ok((table_addr, entry_point_addr)) => {
                log::info!("SMBIOS table published successfully");
                log::info!("  Entry Point: 0x{:X}", entry_point_addr);
                log::info!("  Table Data: 0x{:X}", table_addr);
                log::info!("Use 'smbiosview' in UEFI Shell to view records");
            }
            Err(e) => {
                log::error!("Failed to publish SMBIOS table: {:?}", e);
                // Continue even if publication fails - not critical for boot
            }
        }

        log::info!("SMBIOS platform component initialized successfully");
        Ok(())
    }

    /// Add Type 0 (Platform Firmware/BIOS Information)
    ///
    /// Demonstrates adding firmware version, vendor, and ROM size information.
    fn add_bios_information(smbios: &Service<dyn Smbios>) -> Result<()> {
        let bios_info = Type0PlatformFirmwareInformation {
            header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
            vendor: 1,
            firmware_version: 2,
            bios_starting_address_segment: 0xE800,
            firmware_release_date: 3,
            firmware_rom_size: 0xFF, // 16MB
            characteristics: 0x08,   // PCI supported
            characteristics_ext1: 0x03,
            characteristics_ext2: 0x03,
            system_bios_major_release: 1,
            system_bios_minor_release: 0,
            embedded_controller_major_release: 0xFF,
            embedded_controller_minor_release: 0xFF,
            extended_bios_rom_size: 0,
            string_pool: vec![
                String::from("Example Firmware Vendor"),
                String::from("1.0.0"),
                String::from("10/24/2025"),
            ],
        };

        let handle = smbios.add_record(None, &bios_info).map_err(|e| {
            log::error!("Failed to add BIOS info: {:?}", e);
            patina::error::EfiError::DeviceError
        })?;

        log::info!("  Type 0 (BIOS Info) - Handle 0x{:04X}", handle);
        Ok(())
    }

    /// Add Type 1 (System Information)
    ///
    /// Demonstrates adding system UUID, manufacturer, product name, and serial number.
    fn add_system_information(smbios: &Service<dyn Smbios>) -> Result<()> {
        let system_info = Type1SystemInformation {
            header: SmbiosTableHeader::new(1, 0, SMBIOS_HANDLE_PI_RESERVED),
            manufacturer: 1,
            product_name: 2,
            version: 3,
            serial_number: 4,
            uuid: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0],
            wake_up_type: 0x06, // Power switch
            sku_number: 5,
            family: 6,
            string_pool: vec![
                String::from("Example Corporation"),
                String::from("Example Platform"),
                String::from("1.0"),
                String::from("SYS-12345"),
                String::from("SKU-EXAMPLE-001"),
                String::from("Example Platform Family"),
            ],
        };

        let handle = smbios.add_record(None, &system_info).map_err(|e| {
            log::error!("Failed to add system info: {:?}", e);
            patina::error::EfiError::DeviceError
        })?;

        log::info!("  Type 1 (System Info) - Handle 0x{:04X}", handle);
        Ok(())
    }

    /// Add Type 2 (Baseboard/Motherboard Information)
    ///
    /// Demonstrates adding motherboard manufacturer, serial number, and asset tag.
    fn add_baseboard_information(smbios: &Service<dyn Smbios>) -> Result<()> {
        let baseboard_info = Type2BaseboardInformation {
            header: SmbiosTableHeader::new(2, 0, SMBIOS_HANDLE_PI_RESERVED),
            manufacturer: 1,
            product: 2,
            version: 3,
            serial_number: 4,
            asset_tag: 5,
            feature_flags: 0x01, // Board is a hosting board
            location_in_chassis: 6,
            chassis_handle: 0x0003,
            board_type: 0x0A, // Motherboard
            contained_object_handles: 0,
            string_pool: vec![
                String::from("Example Corporation"),
                String::from("Example Baseboard"),
                String::from("1.0"),
                String::from("MB-67890"),
                String::from("ASSET-MB-001"),
                String::from("Main Board Slot"),
            ],
        };

        let handle = smbios.add_record(None, &baseboard_info).map_err(|e| {
            log::error!("Failed to add baseboard info: {:?}", e);
            patina::error::EfiError::DeviceError
        })?;

        log::info!("  Type 2 (Baseboard Info) - Handle 0x{:04X}", handle);
        Ok(())
    }

    /// Add Type 3 (System Enclosure/Chassis)
    ///
    /// Demonstrates adding chassis type, manufacturer, and serial information.
    fn add_system_enclosure(smbios: &Service<dyn Smbios>) -> Result<()> {
        let enclosure_info = Type3SystemEnclosure {
            header: SmbiosTableHeader::new(3, 0, SMBIOS_HANDLE_PI_RESERVED),
            manufacturer: 1,
            enclosure_type: 0x03, // Desktop
            version: 2,
            serial_number: 3,
            asset_tag_number: 4,
            bootup_state: 0x03,
            power_supply_state: 0x03,
            thermal_state: 0x03,
            security_status: 0x02,
            oem_defined: 0x00000000,
            height: 0x00,
            number_of_power_cords: 0x01,
            contained_element_count: 0x00,
            contained_element_record_length: 0x00,
            string_pool: vec![
                String::from("Example Corporation"),
                String::from("Example Chassis v1.0"),
                String::from("CHASSIS-99999"),
                String::from("ASSET-CHASSIS-001"),
            ],
        };

        let handle = smbios.add_record(None, &enclosure_info).map_err(|e| {
            log::error!("Failed to add system enclosure: {:?}", e);
            patina::error::EfiError::DeviceError
        })?;

        log::info!("  Type 3 (System Enclosure) - Handle 0x{:04X}", handle);
        Ok(())
    }

    /// Add custom vendor-specific OEM record (Type 0x80)
    ///
    /// Demonstrates how platforms can add custom vendor-specific SMBIOS records.
    /// Types 0x80-0xFF are reserved for OEM/vendor use.
    fn add_vendor_oem_record(smbios: &Service<dyn Smbios>) -> Result<()> {
        let vendor_record = VendorOemRecord {
            header: SmbiosTableHeader::new(VendorOemRecord::RECORD_TYPE, 0, SMBIOS_HANDLE_PI_RESERVED),
            oem_field: 0xDEADBEEF, // Example vendor-specific data
            string_pool: vec![String::from("Example Vendor"), String::from("Platform Data v1.0")],
        };

        let handle = smbios.add_record(None, &vendor_record).map_err(|e| {
            log::error!("Failed to add vendor OEM record: {:?}", e);
            patina::error::EfiError::DeviceError
        })?;

        log::info!("  Type 0x80 (Vendor OEM) - Handle 0x{:04X}", handle);
        Ok(())
    }
}
