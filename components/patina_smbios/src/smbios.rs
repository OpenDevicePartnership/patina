#![allow(unused_doc_comments)]
#![allow(dead_code)]
#![allow(private_interfaces)]
use bitfield::bitfield;
use mu_pi::list_entry;
use r_efi::efi;
bitfield! {
    /// Bitfield for baseboard feature flags
    pub struct BaseBoardFeatureFlags(u8);
    impl Debug;
    /// Indicates if the board is a motherboard
    pub motherboard, set_motherboard: 0;
    /// Indicates if the board requires a daughter card
    pub requires_daughter_card, set_requires_daughter_card: 1;
    /// Indicates if the board is removable
    pub removable, set_removable: 2;
    /// Indicates if the board is replaceable
    pub replaceable, set_replaceable: 3;
    /// Indicates if the board is hot swappable
    pub hot_swappable, set_hot_swappable: 4;
    /// Reserved bits
    pub reserved, set_reserved: 7, 5;
}

/// SMBIOS Standard Constants
///
/// This module contains SMBIOS standard definitions converted from
/// TianoCore EDK2's SmbiosStandard.h following UEFI coding standards.
///
/// Reference SMBIOS 2.6, chapter 3.1.2.
/// For v2.1 and later, handle values in the range 0xFF00h to 0xFFFFh are reserved for
/// use by this specification.
pub const SMBIOS_HANDLE_RESERVED_BEGIN: u16 = 0xFF00;

/// Reference SMBIOS 2.7, chapter 6.1.2.
/// The UEFI Platform Initialization Specification reserves handle number FFFEh for its
/// EFI_SMBIOS_PROTOCOL.Add() function to mean "assign an unused handle number automatically."
/// This number is not used for any other purpose by the SMBIOS specification.
pub const SMBIOS_HANDLE_PI_RESERVED: u16 = 0xFFFE;

/// Reference SMBIOS 2.6, chapter 3.1.3.
/// Each text string is limited to 64 significant characters due to system MIF limitations.
/// Reference SMBIOS 2.7, chapter 6.1.3.
/// It will have no limit on the length of each individual text string.
pub const SMBIOS_STRING_MAX_LENGTH: usize = 64;

/// The length of the entire structure table (including all strings) must be reported
/// in the Structure Table Length field of the SMBIOS Structure Table Entry Point,
/// which is a WORD field limited to 65,535 bytes.
pub const SMBIOS_TABLE_MAX_LENGTH: u16 = 0xFFFF;

/// For SMBIOS 3.0, Structure table maximum size in Entry Point structure is DWORD field
/// limited to 0xFFFFFFFF bytes.
pub const SMBIOS_3_0_TABLE_MAX_LENGTH: u32 = 0xFFFFFFFF;

/// Reference SMBIOS 3.4, chapter 5.2.1 SMBIOS 2.1 (32-bit) Entry Point
/// Table 1 - SMBIOS 2.1 (32-bit) Entry Point structure, offset 00h
/// _SM_, specified as four ASCII characters (5F 53 4D 5F).
pub const SMBIOS_ANCHOR_STRING: &[u8; 4] = b"_SM_";
/// Length of SMBIOS anchor string
pub const SMBIOS_ANCHOR_STRING_LENGTH: usize = 4;

/// Reference SMBIOS 3.4, chapter 5.2.2 SMBIOS 3.0 (64-bit) Entry Point
/// Table 2 - SMBIOS 3.0 (64-bit) Entry Point structure, offset 00h
/// _SM3_, specified as five ASCII characters (5F 53 4D 33 5F).
/// SMBIOS 3.0 anchor string
pub const SMBIOS_3_0_ANCHOR_STRING: &[u8; 5] = b"_SM3_";
/// Length of SMBIOS 3.0 anchor string
pub const SMBIOS_3_0_ANCHOR_STRING_LENGTH: usize = 5;

/// SMBIOS type constants according to SMBIOS 3.3.0 specification.
/// BIOS Information (Type 0)
pub const SMBIOS_TYPE_BIOS_INFORMATION: u8 = 0;
/// System Information (Type 1)
pub const SMBIOS_TYPE_SYSTEM_INFORMATION: u8 = 1;
/// Baseboard Information (Type 2)
pub const SMBIOS_TYPE_BASEBOARD_INFORMATION: u8 = 2;
/// System Enclosure (Type 3)
pub const SMBIOS_TYPE_SYSTEM_ENCLOSURE: u8 = 3;
/// Processor Information (Type 4)
pub const SMBIOS_TYPE_PROCESSOR_INFORMATION: u8 = 4;
/// Memory Controller Information (Type 5)
pub const SMBIOS_TYPE_MEMORY_CONTROLLER_INFORMATION: u8 = 5;
/// Memory Module Information (Type 6)
pub const SMBIOS_TYPE_MEMORY_MODULE_INFORMATON: u8 = 6;
/// Cache Information (Type 7)
pub const SMBIOS_TYPE_CACHE_INFORMATION: u8 = 7;
/// Port Connector Information (Type 8)
pub const SMBIOS_TYPE_PORT_CONNECTOR_INFORMATION: u8 = 8;
/// System Slots (Type 9)
pub const SMBIOS_TYPE_SYSTEM_SLOTS: u8 = 9;
/// Onboard Device Information (Type 10)
pub const SMBIOS_TYPE_ONBOARD_DEVICE_INFORMATION: u8 = 10;
/// OEM Strings (Type 11)
pub const SMBIOS_TYPE_OEM_STRINGS: u8 = 11;
/// System Configuration Options (Type 12)
pub const SMBIOS_TYPE_SYSTEM_CONFIGURATION_OPTIONS: u8 = 12;
/// BIOS Language Information (Type 13)
pub const SMBIOS_TYPE_BIOS_LANGUAGE_INFORMATION: u8 = 13;
/// Group Associations (Type 14)
pub const SMBIOS_TYPE_GROUP_ASSOCIATIONS: u8 = 14;
/// System Event Log (Type 15)
pub const SMBIOS_TYPE_SYSTEM_EVENT_LOG: u8 = 15;
/// Physical Memory Array (Type 16)
pub const SMBIOS_TYPE_PHYSICAL_MEMORY_ARRAY: u8 = 16;
/// Memory Device (Type 17)
pub const SMBIOS_TYPE_MEMORY_DEVICE: u8 = 17;
/// 32-bit Memory Error Information (Type 18)
pub const SMBIOS_TYPE_32BIT_MEMORY_ERROR_INFORMATION: u8 = 18;
/// Memory Array Mapped Address (Type 19)
pub const SMBIOS_TYPE_MEMORY_ARRAY_MAPPED_ADDRESS: u8 = 19;
/// Memory Device Mapped Address (Type 20)
pub const SMBIOS_TYPE_MEMORY_DEVICE_MAPPED_ADDRESS: u8 = 20;
/// Built-in Pointing Device (Type 21)
pub const SMBIOS_TYPE_BUILT_IN_POINTING_DEVICE: u8 = 21;
/// Portable Battery (Type 22)
pub const SMBIOS_TYPE_PORTABLE_BATTERY: u8 = 22;
/// System Reset (Type 23)
pub const SMBIOS_TYPE_SYSTEM_RESET: u8 = 23;
/// Hardware Security (Type 24)
pub const SMBIOS_TYPE_HARDWARE_SECURITY: u8 = 24;
/// System Power Controls (Type 25)
pub const SMBIOS_TYPE_SYSTEM_POWER_CONTROLS: u8 = 25;
/// Voltage Probe (Type 26)
pub const SMBIOS_TYPE_VOLTAGE_PROBE: u8 = 26;
/// Cooling Device (Type 27)
pub const SMBIOS_TYPE_COOLING_DEVICE: u8 = 27;
/// Temperature Probe (Type 28)
pub const SMBIOS_TYPE_TEMPERATURE_PROBE: u8 = 28;
/// Electrical Current Probe (Type 29)
pub const SMBIOS_TYPE_ELECTRICAL_CURRENT_PROBE: u8 = 29;
/// Out-of-Band Remote Access (Type 30)
pub const SMBIOS_TYPE_OUT_OF_BAND_REMOTE_ACCESS: u8 = 30;
/// Boot Integrity Service (Type 31)
pub const SMBIOS_TYPE_BOOT_INTEGRITY_SERVICE: u8 = 31;
/// System Boot Information (Type 32)
pub const SMBIOS_TYPE_SYSTEM_BOOT_INFORMATION: u8 = 32;
/// 64-bit Memory Error Information (Type 33)
pub const SMBIOS_TYPE_64BIT_MEMORY_ERROR_INFORMATION: u8 = 33;
/// Management Device (Type 34)
pub const SMBIOS_TYPE_MANAGEMENT_DEVICE: u8 = 34;
/// Management Device Component (Type 35)
pub const SMBIOS_TYPE_MANAGEMENT_DEVICE_COMPONENT: u8 = 35;
/// Management Device Threshold Data (Type 36)
pub const SMBIOS_TYPE_MANAGEMENT_DEVICE_THRESHOLD_DATA: u8 = 36;
/// Memory Channel (Type 37)
pub const SMBIOS_TYPE_MEMORY_CHANNEL: u8 = 37;
/// IPMI Device Information (Type 38)
pub const SMBIOS_TYPE_IPMI_DEVICE_INFORMATION: u8 = 38;
/// System Power Supply (Type 39)
pub const SMBIOS_TYPE_SYSTEM_POWER_SUPPLY: u8 = 39;
/// Additional Information (Type 40)
pub const SMBIOS_TYPE_ADDITIONAL_INFORMATION: u8 = 40;
/// Onboard Devices Extended Information (Type 41)
pub const SMBIOS_TYPE_ONBOARD_DEVICES_EXTENDED_INFORMATION: u8 = 41;
/// Management Controller Host Interface (Type 42)
pub const SMBIOS_TYPE_MANAGEMENT_CONTROLLER_HOST_INTERFACE: u8 = 42;
/// TPM Device (Type 43)
pub const SMBIOS_TYPE_TPM_DEVICE: u8 = 43;
/// Processor Additional Information (Type 44)
pub const SMBIOS_TYPE_PROCESSOR_ADDITIONAL_INFORMATION: u8 = 44;
/// Firmware Inventory Information (Type 45)
pub const SMBIOS_TYPE_FIRMWARE_INVENTORY_INFORMATION: u8 = 45;
/// String Property Information (Type 46)
pub const SMBIOS_TYPE_STRING_PROPERTY_INFORMATION: u8 = 46;

/// Inactive type is added from SMBIOS 2.2. Reference SMBIOS 2.6, chapter 3.3.43.
/// Upper-level software that interprets the SMBIOS structure-table should bypass an
/// Inactive structure just like a structure type that the software does not recognize.
pub const SMBIOS_TYPE_INACTIVE: u16 = 0x007E;

/// End-of-table type is added from SMBIOS 2.2. Reference SMBIOS 2.6, chapter 3.3.44.
/// The end-of-table indicator is used in the last physical structure in a table
pub const SMBIOS_TYPE_END_OF_TABLE: u8 = 0x7F;

/// OEM-specific SMBIOS types range (128-255)
pub const SMBIOS_OEM_BEGIN: u8 = 128;
/// End of OEM-specific SMBIOS types
pub const SMBIOS_OEM_END: u8 = 255;

/// Types 0 through 127 (7Fh) are reserved for and defined by this
/// specification. Types 128 through 256 (80h to FFh) are available for system- and OEM-specific information.
pub type SmbiosType = u8;

/// Specifies the structure's handle, a unique 16-bit number in the range 0 to 0FFFEh (for version
/// 2.0) or 0 to 0FEFFh (for version 2.1 and later). The handle can be used with the Get SMBIOS
/// Structure function to retrieve a specific structure; the handle numbers are not required to be
/// contiguous. For v2.1 and later, handle values in the range 0FF00h to 0FFFFh are reserved for
/// use by this specification.
/// If the system configuration changes, a previously assigned handle might no longer exist.
/// However once a handle has been assigned by the BIOS, the BIOS cannot re-assign that handle
/// number to another structure.
pub type SmbiosHandle = u16;

/// EFI SMBIOS Table Header type alias
pub type EfiSmbiosTableHeader = SmbiosStructure;

// SMBios Table EP Structure
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
/// SMBIOS 2.x Entry Point structure
pub struct SmbiosTableEntryPoint {
    /// Anchor string
    pub anchor_string: [u8; SMBIOS_ANCHOR_STRING_LENGTH],
    /// Entry point structure checksum
    pub entry_point_structure_checksum: u8,
    /// Length of the entry point structure
    pub entry_point_length: u8,
    /// Major version of SMBIOS
    pub major_version: u8,
    /// Minor version of SMBIOS
    pub minor_version: u8,
    /// Maximum structure size
    pub max_structure_size: u16,
    /// Entry point revision
    pub entry_point_revision: u8,
    /// Formatted area
    pub formatted_area: [u8; 5],
    /// Intermediate anchor string
    pub intermediate_anchor_string: [u8; 5],
    /// Intermediate checksum
    pub intermediate_checksum: u8,
    /// Table length
    pub table_length: u16,
    /// Table address
    pub table_address: u32,
    /// Number of SMBIOS structures
    pub number_of_smbios_structures: u16,
    /// SMBIOS BCD revision
    pub smbios_bcd_revision: u8,
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
/// SMBIOS 3.0 Entry Point structure
pub struct SmbiosTable30EntryPoint {
    /// Anchor string
    pub anchor_string: [u8; SMBIOS_3_0_ANCHOR_STRING_LENGTH],
    /// Entry point structure checksum
    pub entry_point_structure_checksum: u8,
    /// Length of the entry point structure
    pub entry_point_length: u8,
    /// Major version of SMBIOS
    pub major_version: u8,
    /// Minor version of SMBIOS
    pub minor_version: u8,
    /// Document revision
    pub doc_rev: u8,
    /// Entry point revision
    pub entry_point_revision: u8,
    /// Reserved byte
    pub reserved: u8,
    /// Maximum table size
    pub table_maximum_size: u32,
    /// Table address
    pub table_address: u64,
}

// Smbios structure header
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
/// SMBIOS structure header
pub struct SmbiosStructure {
    /// Structure type
    pub r#type: SmbiosType,
    /// Structure length
    pub length: u8,
    /// Structure handle
    pub handle: SmbiosHandle,
}

///
/// Text strings associated with a given SMBIOS structure are returned in the dmiStructBuffer, appended directly after
/// the formatted portion of the structure. This method of returning string information eliminates the need for
/// application software to deal with pointers embedded in the SMBIOS structure. Each string is terminated with a null
/// (00h) BYTE and the set of strings is terminated with an additional null (00h) BYTE. When the formatted portion of
/// a SMBIOS structure references a string, it does so by specifying a non-zero string number within the structure's
/// string-set. For example, if a string field contains 02h, it references the second string following the formatted portion
/// of the SMBIOS structure. If a string field references no string, a null (0) is placed in that string field. If the
/// formatted portion of the structure contains string-reference fields and all the string fields are set to 0 (no string
/// references), the formatted section of the structure is followed by two null (00h) BYTES.
///
pub type SmbiosTableString = u8;

///
/// BIOS Characteristics
/// Defines which functions the BIOS supports. PCI, PCMCIA, Flash, etc.
///
/// Bitfield for BIOS characteristics
bitfield! {
    /// BIOS characteristics bitfield
    pub struct MiscBiosCharacteristics(u64);
    impl Debug;
    /// Gets the reserved bit
    pub reserved, set_reserved: 1, 0;
    /// Gets the unknown bit
    pub unknown, set_unknown: 2;
    /// Gets the BIOS characteristics not supported bit
    pub bios_characteristics_not_supported, set_bios_characteristics_not_supported: 3;
    /// Gets the ISA is supported bit
    pub isa_is_supported, set_isa_is_supported: 4;
    /// Gets the MCA is supported bit
    pub mca_is_supported, set_mca_is_supported: 5;
    /// Gets the EISA is supported bit
    pub eisa_is_supported, set_eisa_is_supported: 6;
    /// Gets the PCI is supported bit
    pub pci_is_supported, set_pci_is_supported: 7;
    /// Gets the PCMCIA is supported bit
    pub pcmcia_is_supported, set_pcmcia_is_supported: 8;
    /// Gets the Plug and Play is supported bit
    pub plug_and_play_is_supported, set_plug_and_play_is_supported: 9;
    /// Gets the APM is supported bit
    pub apm_is_supported, set_apm_is_supported: 10;
    /// Gets the BIOS is upgradable bit
    pub bios_is_upgradable, set_bios_is_upgradable: 11;
    /// Gets the BIOS shadowing allowed bit
    pub bios_shadowing_allowed, set_bios_shadowing_allowed: 12;
    /// Gets the VL VESA is supported bit
    pub vl_vesa_is_supported, set_vl_vesa_is_supported: 13;
    /// Gets the ESCD support is available bit
    pub escd_support_is_available, set_escd_support_is_available: 14;
    /// Gets the boot from CD is supported bit
    pub boot_from_cd_is_supported, set_boot_from_cd_is_supported: 15;
    /// Gets the selectable boot is supported bit
    pub selectable_boot_is_supported, set_selectable_boot_is_supported: 16;
    /// Gets the ROM BIOS is socketed bit
    pub rom_bios_is_socketed, set_rom_bios_is_socketed: 17;
    /// Gets the boot from PCMCIA is supported bit
    pub boot_from_pcmcia_is_supported, set_boot_from_pcmcia_is_supported: 18;
    /// Gets the EDD specification is supported bit
    pub edd_specification_is_supported, set_edd_specification_is_supported: 19;
    /// Gets the Japanese NEC floppy is supported bit
    pub japanese_nec_floppy_is_supported, set_japanese_nec_floppy_is_supported: 20;
    /// Gets the Japanese Toshiba floppy is supported bit
    pub japanese_toshiba_floppy_is_supported, set_japanese_toshiba_floppy_is_supported: 21;
    /// Gets the 5.25" 360KB floppy is supported bit
    pub floppy_525_360_is_supported, set_floppy_525_360_is_supported: 22;
    /// Gets the 5.25" 1.2MB floppy is supported bit
    pub floppy_525_12_is_supported, set_floppy_525_12_is_supported: 23;
    /// Gets the 3.5" 720KB floppy is supported bit
    pub floppy_35_720_is_supported, set_floppy_35_720_is_supported: 24;
    /// Gets the 3.5" 2.88MB floppy is supported bit
    pub floppy_35_288_is_supported, set_floppy_35_288_is_supported: 25;
    /// Gets the print screen is supported bit
    pub print_screen_is_supported, set_print_screen_is_supported: 26;
    /// Gets the keyboard 8042 is supported bit
    pub keyboard_8042_is_supported, set_keyboard_8042_is_supported: 27;
    /// Gets the serial is supported bit
    pub serial_is_supported, set_serial_is_supported: 28;
    /// Gets the printer is supported bit
    pub printer_is_supported, set_printer_is_supported: 29;
    /// Gets the CGA mono is supported bit
    pub cga_mono_is_supported, set_cga_mono_is_supported: 30;
    /// Gets the NEC PC-98 bit
    pub nec_pc98, set_nec_pc98: 31;
    /// Gets the reserved for vendor bits
    pub reserved_for_vendor, set_reserved_for_vendor: 63, 32;
}

///
/// BIOS Characteristics Extension Byte 1.
/// This information, available for SMBIOS version 2.1 and later, appears at offset 12h
/// within the BIOS Information structure.
///
/// Bitfield for BIOS Characteristics Extension Byte 1
bitfield! {
    /// BIOS Characteristics Extension Byte 1 bitfield
    pub struct MbceBiosReserved(u8);
    impl Debug;
    /// Gets the ACPI is supported bit
    pub acpi_is_supported, set_acpi_is_supported: 0;
    /// Gets the USB legacy is supported bit
    pub usb_legacy_is_supported, set_usb_legacy_is_supported: 1;
    /// Gets the AGP is supported bit
    pub agp_is_supported, set_agp_is_supported: 2;
    /// Gets the I2O boot is supported bit
    pub i2o_boot_is_supported, set_i2o_boot_is_supported: 3;
    /// Gets the LS-120 boot is supported bit
    pub ls120_boot_is_supported, set_ls120_boot_is_supported: 4;
    /// Gets the ATAPI ZIP drive boot is supported bit
    pub atapi_zip_drive_boot_is_supported, set_atapi_zip_drive_boot_is_supported: 5;
    /// Gets the boot 1394 is supported bit
    pub boot_1394_is_supported, set_boot_1394_is_supported: 6;
    /// Gets the smart battery is supported bit
    pub smart_battery_is_supported, set_smart_battery_is_supported: 7;
}

///
/// BIOS Characteristics Extension Byte 2.
/// This information, available for SMBIOS version 2.3 and later, appears at offset 13h
/// within the BIOS Information structure.
///
/// Bitfield for BIOS Characteristics Extension Byte 2
bitfield! {
    /// BIOS Characteristics Extension Byte 2 bitfield
    pub struct MbceSystemReserved(u8);
    impl Debug;
    /// Gets the ACPI is supported bit
    pub acpi_is_supported, set_acpi_is_supported: 0;
    /// Gets the USB legacy is supported bit
    pub usb_legacy_is_supported, set_usb_legacy_is_supported: 1;
    /// Gets the AGP is supported bit
    pub agp_is_supported, set_agp_is_supported: 2;
    /// Gets the I2O boot is supported bit
    pub i2o_boot_is_supported, set_i2o_boot_is_supported: 3;
    /// Gets the LS-120 boot is supported bit
    pub ls120_boot_is_supported, set_ls120_boot_is_supported: 4;
    /// Gets the ATAPI ZIP drive boot is supported bit
    pub atapi_zip_drive_boot_is_supported, set_atapi_zip_drive_boot_is_supported: 5;
    /// Gets the boot 1394 is supported bit
    pub boot_1394_is_supported, set_boot_1394_is_supported: 6;
    /// Gets the smart battery is supported bit
    pub smart_battery_is_supported, set_smart_battery_is_supported: 7;
}

// BIOS Characteristics Extension Bytes
#[repr(C, packed)]
/// Extended BIOS characteristics flags
pub struct MiscBiosCharacteristicsExt {
    /// BIOS reserved flags
    pub bios_reserved: MbceBiosReserved,
    /// System reserved flags
    pub system_reserved: MbceSystemReserved,
}

// Extended BIOS ROM size (SMBIOS 3.1.0+). Layout: bits 0..=13 size, bits 14..=15 unit.
/// Extended BIOS ROM size (SMBIOS 3.1.0+). Layout: bits 0..=13 size, bits 14..=15 unit.
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExtendedBiosRomSize {
    /// Raw extended BIOS ROM size
    pub raw: u16,
}

impl ExtendedBiosRomSize {
    /// Returns the size portion of the extended BIOS ROM size
    pub fn size(&self) -> u16 {
        self.raw & 0x3FFF
    }
    /// Returns the unit portion of the extended BIOS ROM size
    pub fn unit(&self) -> u8 {
        ((self.raw >> 14) & 0x3) as u8
    }
}

// Bios Information: Type 0
#[repr(C, packed)]
/// BIOS Information (Type 0)
pub struct SmbiosTableType0 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Vendor string
    pub vendor: SmbiosTableString,
    /// BIOS version string
    pub bios_version: SmbiosTableString,
    /// BIOS segment
    pub bios_segment: u16,
    /// BIOS release date string
    pub bios_release_date: SmbiosTableString,
    /// BIOS size
    pub bios_size: u8,
    /// BIOS characteristics
    pub bios_characteristics: MiscBiosCharacteristics,
    /// BIOS characteristics extension bytes
    pub bios_characteristics_ext_bytes: [u8; 2],
    /// System BIOS major release
    pub system_bios_major_release: u8,
    /// System BIOS minor release
    pub system_bios_minor_release: u8,
    /// Embedded controller firmware major release
    pub embedded_controller_firmware_major_release: u8,
    /// Embedded controller firmware minor release
    pub embedded_controller_firmware_minor_release: u8,
    /// Extended BIOS size (SMBIOS 3.1.0+)
    pub extended_bios_size: ExtendedBiosRomSize,
}

// System Wake-up Type
/// System Wake-up Type
#[repr(u8)]
pub enum MiscSystemWakeupType {
    /// Reserved
    SystemWakeupTypeReserved = 0x00,
    /// Other
    SystemWakeupTypeOther = 0x01,
    /// Unknown
    SystemWakeupTypeUnknown = 0x02,
    /// Wakeup by APM timer
    SystemWakeupTypeApmTimer = 0x03,
    /// Wakeup by modem ring
    SystemWakeupTypeModemRing = 0x04,
    /// Wakeup by LAN remote
    SystemWakeupTypeLanRemote = 0x05,
    /// Wakeup by power switch
    SystemWakeupTypePowerSwitch = 0x06,
    /// Wakeup by PCI PME
    SystemWakeupTypePciPme = 0x07,
    /// Wakeup by AC power restored
    SystemWakeupTypeAcPowerRestored = 0x08,
}

///
/// System Information (Type 1).
///
/// The information in this structure defines attributes of the overall system and is
/// intended to be associated with the Component ID group of the system's MIF.
/// An SMBIOS implementation is associated with a single system instance and contains
/// one and only one System Information (Type 1) structure.
///
#[repr(C)]
pub struct SmbiosTableType1 {}
///
///  Base Board - Board Type.
/// Base Board - Board Type.
#[repr(u8)]
pub enum BaseBoardType {
    /// Unknown baseboard type
    Unknown = 0x1,
    /// Other baseboard type
    Other = 0x2,
    /// Server blade baseboard type
    ServerBlade = 0x3,
    /// Connectivity switch baseboard type
    ConnectivitySwitch = 0x4,
    /// System management module baseboard type
    SystemManagementModule = 0x5,
    /// Processor module baseboard type
    ProcessorModule = 0x6,
    /// IO module baseboard type
    IOModule = 0x7,
    /// Memory module baseboard type
    MemoryModule = 0x8,
    /// Daughter board baseboard type
    DaughterBoard = 0x9,
    /// Motherboard baseboard type
    MotherBoard = 0xA,
    /// Processor memory module baseboard type
    ProcessorMemoryModule = 0xB,
    /// Processor IO module baseboard type
    ProcessorIOModule = 0xC,
    /// Interconnect board baseboard type
    InterconnectBoard = 0xD,
}

///
/// Base Board (or Module) Information (Type 2).
///
/// The information in this structure defines attributes of a system baseboard -
/// for example a motherboard, planar, or server blade or other standard system module.
///
#[repr(C, packed)]
pub struct SmbiosTableType2 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Manufacturer string
    pub manufacturer: SmbiosTableString,
    /// Product name string
    pub product_name: SmbiosTableString,
    /// Version string
    pub version: SmbiosTableString,
    /// Serial number string
    pub serial_number: SmbiosTableString,
    /// Asset tag string
    pub asset_tag: SmbiosTableString,
    /// Feature flags
    pub feature_flag: BaseBoardFeatureFlags,
    /// Location in chassis string
    pub location_in_chassis: SmbiosTableString,
    /// Chassis handle
    pub chassis_handle: u16,
    /// Board type
    pub board_type: u8,
    /// Number of contained object handles
    pub number_of_contained_object_handles: u8,
    /// Array of contained object handles
    pub contained_object_handles: [u16; 1],
}

///
/// System Enclosure or Chassis Types
///
pub enum MiscChassisType {
    /// Other chassis type
    MiscChassisTypeOther = 0x01,
    /// Unknown chassis type
    MiscChassisTypeUnknown = 0x02,
    /// Desktop chassis type
    MiscChassisTypeDeskTop = 0x03,
    /// Low profile desktop chassis type
    MiscChassisTypeLowProfileDesktop = 0x04,
    /// Pizza box chassis type
    MiscChassisTypePizzaBox = 0x05,
    /// Mini tower chassis type
    MiscChassisTypeMiniTower = 0x06,
    /// Tower chassis type
    MiscChassisTypeTower = 0x07,
    /// Portable chassis type
    MiscChassisTypePortable = 0x08,
    /// Laptop chassis type
    MiscChassisTypeLapTop = 0x09,
    /// Notebook chassis type
    MiscChassisTypeNotebook = 0x0A,
    /// Handheld chassis type
    MiscChassisTypeHandHeld = 0x0B,
    /// Docking station chassis type
    MiscChassisTypeDockingStation = 0x0C,
    /// All-in-one chassis type
    MiscChassisTypeAllInOne = 0x0D,
    /// Sub-notebook chassis type
    MiscChassisTypeSubNotebook = 0x0E,
    /// Space-saving chassis type
    MiscChassisTypeSpaceSaving = 0x0F,
    /// Lunch box chassis type
    MiscChassisTypeLunchBox = 0x10,
    /// Main server chassis type
    MiscChassisTypeMainServerChassis = 0x11,
    /// Expansion chassis type
    MiscChassisTypeExpansionChassis = 0x12,
    /// Sub chassis type
    MiscChassisTypeSubChassis = 0x13,
    /// Bus expansion chassis type
    MiscChassisTypeBusExpansionChassis = 0x14,
    /// Peripheral chassis type
    MiscChassisTypePeripheralChassis = 0x15,
    /// RAID chassis type
    MiscChassisTypeRaidChassis = 0x16,
    /// Rack mount chassis type
    MiscChassisTypeRackMountChassis = 0x17,
    /// Sealed case PC chassis type
    MiscChassisTypeSealedCasePc = 0x18,
    /// Multi-system chassis type
    MiscChassisMultiSystemChassis = 0x19,
    /// CompactPCI chassis type
    MiscChassisCompactPCI = 0x1A,
    /// AdvancedTCA chassis type
    MiscChassisAdvancedTCA = 0x1B,
    /// Blade chassis type
    MiscChassisBlade = 0x1C,
    /// Blade enclosure chassis type
    MiscChassisBladeEnclosure = 0x1D,
    /// Tablet chassis type
    MiscChassisTablet = 0x1E,
    /// Convertible chassis type
    MiscChassisConvertible = 0x1F,
    /// Detachable chassis type
    MiscChassisDetachable = 0x20,
    /// IoT gateway chassis type
    MiscChassisIoTGateway = 0x21,
    /// Embedded PC chassis type
    MiscChassisEmbeddedPc = 0x22,
    /// Mini PC chassis type
    MiscChassisMiniPc = 0x23,
    /// Stick PC chassis type
    MiscChassisStickPc = 0x24,
}

///
/// System Enclosure or Chassis States .
///
#[repr(u8)]
pub enum MiscChassisState {
    /// Other chassis state
    ChassisStateOther = 0x01,
    /// Unknown chassis state
    ChassisStateUnknown = 0x02,
    /// Safe chassis state
    ChassisStateSafe = 0x03,
    /// Chassis state warning
    ChassisStateWarning = 0x04,
    /// Chassis state critical
    ChassisStateCritical = 0x05,
    /// Chassis state non-recoverable
    ChassisStateNonRecoverable = 0x06,
}

///
/// System Enclosure or Chassis Security Status.
///
#[repr(u8)]
pub enum MiscChassisSecurityState {
    /// Chassis security status other
    ChassisSecurityStatusOther = 0x01,
    /// Chassis security status unknown
    ChassisSecurityStatusUnknown = 0x02,
    /// Chassis security status none
    ChassisSecurityStatusNone = 0x03,
    /// Chassis security status external interface locked out
    ChassisSecurityStatusExternalInterfaceLockedOut = 0x04,
    /// Chassis security status external interface locked enabled
    ChassisSecurityStatusExternalInterfaceLockedEnabled = 0x05,
}

///
/// Contained Element record
///
#[repr(C, packed)]
pub struct ContainedElement {
    /// Type of contained element
    pub contained_element_type: u8,
    /// Minimum number of contained elements
    pub contained_element_minimum: u8,
    /// Maximum number of contained elements
    pub contained_element_maximum: u8,
}

///
/// System Enclosure or Chassis (Type 3).
///
/// The information in this structure defines attributes of the system's mechanical enclosure(s).
/// For example, if a system included a separate enclosure for its peripheral devices,
/// two structures would be returned: one for the main, system enclosure and the second for
/// the peripheral device enclosure.  The additions to this structure in v2.1 of this specification
/// support the population of the CIM_Chassis class.
///
#[repr(C, packed)]
pub struct SmbiosTableType3 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Manufacturer string
    pub manufacturer: SmbiosTableString,
    /// Chassis type
    pub r#type: u8,
    /// Version string
    pub version: SmbiosTableString,
    /// Serial number string
    pub serial_number: SmbiosTableString,
    /// Asset tag string
    pub asset_tag: SmbiosTableString,
    /// Bootup state
    pub bootup_state: u8,
    /// Power supply state
    pub power_supply_state: u8,
    /// Thermal state
    pub thermal_state: u8,
    /// Security status
    pub security_status: u8,
    /// OEM defined data
    pub oem_defined: [u8; 4],
    /// Chassis height
    pub height: u8,
    /// Number of power cords
    pub numberof_power_cords: u8,
    /// Number of contained elements
    pub contained_element_count: u8,
    /// Length of contained element record
    pub contained_element_record_length: u8,
}

///
/// Processor Information - Processor Type.
///
#[repr(u8)]
pub enum ProcessorTypeData {
    /// Other processor type
    ProcessorOther = 0x01,
    /// Unknown processor type
    ProcessorUnknown = 0x02,
    /// Central processor
    CentralProcessor = 0x03,
    /// Math processor
    MathProcessor = 0x04,
    /// DSP processor
    DspProcessor = 0x05,
    /// Video processor
    VideoProcessor = 0x06,
}

///
/// Processor Information - Processor Family.
///
#[repr(u8)]
pub enum ProcessorFamilyData {
    /// Other processor family
    ProcessorFamilyOther = 0x01,
    /// Unknown processor family
    ProcessorFamilyUnknown = 0x02,
    /// Intel 8086 processor family
    ProcessorFamily8086 = 0x03,
    /// Intel 80286 processor family
    ProcessorFamily80286 = 0x04,
    /// Intel 386 processor family
    ProcessorFamilyIntel386 = 0x05,
    /// Intel 486 processor family
    ProcessorFamilyIntel486 = 0x06,
    /// Intel 8087 processor family
    ProcessorFamily8087 = 0x07,
    /// Intel 80287 processor family
    ProcessorFamily80287 = 0x08,
    /// Intel 80387 processor family
    ProcessorFamily80387 = 0x09,
    /// Intel 80487 processor family
    ProcessorFamily80487 = 0x0A,
    /// Intel Pentium processor family
    ProcessorFamilyPentium = 0x0B,
    /// Intel Pentium Pro processor family
    ProcessorFamilyPentiumPro = 0x0C,
    /// Intel Pentium II processor family
    ProcessorFamilyPentiumII = 0x0D,
    /// Intel Pentium MMX processor family
    ProcessorFamilyPentiumMMX = 0x0E,
    /// Intel Celeron processor family
    ProcessorFamilyCeleron = 0x0F,
    /// Intel Pentium II Xeon processor family
    ProcessorFamilyPentiumIIXeon = 0x10,
    /// Intel Pentium III processor family
    ProcessorFamilyPentiumIII = 0x11,
    /// M1 processor family
    ProcessorFamilyM1 = 0x12,
    /// M2 processor family
    ProcessorFamilyM2 = 0x13,
    /// Intel Celeron M processor family
    ProcessorFamilyIntelCeleronM = 0x14,
    /// Intel Pentium 4 HT processor family
    ProcessorFamilyIntelPentium4Ht = 0x15,
    /// AMD Duron processor family
    ProcessorFamilyAmdDuron = 0x18,
    /// AMD K5 processor family
    ProcessorFamilyK5 = 0x19,
    /// AMD K6 processor family
    ProcessorFamilyK6 = 0x1A,
    /// AMD K6-2 processor family
    ProcessorFamilyK6_2 = 0x1B,
    /// AMD K6-3 processor family
    ProcessorFamilyK6_3 = 0x1C,
    /// AMD Athlon processor family
    ProcessorFamilyAmdAthlon = 0x1D,
    /// AMD 29000 processor family
    ProcessorFamilyAmd29000 = 0x1E,
    /// AMD K6-2 Plus processor family
    ProcessorFamilyK6_2Plus = 0x1F,
    /// PowerPC processor family
    ProcessorFamilyPowerPC = 0x20,
    /// PowerPC 601 family
    ProcessorFamilyPowerPC601 = 0x21,
    /// PowerPC 603 family
    ProcessorFamilyPowerPC603 = 0x22,
    /// PowerPC 603 Plus family
    ProcessorFamilyPowerPC603Plus = 0x23,
    /// PowerPC 604 family
    ProcessorFamilyPowerPC604 = 0x24,
    /// PowerPC 620 family
    ProcessorFamilyPowerPC620 = 0x25,
    /// PowerPC x704 family
    ProcessorFamilyPowerPCx704 = 0x26,
    /// PowerPC 750 family
    ProcessorFamilyPowerPC750 = 0x27,
    /// Intel Core Duo family
    ProcessorFamilyIntelCoreDuo = 0x28,
    /// Intel Core Duo Mobile family
    ProcessorFamilyIntelCoreDuoMobile = 0x29,
    /// Intel Core Solo Mobile family
    ProcessorFamilyIntelCoreSoloMobile = 0x2A,
    /// Intel Atom family
    ProcessorFamilyIntelAtom = 0x2B,
    /// Intel Core M family
    ProcessorFamilyIntelCoreM = 0x2C,
    /// Intel Core m3 family
    ProcessorFamilyIntelCorem3 = 0x2D,
    /// Intel Core m5 family
    ProcessorFamilyIntelCorem5 = 0x2E,
    /// Intel Core m7 family
    ProcessorFamilyIntelCorem7 = 0x2F,
    /// Alpha family
    ProcessorFamilyAlpha = 0x30,
    /// Alpha 21064 family
    ProcessorFamilyAlpha21064 = 0x31,
    /// Alpha 21066 family
    ProcessorFamilyAlpha21066 = 0x32,
    /// Alpha 21164 family
    ProcessorFamilyAlpha21164 = 0x33,
    /// Alpha 21164PC family
    ProcessorFamilyAlpha21164PC = 0x34,
    /// Alpha 21164a family
    ProcessorFamilyAlpha21164a = 0x35,
    /// Alpha 21264 family
    ProcessorFamilyAlpha21264 = 0x36,
    /// Alpha 21364 family
    ProcessorFamilyAlpha21364 = 0x37,
    /// AMD Turion II Ultra Dual Core Mobile M family
    ProcessorFamilyAmdTurionIIUltraDualCoreMobileM = 0x38,
    /// AMD Turion II Dual Core Mobile M family
    ProcessorFamilyAmdTurionIIDualCoreMobileM = 0x39,
    /// AMD Athlon II Dual Core M family
    ProcessorFamilyAmdAthlonIIDualCoreM = 0x3A,
    /// AMD Opteron 6100 Series family
    ProcessorFamilyAmdOpteron6100Series = 0x3B,
    /// AMD Opteron 4100 Series family
    ProcessorFamilyAmdOpteron4100Series = 0x3C,
    /// AMD Opteron 6200 Series family
    ProcessorFamilyAmdOpteron6200Series = 0x3D,
    /// AMD Opteron 4200 Series family
    ProcessorFamilyAmdOpteron4200Series = 0x3E,
    /// AMD FX Series family
    ProcessorFamilyAmdFxSeries = 0x3F,
    /// MIPS family
    ProcessorFamilyMips = 0x40,
    /// MIPS R4000 family
    ProcessorFamilyMIPSR4000 = 0x41,
    /// MIPS R4200 family
    ProcessorFamilyMIPSR4200 = 0x42,
    /// MIPS R4400 family
    ProcessorFamilyMIPSR4400 = 0x43,
    /// MIPS R4600 family
    ProcessorFamilyMIPSR4600 = 0x44,
    /// MIPS R10000 family
    ProcessorFamilyMIPSR10000 = 0x45,
    /// AMD C Series family
    ProcessorFamilyAmdCSeries = 0x46,
    /// AMD E Series family
    ProcessorFamilyAmdESeries = 0x47,
    /// AMD A Series family (SMBIOS spec 2.8.0 updated the name)
    ProcessorFamilyAmdASeries = 0x48,
    /// AMD G Series family
    ProcessorFamilyAmdGSeries = 0x49,
    /// AMD Z Series family
    ProcessorFamilyAmdZSeries = 0x4A,
    /// AMD R Series family
    ProcessorFamilyAmdRSeries = 0x4B,
    /// AMD Opteron 4300 family
    ProcessorFamilyAmdOpteron4300 = 0x4C,
    /// AMD Opteron 6300 family
    ProcessorFamilyAmdOpteron6300 = 0x4D,
    /// AMD Opteron 3300 family
    ProcessorFamilyAmdOpteron3300 = 0x4E,
    /// AMD FirePro Series family
    ProcessorFamilyAmdFireProSeries = 0x4F,
    /// SPARC family
    ProcessorFamilySparc = 0x50,
    /// SuperSPARC family
    ProcessorFamilySuperSparc = 0x51,
    /// microSPARC II family
    ProcessorFamilymicroSparcII = 0x52,
    /// microSPARC IIep family
    ProcessorFamilymicroSparcIIep = 0x53,
    /// UltraSPARC family
    ProcessorFamilyUltraSparc = 0x54,
    /// UltraSPARC II family
    ProcessorFamilyUltraSparcII = 0x55,
    /// UltraSPARC III family
    ProcessorFamilyUltraSparcIii = 0x56,
    /// UltraSPARC III family
    ProcessorFamilyUltraSparcIII = 0x57,
    /// UltraSPARC IIIi family
    ProcessorFamilyUltraSparcIIIi = 0x58,
    /// Motorola 68040 family
    ProcessorFamily68040 = 0x60,
    /// Motorola 68xxx family
    ProcessorFamily68xxx = 0x61,
    /// Motorola 68000 family
    ProcessorFamily68000 = 0x62,
    /// Motorola 68010 family
    ProcessorFamily68010 = 0x63,
    /// Motorola 68020 family
    ProcessorFamily68020 = 0x64,
    /// Motorola 68030 family
    ProcessorFamily68030 = 0x65,
    /// AMD Athlon X4 Quad Core family
    ProcessorFamilyAmdAthlonX4QuadCore = 0x66,
    /// AMD Opteron X1000 Series family
    ProcessorFamilyAmdOpteronX1000Series = 0x67,
    /// AMD Opteron X2000 Series family
    ProcessorFamilyAmdOpteronX2000Series = 0x68,
    /// AMD Opteron A Series family
    ProcessorFamilyAmdOpteronASeries = 0x69,
    /// AMD Opteron X3000 Series family
    ProcessorFamilyAmdOpteronX3000Series = 0x6A,
    /// AMD Zen family
    ProcessorFamilyAmdZen = 0x6B,
    /// Hobbit family
    ProcessorFamilyHobbit = 0x70,
    /// Crusoe TM5000 family
    ProcessorFamilyCrusoeTM5000 = 0x78,
    /// Crusoe TM3000 family
    ProcessorFamilyCrusoeTM3000 = 0x79,
    /// Efficeon TM8000 family
    ProcessorFamilyEfficeonTM8000 = 0x7A,
    /// Weitek family
    ProcessorFamilyWeitek = 0x80,
    /// Itanium family
    ProcessorFamilyItanium = 0x82,
    /// AMD Athlon 64 family
    ProcessorFamilyAmdAthlon64 = 0x83,
    /// AMD Opteron family
    ProcessorFamilyAmdOpteron = 0x84,
    /// AMD Sempron family
    ProcessorFamilyAmdSempron = 0x85,
    /// AMD Turion 64 Mobile family
    ProcessorFamilyAmdTurion64Mobile = 0x86,
    /// Dual Core AMD Opteron family
    ProcessorFamilyDualCoreAmdOpteron = 0x87,
    /// AMD Athlon 64 X2 Dual Core family
    ProcessorFamilyAmdAthlon64X2DualCore = 0x88,
    /// AMD Turion 64 X2 Mobile family
    ProcessorFamilyAmdTurion64X2Mobile = 0x89,
    /// Quad Core AMD Opteron family
    ProcessorFamilyQuadCoreAmdOpteron = 0x8A,
    /// Third Generation AMD Opteron family
    ProcessorFamilyThirdGenerationAmdOpteron = 0x8B,
    /// AMD Phenom FX Quad Core family
    ProcessorFamilyAmdPhenomFxQuadCore = 0x8C,
    /// AMD Phenom X4 Quad Core family
    ProcessorFamilyAmdPhenomX4QuadCore = 0x8D,
    /// AMD Phenom X2 Dual Core family
    ProcessorFamilyAmdPhenomX2DualCore = 0x8E,
    /// AMD Athlon X2 Dual Core family
    ProcessorFamilyAmdAthlonX2DualCore = 0x8F,
    /// PA-RISC family
    ProcessorFamilyPARISC = 0x90,
    /// PA-RISC 8500 family
    ProcessorFamilyPaRisc8500 = 0x91,
    /// PA-RISC 8000 family
    ProcessorFamilyPaRisc8000 = 0x92,
    /// PA-RISC 7300LC family
    ProcessorFamilyPaRisc7300LC = 0x93,
    /// PA-RISC 7200 family
    ProcessorFamilyPaRisc7200 = 0x94,
    /// PA-RISC 7100LC family
    ProcessorFamilyPaRisc7100LC = 0x95,
    /// PA-RISC 7100 family
    ProcessorFamilyPaRisc7100 = 0x96,
    /// V30 family
    ProcessorFamilyV30 = 0xA0,
    /// Quad Core Intel Xeon 3200 Series family
    ProcessorFamilyQuadCoreIntelXeon3200Series = 0xA1,
    /// Dual Core Intel Xeon 3000 Series family
    ProcessorFamilyDualCoreIntelXeon3000Series = 0xA2,
    /// Quad Core Intel Xeon 5300 Series family
    ProcessorFamilyQuadCoreIntelXeon5300Series = 0xA3,
    /// Dual-core Intel Xeon 5100 Series processor family
    ProcessorFamilyDualCoreIntelXeon5100Series = 0xA4,
    /// Dual-core Intel Xeon 5000 Series processor family
    ProcessorFamilyDualCoreIntelXeon5000Series = 0xA5,
    /// Dual-core Intel Xeon LV processor family
    ProcessorFamilyDualCoreIntelXeonLV = 0xA6,
    /// Dual-core Intel Xeon ULV processor family
    ProcessorFamilyDualCoreIntelXeonULV = 0xA7,
    /// Dual-core Intel Xeon 7100 Series processor family
    ProcessorFamilyDualCoreIntelXeon7100Series = 0xA8,
    /// Quad-core Intel Xeon 5400 Series processor family
    ProcessorFamilyQuadCoreIntelXeon5400Series = 0xA9,
    /// Quad-core Intel Xeon processor family
    ProcessorFamilyQuadCoreIntelXeon = 0xAA,
    /// Dual-core Intel Xeon 5200 Series processor family
    ProcessorFamilyDualCoreIntelXeon5200Series = 0xAB,
    /// Dual-core Intel Xeon 7200 Series processor family
    ProcessorFamilyDualCoreIntelXeon7200Series = 0xAC,
    /// Quad-core Intel Xeon 7300 Series processor family
    ProcessorFamilyQuadCoreIntelXeon7300Series = 0xAD,
    /// Quad-core Intel Xeon 7400 Series processor family
    ProcessorFamilyQuadCoreIntelXeon7400Series = 0xAE,
    /// Multi-core Intel Xeon 7400 Series processor family
    ProcessorFamilyMultiCoreIntelXeon7400Series = 0xAF,
    /// Pentium III Xeon processor family
    ProcessorFamilyPentiumIIIXeon = 0xB0,
    /// Pentium III SpeedStep processor family
    ProcessorFamilyPentiumIIISpeedStep = 0xB1,
    /// Pentium 4 processor family
    ProcessorFamilyPentium4 = 0xB2,
    /// Intel Xeon processor family
    ProcessorFamilyIntelXeon = 0xB3,
    /// AS400 processor family
    ProcessorFamilyAS400 = 0xB4,
    /// Intel Xeon MP processor family
    ProcessorFamilyIntelXeonMP = 0xB5,
    /// AMD Athlon XP processor family
    ProcessorFamilyAMDAthlonXP = 0xB6,
    /// AMD Athlon MP processor family
    ProcessorFamilyAMDAthlonMP = 0xB7,
    /// Intel Itanium 2 processor family
    ProcessorFamilyIntelItanium2 = 0xB8,
    /// Intel Pentium M processor family
    ProcessorFamilyIntelPentiumM = 0xB9,
    /// Intel Celeron D processor family
    ProcessorFamilyIntelCeleronD = 0xBA,
    /// Intel Pentium D processor family
    ProcessorFamilyIntelPentiumD = 0xBB,
    /// Intel Pentium Ex processor family
    ProcessorFamilyIntelPentiumEx = 0xBC,
    /// Intel Core Solo processor family (SMBIOS spec 2.6 updated this value)
    ProcessorFamilyIntelCoreSolo = 0xBD, // SMBIOS spec 2.6 updated this value
    /// Reserved processor family
    ProcessorFamilyReserved = 0xBE,
    /// Intel Core 2 processor family
    ProcessorFamilyIntelCore2 = 0xBF,
    /// Intel Core 2 Solo processor family
    ProcessorFamilyIntelCore2Solo = 0xC0,
    /// Intel Core 2 Extreme processor family
    ProcessorFamilyIntelCore2Extreme = 0xC1,
    /// Intel Core 2 Quad processor family
    ProcessorFamilyIntelCore2Quad = 0xC2,
    /// Intel Core 2 Extreme Mobile processor family
    ProcessorFamilyIntelCore2ExtremeMobile = 0xC3,
    /// Intel Core 2 Duo Mobile processor family
    ProcessorFamilyIntelCore2DuoMobile = 0xC4,
    /// Intel Core 2 Solo Mobile processor family
    ProcessorFamilyIntelCore2SoloMobile = 0xC5,
    /// Intel Core i7 processor family
    ProcessorFamilyIntelCoreI7 = 0xC6,
    /// Dual-core Intel Celeron processor family
    ProcessorFamilyDualCoreIntelCeleron = 0xC7,
    /// IBM 390 processor family
    ProcessorFamilyIBM390 = 0xC8,
    /// Intel i860 processor family
    ProcessorFamilyi860 = 0xFA,
    /// Intel i960 processor family
    ProcessorFamilyi960 = 0xFB,
    /// Indicator for Processor Family 2
    ProcessorFamilyIndicatorFamily2 = 0xFE,
    /// Reserved processor family
    ProcessorFamilyReserved1 = 0xFF,
}

///
/// Processor Information2 - Processor Family2.
///
pub enum ProcessorFamily2Data {
    /// ARMv7 processor family
    ProcessorFamilyARMv7 = 0x0100,
    /// ARMv8 processor family
    ProcessorFamilyARMv8 = 0x0101,
    /// ARMv9 processor family
    ProcessorFamilyARMv9 = 0x0102,
    /// SH3 processor family
    ProcessorFamilySH3 = 0x0104,
    /// SH4 processor family
    ProcessorFamilySH4 = 0x0105,
    /// ARM processor family
    ProcessorFamilyARM = 0x0118,
    /// StrongARM processor family
    ProcessorFamilyStrongARM = 0x0119,
    /// 6x86 processor family
    ProcessorFamily6x86 = 0x012C,
    /// MediaGX processor family
    ProcessorFamilyMediaGX = 0x012D,
    /// MII processor family
    ProcessorFamilyMII = 0x012E,
    /// WinChip processor family
    ProcessorFamilyWinChip = 0x0140,
    /// DSP processor family
    ProcessorFamilyDSP = 0x015E,
    /// Video processor family
    ProcessorFamilyVideoProcessor = 0x01F4,
    /// RISC-V RV32 processor family
    ProcessorFamilyRiscvRV32 = 0x0200,
    /// RISC-V RV64 processor family
    ProcessorFamilyRiscVRV64 = 0x0201,
    /// RISC-V RV128 processor family
    ProcessorFamilyRiscVRV128 = 0x0202,
    /// LoongArch processor family
    ProcessorFamilyLoongArch = 0x0258,
    /// Loongson 1 processor family
    ProcessorFamilyLoongson1 = 0x0259,
    /// Loongson 2 processor family
    ProcessorFamilyLoongson2 = 0x025A,
    /// Loongson 3 processor family
    ProcessorFamilyLoongson3 = 0x025B,
    /// Loongson 2K processor family
    ProcessorFamilyLoongson2K = 0x025C,
    /// Loongson 3A processor family
    ProcessorFamilyLoongson3A = 0x025D,
    /// Loongson 3B processor family
    ProcessorFamilyLoongson3B = 0x025E,
    /// Loongson 3C processor family
    ProcessorFamilyLoongson3C = 0x025F,
    /// Loongson 3D processor family
    ProcessorFamilyLoongson3D = 0x0260,
    /// Loongson 3E processor family
    ProcessorFamilyLoongson3E = 0x0261,
    /// Dual-core Loongson 2K processor family
    ProcessorFamilyDualCoreLoongson2K = 0x0262,
    /// Quad-core Loongson 3A processor family
    ProcessorFamilyQuadCoreLoongson3A = 0x026C,
    /// Multi-core Loongson 3A processor family
    ProcessorFamilyMultiCoreLoongson3A = 0x026D,
    /// Quad-core Loongson 3B processor family
    ProcessorFamilyQuadCoreLoongson3B = 0x026E,
    /// Multi-core Loongson 3B processor family
    ProcessorFamilyMultiCoreLoongson3B = 0x026F,
    /// Multi-core Loongson 3C processor family
    ProcessorFamilyMultiCoreLoongson3C = 0x0270,
    /// Multi-core Loongson 3D processor family
    ProcessorFamilyMultiCoreLoongson3D = 0x0271,
    /// Intel Core 3 processor family
    ProcessorFamilyIntelCore3 = 0x0300,
    /// Intel Core 5 processor family
    ProcessorFamilyIntelCore5 = 0x0301,
    /// Intel Core 7 processor family
    ProcessorFamilyIntelCore7 = 0x0302,
    /// Intel Core 9 processor family
    ProcessorFamilyIntelCore9 = 0x0303,
    /// Intel Core Ultra 3 processor family
    ProcessorFamilyIntelCoreUltra3 = 0x0304,
    /// Intel Core Ultra 5 processor family
    ProcessorFamilyIntelCoreUltra5 = 0x0305,
    /// Intel Core Ultra 7 processor family
    ProcessorFamilyIntelCoreUltra7 = 0x0306,
    /// Intel Core Ultra 9 processor family
    ProcessorFamilyIntelCoreUltra9 = 0x0307,
}

///
/// Processor Information - Voltage.
///
bitfield! {
    /// Bitfield for processor voltage
    pub struct ProcessorVoltage (u8);
    impl Debug;
    /// Gets the processor voltage capability 5V bit
    pub processor_voltage_capability_5v, set_processor_voltage_capability_5v: 0;
    /// Gets the processor voltage capability 3.3V bit
    pub processor_voltage_capability_3_3v, set_processor_voltage_capability_3_3v: 1;
    /// Gets the processor voltage capability 2.9V bit
    pub processor_voltage_capability_2_9v, set_processor_voltage_capability_2_9v: 2;
    /// Gets the processor voltage capability reserved bit
    pub processor_voltage_capability_reserved, set_processor_voltage_capability_reserved: 3;
    /// Gets the processor voltage reserved bits
    pub processor_voltage_reserved, set_processor_voltage_reserved: 6, 4;
    /// Indicates legacy processor voltage
    pub processor_voltage_indicate_legacy, set_processor_voltage_indicate_legacy: 7;
}

///
/// Processor Information - Processor Upgrade.
///
pub enum ProcessorUpgrade {
    /// Other processor upgrade
    Other = 0x01,
    /// Unknown processor upgrade
    Unknown = 0x02,
    /// Daughter board processor upgrade
    DaughterBoard = 0x03,
    /// ZIF socket processor upgrade
    ZIFSocket = 0x04,
    /// PiggyBack processor upgrade (replaceable)
    PiggyBack = 0x05, //  Replaceable.
    /// No processor upgrade
    None = 0x06,
    /// LIF socket processor upgrade
    LIFSocket = 0x07,
    /// Slot 1 processor upgrade
    Slot1 = 0x08,
    /// Slot 2 processor upgrade
    Slot2 = 0x09,
    /// Pin 370 socket processor upgrade
    Pin370Socket = 0x0A,
    /// Slot A processor upgrade
    SlotA = 0x0B,
    /// Slot M processor upgrade
    SlotM = 0x0C,
    /// Socket 423 processor upgrade
    Socket423 = 0x0D,
    /// Socket A processor upgrade (Socket 462)
    SocketA = 0x0E, //  Socket 462.
    /// Socket 478 processor upgrade
    Socket478 = 0x0F,
    /// Socket 754 processor upgrade
    Socket754 = 0x10,
    /// Socket 940 processor upgrade
    Socket940 = 0x11,
    /// Socket 939 processor upgrade
    Socket939 = 0x12,
    /// Socket mPGA604 processor upgrade
    SocketmPGA604 = 0x13,
    /// Socket LGA771 processor upgrade
    SocketLGA771 = 0x14,
    /// Socket LGA775 processor upgrade
    SocketLGA775 = 0x15,
    /// Socket S1 processor upgrade
    SocketS1 = 0x16,
    /// AM2 processor upgrade
    AM2 = 0x17,
    /// F1207 processor upgrade
    F1207 = 0x18,
    /// Socket LGA1366 processor upgrade
    SocketLGA1366 = 0x19,
    /// Socket G34 processor upgrade
    SocketG34 = 0x1A,
    /// Socket AM3 processor upgrade
    SocketAM3 = 0x1B,
    /// Socket C32 processor upgrade
    SocketC32 = 0x1C,
    /// Socket LGA1156 processor upgrade
    SocketLGA1156 = 0x1D,
    /// Socket LGA1567 processor upgrade
    SocketLGA1567 = 0x1E,
    /// Socket PGA988A processor upgrade
    SocketPGA988A = 0x1F,
    /// Socket BGA1288 processor upgrade
    SocketBGA1288 = 0x20,
    /// Socket rPGA988B processor upgrade
    SocketrPGA988B = 0x21,
    /// Socket BGA1023 processor upgrade
    SocketBGA1023 = 0x22,
    /// Socket BGA1224 processor upgrade
    SocketBGA1224 = 0x23,
    /// Socket LGA1155 processor upgrade (SMBIOS spec 2.8.0 updated the name)
    SocketLGA1155 = 0x24, //  SMBIOS spec 2.8.0 updated the name
    /// Socket LGA1356 processor upgrade
    SocketLGA1356 = 0x25,
    /// Socket LGA2011 processor upgrade
    SocketLGA2011 = 0x26,
    /// Socket FS1 processor upgrade
    SocketFS1 = 0x27,
    /// Socket FS2 processor upgrade
    SocketFS2 = 0x28,
    /// Socket FM1 processor upgrade
    SocketFM1 = 0x29,
    /// Socket FM2 processor upgrade
    SocketFM2 = 0x2A,
    /// Socket LGA2011-3 processor upgrade
    SocketLGA2011_3 = 0x2B,
    /// Socket LGA1356-3 processor upgrade
    SocketLGA1356_3 = 0x2C,
    /// Socket LGA1150 processor upgrade
    SocketLGA1150 = 0x2D,
    /// Socket BGA1168 processor upgrade
    SocketBGA1168 = 0x2E,
    /// Socket BGA1234 processor upgrade
    SocketBGA1234 = 0x2F,
    /// Socket BGA1364 processor upgrade
    SocketBGA1364 = 0x30,
    /// Socket AM4 processor upgrade
    SocketAM4 = 0x31,
    /// Socket LGA1151 processor upgrade
    SocketLGA1151 = 0x32,
    /// Socket BGA1356 processor upgrade
    SocketBGA1356 = 0x33,
    /// Socket BGA1440 processor upgrade
    SocketBGA1440 = 0x34,
    /// Socket BGA1515 processor upgrade
    SocketBGA1515 = 0x35,
    /// Socket LGA3647-1 processor upgrade
    SocketLGA3647_1 = 0x36,
    /// Socket SP3 processor upgrade
    SocketSP3 = 0x37,
    /// Socket SP3r2 processor upgrade
    SocketSP3r2 = 0x38,
    /// Socket LGA2066 processor upgrade
    SocketLGA2066 = 0x39,
    /// Socket BGA1392 processor upgrade
    SocketBGA1392 = 0x3A,
    /// Socket BGA1510 processor upgrade
    SocketBGA1510 = 0x3B,
    /// Socket BGA1528 processor upgrade
    SocketBGA1528 = 0x3C,
    /// Socket LGA4189 processor upgrade
    SocketLGA4189 = 0x3D,
    /// Socket LGA1200 processor upgrade
    SocketLGA1200 = 0x3E,
    /// Socket LGA4677 processor upgrade
    SocketLGA4677 = 0x3F,
    /// Socket LGA1700 processor upgrade
    SocketLGA1700 = 0x40,
    /// Socket BGA1744 processor upgrade
    SocketBGA1744 = 0x41,
    /// Socket BGA1781 processor upgrade
    SocketBGA1781 = 0x42,
    /// Socket BGA1211 processor upgrade
    SocketBGA1211 = 0x43,
    /// Socket BGA2422 processor upgrade
    SocketBGA2422 = 0x44,
    /// Socket LGA1211 processor upgrade
    SocketLGA1211 = 0x45,
    /// Socket LGA2422 processor upgrade
    SocketLGA2422 = 0x46,
    /// Socket LGA5773 processor upgrade
    SocketLGA5773 = 0x47,
    /// Socket BGA5773 processor upgrade
    SocketBGA5773 = 0x48,
    /// Socket AM5 processor upgrade
    SocketAM5 = 0x49,
    /// Socket SP5 processor upgrade
    SocketSP5 = 0x4A,
    /// Socket SP6 processor upgrade
    SocketSP6 = 0x4B,
    /// Socket BGA883 processor upgrade
    SocketBGA883 = 0x4C,
    /// Socket BGA1190 processor upgrade
    SocketBGA1190 = 0x4D,
    /// Socket BGA4129 processor upgrade
    SocketBGA4129 = 0x4E,
    /// Socket LGA4710 processor upgrade
    SocketLGA4710 = 0x4F,
    /// Socket LGA7529 processor upgrade
    SocketLGA7529 = 0x50,
    /// Socket BGA1964 processor upgrade
    SocketBGA1964 = 0x51,
    /// Socket BGA1792 processor upgrade
    SocketBGA1792 = 0x52,
    /// Socket BGA2049 processor upgrade
    SocketBGA2049 = 0x53,
    /// Socket BGA2551 processor upgrade
    SocketBGA2551 = 0x54,
    /// Socket LGA1851 processor upgrade
    SocketLGA1851 = 0x55,
    /// Socket BGA2114 processor upgrade
    SocketBGA2114 = 0x56,
    /// Socket BGA2833 processor upgrade
    SocketBGA2833 = 0x57,
}

///
/// Processor ID Field Description
///
bitfield! {
    /// Bitfield for processor signature
    pub struct ProcessorSignature (u32);
    impl Debug;
    /// Processor stepping ID
    pub processor_stepping_id, set_processor_stepping_id: 3, 0;
    /// Processor model
    pub processor_model, set_processor_model: 7, 4;
    /// Processor family
    pub processor_family, set_processor_family: 11, 8;
    /// Processor type
    pub processor_type, set_processor_type: 13, 12;
    /// Reserved bits 1
    pub processor_reserved1, set_processor_reserved1: 15, 14;
    /// Extended processor model
    pub processor_x_model, set_processor_x_model: 19, 16;
    /// Extended processor family
    pub processor_x_family, set_processor_x_family: 27, 20;
    /// Reserved bits 2
    pub processor_reserved2, set_processor_reserved2: 31, 28;
}

// PROCESSOR_FEATURE_FLAGS
bitfield! {
    /// Bitfield for processor feature flags
    pub struct ProcessorFeatureFlags (u32);
    impl Debug;
    /// Floating Point Unit (FPU) present
    pub processor_fpu, set_processor_fpu: 0;
    /// Virtual Mode Extensions (VME) present
    pub processor_vme, set_processor_vme: 1;
    /// Debugging Extensions (DE) present
    pub processor_de, set_processor_de: 2;
    /// Page Size Extensions (PSE) present
    pub processor_pse, set_processor_pse: 3;
    /// Time Stamp Counter (TSC) present
    pub processor_tsc, set_processor_tsc: 4;
    /// Model Specific Registers (MSR) present
    pub processor_msr, set_processor_msr: 5;
    /// Physical Address Extension (PAE) present
    pub processor_pae, set_processor_pae: 6;
    /// Machine Check Exception (MCE) present
    pub processor_mce, set_processor_mce: 7;
    /// CMPXCHG8B instruction present
    pub processor_cx8, set_processor_cx8: 8;
    /// APIC present
    pub processor_apic, set_processor_apic: 9;
    /// Reserved bits 1
    pub processor_reserved1, set_processor_reserved1: 10;
    /// SYSENTER/SYSEXIT present
    pub processor_sep, set_processor_sep: 11;
    /// Memory Type Range Registers (MTRR) present
    pub processor_mtrr, set_processor_mtrr: 12;
    /// Page Global Enable (PGE) present
    pub processor_pge, set_processor_pge: 13;
    /// Machine Check Architecture (MCA) present
    pub processor_mca, set_processor_mca: 14;
    /// Conditional Move (CMOV) present
    pub processor_cmov, set_processor_cmov: 15;
    /// Page Attribute Table (PAT) present
    pub processor_pat, set_processor_pat: 16;
    /// 36-bit Page Size Extension (PSE36) present
    pub processor_pse36, set_processor_pse36: 17;
    /// Processor Serial Number (PSN) present
    pub processor_psn, set_processor_psn: 18;
    /// CLFLUSH instruction present
    pub processor_clfsh, set_processor_clfsh: 19;
    /// Reserved bits 2
    pub processor_reserved2, set_processor_reserved2: 20;
    /// Debug Store (DS) present
    pub processor_ds, set_processor_ds: 21;
    /// ACPI present
    pub processor_acpi, set_processor_acpi: 22;
    /// MMX present
    pub processor_mmx, set_processor_mmx: 23;
    /// FXSAVE/FXRSTOR present
    pub processor_fxsr, set_processor_fxsr: 24;
    /// SSE present
    pub processor_sse, set_processor_sse: 25;
    /// SSE2 present
    pub processor_sse2, set_processor_sse2: 26;
    /// Self Snoop present
    pub processor_ss, set_processor_ss: 27;
    /// Reserved bits 3
    pub processor_reserved3, set_processor_reserved3: 28;
    /// Thermal Monitor (TM) present
    pub processor_tm, set_processor_tm: 29;
    /// Reserved bits 4
    pub processor_reserved4, set_processor_reserved4: 31, 30;
}

// PROCESSOR_CHARACTERISTIC_FLAGS
bitfield! {
    /// Bitfield for processor characteristic flags
    pub struct ProcessorCharacteristics(u16);
    impl Debug;
    /// Returns true if the processor is 64-bit capable
    pub is_64_bit_capable, set_is_64_bit_capable: 0;
    /// Returns true if the processor is multi-core
    pub is_multi_core, set_is_multi_core: 1;
    /// Returns true if the processor supports hardware threads
    pub is_hardware_thread, set_is_hardware_thread: 2;
    /// Returns true if the processor supports execute protection
    pub is_execute_protection, set_is_execute_protection: 3;
    /// Returns true if the processor supports enhanced virtualization
    pub is_enhanced_virtualization, set_is_enhanced_virtualization: 4;
    /// Returns true if the processor supports power/performance control
    pub is_power_performance_control, set_is_power_performance_control: 5;
    /// Reserved bits
    pub reserved, set_reserved: 6, 15;
}
//
bitfield! {
    /// Bitfield for processor status bits
    #[derive(Copy, Clone)]
    pub struct ProcessorStatusBits(u8);
    impl Debug;
    /// Indicates the status of the processor
    pub cpu_status, set_cpu_status: 2, 0;
    /// Reserved for future use. Must be set to zero
    pub reserved1, set_reserved1: 5, 3;
    /// Indicates if the processor socket is populated or not
    pub socket_populated, set_socket_populated: 6;
    /// Reserved for future use. Must be set to zero
    pub reserved2, set_reserved2: 7;
}

/// Union for processor status data
#[derive(Copy, Clone)]
pub union ProcessorStatusData {
    /// Status bits
    pub bits: ProcessorStatusBits,
    /// Raw status data
    pub data: u8,
}

/// Structure for processor ID data
#[repr(C, packed)]
pub struct ProcessorIdData {
    /// Processor signature
    pub signature: ProcessorSignature,
    /// Processor feature flags
    pub feature_flags: ProcessorFeatureFlags,
}

///
/// Processor Information (Type 4).
///
/// The information in this structure defines the attributes of a single processor;
/// a separate structure instance is provided for each system processor socket/slot.
/// For example, a system with an IntelDX2 processor would have a single
/// structure instance, while a system with an IntelSX2 processor would have a structure
/// to describe the main CPU, and a second structure to describe the 80487 co-processor.
///
/// Processor Information (Type 4)
#[repr(C, packed)]
pub struct SmbiosTableType4 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Processor socket string
    pub socket: SmbiosTableString,
    /// Processor type (enum value from PROCESSOR_TYPE_DATA)
    pub processor_type: u8, //  The enumeration value from PROCESSOR_TYPE_DATA.
    /// Processor family (enum value from PROCESSOR_FAMILY_DATA)
    pub processor_family: u8, //  The enumeration value from PROCESSOR_FAMILY_DATA.
    /// Processor manufacturer string
    pub processor_manufacturer: SmbiosTableString,
    /// Processor ID data
    pub processor_id: ProcessorIdData,
    /// Processor version string
    pub processor_version: SmbiosTableString,
    /// Processor voltage
    pub voltage: ProcessorVoltage,
    /// External clock speed (MHz)
    pub external_clock: u16,
    /// Maximum processor speed (MHz)
    pub max_speed: u16,
    /// Current processor speed (MHz)
    pub current_speed: u16,
    /// Processor status
    pub status: u8,
    /// Processor upgrade type (enum value from PROCESSOR_UPGRADE)
    pub processor_upgrade: u8, //  The enumeration value from PROCESSOR_UPGRADE.
    /// L1 cache handle
    pub l1_cache_handle: u16,
    /// L2 cache handle
    pub l2_cache_handle: u16,
    /// L3 cache handle
    pub l3_cache_handle: u16,
    /// Processor serial number string
    pub serial_number: SmbiosTableString,
    /// Processor asset tag string
    pub asset_tag: SmbiosTableString,
    /// Processor part number string
    pub part_number: SmbiosTableString,
    //
    // Add for smbios 2.5
    //
    /// Number of processor cores
    pub core_count: u8,
    /// Number of enabled processor cores
    pub enabled_core_count: u8,
    /// Number of processor threads
    pub thread_count: u8,
    /// Processor characteristics
    pub processor_characteristics: u16,
    //
    // Add for smbios 2.6
    //
    /// Processor family 2
    pub processor_family2: u16,
    //
    // Add for smbios 3.0
    //
    /// Number of processor cores (SMBIOS 3.0)
    pub core_count2: u16,
    /// Number of enabled processor cores (SMBIOS 3.0)
    pub enabled_core_count2: u16,
    /// Number of processor threads (SMBIOS 3.0)
    pub thread_count2: u16,
    //
    // Add for smbios 3.6
    //
    /// Number of enabled threads (SMBIOS 3.6)
    pub thread_enabled: u16,
    //
    // Add for smbios 3.8
    //
    /// Socket type string (SMBIOS 3.8)
    pub socket_type: SmbiosTableString,
}

/// Memory Controller Information (Type 5)
#[repr(C, packed)]
pub struct SmbiosTableType5 {
    /// Structure header
    pub hdr: SmbiosStructure,
    // TODO: Add fields
}

/// Memory Module Information (Type 6)
#[repr(C, packed)]
pub struct SmbiosTableType6 {
    /// Structure header
    pub hdr: SmbiosStructure,
    // TODO: Add fields
}

/// On Board Devices Information (Type 10)
#[repr(C, packed)]
pub struct SmbiosTableType10 {
    /// Structure header
    pub hdr: SmbiosStructure,
    // TODO: Add fields
}

///
/// Memory Controller Error Detecting Method.
///
/// Memory Controller Error Detecting Method
pub enum MemoryErrorDetectMethod {
    /// Other error detect method
    Other = 0x01,
    /// Unknown error detect method
    Unknown = 0x02,
    /// No error detect method
    None = 0x03,
    /// Parity error detect method
    Parity = 0x04,
    /// ECC 32 error detect method
    Ecc32 = 0x05,
    /// ECC 64 error detect method
    Ecc64 = 0x06,
    /// ECC 128 error detect method
    Ecc128 = 0x07,
    /// CRC error detect method
    Crc = 0x08,
}

///
/// Memory Controller Error Correcting Capability.
///
/// Memory Controller Error Correcting Capability bitfield
bitfield! {
    /// Bitfield for error correcting capability
    pub struct MemoryErrorCorrectCapability(u8);
    impl Debug;
    /// Other error correct capability
    pub other, set_other: 0;
    /// Unknown error correct capability
    pub unknown, set_unknown: 1;
    /// No error correct capability
    pub none, set_none: 2;
    /// Single-bit error correct capability
    pub single_bit_error_correct, set_single_bit_error_correct: 3;
    /// Double-bit error correct capability
    pub double_bit_error_correct, set_double_bit_error_correct: 4;
    /// Error scrubbing capability
    pub error_scrubbing, set_error_scrubbing: 5;
    /// Reserved bits
    pub reserved, set_reserved: 7, 6;
}

///
/// Memory Controller Information - Interleave Support.
///
pub enum MemorySupportInterleaveType {
    /// Other memory interleave
    MemoryInterleaveOther = 0x01,
    /// Unknown memory interleave
    MemoryInterleaveUnknown = 0x02,
    /// One-way interleave
    MemoryInterleaveOneWay = 0x03,
    /// Two-way interleave
    MemoryInterleaveTwoWay = 0x04,
    /// Four-way interleave
    MemoryInterleaveFourWay = 0x05,
    /// Eight-way interleave
    MemoryInterleaveEightWay = 0x06,
    /// Sixteen-way interleave
    MemoryInterleaveSixteenWay = 0x07,
}

///
/// Memory Controller Information - Memory Speeds.
///
bitfield! {
    /// Memory speed type bitfield
    pub struct MemorySpeedType(u16);
    impl Debug;
    /// Other
    pub other, set_other: 0;
    /// Unknown
    pub unknown, set_unknown: 1;
    /// 70ns
    pub seventy_ns, set_seventy_ns: 2;
    /// 60ns
    pub sixty_ns, set_sixty_ns: 3;
    /// 50ns
    pub fifty_ns, set_fifty_ns: 4;
    /// Reserved
    pub reserved, set_reserved: 15, 5;
}

///
/// Memory Module Information - Memory Types
///
bitfield! {
    /// Memory current type bitfield
    pub struct MemoryCurrentType(u16);
    impl Debug;
    /// Other
    pub other, set_other: 0;
    /// Unknown
    pub unknown, set_unknown: 1;
    /// Standard
    pub standard, set_standard: 2;
    /// Fast page mode
    pub fast_page_mode, set_fast_page_mode: 3;
    /// EDO
    pub edo, set_edo: 4;
    /// Parity
    pub parity, set_parity: 5;
    /// ECC
    pub ecc, set_ecc: 6;
    /// SIMM
    pub simm, set_simm: 7;
    /// DIMM
    pub dimm, set_dimm: 8;
    /// Burst EDO
    pub burst_edo, set_burst_edo: 9;
    /// SDRAM
    pub sdram, set_sdram: 10;
    /// Reserved
    pub reserved, set_reserved: 15, 11;
}

///
/// Memory Module Information - Memory Size.
///
bitfield! {
    /// Memory installed/enabled size bitfield
    pub struct MemoryInstalledEnabledSize(u8);
    impl Debug;
    /// Installed or enabled size
    pub installed_or_enabled_size, set_installed_or_enabled_size: 0, 6;
    /// Single or double bank
    pub single_or_double_bank, set_single_or_double_bank: 7;
}

///
/// Cache Information - SRAM Type.
///
bitfield! {
    /// Cache SRAM type bitfield
    pub struct CacheSramTypeData(u16);
    impl Debug;
    /// Other
    pub other, set_other: 0;
    /// Unknown
    pub unknown, set_unknown: 1;
    /// Non-burst
    pub non_burst, set_non_burst: 2;
    /// Burst
    pub burst, set_burst: 3;
    /// Pipeline burst
    pub pipeline_burst, set_pipeline_burst: 4;
    /// Synchronous
    pub synchronous, set_synchronous: 5;
    /// Asynchronous
    pub asynchronous, set_asynchronous: 6;
    /// Reserved
    pub reserved, set_reserved: 15, 7;
}

///
/// Cache Information - Error Correction Type.
///
pub enum CacheErrorTypeData {
    /// Other cache error
    CacheErrorOther = 0x01,
    /// Unknown cache error
    CacheErrorUnknown = 0x02,
    /// No cache error
    CacheErrorNone = 0x03,
    /// Parity error
    CacheErrorParity = 0x04,
    /// Single-bit ECC error
    CacheErrorSingleBit = 0x05,
    /// Multi-bit ECC error
    CacheErrorMultiBit = 0x06,
}

///
/// Cache Information - System Cache Type.
///
pub enum CacheTypeData {
    /// Other cache type
    CacheTypeOther = 0x01,
    /// Unknown cache type
    CacheTypeUnknown = 0x02,
    /// Instruction cache
    CacheTypeInstruction = 0x03,
    /// Data cache
    CacheTypeData = 0x04,
    /// Unified cache
    CacheTypeUnified = 0x05,
}

///
/// Cache Information - Associativity.
///
pub enum CacheAssociativityData {
    /// Other associativity
    CacheAssociativityOther = 0x01,
    /// Unknown associativity
    CacheAssociativityUnknown = 0x02,
    /// Direct mapped
    CacheAssociativityDirectMapped = 0x03,
    /// 2-way associativity
    CacheAssociativityWay2 = 0x04,
    /// 4-way associativity
    CacheAssociativityWay4 = 0x05,
    /// Fully associative
    CacheAssociativityFully = 0x06,
    /// 8-way associativity
    CacheAssociativityWay8 = 0x07,
    /// 16-way associativity
    CacheAssociativityWay16 = 0x08,
    /// 12-way associativity
    CacheAssociativityWay12 = 0x09,
    /// 24-way associativity
    CacheAssociativityWay24 = 0x0A,
    /// 32-way associativity
    CacheAssociativityWay32 = 0x0B,
    /// 48-way associativity
    CacheAssociativityWay48 = 0x0C,
    /// 64-way associativity
    CacheAssociativityWay64 = 0x0D,
    /// 20-way associativity
    CacheAssociativityWay20 = 0x0E,
}

///
/// Cache Information (Type 7).
///
/// The information in this structure defines the attributes of CPU cache device in the system.
/// One structure is specified for each such device, whether the device is internal to
/// or external to the CPU module.  Cache modules can be associated with a processor structure
/// in one or two ways, depending on the SMBIOS version.
///
#[repr(C, packed)]
pub struct SmbiosTableType7 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Socket designation string
    pub socket_designation: SmbiosTableString,
    /// Cache configuration
    pub cache_configuration: u16,
    /// Maximum cache size
    pub maximum_cache_size: u16,
    /// Installed cache size
    pub installed_size: u16,
    /// Supported SRAM type
    pub supported_sram_type: CacheSramTypeData,
    /// Current SRAM type
    pub current_sram_type: CacheSramTypeData,
    /// Cache speed
    pub cache_speed: u8,
    /// Error correction type (CACHE_ERROR_TYPE_DATA)
    pub error_correction_type: u8,
    /// System cache type (CACHE_TYPE_DATA)
    pub system_cache_type: u8,
    /// Associativity (CACHE_ASSOCIATIVITY_DATA)
    pub associativity: u8,
    //
    // Add for smbios 3.1.0
    //
    /// Maximum cache size 2
    pub maximum_cache_size2: u32,
    /// Installed cache size 2
    pub installed_size2: u32,
}

///
/// Port Connector Information - Connector Types.
///
pub enum MiscPortConnectorType {
    /// None
    PortConnectorTypeNone = 0x00,
    /// Centronics
    PortConnectorTypeCentronics = 0x01,
    /// Mini Centronics
    PortConnectorTypeMiniCentronics = 0x02,
    /// Proprietary
    PortConnectorTypeProprietar = 0x03,
    /// DB25 Male
    PortConnectorTypeDB25Male = 0x04,
    /// DB25 Female
    PortConnectorTypeDB25Female = 0x05,
    /// DB15 Male
    PortConnectorTypeDB15Male = 0x06,
    /// DB15 Female
    PortConnectorTypeDB15Female = 0x07,
    /// DB9 Male
    PortConnectorTypeDB9Male = 0x08,
    /// DB9 Female
    PortConnectorTypeDB9Female = 0x09,
    /// RJ11
    PortConnectorTypeRJ11 = 0x0A,
    /// RJ45
    PortConnectorTypeRJ45 = 0x0B,
    /// 50-pin Mini SCSI
    PortConnectorType50PinMiniScsi = 0x0C,
    /// Mini DIN
    PortConnectorTypeMiniDin = 0x0D,
    /// Micro DIN
    PortConnectorTypeMicroDin = 0x0E,
    /// PS/2
    PortConnectorTypePS2 = 0x0F,
    /// Infrared
    PortConnectorTypeInfrared = 0x10,
    /// HP-HIL
    PortConnectorTypeHpHil = 0x11,
    /// USB
    PortConnectorTypeUsb = 0x12,
    /// SSA SCSI
    PortConnectorTypeSsaScsi = 0x13,
    /// Circular DIN 8 Male
    PortConnectorTypeCircularDin8Male = 0x14,
    /// Circular DIN 8 Female
    PortConnectorTypeCircularDin8Female = 0x15,
    /// Onboard IDE
    PortConnectorTypeOnboardIde = 0x16,
    /// Onboard Floppy
    PortConnectorTypeOnboardFloppy = 0x17,
    /// 9-pin Dual Inline
    PortConnectorType9PinDualInline = 0x18,
    /// 25-pin Dual Inline
    PortConnectorType25PinDualInline = 0x19,
    /// 50-pin Dual Inline
    PortConnectorType50PinDualInline = 0x1A,
    /// 68-pin Dual Inline
    PortConnectorType68PinDualInline = 0x1B,
    /// Onboard sound input
    PortConnectorTypeOnboardSoundInput = 0x1C,
    /// Mini Centronics Type 14
    PortConnectorTypeMiniCentronicsType14 = 0x1D,
    /// Mini Centronics Type 26
    PortConnectorTypeMiniCentronicsType26 = 0x1E,
    /// Headphone mini jack
    PortConnectorTypeHeadPhoneMiniJack = 0x1F,
    /// BNC
    PortConnectorTypeBNC = 0x20,
    /// IEEE 1394 (FireWire)
    PortConnectorType1394 = 0x21,
    /// SAS/SATA
    PortConnectorTypeSasSata = 0x22,
    /// USB Type-C
    PortConnectorTypeUsbTypeC = 0x23,
    /// PC98
    PortConnectorTypePC98 = 0xA0,
    /// PC98 Hireso
    PortConnectorTypePC98Hireso = 0xA1,
    /// PCH98
    PortConnectorTypePCH98 = 0xA2,
    /// PC98 Note
    PortConnectorTypePC98Note = 0xA3,
    /// PC98 Full
    PortConnectorTypePC98Full = 0xA4,
    /// Other
    PortConnectorTypeOther = 0xFF,
}

///
/// Port Connector Information - Port Types
///
#[repr(u8)]
pub enum MiscPortType {
    /// None
    None = 0x00,
    /// Parallel XT/AT compatible port
    ParallelXtAtCompatible = 0x01,
    /// Parallel port PS/2
    ParallelPortPs2 = 0x02,
    /// Parallel port ECP
    ParallelPortEcp = 0x03,
    /// Parallel port EPP
    ParallelPortEpp = 0x04,
    /// Parallel port ECP/EPP
    ParallelPortEcpEpp = 0x05,
    /// Serial XT/AT compatible port
    SerialXtAtCompatible = 0x06,
    /// Serial 16450 compatible port
    Serial16450Compatible = 0x07,
    /// Serial 16550 compatible port
    Serial16550Compatible = 0x08,
    /// Serial 16550A compatible port
    Serial16550ACompatible = 0x09,
    /// SCSI port
    Scsi = 0x0A,
    /// MIDI port
    Midi = 0x0B,
    /// Joystick port
    JoyStick = 0x0C,
    /// Keyboard port
    Keyboard = 0x0D,
    /// Mouse port
    Mouse = 0x0E,
    /// SSA SCSI port
    SsaScsi = 0x0F,
    /// USB port
    Usb = 0x10,
    /// FireWire port
    FireWire = 0x11,
    /// PCMCIA Type I port
    PcmciaTypeI = 0x12,
    /// PCMCIA Type II port
    PcmciaTypeII = 0x13,
    /// PCMCIA Type III port
    PcmciaTypeIII = 0x14,
    /// CardBus
    CardBus = 0x15,
    /// Access Bus Port
    AccessBusPort = 0x16,
    /// SCSI II
    ScsiII = 0x17,
    /// SCSI Wide
    ScsiWide = 0x18,
    /// PC98
    Pc98 = 0x19,
    /// PC98 Hireso
    Pc98Hireso = 0x1A,
    /// PCH98
    Pch98 = 0x1B,
    /// Video port
    VideoPort = 0x1C,
    /// Audio port
    AudioPort = 0x1D,
    /// Modem port
    ModemPort = 0x1E,
    /// Network port
    NetworkPort = 0x1F,
    /// SATA
    Sata = 0x20,
    /// SAS
    Sas = 0x21,
    /// Multi-Function Display Port
    Mfdp = 0x22,
    /// Thunderbolt
    Thunderbolt = 0x23,
    /// Compatible 8251
    Compatible8251 = 0xA0,
    /// Compatible 8251 FIFO
    Compatible8251Fifo = 0xA1,
    /// Other
    Other = 0xFF,
}

///
/// Port Connector Information (Type 8).
///
/// The information in this structure defines the attributes of a system port connector,
/// e.g. parallel, serial, keyboard, or mouse ports.  The port's type and connector information
/// are provided. One structure is present for each port provided by the system.
///
#[repr(C, packed)]
pub struct SmbiosTableType8 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Internal reference designator string
    pub internal_reference_designator: SmbiosTableString,
    /// Internal connector type (MISC_PORT_CONNECTOR_TYPE)
    pub internal_connector_type: u8,
    /// External reference designator string
    pub external_reference_designator: SmbiosTableString,
    /// External connector type (MISC_PORT_CONNECTOR_TYPE)
    pub external_connector_type: u8,
    /// Port type (MISC_PORT_TYPE)
    pub port_type: u8,
}

///
/// System Slots - Slot Type
///
#[repr(u8)]
pub enum MiscSlotType {
    /// Other slot type
    Other = 0x01,
    /// Unknown slot type
    Unknown = 0x02,
    /// ISA slot
    Isa = 0x03,
    /// MCA slot
    Mca = 0x04,
    /// EISA slot
    Eisa = 0x05,
    /// PCI slot
    Pci = 0x06,
    /// PCMCIA slot
    Pcmcia = 0x07,
    /// VL-VESA slot
    VlVesa = 0x08,
    /// Proprietary slot
    Proprietary = 0x09,
    /// Processor card slot
    ProcessorCardSlot = 0x0A,
    /// Proprietary memory card slot
    ProprietaryMemoryCardSlot = 0x0B,
    /// IO riser card slot
    IORiserCardSlot = 0x0C,
    /// NuBus slot
    NuBus = 0x0D,
    /// PCI 66MHz capable slot
    Pci66MhzCapable = 0x0E,
    /// AGP slot
    Agp = 0x0F,
    /// APG 2X slot
    Apg2X = 0x10,
    /// AGP 4X slot
    Agp4X = 0x11,
    /// PCI-X slot
    PciX = 0x12,
    /// AGP 8X slot
    Agp8X = 0x13,
    /// M.2 Socket 1 DP slot
    M2Socket1Dp = 0x14,
    /// M.2 Socket 1 SD slot
    M2Socket1Sd = 0x15,
    /// M.2 Socket 2 slot
    M2Socket2 = 0x16,
    /// M.2 Socket 3 slot
    M2Socket3 = 0x17,
    /// MXM Type I slot
    MxmTypeI = 0x18,
    /// MXM Type II slot
    MxmTypeII = 0x19,
    /// MXM Type III Standard slot
    MxmTypeIIIStandard = 0x1A,
    /// MXM Type III HE slot
    MxmTypeIIIHe = 0x1B,
    /// MXM Type IV slot
    MxmTypeIV = 0x1C,
    /// MXM 3.0 Type A slot
    Mxm30TypeA = 0x1D,
    /// MXM 3.0 Type B slot
    Mxm30TypeB = 0x1E,
    /// PCI Express Gen2 SFF-8639 slot
    PciExpressGen2Sff8639 = 0x1F,
    /// PCI Express Gen3 SFF-8639 slot
    PciExpressGen3Sff8639 = 0x20,
    /// PCI Express Mini 52-pin with BSKO slot
    PciExpressMini52pinWithBsko = 0x21,
    /// PCI Express Mini 52-pin without BSKO slot
    PciExpressMini52pinWithoutBsko = 0x22,
    /// PCI Express Mini 76-pin slot
    PciExpressMini76pin = 0x23,
    /// PCI Express Gen4 SFF-8639 slot
    PciExpressGen4Sff8639 = 0x24,
    /// PCI Express Gen5 SFF-8639 slot
    PciExpressGen5Sff8639 = 0x25,
    /// OCP NIC 3.0 small form factor slot
    OcpNic30SmallFormFactor = 0x26,
    /// OCP NIC 3.0 large form factor slot
    OcpNic30LargeFormFactor = 0x27,
    /// OCP NIC prior to 3.0 slot
    OcpNicPriorto30 = 0x28,
    /// CXL Flexbus 1.0 slot
    CxlFlexbus10 = 0x30,
    /// PC98 C20 slot
    Pc98C20 = 0xA0,
    /// PC98 C24 slot
    Pc98C24 = 0xA1,
    /// PC98 E slot
    Pc98E = 0xA2,
    /// PC98 local bus slot
    Pc98LocalBus = 0xA3,
    /// PC98 card slot
    Pc98Card = 0xA4,
    /// PCI Express slot
    PciExpress = 0xA5,
    /// PCI Express x1 slot
    PciExpressX1 = 0xA6,
    /// PCI Express x2 slot
    PciExpressX2 = 0xA7,
    /// PCI Express x4 slot
    PciExpressX4 = 0xA8,
    /// PCI Express x8 slot
    PciExpressX8 = 0xA9,
    /// PCI Express x16 slot
    PciExpressX16 = 0xAA,
    /// PCI Express Gen2 slot
    PciExpressGen2 = 0xAB,
    /// PCI Express Gen2 x1 slot
    PciExpressGen2X1 = 0xAC,
    /// PCI Express Gen2 x2 slot
    PciExpressGen2X2 = 0xAD,
    /// PCI Express Gen2 x4 slot
    PciExpressGen2X4 = 0xAE,
    /// PCI Express Gen2 x8 slot
    PciExpressGen2X8 = 0xAF,
    /// PCI Express Gen2 x16 slot
    PciExpressGen2X16 = 0xB0,
    /// PCI Express Gen3 slot
    PciExpressGen3 = 0xB1,
    /// PCI Express Gen3 x1 slot
    PciExpressGen3X1 = 0xB2,
    /// PCI Express Gen3 x2 slot
    PciExpressGen3X2 = 0xB3,
    /// PCI Express Gen3 x4 slot
    PciExpressGen3X4 = 0xB4,
    /// PCI Express Gen3 x8 slot
    PciExpressGen3X8 = 0xB5,
    /// PCI Express Gen3 x16 slot
    PciExpressGen3X16 = 0xB6,
    /// PCI Express Gen4 slot
    PciExpressGen4 = 0xB8,
    /// PCI Express Gen4 x1 slot
    PciExpressGen4X1 = 0xB9,
    /// PCI Express Gen4 x2 slot
    PciExpressGen4X2 = 0xBA,
    /// PCI Express Gen4 x4 slot
    PciExpressGen4X4 = 0xBB,
    /// PCI Express Gen4 x8 slot
    PciExpressGen4X8 = 0xBC,
    /// PCI Express Gen4 x16 slot
    PciExpressGen4X16 = 0xBD,
    /// PCI Express Gen5 slot
    PciExpressGen5 = 0xBE,
    /// PCI Express Gen5 x1 slot
    PciExpressGen5X1 = 0xBF,
    /// PCI Express Gen5 x2 slot
    PciExpressGen5X2 = 0xC0,
    /// PCI Express Gen5 x4 slot
    PciExpressGen5X4 = 0xC1,
    /// PCI Express Gen5 x8 slot
    PciExpressGen5X8 = 0xC2,
    /// PCI Express Gen5 x16 slot
    PciExpressGen5X16 = 0xC3,
    /// PCI Express Gen6 and beyond slot
    PciExpressGen6andBeyond = 0xC4,
    /// Enterprise and Datacenter 1U E1 Form Factor slot
    EnterpriseandDatacenter1UE1FormFactorSlot = 0xC5,
    /// Enterprise and Datacenter 3E3 Form Factor slot
    EnterpriseandDatacenter3E3FormFactorSlot = 0xC6,
}

///
/// System Slots - Slot Data Bus Width.
///
#[repr(u8)]
pub enum MiscSlotDataBusWidth {
    /// Other data bus width
    Other = 0x01,
    /// Unknown data bus width
    Unknown = 0x02,
    /// 8-bit width
    Width8Bit = 0x03,
    /// 16-bit width
    Width16Bit = 0x04,
    /// 32-bit width
    Width32Bit = 0x05,
    /// 64-bit width
    Width64Bit = 0x06,
    /// 128-bit width
    Width128Bit = 0x07,
    /// x1 width
    Width1X = 0x08,
    /// x2 width
    Width2X = 0x09,
    /// x4 width
    Width4X = 0x0A,
    /// x8 width
    Width8X = 0x0B,
    /// x12 width
    Width12X = 0x0C,
    /// x16 width
    Width16X = 0x0D,
    /// x32 width
    Width32X = 0x0E,
}

///
/// System Slots - Slot Physical Width.
///
#[repr(u8)]
pub enum MiscSlotPhysicalWidth {
    /// Other physical width
    Other = 0x01,
    /// Unknown physical width
    Unknown = 0x02,
    /// 8-bit width
    Width8Bit = 0x03,
    /// 16-bit width
    Width16Bit = 0x04,
    /// 32-bit width
    Width32Bit = 0x05,
    /// 64-bit width
    Width64Bit = 0x06,
    /// 128-bit width
    Width128Bit = 0x07,
    /// x1 width
    Width1X = 0x08,
    /// x2 width
    Width2X = 0x09,
    /// x4 width
    Width4X = 0x0A,
    /// x8 width
    Width8X = 0x0B,
    /// x12 width
    Width12X = 0x0C,
    /// x16 width
    Width16X = 0x0D,
    /// x32 width
    Width32X = 0x0E,
}

///
/// System Slots - Slot Information.
///
#[repr(u8)]
pub enum MiscSlotInformation {
    /// Other slot information
    Others = 0x00,
    /// Generation 1
    Gen1 = 0x01,
    /// Generation 2
    Gen2 = 0x02,
    /// Generation 3
    Gen3 = 0x03,
    /// Generation 4
    Gen4 = 0x04,
    /// Generation 5
    Gen5 = 0x05,
    /// Generation 6
    Gen6 = 0x06,
}

///
/// System Slots - Current Usage.
///
#[repr(u8)]
pub enum MiscSlotUsage {
    /// Other usage
    Other = 0x01,
    /// Unknown usage
    Unknown = 0x02,
    /// Available
    Available = 0x03,
    /// In use
    InUse = 0x04,
    /// Unavailable
    Unavailable = 0x05,
}

///
/// System Slots - Slot Length.
///
#[repr(u8)]
pub enum MiscSlotLength {
    /// Other length
    Other = 0x01,
    /// Unknown length
    Unknown = 0x02,
    /// Short
    Short = 0x03,
    /// Long
    Long = 0x04,
}

///
/// System Slots - Slot Characteristics 1.
///
bitfield! {
    /// System slot characteristics 1 bitfield
    pub struct MiscSlotCharacteristics1(u8);
    impl Debug;
    /// Characteristics unknown
    pub characteristics_unknown, set_characteristics_unknown: 0;
    /// Provides 5.0 volts
    pub provides_50_volts, set_provides_50_volts: 1;
    /// Provides 3.3 volts
    pub provides_33_volts, set_provides_33_volts: 2;
    /// Shared slot
    pub shared_slot, set_shared_slot: 3;
    /// PC Card 16 supported
    pub pc_card_16_supported, set_pc_card_16_supported: 4;
    /// CardBus supported
    pub card_bus_supported, set_card_bus_supported: 5;
    /// Zoom video supported
    pub zoom_video_supported, set_zoom_video_supported: 6;
    /// Modem ring resume supported
    pub modem_ring_resume_supported, set_modem_ring_resume_supported: 7;
}

///
/// System Slots - Slot Characteristics 2.
///
bitfield! {
    /// System slot characteristics 2 bitfield
    pub struct MiscSlotCharacteristics2(u8);
    impl Debug;
    /// PME signal supported
    pub pme_signal_supported, set_pme_signal_supported: 0;
    /// Hot plug devices supported
    pub hot_plug_devices_supported, set_hot_plug_devices_supported: 1;
    /// SMBus signal supported
    pub smbus_signal_supported, sset_mbus_signal_supported: 2;
    /// Bifurcation supported
    pub bifurcation_supported, set_bifurcation_supported: 3;
    /// Asynchronous surprise removal
    pub async_surprise_removal, set_async_surprise_removal: 4;
    /// Flexbus slot CXL 1.0 capable
    pub flexbus_slot_cxl_10_capable, set_flexbus_slot_cxl_10_capable: 5;
    /// Flexbus slot CXL 2.0 capable
    pub flexbus_slot_cxl_20_capable, set_flexbus_slot_cxl_20_capable: 6;
    /// Flexbus slot CXL 3.0 capable
    pub flexbus_slot_cxl_30_capable, set_flexbus_slot_cxl_30_capable: 7;
}

///
/// System Slots - Slot Height
///
#[repr(u8)]
pub enum MiscSlotHeight {
    /// No slot height
    None = 0x00,
    /// Other slot height
    Other = 0x01,
    /// Unknown slot height
    Unknown = 0x02,
    /// Full height
    FullHeight = 0x03,
    /// Low profile
    LowProfile = 0x04,
}

///
/// System Slots - Peer Segment/Bus/Device/Function/Width Groups
///
#[repr(C, packed)]
pub struct MiscSlotPeerGroup {
    /// Segment group number
    pub segment_group_num: u16,
    /// Bus number
    pub bus_num: u8,
    /// Device/function number
    pub dev_func_num: u8,
    /// Data bus width
    pub data_bus_width: u8,
}

///
/// System Slots (Type 9)
///
/// The information in this structure defines the attributes of a system slot.
/// One structure is provided for each slot in the system.
///
///
#[repr(C, packed)]
pub struct SmbiosTableType9 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Slot designation string
    pub slot_designation: SmbiosTableString,
    /// Slot type (MISC_SLOT_TYPE)
    pub slot_type: u8,
    /// Slot data bus width (MISC_SLOT_DATA_BUS_WIDTH)
    pub slot_data_bus_width: u8,
    /// Current usage (MISC_SLOT_USAGE)
    pub current_usage: u8,
    /// Slot length (MISC_SLOT_LENGTH)
    pub slot_length: u8,
    /// Slot ID
    pub slot_id: u16,
    /// Slot characteristics 1
    pub slot_characteristics1: MiscSlotCharacteristics1,
    /// Slot characteristics 2
    pub slot_characteristics2: MiscSlotCharacteristics2,
    //
    // Add for smbios 2.6
    //
    /// Segment group number
    pub segment_group_num: u16,
    /// Bus number
    pub bus_num: u8,
    /// Device/function number
    pub dev_func_num: u8,
    //
    // Add for smbios 3.2
    //
    /// Data bus width
    pub data_bus_width: u8,
    /// Peer grouping count
    pub peer_grouping_count: u8,
    // Variable-length tail array of peer groups (SMBIOS Type 9, added in v3.2).
    // The spec allows Peer Grouping Count to be zero, so represent this with a
    // zero-length sentinel array. Actual bytes present:
    //   peer_grouping_count * size_of::<MiscSlotPeerGroup>()
    // Safety: Any code iterating these must bounds-check against the structure's
    // length field before dereferencing.
    /// Peer groups (variable-length array)
    pub peer_groups: [MiscSlotPeerGroup; 0],
    //
    // Since PeerGroups has a variable number of entries, must not define new
    // fields in the structure. Remaining fields can be referenced using
    // SMBIOS_TABLE_TYPE9_EXTENDED structure
    //
}

///
/// Extended structure for System Slots (Type 9)
///
#[repr(C, packed)]
pub struct SmbiosTableType9Extended {
    //
    // Add for smbios 3.4
    //
    /// Slot information
    pub slot_information: u8,
    /// Slot physical width
    pub slot_physical_width: u8,
    /// Slot pitch
    pub slot_pitch: u16,
    //
    // Add for smbios 3.5
    //
    /// Slot height (enumeration value from MISC_SLOT_HEIGHT)
    pub slot_height: u8, // < The enumeration value from MISC_SLOT_HEIGHT.
}

///
/// On Board Devices Information - Device Types.
///
#[repr(u8)]
pub enum MiscOnboardDeviceType {
    /// Other device type
    Other = 0x01,
    /// Unknown device type
    Unknown = 0x02,
    /// Video device type
    Video = 0x03,
    /// SCSI controller device type
    ScsiController = 0x04,
    /// Ethernet device type
    Ethernet = 0x05,
    /// Token Ring device type
    TokenRing = 0x06,
    /// Sound device type
    Sound = 0x07,
    /// PATA controller device type
    PataController = 0x08,
    /// SATA controller device type
    SataController = 0x09,
    /// SAS controller device type
    SasController = 0x0A,
}

///
/// Device Item Entry
///
#[repr(C, packed)]
pub struct DeviceStruct {
    /// Device type (bit \[6:0\] - enumeration type from MISC_ONBOARD_DEVICE_TYPE)
    pub device_type: u8, // < Bit [6:0] - enumeration type of device from MISC_ONBOARD_DEVICE_TYPE.
    // < Bit 7     - 1 : device enabled, 0 : device disabled.
    /// Device description string
    pub description_string: SmbiosTableString,
}

///
/// OEM Strings (Type 11).
/// This structure contains free form strings defined by the OEM. Examples of this are:
/// Part Numbers for Reference Documents for the system, contact information for the manufacturer, etc.
///
#[repr(C, packed)]
/// OEM Strings (Type 11)
pub struct SmbiosTableType11 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Number of strings
    pub string_count: u8,
}

///
/// System Configuration Options (Type 12).
///
/// This structure contains information required to configure the base board's Jumpers and Switches.
///
#[repr(C, packed)]
/// System Configuration Options (Type 12)
pub struct SmbiosTableType12 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Number of strings
    pub string_count: u8,
}

///
/// BIOS Language Information (Type 13).
///
/// The information in this structure defines the installable language attributes of the BIOS.
///
#[repr(C, packed)]
/// BIOS Language Information (Type 13)
pub struct SmbiosTableType13 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Number of installable languages
    pub installable_languages: u8,
    /// Flags
    pub flags: u8,
    /// Reserved bytes
    pub reserved: [u8; 15],
    /// Current language string
    pub current_languages: SmbiosTableString,
}

///
/// Group Item Entry
///
#[repr(C, packed)]
/// Group Item Entry
pub struct GroupStruct {
    /// Item type
    pub item_type: u8,
    /// Item handle
    pub item_handle: u16,
}

///
/// Group Associations (Type 14).
///
/// The Group Associations structure is provided for OEMs who want to specify
/// the arrangement or hierarchy of certain components (including other Group Associations)
/// within the system.
///
#[repr(C, packed)]
/// Group Associations (Type 14)
pub struct SmbiosTableType14 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Group name string
    pub group_name: SmbiosTableString,
    /// Group array
    pub group: [GroupStruct; 1],
}

///
/// System Event Log - Event Log Types.
///
#[repr(u8)]
pub enum EventLogTypeData {
    /// Reserved event log type
    Reserved = 0x00,
    /// Single-bit ECC event log type
    SingleBitEcc = 0x01,
    /// Multi-bit ECC event log type
    MultiBitEcc = 0x02,
    /// Parity memory error event log type
    ParityMemErr = 0x03,
    /// Bus timeout event log type
    BusTimeOut = 0x04,
    /// IO channel check event log type
    IoChannelCheck = 0x05,
    /// Software NMI event log type
    SoftwareNmi = 0x06,
    /// POST memory resize event log type
    PostMemResize = 0x07,
    /// POST error event log type
    PostErr = 0x08,
    /// PCI parity error event log type
    PciParityErr = 0x09,
    /// PCI system error event log type
    PciSystemErr = 0x0A,
    /// CPU failure event log type
    CpuFailure = 0x0B,
    /// EISA timeout event log type
    EisaTimeOut = 0x0C,
    /// Memory log disabled event log type
    MemLogDisabled = 0x0D,
    /// Logging disabled event log type
    LoggingDisabled = 0x0E,
    /// System limit exceeded event log type
    SysLimitExce = 0x10,
    /// Asynchronous hardware timer event log type
    AsyncHwTimer = 0x11,
    /// System configuration info event log type
    SysConfigInfo = 0x12,
    /// Hard drive info event log type
    HdInfo = 0x13,
    /// System reconfiguration event log type
    SysReconfig = 0x14,
    /// Uncorrectable CPU error event log type
    UncorrectableCpuErr = 0x15,
    /// Area reset and clear event log type
    AreaResetAndClr = 0x16,
    /// System boot event log type
    SystemBoot = 0x17,
    /// Unused event log type (0x18 - 0x7F)
    Unused = 0x18, // < 0x18 - 0x7F
    /// Available for system event log type (0x80 - 0xFE)
    AvailForSys = 0x80, // < 0x80 - 0xFE
    /// End of log event log type
    EndOfLog = 0xFF,
}

///
/// System Event Log - Variable Data Format Types.
///
#[repr(u8)]
pub enum EventLogVariableData {
    /// No variable data
    None = 0x00,
    /// Handle variable data
    Handle = 0x01,
    /// Multi-event variable data
    MutilEvent = 0x02,
    /// Multi-event handle variable data
    MutilEventHandle = 0x03,
    /// POST result bitmap variable data
    PostResultBitmap = 0x04,
    /// System management type variable data
    SysManagementType = 0x05,
    /// Multi-event system management type variable data
    MutliEventSysManagementType = 0x06,
    /// Unused variable data
    Unused = 0x07,
    /// OEM assigned variable data
    OemAssigned = 0x80,
}

///
/// Event Log Type Descriptors
///
#[repr(C, packed)]
pub struct EventLogType {
    /// Log type (enumeration value from EVENT_LOG_TYPE_DATA)
    pub log_type: u8, // < The enumeration value from EVENT_LOG_TYPE_DATA.
    /// Data format type
    pub data_format_type: u8,
}

///
/// System Event Log (Type 15).
///
/// The presence of this structure within the SMBIOS data returned for a system indicates
/// that the system supports an event log.  An event log is a fixed-length area within a
/// non-volatile storage element, starting with a fixed-length (and vendor-specific) header
/// record, followed by one or more variable-length log records.
///
#[repr(C, packed)]
pub struct SmbiosTableType15 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Log area length
    pub log_area_length: u16,
    /// Log header start offset
    pub log_header_start_offset: u16,
    /// Log data start offset
    pub log_data_start_offset: u16,
    /// Access method
    pub access_method: u8,
    /// Log status
    pub log_status: u8,
    /// Log change token
    pub log_change_token: u32,
    /// Access method address
    pub access_method_address: u32,
    /// Log header format
    pub log_header_format: u8,
    /// Number of supported log type descriptors
    pub number_of_supported_log_type_descriptors: u8,
    /// Length of log type descriptor
    pub length_of_log_type_descriptor: u8,
    /// Event log type descriptors
    pub event_log_type_descriptors: [EventLogType; 1],
}

///
/// Physical Memory Array - Location.
///
#[repr(u8)]
pub enum MemoryArrayLocation {
    /// Other location
    Other = 0x01,
    /// Unknown location
    Unknown = 0x02,
    /// System board
    SystemBoard = 0x03,
    /// ISA add-on card
    IsaAddonCard = 0x04,
    /// EISA add-on card
    EisaAddonCard = 0x05,
    /// PCI add-on card
    PciAddonCard = 0x06,
    /// MCA add-on card
    McaAddonCard = 0x07,
    /// PCMCIA add-on card
    PcmciaAddonCard = 0x08,
    /// Proprietary add-on card
    ProprietaryAddonCard = 0x09,
    /// NuBus
    NuBus = 0x0A,
    /// PC98 C20 add-on card
    Pc98C20AddonCard = 0xA0,
    /// PC98 C24 add-on card
    Pc98C24AddonCard = 0xA1,
    /// PC98 E add-on card
    Pc98EAddonCard = 0xA2,
    /// PC98 local bus add-on card
    Pc98LocalBusAddonCard = 0xA3,
    /// CXL add-on card
    CxlAddonCard = 0xA4,
}

///
/// Physical Memory Array - Use.
///
#[repr(u8)]
pub enum MemoryArrayUse {
    /// Other use
    Other = 0x01,
    /// Unknown use
    Unknown = 0x02,
    /// System memory
    SystemMemory = 0x03,
    /// Video memory
    VideoMemory = 0x04,
    /// Flash memory
    FlashMemory = 0x05,
    /// Non-volatile RAM
    NonVolatileRam = 0x06,
    /// Cache memory
    CacheMemory = 0x07,
}

///
/// Physical Memory Array - Error Correction Types.
///
#[repr(u8)]
pub enum MemoryErrorCorrection {
    /// Other error correction
    Other = 0x01,
    /// Unknown error correction
    Unknown = 0x02,
    /// No error correction
    None = 0x03,
    /// Parity
    Parity = 0x04,
    /// Single-bit ECC
    SingleBitEcc = 0x05,
    /// Multi-bit ECC
    MultiBitEcc = 0x06,
    /// CRC
    Crc = 0x07,
}

///
/// Physical Memory Array (Type 16).
///
/// This structure describes a collection of memory devices that operate
/// together to form a memory address space.
///
#[repr(C, packed)]
pub struct SmbiosTableType16 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Location (MEMORY_ARRAY_LOCATION)
    pub location: u8,
    /// Use (MEMORY_ARRAY_USE)
    pub use_: u8,
    /// Error correction (MEMORY_ERROR_CORRECTION)
    pub memory_error_correction: u8,
    /// Maximum capacity
    pub maximum_capacity: u32,
    /// Memory error information handle
    pub memory_error_information_handle: u16,
    /// Number of memory devices
    pub number_of_memory_devices: u16,
    //
    // Add for smbios 2.7
    //
    /// Extended maximum capacity
    pub extended_maximum_capacity: u64,
}

///
/// Memory Device - Form Factor.
///
#[repr(u8)]
pub enum MemoryFormFactor {
    /// Other form factor
    Other = 0x01,
    /// Unknown form factor
    Unknown = 0x02,
    /// SIMM
    Simm = 0x03,
    /// SIP
    Sip = 0x04,
    /// Chip
    Chip = 0x05,
    /// DIP
    Dip = 0x06,
    /// ZIP
    Zip = 0x07,
    /// Proprietary card
    ProprietaryCard = 0x08,
    /// DIMM
    Dimm = 0x09,
    /// TSOP
    Tsop = 0x0A,
    /// Row of chips
    RowOfChips = 0x0B,
    /// RIMM
    Rimm = 0x0C,
    /// SODIMM
    Sodimm = 0x0D,
    /// SRIMM
    Srimm = 0x0E,
    /// FBDIMM
    FbDimm = 0x0F,
    /// Die
    Die = 0x10,
}

///
/// Memory Device - Type
///
#[repr(u8)]
pub enum MemoryDeviceType {
    /// Other device type
    Other = 0x01,
    /// Unknown device type
    Unknown = 0x02,
    /// DRAM
    Dram = 0x03,
    /// EDRAM
    Edram = 0x04,
    /// VRAM
    Vram = 0x05,
    /// SRAM
    Sram = 0x06,
    /// RAM
    Ram = 0x07,
    /// ROM
    Rom = 0x08,
    /// Flash
    Flash = 0x09,
    /// EEPROM
    Eeprom = 0x0A,
    /// FEPROM
    Feprom = 0x0B,
    /// EPROM
    Eprom = 0x0C,
    /// CDRAM
    Cdram = 0x0D,
    /// 3DRAM
    ThreeDram = 0x0E,
    /// SDRAM
    Sdram = 0x0F,
    /// SGRAM
    Sgram = 0x10,
    /// RDRAM
    Rdram = 0x11,
    /// DDR
    Ddr = 0x12,
    /// DDR2
    Ddr2 = 0x13,
    /// DDR2 FBDIMM
    Ddr2FbDimm = 0x14,
    /// DDR3
    Ddr3 = 0x18,
    /// FBD2
    Fbd2 = 0x19,
    /// DDR4
    Ddr4 = 0x1A,
    /// LPDDR
    Lpddr = 0x1B,
    /// LPDDR2
    Lpddr2 = 0x1C,
    /// LPDDR3
    Lpddr3 = 0x1D,
    /// LPDDR4
    Lpddr4 = 0x1E,
    /// Logical non-volatile device
    LogicalNonVolatileDevice = 0x1F,
    /// HBM
    Hbm = 0x20,
    /// HBM2
    Hbm2 = 0x21,
    /// DDR5
    Ddr5 = 0x22,
    /// LPDDR5
    Lpddr5 = 0x23,
    /// HBM3
    Hbm3 = 0x24,
}

///
/// Memory Device - Type Detail
///
bitfield! {
    /// Memory device type details bitfield
    pub struct MemoryDeviceTypeDetails(u16);
    impl Debug;
    /// Reserved bit
    pub reserved, set_reserved: 0;
    /// Other
    pub other, set_other: 1;
    /// Unknown
    pub unknown, set_unknown: 2;
    /// Fast-paged
    pub fast_paged, set_fast_paged: 3;
    /// Static column
    pub static_column, set_static_column: 4;
    /// Pseudo-static
    pub pseudo_static, set_pseudo_static: 5;
    /// Rambus
    pub rambus, set_rambus: 6;
    /// Synchronous
    pub synchronous, set_synchronous: 7;
    /// CMOS
    pub cmos, set_cmos: 8;
    /// EDO
    pub edo, set_edo: 9;
    /// Window DRAM
    pub window_dram, set_window_dram: 10;
    /// Cache DRAM
    pub cache_dram, set_cache_dram: 11;
    /// Nonvolatile
    pub nonvolatile, set_nonvolatile: 12;
    /// Registered
    pub registered, set_registered: 13;
    /// Unbuffered
    pub unbuffered, set_unbuffered: 14;
    /// LR-DIMM
    pub lr_dimm, set_lr_dimm: 15;
}

///
/// Memory Device - Memory Technology
///
#[repr(u8)]
pub enum MemoryDeviceTechnology {
    /// Other technology
    Other = 0x01,
    /// Unknown technology
    Unknown = 0x02,
    /// DRAM technology
    Dram = 0x03,
    /// NVDIMM-N technology
    NvdimmN = 0x04,
    /// NVDIMM-F technology
    NvdimmF = 0x05,
    /// NVDIMM-P technology
    NvdimmP = 0x06,
    /// Intel Optane Persistent Memory
    IntelOptanePersistentMemory = 0x07,
}

///
/// Memory Device - Memory Operating Mode Capability
///
bitfield! {
    /// Memory device operating mode capability bits
    #[derive(Copy, Clone)]
    pub struct MemoryDeviceOperatingModeCapabilityBits(u16);
    impl Debug;
    /// Reserved bit
    pub reserved, set_reserved: 0;
    /// Other capability
    pub other, set_other: 1;
    /// Unknown capability
    pub unknown, set_unknown: 2;
    /// Volatile memory capability
    pub volatile_memory, set_volatile_memory: 3;
    /// Byte-accessible persistent memory capability
    pub byte_accessible_persistent_memory, set_byte_accessible_persistent_memory: 4;
    /// Block-accessible persistent memory capability
    pub block_accessible_persistent_memory, set_block_accessible_persistent_memory: 5;
    /// Reserved bits
    pub reserved2, set_reserved2: 15, 6;
}

#[derive(Copy, Clone)]
/// Union for memory device operating mode capability
pub union MemoryDeviceOperatingModeCapability {
    /// Capability bits
    pub bits: MemoryDeviceOperatingModeCapabilityBits,
    /// Raw u16 value
    pub uint16: u16,
}

///
/// Memory Device (Type 17).
///
/// This structure describes a single memory device that is part of
/// a larger Physical Memory Array (Type 16).
/// Note:  If a system includes memory-device sockets, the SMBIOS implementation
/// includes a Memory Device structure instance for each slot, whether or not the
/// socket is currently populated.
///
#[repr(C, packed)]
pub struct SmbiosTableType17 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Memory array handle
    pub memory_array_handle: u16,
    /// Memory error information handle
    pub memory_error_information_handle: u16,
    /// Total width
    pub total_width: u16,
    /// Data width
    pub data_width: u16,
    /// Size
    pub size: u16,
    /// Form factor (MEMORY_FORM_FACTOR)
    pub form_factor: u8,
    /// Device set
    pub device_set: u8,
    /// Device locator string
    pub device_locator: SmbiosTableString,
    /// Bank locator string
    pub bank_locator: SmbiosTableString,
    /// Memory type (MEMORY_DEVICE_TYPE)
    pub memory_type: u8,
    /// Type detail
    pub type_detail: MemoryDeviceTypeDetails,
    /// Speed
    pub speed: u16,
    /// Manufacturer string
    pub manufacturer: SmbiosTableString,
    /// Serial number string
    pub serial_number: SmbiosTableString,
    /// Asset tag string
    pub asset_tag: SmbiosTableString,
    /// Part number string
    pub part_number: SmbiosTableString,
    /// Attributes
    pub attributes: u8,
    /// Extended size
    pub extended_size: u32,
    /// Configured memory clock speed
    pub configured_memory_clock_speed: u16,
    /// Minimum voltage
    pub minimum_voltage: u16,
    /// Maximum voltage
    pub maximum_voltage: u16,
    /// Configured voltage
    pub configured_voltage: u16,
    /// Memory technology (MEMORY_DEVICE_TECHNOLOGY)
    pub memory_technology: u8,
    /// Memory operating mode capability
    pub memory_operating_mode_capability: MemoryDeviceOperatingModeCapability,
    /// Firmware version string
    pub firmware_version: SmbiosTableString,
    /// Module manufacturer ID
    pub module_manufacturer_id: u16,
    /// Module product ID
    pub module_product_id: u16,
    /// Memory subsystem controller manufacturer ID
    pub memory_subsystem_controller_manufacturer_id: u16,
    /// Memory subsystem controller product ID
    pub memory_subsystem_controller_product_id: u16,
    /// Non-volatile size
    pub non_volatile_size: u64,
    /// Volatile size
    pub volatile_size: u64,
    /// Cache size
    pub cache_size: u64,
    /// Logical size
    pub logical_size: u64,
    /// Extended speed
    pub extended_speed: u32,
    /// Extended configured memory speed
    pub extended_configured_memory_speed: u32,
    /// PMIC0 manufacturer ID
    pub pmic0_manufacturer_id: u16,
    /// PMIC0 revision number
    pub pmic0_revision_number: u16,
    /// RCD manufacturer ID
    pub rcd_manufacturer_id: u16,
    /// RCD revision number
    pub rcd_revision_number: u16,
}

///
/// 32-bit Memory Error Information - Error Type.
///
#[repr(u8)]
pub enum MemoryErrorType {
    /// Other error type
    Other = 0x01,
    /// Unknown error type
    Unknown = 0x02,
    /// OK
    Ok = 0x03,
    /// Bad read
    BadRead = 0x04,
    /// Parity error
    Parity = 0x05,
    /// Single bit error
    SigleBit = 0x06,
    /// Double bit error
    DoubleBit = 0x07,
    /// Multi bit error
    MultiBit = 0x08,
    /// Nibble error
    Nibble = 0x09,
    /// Checksum error
    Checksum = 0x0A,
    /// CRC error
    Crc = 0x0B,
    /// Correct single bit error
    CorrectSingleBit = 0x0C,
    /// Corrected error
    Corrected = 0x0D,
    /// Uncorrectable error
    UnCorrectable = 0x0E,
}

///
/// 32-bit Memory Error Information - Error Granularity.
///
#[repr(u8)]
pub enum MemoryErrorGranularity {
    /// Other granularity
    Other = 0x01,
    /// Other unknown granularity
    OtherUnknown = 0x02,
    /// Device level granularity
    DeviceLevel = 0x03,
    /// Memory partition level granularity
    MemPartitionLevel = 0x04,
}

///
/// 32-bit Memory Error Information - Error Operation.
///
#[repr(u8)]
pub enum MemoryErrorOperation {
    /// Other operation
    Other = 0x01,
    /// Unknown operation
    Unknown = 0x02,
    /// Read operation
    Read = 0x03,
    /// Write operation
    Write = 0x04,
    /// Partial write operation
    PartialWrite = 0x05,
}

///
/// 32-bit Memory Error Information (Type 18).
///
/// This structure identifies the specifics of an error that might be detected
/// within a Physical Memory Array.
///
#[repr(C, packed)]
pub struct SmbiosTableType18 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Error type (MEMORY_ERROR_TYPE)
    pub error_type: u8,
    /// Error granularity (MEMORY_ERROR_GRANULARITY)
    pub error_granularity: u8,
    /// Error operation (MEMORY_ERROR_OPERATION)
    pub error_operation: u8,
    /// Vendor syndrome
    pub vendor_syndrome: u32,
    /// Memory array error address
    pub memory_array_error_address: u32,
    /// Device error address
    pub device_error_address: u32,
    /// Error resolution
    pub error_resolution: u32,
}

///
/// Memory Array Mapped Address (Type 19).
///
/// This structure provides the address mapping for a Physical Memory Array.
/// One structure is present for each contiguous address range described.
///
#[repr(C, packed)]
pub struct SmbiosTableType19 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Starting address
    pub starting_address: u32,
    /// Ending address
    pub ending_address: u32,
    /// Memory array handle
    pub memory_array_handle: u16,
    /// Partition width
    pub partition_width: u8,
    //
    // Add for smbios 2.7
    //
    /// Extended starting address
    pub extended_starting_address: u64,
    /// Extended ending address
    pub extended_ending_address: u64,
}

///
/// Memory Device Mapped Address (Type 20).
///
/// This structure maps memory address space usually to a device-level granularity.
/// One structure is present for each contiguous address range described.
///
#[repr(C, packed)]
pub struct SmbiosTableType20 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Starting address
    pub starting_address: u32,
    /// Ending address
    pub ending_address: u32,
    /// Memory device handle
    pub memory_device_handle: u16,
    /// Memory array mapped address handle
    pub memory_array_mapped_address_handle: u16,
    /// Partition row position
    pub partition_row_position: u8,
    /// Interleave position
    pub interleave_position: u8,
    /// Interleaved data depth
    pub interleaved_data_depth: u8,
    //
    // Add for smbios 2.7
    //
    /// Extended starting address
    pub extended_starting_address: u64,
    /// Extended ending address
    pub extended_ending_address: u64,
}

///
/// Built-in Pointing Device - Type
///
#[repr(u8)]
pub enum BuiltinPointingDeviceType {
    /// Other device type
    Other = 0x01,
    /// Unknown device type
    Unknown = 0x02,
    /// Mouse
    Mouse = 0x03,
    /// TrackBall
    TrackBall = 0x04,
    /// TrackPoint
    TrackPoint = 0x05,
    /// GlidePoint
    GlidePoint = 0x06,
    /// TouchPad
    TouchPad = 0x07,
    /// TouchScreen
    TouchScreen = 0x08,
    /// Optical sensor
    OpticalSensor = 0x09,
}

///
/// Built-in Pointing Device - Interface.
///
#[repr(u8)]
pub enum BuiltinPointingDeviceInterface {
    /// Other interface
    Other = 0x01,
    /// Unknown interface
    Unknown = 0x02,
    /// Serial
    Serial = 0x03,
    /// PS/2
    Ps2 = 0x04,
    /// Infrared
    Infrared = 0x05,
    /// HP-HIL
    HpHil = 0x06,
    /// Bus mouse
    BusMouse = 0x07,
    /// ADB
    Adb = 0x08,
    /// Bus mouse DB9
    BusMouseDb9 = 0xA0,
    /// Bus mouse micro DIN
    BusMouseMicroDin = 0xA1,
    /// USB
    Usb = 0xA2,
    /// I2C
    I2c = 0xA3,
    /// SPI
    Spi = 0xA4,
}

///
/// Built-in Pointing Device (Type 21).
///
/// This structure describes the attributes of the built-in pointing device for the
/// system. The presence of this structure does not imply that the built-in
/// pointing device is active for the system's use!
///
#[repr(C, packed)]
pub struct SmbiosTableType21 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Device type (BUILTIN_POINTING_DEVICE_TYPE)
    pub type_: u8,
    /// Interface (BUILTIN_POINTING_DEVICE_INTERFACE)
    pub interface: u8,
    /// Number of buttons
    pub number_of_buttons: u8,
}

///
/// Portable Battery - Device Chemistry
///
#[repr(u8)]
pub enum PortableBatteryDeviceChemistry {
    /// Other chemistry
    Other = 0x01,
    /// Unknown chemistry
    Unknown = 0x02,
    /// Lead acid
    LeadAcid = 0x03,
    /// Nickel cadmium
    NickelCadmium = 0x04,
    /// Nickel metal hydride
    NickelMetalHydride = 0x05,
    /// Lithium ion
    LithiumIon = 0x06,
    /// Zinc air
    ZincAir = 0x07,
    /// Lithium polymer
    LithiumPolymer = 0x08,
}

///
/// Portable Battery (Type 22).
///
/// This structure describes the attributes of the portable battery(s) for the system.
/// The structure contains the static attributes for the group.  Each structure describes
/// a single battery pack's attributes.
///
#[repr(C, packed)]
pub struct SmbiosTableType22 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Location string
    pub location: SmbiosTableString,
    /// Manufacturer string
    pub manufacturer: SmbiosTableString,
    /// Manufacture date string
    pub manufacture_date: SmbiosTableString,
    /// Serial number string
    pub serial_number: SmbiosTableString,
    /// Device name string
    pub device_name: SmbiosTableString,
    /// Device chemistry (PORTABLE_BATTERY_DEVICE_CHEMISTRY)
    pub device_chemistry: u8,
    /// Device capacity
    pub device_capacity: u16,
    /// Design voltage
    pub design_voltage: u16,
    /// SBDS version number string
    pub sbds_version_number: SmbiosTableString,
    /// Maximum error in battery data
    pub maximum_error_in_battery_data: u8,
    /// SBDS serial number
    pub sbds_serial_number: u16,
    /// SBDS manufacture date
    pub sbds_manufacture_date: u16,
    /// SBDS device chemistry string
    pub sbds_device_chemistry: SmbiosTableString,
    /// Design capacity multiplier
    pub design_capacity_multiplier: u8,
    /// OEM specific
    pub oem_specific: u32,
}

///
/// System Reset (Type 23)
///
/// This structure describes whether Automatic System Reset functions enabled (Status).
/// If the system has a watchdog Timer and the timer is not reset (Timer Reset)
/// before the Interval elapses, an automatic system reset will occur. The system will re-boot
/// according to the Boot Option. This function may repeat until the Limit is reached, at which time
/// the system will re-boot according to the Boot Option at Limit.
///
#[repr(C, packed)]
pub struct SmbiosTableType23 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Capabilities
    pub capabilities: u8,
    /// Reset count
    pub reset_count: u16,
    /// Reset limit
    pub reset_limit: u16,
    /// Timer interval
    pub timer_interval: u16,
    /// Timeout
    pub timeout: u16,
}

///
/// Hardware Security (Type 24).
///
/// This structure describes the system-wide hardware security settings.
///
#[repr(C, packed)]
pub struct SmbiosTableType24 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Hardware security settings
    pub hardware_security_settings: u8,
}

///
/// System Power Controls (Type 25).
///
/// This structure describes the attributes for controlling the main power supply to the system.
/// Software that interprets this structure uses the month, day, hour, minute, and second values
/// to determine the number of seconds until the next power-on of the system.  The presence of
/// this structure implies that a timed power-on facility is available for the system.
///
#[repr(C, packed)]
pub struct SmbiosTableType25 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Next scheduled power-on month
    pub next_scheduled_power_on_month: u8,
    /// Next scheduled power-on day of month
    pub next_scheduled_power_on_day_of_month: u8,
    /// Next scheduled power-on hour
    pub next_scheduled_power_on_hour: u8,
    /// Next scheduled power-on minute
    pub next_scheduled_power_on_minute: u8,
    /// Next scheduled power-on second
    pub next_scheduled_power_on_second: u8,
}

///
/// Voltage Probe - Location and Status.
///
#[repr(C, packed)]
pub struct MiscVoltageProbeLocation {
    /// Voltage probe site (5 bits)
    pub voltage_probe_site: u8,
    /// Voltage probe status (3 bits)
    pub voltage_probe_status: u8,
}

///
/// Voltage Probe (Type 26)
///
/// This describes the attributes for a voltage probe in the system.
/// Each structure describes a single voltage probe.
///
#[repr(C, packed)]
pub struct SmbiosTableType26 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Description string
    pub description: SmbiosTableString,
    /// Location and status
    pub location_and_status: MiscVoltageProbeLocation,
    /// Maximum value
    pub maximum_value: u16,
    /// Minimum value
    pub minimum_value: u16,
    /// Resolution
    pub resolution: u16,
    /// Tolerance
    pub tolerance: u16,
    /// Accuracy
    pub accuracy: u16,
    /// OEM defined value
    pub oem_defined: u32,
    /// Nominal value
    pub nominal_value: u16,
}

///
/// Cooling Device - Device Type and Status.
///
#[repr(C, packed)]
pub struct MiscCoolingDeviceType {
    /// Cooling device (5 bits)
    pub cooling_device: u8,
    /// Cooling device status (3 bits)
    pub cooling_device_status: u8,
}

///
/// Cooling Device (Type 27)
///
/// This structure describes the attributes for a cooling device in the system.
/// Each structure describes a single cooling device.
///
#[repr(C, packed)]
pub struct SmbiosTableType27 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Temperature probe handle
    pub temperature_probe_handle: u16,
    /// Device type and status
    pub device_type_and_status: MiscCoolingDeviceType,
    /// Cooling unit group
    pub cooling_unit_group: u8,
    /// OEM defined value
    pub oem_defined: u32,
    /// Nominal speed
    pub nominal_speed: u16,
    /// Description string
    pub description: SmbiosTableString,
}

///
/// Temperature Probe - Location and Status.
///
bitfield! {
    /// Temperature probe location bitfield
    pub struct MiscTemperatureProbeLocation(u8);
    impl Debug;
    /// Temperature probe site
    pub temperature_probe_site, set_temperature_probe_site: 4, 0;
    /// Temperature probe status
    pub temperature_prove_status, set_temperature_prove_status: 7, 5;
}

///
/// Temperature Probe (Type 28).
///
/// This structure describes the attributes for a temperature probe in the system.
/// Each structure describes a single temperature probe.
///
#[repr(C, packed)]
pub struct SmbiosTableType28 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Description string
    pub description: SmbiosTableString,
    /// Location and status
    pub location_and_status: MiscTemperatureProbeLocation,
    /// Maximum value
    pub maximum_value: u16,
    /// Minimum value
    pub minimum_value: u16,
    /// Resolution
    pub resolution: u16,
    /// Tolerance
    pub tolerance: u16,
    /// Accuracy
    pub accuracy: u16,
    /// OEM defined value
    pub oem_defined: u32,
    /// Nominal value
    pub nominal_value: u16,
}

///
/// Electrical Current Probe - Location and Status.
///
bitfield! {
    /// Electrical current probe location bitfield
    pub struct MiscElectricalCurrentProbeLocation(u8);
    impl Debug;
    /// Electrical current probe site
    pub electrical_current_probe_site, set_electrical_current_probe_site: 4, 0;
    /// Electrical current probe status
    pub electrical_current_probe_status, set_electrical_current_probe_status: 7, 5;
}

//
/// Electrical Current Probe (Type 29).
///
/// This structure describes the attributes for an electrical current probe in the system.
/// Each structure describes a single electrical current probe.
///
#[repr(C, packed)]
pub struct SmbiosTableType29 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Description string
    pub description: SmbiosTableString,
    /// Location and status
    pub location_and_status: MiscElectricalCurrentProbeLocation,
    /// Maximum value
    pub maximum_value: u16,
    /// Minimum value
    pub minimum_value: u16,
    /// Resolution
    pub resolution: u16,
    /// Tolerance
    pub tolerance: u16,
    /// Accuracy
    pub accuracy: u16,
    /// OEM defined value
    pub oem_defined: u32,
    /// Nominal value
    pub nominal_value: u16,
}

///
/// Out-of-Band Remote Access (Type 30).
///
/// This structure describes the attributes and policy settings of a hardware facility
/// that may be used to gain remote access to a hardware system when the operating system
/// is not available due to power-down status, hardware failures, or boot failures.
///
#[repr(C, packed)]
pub struct SmbiosTableType30 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Manufacturer name string
    pub manufacturer_name: SmbiosTableString,
    /// Number of connections
    pub connections: u8,
}

///
/// Boot Integrity Services (BIS) Entry Point (Type 31).
///
/// Structure type 31 (decimal) is reserved for use by the Boot Integrity Services (BIS).
///
#[repr(C, packed)]
pub struct SmbiosTableType31 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Checksum
    pub checksum: u8,
    /// Reserved field 1
    pub reserved1: u8,
    /// Reserved field 2
    pub reserved2: u16,
    /// BIS entry 16
    pub bis_entry16: u32,
    /// BIS entry 32
    pub bis_entry32: u32,
    /// Reserved field 3
    pub reserved3: u64,
    /// Reserved field 4
    pub reserved4: u32,
}

///
/// System Boot Information - System Boot Status.
///
#[repr(u8)]
pub enum MiscBootInformationStatusDataType {
    /// No error
    NoError = 0x00,
    /// No bootable media
    NoBootableMedia = 0x01,
    /// Normal OS failed loading
    NormalOsFailedLoading = 0x02,
    /// Firmware detected failure
    FirmwareDetectedFailure = 0x03,
    /// OS detected failure
    OsDetectedFailure = 0x04,
    /// User requested boot
    UserRequestedBoot = 0x05,
    /// System security violation
    SystemSecurityViolation = 0x06,
    /// Previous requested image
    PreviousRequestedImage = 0x07,
    /// Watchdog timer expired
    WatchdogTimerExpired = 0x08,
    /// Reserved
    StartReserved = 0x09,
    /// OEM specific
    StartOemSpecific = 0x80,
    /// Product specific
    StartProductSpecific = 0xC0,
}

///
/// System Boot Information (Type 32).
///
/// The client system firmware, e.g. BIOS, communicates the System Boot Status to the
/// client's Pre-boot Execution Environment (PXE) boot image or OS-present management
/// application via this structure. When used in the PXE environment, for example,
/// this code identifies the reason the PXE was initiated and can be used by boot-image
/// software to further automate an enterprise's PXE sessions.  For example, an enterprise
/// could choose to automatically download a hardware-diagnostic image to a client whose
/// reason code indicated either a firmware- or operating system-detected hardware failure.
///
#[repr(C, packed)]
pub struct SmbiosTableType32 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Reserved bytes
    pub reserved: [u8; 6],
    /// Boot status (MISC_BOOT_INFORMATION_STATUS_DATA_TYPE)
    pub boot_status: u8,
}

///
/// 64-bit Memory Error Information (Type 33).
///
/// This structure describes an error within a Physical Memory Array,
/// when the error address is above 4G (0xFFFFFFFF).
///
#[repr(C, packed)]
pub struct SmbiosTableType33 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Error type (MEMORY_ERROR_TYPE)
    pub error_type: u8,
    /// Error granularity (MEMORY_ERROR_GRANULARITY)
    pub error_granularity: u8,
    /// Error operation (MEMORY_ERROR_OPERATION)
    pub error_operation: u8,
    /// Vendor syndrome
    pub vendor_syndrome: u32,
    /// Memory array error address
    pub memory_array_error_address: u64,
    /// Device error address
    pub device_error_address: u64,
    /// Error resolution
    pub error_resolution: u32,
}

///
/// Management Device -  Type.
///
#[repr(u8)]
pub enum MiscManagementDeviceType {
    /// Other device
    Other = 0x01,
    /// Unknown device
    Unknown = 0x02,
    /// LM75 device
    Lm75 = 0x03,
    /// LM78 device
    Lm78 = 0x04,
    /// LM79 device
    Lm79 = 0x05,
    /// LM80 device
    Lm80 = 0x06,
    /// LM81 device
    Lm81 = 0x07,
    /// ADM9240 device
    Adm9240 = 0x08,
    /// DS1780 device
    Ds1780 = 0x09,
    /// Maxim1617 device
    Maxim1617 = 0x0A,
    /// GL518SM device
    Gl518Sm = 0x0B,
    /// W83781D device
    W83781D = 0x0C,
    /// HT82H791 device
    Ht82H791 = 0x0D,
}

///
/// Management Device -  Address Type.
///
#[repr(u8)]
pub enum MiscManagementDeviceAddressType {
    /// Other address type
    Other = 0x01,
    /// Unknown address type
    Unknown = 0x02,
    /// I/O port address type
    IoPort = 0x03,
    /// Memory address type
    Memory = 0x04,
    /// SMBus address type
    Smbus = 0x05,
}

///
/// Management Device (Type 34).
///
/// The information in this structure defines the attributes of a Management Device.
/// A Management Device might control one or more fans or voltage, current, or temperature
/// probes as defined by one or more Management Device Component structures.
///
#[repr(C, packed)]
pub struct SmbiosTableType34 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Description string
    pub description: SmbiosTableString,
    /// Device type (MISC_MANAGEMENT_DEVICE_TYPE)
    pub type_: u8,
    /// Address
    pub address: u32,
    /// Address type (MISC_MANAGEMENT_DEVICE_ADDRESS_TYPE)
    pub address_type: u8,
}

///
/// Management Device Component (Type 35)
///
/// This structure associates a cooling device or environmental probe with structures
/// that define the controlling hardware device and (optionally) the component's thresholds.
///
#[repr(C, packed)]
pub struct SmbiosTableType35 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Description string
    pub description: SmbiosTableString,
    /// Management device handle
    pub management_device_handle: u16,
    /// Component handle
    pub component_handle: u16,
    /// Threshold handle
    pub threshold_handle: u16,
}

///
/// Management Device Threshold Data (Type 36).
///
/// The information in this structure defines threshold information for
/// a component (probe or cooling-unit) contained within a Management Device.
///
#[repr(C, packed)]
pub struct SmbiosTableType36 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Lower threshold non-critical
    pub lower_threshold_non_critical: u16,
    /// Upper threshold non-critical
    pub upper_threshold_non_critical: u16,
    /// Lower threshold critical
    pub lower_threshold_critical: u16,
    /// Upper threshold critical
    pub upper_threshold_critical: u16,
    /// Lower threshold non-recoverable
    pub lower_threshold_non_recoverable: u16,
    /// Upper threshold non-recoverable
    pub upper_threshold_non_recoverable: u16,
}

///
/// Memory Channel Entry.
///
#[repr(C, packed)]
pub struct MemoryDevice {
    /// Device load
    pub device_load: u8,
    /// Device handle
    pub device_handle: u16,
}

///
/// Memory Channel - Channel Type.
///
#[repr(u8)]
pub enum MemoryChannelType {
    /// Other channel type
    Other = 0x01,
    /// Unknown channel type
    Unknown = 0x02,
    /// Rambus channel type
    Rambus = 0x03,
    /// SyncLink channel type
    SyncLink = 0x04,
}

///
/// Memory Channel (Type 37)
///
/// The information in this structure provides the correlation between a Memory Channel
/// and its associated Memory Devices.  Each device presents one or more loads to the channel.
/// The sum of all device loads cannot exceed the channel's defined maximum.
///
#[repr(C, packed)]
pub struct SmbiosTableType37 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Channel type
    pub channel_type: u8,
    /// Maximum channel load
    pub maximum_channel_load: u8,
    /// Memory device count
    pub memory_device_count: u8,
    /// Memory device array
    pub memory_device: [MemoryDevice; 1],
}

///
/// IPMI Device Information - BMC Interface Type
///
#[repr(u8)]
pub enum BmcInterfaceType {
    /// Unknown interface type
    Unknown = 0x00,
    /// Keyboard Controller Style
    Kcs = 0x01,
    /// Server Management Interface Chip
    Smic = 0x02,
    /// Block Transfer
    Bt = 0x03,
    /// SMBus System Interface
    Ssif = 0x04,
}

///
/// IPMI Device Information (Type 38).
///
/// The information in this structure defines the attributes of an
/// Intelligent Platform Management Interface (IPMI) Baseboard Management Controller (BMC).
///
/// The Type 42 structure can also be used to describe a physical management controller
/// host interface and one or more protocols that share that interface. If IPMI is not
/// shared with other protocols, either the Type 38 or Type 42 structures can be used.
/// Providing Type 38 is recommended for backward compatibility.
///
#[repr(C, packed)]
pub struct SmbiosTableType38 {
    hdr: SmbiosStructure,
    interface_type: u8, // < The enumeration value from BMC_INTERFACE_TYPE.
    ipmi_specification_revision: u8,
    i2c_slave_address: u8,
    nv_storage_device_address: u8,
    base_address: u64,
    base_address_modifier_interrupt_info: u8,
    interrupt_number: u8,
}

///
/// System Power Supply - Power Supply Characteristics.
///
bitfield! {
    /// System power supply characteristics bitfield
    pub struct SysPowerSupplyCharacteristics(u16);
    impl Debug;
    /// Power supply hot replaceable
    pub power_supply_hot_replaceable, set_power_supply_hot_replaceable: 0;
    /// Power supply present
    pub power_supply_present, set_power_supply_present: 1;
    /// Power supply unplugged
    pub power_supply_unplugged, set_power_supply_unplugged: 2;
    /// Input voltage range switch
    pub input_voltage_range_switch, set_input_voltage_range_switch: 6, 3;
    /// Power supply status
    pub power_supply_status, set_power_supply_status: 9, 7;
    /// Power supply type
    pub power_supply_type, set_power_supply_type: 13, 10;
    /// Reserved bits
    pub reserved, set_reserved: 15, 14;
}

///
/// System Power Supply (Type 39).
///
/// This structure identifies attributes of a system power supply. One instance
/// of this record is present for each possible power supply in a system.
///
#[repr(C, packed)]
pub struct SmbiosTableType39 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Power unit group
    pub power_unit_group: u8,
    /// Location string
    pub location: SmbiosTableString,
    /// Device name string
    pub device_name: SmbiosTableString,
    /// Manufacturer string
    pub manufacturer: SmbiosTableString,
    /// Serial number string
    pub serial_number: SmbiosTableString,
    /// Asset tag number string
    pub asset_tag_number: SmbiosTableString,
    /// Model part number string
    pub model_part_number: SmbiosTableString,
    /// Revision level string
    pub revision_level: SmbiosTableString,
    /// Maximum power capacity
    pub max_power_capacity: u16,
    /// Power supply characteristics
    pub power_supply_characteristics: SysPowerSupplyCharacteristics,
    /// Input voltage probe handle
    pub input_voltage_probe_handle: u16,
    /// Cooling device handle
    pub cooling_device_handle: u16,
    /// Input current probe handle
    pub input_current_probe_handle: u16,
}

///
/// Additional Information Entry Format.
///
#[repr(C, packed)]
pub struct AdditionalInformationEntry {
    /// Entry length
    pub entry_length: u8,
    /// Referenced handle
    pub referenced_handle: u16,
    /// Referenced offset
    pub referenced_offset: u8,
    /// Entry string
    pub entry_string: SmbiosTableString,
    /// Value
    pub value: [u8; 1],
}

///
/// Additional Information (Type 40).
///
/// This structure is intended to provide additional information for handling unspecified
/// enumerated values and interim field updates in another structure.
///
#[repr(C, packed)]
pub struct SmbiosTableType40 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Number of additional information entries
    pub number_of_additional_information_entries: u8,
    /// Additional information entries array
    pub additional_info_entries: [AdditionalInformationEntry; 1],
}

///
/// Onboard Devices Extended Information - Onboard Device Types.
///
#[repr(u8)]
pub enum OnboardDeviceExtendedInfoType {
    /// Other device type
    Other = 0x01,
    /// Unknown device type
    Unknown = 0x02,
    /// Video device type
    Video = 0x03,
    /// SCSI controller device type
    ScsiController = 0x04,
    /// Ethernet device type
    Ethernet = 0x05,
    /// Token Ring device type
    TokenRing = 0x06,
    /// Sound device type
    Sound = 0x07,
    /// PATA controller device type
    PataController = 0x08,
    /// SATA controller device type
    SataController = 0x09,
    /// SAS controller device type
    SasController = 0x0A,
    /// Wireless LAN device type
    WirelessLan = 0x0B,
    /// Bluetooth device
    Bluetooth = 0x0C,
    /// WWAN device
    Wwan = 0x0D,
    /// eMMC device
    EMmc = 0x0E,
    /// NVMe device
    Nvme = 0x0F,
    /// UFC device
    Ufc = 0x10,
}

///
/// Onboard Devices Extended Information (Type 41).
///
/// The information in this structure defines the attributes of devices that
/// are onboard (soldered onto) a system element, usually the baseboard.
/// In general, an entry in this table implies that the BIOS has some level of
/// control over the enabling of the associated device for use by the system.
///
#[repr(C, packed)]
pub struct SmbiosTableType41 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Reference designation string
    pub reference_designation: SmbiosTableString,
    /// Device type (ONBOARD_DEVICE_EXTENDED_INFO_TYPE)
    pub device_type: u8,
    /// Device type instance
    pub device_type_instance: u8,
    /// Segment group number
    pub segment_group_num: u16,
    /// Bus number
    pub bus_num: u8,
    /// Device/function number
    pub dev_func_num: u8,
}

///
///  Management Controller Host Interface - Protocol Record Data Format.
///
#[repr(C, packed)]
pub struct McHostInterfaceProtocolRecord {
    /// Protocol type
    pub protocol_type: u8,
    /// Protocol type data length
    pub protocol_type_data_len: u8,
    /// Protocol type data
    pub protocol_type_data: [u8; 1],
}

///
/// Management Controller Host Interface - Interface Types.
/// 00h - 3Fh: MCTP Host Interfaces
///
#[repr(u8)]
pub enum McHostInterfaceType {
    /// Network Host Interface
    NetworkHostInterface = 0x40,
    /// OEM Defined
    OemDefined = 0xF0,
}

///
/// Management Controller Host Interface - Protocol Types.
///
#[repr(u8)]
pub enum McHostInterfaceProtocolType {
    /// IPMI protocol
    Ipmi = 0x02,
    /// MCTP protocol
    Mctp = 0x03,
    /// Redfish over IP protocol
    RedfishOverIp = 0x04,
    /// OEM Defined protocol
    OemDefined = 0xF0,
}

///
/// Management Controller Host Interface (Type 42).
///
/// The information in this structure defines the attributes of a Management
/// Controller Host Interface that is not discoverable by "Plug and Play" mechanisms.
///
/// Type 42 should be used for management controller host interfaces that use protocols
/// other than IPMI or that use multiple protocols on a single host interface type.
///
/// This structure should also be provided if IPMI is shared with other protocols
/// over the same interface hardware. If IPMI is not shared with other protocols,
/// either the Type 38 or Type 42 structures can be used. Providing Type 38 is
/// recommended for backward compatibility. The structures are not required to
/// be mutually exclusive. Type 38 and Type 42 structures may be implemented
/// simultaneously to provide backward compatibility with IPMI applications or drivers
/// that do not yet recognize the Type 42 structure.
///
#[repr(C, packed)]
pub struct SmbiosTableType42 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Interface type (MC_HOST_INTERFACE_TYPE)
    pub interface_type: u8,
    /// Interface type specific data length
    pub interface_type_specific_data_length: u8,
    /// Interface type specific data (minimum 4 bytes)
    pub interface_type_specific_data: [u8; 4],
}

///
/// Processor Specific Block - Processor Architecture Type
///
#[repr(u8)]
pub enum ProcessorSpecificBlockArchType {
    /// Reserved architecture
    Reserved = 0x00,
    /// IA-32 architecture
    Ia32 = 0x01,
    /// x64 architecture
    X64 = 0x02,
    /// Itanium architecture
    Itanium = 0x03,
    /// AArch32 architecture
    Aarch32 = 0x04,
    /// AArch64 architecture
    Aarch64 = 0x05,
    /// RISC-V RV32 architecture
    RiscVRv32 = 0x06,
    /// RISC-V RV64 architecture
    RiscVRv64 = 0x07,
    /// RISC-V RV128 architecture
    RiscVRv128 = 0x08,
    /// LoongArch32 architecture
    LoongArch32 = 0x09,
    /// LoongArch64 architecture
    LoongArch64 = 0x0A,
}

///
/// Processor Specific Block is the standard container of processor-specific data.
///
#[repr(C, packed)]
pub struct ProcessorSpecificBlock {
    /// Length of block
    pub length: u8,
    /// Processor architecture type
    pub processor_arch_type: u8,
    //
    //  Below followed by Processor-specific data
    //
    //
}

///
/// Processor Additional Information(Type 44).
///
/// The information in this structure defines the processor additional information in case
/// SMBIOS type 4 is not sufficient to describe processor characteristics.
/// The SMBIOS type 44 structure has a reference handle field to link back to the related
/// SMBIOS type 4 structure. There may be multiple SMBIOS type 44 structures linked to the
/// same SMBIOS type 4 structure. For example, when cores are not identical in a processor,
/// SMBIOS type 44 structures describe different core-specific information.
///
/// SMBIOS type 44 defines the standard header for the processor-specific block, while the
/// contents of processor-specific data are maintained by processor
/// architecture workgroups or vendors in separate documents.
///
#[repr(C, packed)]
pub struct SmbiosTableType44 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Reference handle to associated SMBIOS type 4
    pub ref_handle: SmbiosHandle,
    /// Processor-specific block
    pub processor_specific_block: ProcessorSpecificBlock,
}

///
/// TPM Device (Type 43).
///
#[repr(C, packed)]
pub struct SmbiosTableType43 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Vendor ID
    pub vendor_id: [u8; 4],
    /// Major specification version
    pub major_spec_version: u8,
    /// Minor specification version
    pub minor_spec_version: u8,
    /// Firmware version 1
    pub firmware_version1: u32,
    /// Firmware version 2
    pub firmware_version2: u32,
    /// Description string
    pub description: SmbiosTableString,
    /// Characteristics
    pub characteristics: u64,
    /// OEM defined value
    pub oem_defined: u32,
}

///
/// Firmware Inventory Version Format Type (Type 45).
///
#[repr(u8)]
pub enum FirmwareInventoryVersionFormatType {
    /// Free form version
    FreeForm = 0x00,
    /// Major/minor version
    MajorMinor = 0x01,
    /// 32-bit hex version
    ThirtyTwoBitHex = 0x02,
    /// 64-bit hex version
    SixtyFourBitHex = 0x03,
    /// Reserved
    Reserved = 0x04,
    /// OEM-specific version
    Oem = 0x80,
}

///
/// Firmware Inventory Firmware Id Format Type (Type 45).
///
#[repr(u8)]
pub enum FirmwareInventoryFirmwareIdFormatType {
    /// Free form ID
    FreeForm = 0x00,
    /// UUID ID
    Uuid = 0x01,
    /// Reserved
    Reserved = 0x04,
    /// OEM-specific ID
    Oem = 0x80,
}

///
/// Firmware Inventory Firmware Characteristics (Type 45).
///
bitfield! {
    /// Firmware characteristics bitfield
    pub struct FirmwareCharacteristics(u16);
    impl Debug;
    /// Updatable flag
    pub updatable, set_updatable: 0;
    /// Write protected flag
    pub write_protected, set_write_protected: 1;
    /// Reserved bits
    pub reserved, set_reserved: 15, 3;
}

///
/// Firmware Inventory State Information (Type 45).
///
#[repr(u8)]
pub enum FirmwareInventoryState {
    /// Other state
    Other = 0x01,
    /// Unknown state
    Unknown = 0x02,
    /// Disabled state
    Disabled = 0x03,
    /// Enabled state
    Enabled = 0x04,
    /// Absent state
    Absent = 0x05,
    /// Standby offline state
    StandbyOffline = 0x06,
    /// Standby spare state
    StandbySpare = 0x07,
    /// Unavailable offline state
    UnavailableOffline = 0x08,
}

///
/// Firmware Inventory Information (Type 45)
///
/// The information in this structure defines an inventory of firmware
/// components in the system. This can include firmware components such as
/// BIOS, BMC, as well as firmware for other devices in the system.
/// The information can be used by software to display the firmware inventory
/// in a uniform manner. It can also be used by a management controller,
/// such as a BMC, for remote system management.
/// This structure is not intended to replace other standard programmatic
/// interfaces for firmware updates.
/// One Type 45 structure is provided for each firmware component.
///
#[repr(C, packed)]
pub struct SmbiosTableType45 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// Firmware component name
    pub firmware_component_name: SmbiosTableString,
    /// Firmware version string
    pub firmware_version: SmbiosTableString,
    /// Firmware version format (FIRMWARE_INVENTORY_VERSION_FORMAT_TYPE)
    pub firmware_version_format: u8,
    /// Firmware ID string
    pub firmware_id: SmbiosTableString,
    /// Firmware ID format (FIRMWARE_INVENTORY_FIRMWARE_ID_FORMAT_TYPE)
    pub firmware_id_format: u8,
    /// Release date string
    pub release_date: SmbiosTableString,
    /// Manufacturer string
    pub manufacturer: SmbiosTableString,
    /// Lowest supported version string
    pub lowest_supported_version: SmbiosTableString,
    /// Image size
    pub image_size: u64,
    /// Firmware characteristics
    pub characteristics: FirmwareCharacteristics,
    /// State (FIRMWARE_INVENTORY_STATE)
    pub state: u8,
    /// Associated component count
    pub associated_component_count: u8,
    //
    //  zero or n-number of handles depends on AssociatedComponentCount
    //  handles are of type SMBIOS_HANDLE
    //
}

///
/// String Property IDs (Type 46).
///
#[repr(u16)]
pub enum StringPropertyId {
    /// No property
    None = 0x0000,
    /// Device path property
    DevicePath = 0x0001,
    /// Reserved property
    Reserved = 0x0002,
    /// BIOS vendor property
    BiosVendor = 0x8000,
    /// OEM property
    Oem = 0xC000,
}

///
/// This structure defines a string property for another structure.
/// This allows adding string properties that are common to several structures
/// without having to modify the definitions of these structures.
/// Multiple type 46 structures can add string properties to the same
/// parent structure.
///
#[repr(C, packed)]
pub struct SmbiosTableType46 {
    /// Structure header
    pub hdr: SmbiosStructure,
    /// String property ID (STRING_PROPERTY_ID)
    pub string_property_id: u16,
    /// String property value
    pub string_property_value: SmbiosTableString,
    /// Parent handle
    pub parent_handle: SmbiosHandle,
}

///
/// Inactive (Type 126)
///
#[repr(C, packed)]
pub struct SmbiosTableType126 {
    /// Structure header
    pub hdr: SmbiosStructure,
}

///
/// End-of-Table (Type 127)
///
#[repr(C, packed)]
pub struct SmbiosTableType127 {
    /// Structure header
    pub hdr: SmbiosStructure,
}

///
/// Union of all the possible SMBIOS record types.
///
#[repr(C)]
pub enum SmbiosStructurePointer {
    /// Pointer to SmbiosStructure
    Hdr(*mut SmbiosStructure),
    /// Pointer to Type 0 structure
    Type0(*mut SmbiosTableType0),
    /// Pointer to Type 1 structure
    Type1(*mut SmbiosTableType1),
    /// Pointer to Type 2 structure
    Type2(*mut SmbiosTableType2),
    /// Pointer to Type 3 structure
    Type3(*mut SmbiosTableType3),
    /// Pointer to Type 4 structure
    Type4(*mut SmbiosTableType4),
    /// Pointer to Type 5 structure
    Type5(*mut SmbiosTableType5),
    /// Pointer to Type 6 structure
    Type6(*mut SmbiosTableType6),
    /// Pointer to Type 7 structure
    Type7(*mut SmbiosTableType7),
    /// Pointer to Type 8 structure
    Type8(*mut SmbiosTableType8),
    /// Pointer to Type 9 structure
    Type9(*mut SmbiosTableType9),
    /// Pointer to Type 10 structure
    Type10(*mut SmbiosTableType10),
    /// Pointer to Type 11 structure
    Type11(*mut SmbiosTableType11),
    /// Pointer to Type 12 structure
    Type12(*mut SmbiosTableType12),
    /// Pointer to Type 13 structure
    Type13(*mut SmbiosTableType13),
    /// Pointer to Type 14 structure
    Type14(*mut SmbiosTableType14),
    /// Pointer to Type 15 structure
    Type15(*mut SmbiosTableType15),
    /// Pointer to Type 16 structure
    Type16(*mut SmbiosTableType16),
    /// Pointer to Type 17 structure
    Type17(*mut SmbiosTableType17),
    /// Pointer to Type 18 structure
    Type18(*mut SmbiosTableType18),
    /// Pointer to Type 19 structure
    Type19(*mut SmbiosTableType19),
    /// Pointer to Type 20 structure
    Type20(*mut SmbiosTableType20),
    /// Pointer to Type 21 structure
    Type21(*mut SmbiosTableType21),
    /// Pointer to Type 22 structure
    Type22(*mut SmbiosTableType22),
    /// Pointer to Type 23 structure
    Type23(*mut SmbiosTableType23),
    /// Pointer to Type 24 structure
    Type24(*mut SmbiosTableType24),
    /// Pointer to Type 25 structure
    Type25(*mut SmbiosTableType25),
    /// Pointer to Type 26 structure
    Type26(*mut SmbiosTableType26),
    /// Pointer to Type 27 structure
    Type27(*mut SmbiosTableType27),
    /// Pointer to Type 28 structure
    Type28(*mut SmbiosTableType28),
    /// Pointer to Type 29 structure
    Type29(*mut SmbiosTableType29),
    /// Pointer to Type 30 structure
    Type30(*mut SmbiosTableType30),
    /// Pointer to Type 31 structure
    Type31(*mut SmbiosTableType31),
    /// Pointer to Type 32 structure
    Type32(*mut SmbiosTableType32),
    /// Pointer to Type 33 structure
    Type33(*mut SmbiosTableType33),
    /// Pointer to Type 34 structure
    Type34(*mut SmbiosTableType34),
    /// Pointer to Type 35 structure
    Type35(*mut SmbiosTableType35),
    /// Pointer to Type 36 structure
    Type36(*mut SmbiosTableType36),
    /// Pointer to Type 37 structure
    Type37(*mut SmbiosTableType37),
    /// Pointer to Type 38 structure
    Type38(*mut SmbiosTableType38),
    /// Pointer to Type 39 structure
    Type39(*mut SmbiosTableType39),
    /// Pointer to Type 40 structure
    Type40(*mut SmbiosTableType40),
    /// Pointer to Type 41 structure
    Type41(*mut SmbiosTableType41),
    /// Pointer to Type 42 structure
    Type42(*mut SmbiosTableType42),
    /// Pointer to Type 43 structure
    Type43(*mut SmbiosTableType43),
    /// Pointer to Type 44 structure
    Type44(*mut SmbiosTableType44),
    /// Pointer to Type 45 structure
    Type45(*mut SmbiosTableType45),
    /// Pointer to Type 46 structure
    Type46(*mut SmbiosTableType46),
    /// Pointer to Type 126 structure
    Type126(*mut SmbiosTableType126),
    /// Pointer to Type 127 structure
    Type127(*mut SmbiosTableType127),
    /// Pointer to raw data
    Raw(*mut u8),
}

// ---------------------- SmbiosDxe Header Start ----------------------

const SMBIOS_INSTANCE_SIGNATURE: u32 = signature_32(b'S', b'B', b'i', b's');
const EFI_SMBIOS_ENTRY_SIGNATURE: u32 = signature_32(b'S', b'r', b'e', b'c');
const SMBIOS_HANDLE_ENTRY_SIGNATURE: u32 = signature_32(b'S', b'h', b'r', b'd');
const EFI_SMBIOS_RECORD_HEADER_VERSION: u16 = 0x0100;

// Helper function to create signature (equivalent to SIGNATURE_32 macro)
const fn signature_32(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

/// Unsigned integer type for SMBIOS
pub type UIntn = usize;

/// SMBIOS instance structure
#[repr(C, packed)]
pub struct SmbiosInstance {
    /// Instance signature
    pub signature: u32,
    /// Handle
    pub handle: core::ffi::c_void,
    /// SMBIOS protocol (optional)
    pub smbios: Option<i32>,
    /// Data list head
    pub data_list_head: list_entry::Entry,
    /// Allocated handle list head
    pub allocated_handle_list_head: list_entry::Entry,
}

// TODO decide if we implement SMBIOS_INSTANCE_FROM_THIS macro
/*
impl SmbiosInstance {
    pub fn from_smbios_protocol(smbios: &EfiSmbiosProtocol) -> &Self {
        // This is a simplified version - need proper offset calculation
        unsafe {
            let instance_ptr = (smbios as *const EfiSmbiosProtocol)
                .cast::<u8>()
                .sub(offset_of!(SmbiosInstance, smbios))
                .cast::<SmbiosInstance>();
            &*instance_ptr
        }
    }
}
*/

/// SMBIOS record Header
///
/// An SMBIOS internal Record is an EFI_SMBIOS_RECORD_HEADER followed by
/// (RecordSize - HeaderSize) bytes of data. The format of the data is
/// defined by the SMBIOS spec.
///
/// TODO: Are these internal for Rust usage, or still used with C FFI?
/// If they are only intended for Rust usage, consider making safer by avoiding types with header
/// plus untyped/unconstrained data after them and using e.g. a slice (or a Vec) and accessor routines.
/// If they are intended for use in FFI, then this is probably fine to match the C style approach.
#[repr(C, packed)]
pub struct EfiSmbiosRecordHeader {
    /// Record version
    pub version: u16,
    /// Header size
    pub header_size: u16,
    /// Record size
    pub record_size: UIntn,
    /// Producer handle
    pub producer_handle: core::ffi::c_void,
    /// Number of strings
    pub number_of_strings: UIntn,
}

/// Private data structure to contain the SMBIOS record. One record per
/// structure. SmbiosRecord is a copy of the data passed in and follows RecordHeader.
#[repr(C, packed)]
pub struct EfiSmbiosEntry {
    /// Entry signature
    pub signature: u32,
    /// Linked list entry
    pub link: list_entry::Entry,
    /// Record header
    pub record_header: Option<EfiSmbiosRecordHeader>,
    /// Record size
    pub record_size: UIntn,
    /// Indicates if record is in 32-bit table
    pub smbios_32bit_table: bool,
    /// Indicates if record is in 64-bit table
    pub smbios_64bit_table: bool,
}

/// Private data to contain the Smbios handle that already allocated.
#[repr(C, packed)]
pub struct SmbiosHandleEntry {
    /// Entry signature
    pub signature: u32,
    /// Linked list entry
    pub link: list_entry::Entry,
    /// SMBIOS handle
    pub smbios_handle: core::ffi::c_void,
}

// TODO decide if we implement SMBIOS_HANDLE_ENTRY_FROM_LINK macro
/*
impl SmbiosHandleEntry {
    /// Equivalent to SMBIOS_HANDLE_ENTRY_FROM_LINK macro
    pub fn from_link(link: &list_entry::Entry) -> &Self {
        unsafe {
            let entry_ptr = (link as *const list_entry::Entry)
                .cast::<u8>()
                .sub(offset_of!(SmbiosHandleEntry, link))
                .cast::<SmbiosHandleEntry>();
            &*entry_ptr
        }
    }
}
*/

#[repr(C, packed)]
/// EFI SMBIOS Table End Structure
pub struct EfiSmbiosTableEndStructure {
    /// Table end header
    pub header: EfiSmbiosTableHeader,
    /// Tailing bytes
    pub tailing: [u8; 2],
}

/// Function pointer type for SMBIOS table validation
pub type IsSmbiosTableValid = fn(
    table_entry: *const core::ffi::c_void,
    table_address: &mut *const core::ffi::c_void,
    table_maximum_size: &mut UIntn,
    major_version: &mut u8,
    minor_version: &mut u8,
) -> bool;

/// Structure to hold SMBIOS table validation info
pub struct IsSmbiosTableValidEntry {
    /// GUID for table validation
    pub guid: efi::Guid,
    /// Validation function pointer
    pub is_valid: IsSmbiosTableValid,
}
