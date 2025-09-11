#![no_std]

use bitfield::bitfield;
use r_efi::efi;
use mu_pi::list_entry;
use patina_dxe_core::tpl_lock;

/// SMBIOS Standard Constants
///
/// This module contains SMBIOS standard definitions converted from
/// TianoCore EDK2's SmbiosStandard.h following UEFI coding standards.

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
pub const SMBIOS_ANCHOR_STRING_LENGTH: usize = 4;

/// Reference SMBIOS 3.4, chapter 5.2.2 SMBIOS 3.0 (64-bit) Entry Point
/// Table 2 - SMBIOS 3.0 (64-bit) Entry Point structure, offset 00h
/// _SM3_, specified as five ASCII characters (5F 53 4D 33 5F).
pub const SMBIOS_3_0_ANCHOR_STRING: &[u8; 5] = b"_SM3_";
pub const SMBIOS_3_0_ANCHOR_STRING_LENGTH: usize = 5;

/// SMBIOS type constants according to SMBIOS 3.3.0 specification.
pub const SMBIOS_TYPE_BIOS_INFORMATION: u8 = 0;
pub const SMBIOS_TYPE_SYSTEM_INFORMATION: u8 = 1;
pub const SMBIOS_TYPE_BASEBOARD_INFORMATION: u8 = 2;
pub const SMBIOS_TYPE_SYSTEM_ENCLOSURE: u8 = 3;
pub const SMBIOS_TYPE_PROCESSOR_INFORMATION: u8 = 4;
pub const SMBIOS_TYPE_MEMORY_CONTROLLER_INFORMATION: u8 = 5;
pub const SMBIOS_TYPE_MEMORY_MODULE_INFORMATON: u8 = 6;
pub const SMBIOS_TYPE_CACHE_INFORMATION: u8 = 7;
pub const SMBIOS_TYPE_PORT_CONNECTOR_INFORMATION: u8 = 8;
pub const SMBIOS_TYPE_SYSTEM_SLOTS: u8 = 9;
pub const SMBIOS_TYPE_ONBOARD_DEVICE_INFORMATION: u8 = 10;
pub const SMBIOS_TYPE_OEM_STRINGS: u8 = 11;
pub const SMBIOS_TYPE_SYSTEM_CONFIGURATION_OPTIONS: u8 = 12;
pub const SMBIOS_TYPE_BIOS_LANGUAGE_INFORMATION: u8 = 13;
pub const SMBIOS_TYPE_GROUP_ASSOCIATIONS: u8 = 14;
pub const SMBIOS_TYPE_SYSTEM_EVENT_LOG: u8 = 15;
pub const SMBIOS_TYPE_PHYSICAL_MEMORY_ARRAY: u8 = 16;
pub const SMBIOS_TYPE_MEMORY_DEVICE: u8 = 17;
pub const SMBIOS_TYPE_32BIT_MEMORY_ERROR_INFORMATION: u8 = 18;
pub const SMBIOS_TYPE_MEMORY_ARRAY_MAPPED_ADDRESS: u8 = 19;
pub const SMBIOS_TYPE_MEMORY_DEVICE_MAPPED_ADDRESS: u8 = 20;
pub const SMBIOS_TYPE_BUILT_IN_POINTING_DEVICE: u8 = 21;
pub const SMBIOS_TYPE_PORTABLE_BATTERY: u8 = 22;
pub const SMBIOS_TYPE_SYSTEM_RESET: u8 = 23;
pub const SMBIOS_TYPE_HARDWARE_SECURITY: u8 = 24;
pub const SMBIOS_TYPE_SYSTEM_POWER_CONTROLS: u8 = 25;
pub const SMBIOS_TYPE_VOLTAGE_PROBE: u8 = 26;
pub const SMBIOS_TYPE_COOLING_DEVICE: u8 = 27;
pub const SMBIOS_TYPE_TEMPERATURE_PROBE: u8 = 28;
pub const SMBIOS_TYPE_ELECTRICAL_CURRENT_PROBE: u8 = 29;
pub const SMBIOS_TYPE_OUT_OF_BAND_REMOTE_ACCESS: u8 = 30;
pub const SMBIOS_TYPE_BOOT_INTEGRITY_SERVICE: u8 = 31;
pub const SMBIOS_TYPE_SYSTEM_BOOT_INFORMATION: u8 = 32;
pub const SMBIOS_TYPE_64BIT_MEMORY_ERROR_INFORMATION: u8 = 33;
pub const SMBIOS_TYPE_MANAGEMENT_DEVICE: u8 = 34;
pub const SMBIOS_TYPE_MANAGEMENT_DEVICE_COMPONENT: u8 = 35;
pub const SMBIOS_TYPE_MANAGEMENT_DEVICE_THRESHOLD_DATA: u8 = 36;
pub const SMBIOS_TYPE_MEMORY_CHANNEL: u8 = 37;
pub const SMBIOS_TYPE_IPMI_DEVICE_INFORMATION: u8 = 38;
pub const SMBIOS_TYPE_SYSTEM_POWER_SUPPLY: u8 = 39;
pub const SMBIOS_TYPE_ADDITIONAL_INFORMATION: u8 = 40;
pub const SMBIOS_TYPE_ONBOARD_DEVICES_EXTENDED_INFORMATION: u8 = 41;
pub const SMBIOS_TYPE_MANAGEMENT_CONTROLLER_HOST_INTERFACE: u8 = 42;
pub const SMBIOS_TYPE_TPM_DEVICE: u8 = 43;
pub const SMBIOS_TYPE_PROCESSOR_ADDITIONAL_INFORMATION: u8 = 44;
pub const SMBIOS_TYPE_FIRMWARE_INVENTORY_INFORMATION: u8 = 45;
pub const SMBIOS_TYPE_STRING_PROPERTY_INFORMATION: u8 = 46;

/// Inactive type is added from SMBIOS 2.2. Reference SMBIOS 2.6, chapter 3.3.43.
/// Upper-level software that interprets the SMBIOS structure-table should bypass an
/// Inactive structure just like a structure type that the software does not recognize.
pub const SMBIOS_TYPE_INACTIVE: u16 = 0x007E;

/// End-of-table type is added from SMBIOS 2.2. Reference SMBIOS 2.6, chapter 3.3.44.
/// The end-of-table indicator is used in the last physical structure in a table
pub const SMBIOS_TYPE_END_OF_TABLE: u8 = 0x7F;

/// OEM-specific SMBIOS types range
pub const SMBIOS_OEM_BEGIN: u8 = 128;
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

pub type EfiSmbiosTableHeader = SmbiosStructure;

// SMBios Table EP Structure
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SmbiosTableEntryPoint {
    pub anchor_string: [u8; SMBIOS_ANCHOR_STRING_LENGTH],
    pub entry_point_structure_checksum: u8,
    pub entry_point_length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub max_structure_size: u16,
    pub entry_point_revision: u8,
    pub formatted_area: [u8; 5],
    pub intermediate_anchor_string: [u8; 5],
    pub intermediate_checksum: u8,
    pub table_length: u16,
    pub table_address: u32,
    pub number_of_smbios_structures: u16,
    pub smbios_bcd_revision: u8,
}

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SmbiosTable30EntryPoint {
    pub anchor_string: [u8; SMBIOS_3_0_ANCHOR_STRING_LENGTH],
    pub entry_point_structure_checksum: u8,
    pub entry_point_length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub doc_rev: u8,
    pub entry_point_revision: u8,
    pub reserved: u8,
    pub table_maximum_size: u32,
    pub table_address: u64,
}

// Smbios structure header
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SmbiosStructure {
    pub r#type: SmbiosType,
    pub length: u8,
    pub handle: SmbiosHandle,
}

///
/// Text strings associated with a given SMBIOS structure are returned in the dmiStrucBuffer, appended directly after
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
bitfield! {
    pub struct MiscBiosCharacteristics(u64);
    impl Debug;
    pub reserved, set_reserved: 1, 0;
    pub unknown, set_unknown: 2;
    pub bios_characteristics_not_supported, set_bios_characteristics_not_supported: 3;
    pub isa_is_supported, set_isa_is_supported: 4;
    pub mca_is_supported, set_mca_is_supported: 5;
    pub eisa_is_supported, set_eisa_is_supported: 6;
    pub pci_is_supported, set_pci_is_supported: 7;
    pub pcmcia_is_supported, set_pcmcia_is_supported: 8;
    pub plug_and_play_is_supported, set_plug_and_play_is_supported: 9;
    pub apm_is_supported, set_apm_is_supported: 10;
    pub bios_is_upgradable, set_bios_is_upgradable: 11;
    pub bios_shadowing_allowed, set_bios_shadowing_allowed: 12;
    pub vl_vesa_is_supported, set_vl_vesa_is_supported: 13;
    pub escd_support_is_available, set_escd_support_is_available: 14;
    pub boot_from_cd_is_supported, set_boot_from_cd_is_supported: 15;
    pub selectable_boot_is_supported, set_selectable_boot_is_supported: 16;
    pub rom_bios_is_socketed, set_rom_bios_is_socketed: 17;
    pub boot_from_pcmcia_is_supported, set_boot_from_pcmcia_is_supported: 18;
    pub edd_specification_is_supported, set_edd_specification_is_supported: 19;
    pub japanese_nec_floppy_is_supported, set_japanese_nec_floppy_is_supported: 20;
    pub japanese_toshiba_floppy_is_supported, set_japanese_toshiba_floppy_is_supported: 21;
    pub floppy_525_360_is_supported, set_floppy_525_360_is_supported: 22;
    pub floppy_525_12_is_supported, set_floppy_525_12_is_supported: 23;
    pub floppy_35_720_is_supported, set_floppy_35_720_is_supported: 24;
    pub floppy_35_288_is_supported, set_floppy_35_288_is_supported: 25;
    pub print_screen_is_supported, set_print_screen_is_supported: 26;
    pub keyboard_8042_is_supported, set_keyboard_8042_is_supported: 27;
    pub serial_is_supported, set_serial_is_supported: 28;
    pub printer_is_supported, set_printer_is_supported: 29;
    pub cga_mono_is_supported, set_cga_mono_is_supported: 30;
    pub nec_pc98, set_nec_pc98: 31;
    pub reserved_for_vendor, set_reserved_for_vendor: 63, 32;
}

///
/// BIOS Characteristics Extension Byte 1.
/// This information, available for SMBIOS version 2.1 and later, appears at offset 12h
/// within the BIOS Information structure.
///
bitfield! {
    pub struct MbceBiosReserved(u8);
    impl Debug;
    pub acpi_is_supported, set_acpi_is_supported: 0;
    pub usb_legacy_is_supported, set_usb_legacy_is_supported: 1;
    pub agp_is_supported, set_agp_is_supported: 2;
    pub i2o_boot_is_supported, set_i2o_boot_is_supported: 3;
    pub ls120_boot_is_supported, set_ls120_boot_is_supported: 4;
    pub atapi_zip_drive_boot_is_supported, set_atapi_zip_drive_boot_is_supported: 5;
    pub boot_1394_is_supported, set_boot_1394_is_supported: 6;
    pub smart_battery_is_supported, set_smart_battery_is_supported: 7;
}

///
/// BIOS Characteristics Extension Byte 2.
/// This information, available for SMBIOS version 2.3 and later, appears at offset 13h
/// within the BIOS Information structure.
///
bitfield! {
    pub struct MbceSystemReserved(u8);
    impl Debug;
    pub acpi_is_supported, set_acpi_is_supported: 0;
    pub usb_legacy_is_supported, set_usb_legacy_is_supported: 1;
    pub agp_is_supported, set_agp_is_supported: 2;
    pub i2o_boot_is_supported, set_i2o_boot_is_supported: 3;
    pub ls120_boot_is_supported, set_ls120_boot_is_supported: 4;
    pub atapi_zip_drive_boot_is_supported, set_atapi_zip_drive_boot_is_supported: 5;
    pub boot_1394_is_supported, set_boot_1394_is_supported: 6;
    pub smart_battery_is_supported, set_smart_battery_is_supported: 7;
}

// BIOS Characteristics Extension Bytes
#[repr(C, packed)]
pub struct MiscBiosCharacteristicsExt {
    pub bios_reserved: MbceBiosReserved,
    pub system_reserved: MbceSystemReserved,
}

// Extended BIOS ROM size
#[repr(C, packed)]
pub struct ExtendedBiosRomSize {
    pub size: B14,
    pub unit: B2,
}

// Bios Information: Type 0
#[repr(C, packed)]
pub struct SmbiosTableType0 {
    pub hdr: SmbiosStructure,
    pub vendor: SmbiosTableString,
    pub bios_version: SmbiosTableString,
    pub bios_segment: u16,
    pub bios_release_date: SmbiosTableString,
    pub bios_size: u8,
    pub bios_characteristics: MiscBiosCharacteristics,
    pub bios_characteristics_ext_bytes: [u8; 2],
    pub system_bios_major_release: u8,
    pub system_bios_minor_release: u8,
    pub embedded_controller_firmware_major_release: u8,
    pub embedded_controller_firmware_minor_release: u8,
    // add for smbios 3.1.0
    pub extended_bios_size: ExtendedBiosRomSize,
}

// System Wake-up Type
#[repr(u8)]
pub enum MiscSystemWakeupType {
    SystemWakeupTypeReserved = 0x00,
    SystemWakeupTypeOther = 0x01,
    SystemWakeupTypeUnknown = 0x02,
    SystemWakeupTypeApmTimer = 0x03,
    SystemWakeupTypeModemRing = 0x04,
    SystemWakeupTypeLanRemote = 0x05,
    SystemWakeupTypePowerSwitch = 0x06,
    SystemWakeupTypePciPme = 0x07,
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
#[repr(C, packed)]
pub struct SmbiosTableType1 {
    hdr: SmbiosStructure,
    manufacturer: SmbiosTableString,
    product_name: SmbiosTableString,
    version: SmbiosTableString,
    serial_number: SmbiosTableString,
    uuid: efi::Guid,
    wake_up_type: u8,
    sku_number: SmbiosTableString,
    family: SmbiosTableString,
}

///
///  Base Board - Feature Flags.
///
bitfield! {
    struct BaseBoardFeatureFlags(u8);
    impl Debug;
    pub motherboard, set_motherboard: 0;
    pub requires_daughter_card, set_requires_daughter_card: 1;
    pub removable, set_removable: 2;
    pub replaceable, set_replaceable: 3;
    pub hot_swappable, set_hot_swappable: 4;
    pub reserved, set_reserved: 7, 5;
}

///
///  Base Board - Board Type.
///
#[repr(u8)]
pub enum BaseBoardType {
    BaseBoardTypeUnknown = 0x1,
    BaseBoardTypeOther = 0x2,
    BaseBoardTypeServerBlade = 0x3,
    BaseBoardTypeConnectivitySwitch = 0x4,
    BaseBoardTypeSystemManagementModule = 0x5,
    BaseBoardTypeProcessorModule = 0x6,
    BaseBoardTypeIOModule = 0x7,
    BaseBoardTypeMemoryModule = 0x8,
    BaseBoardTypeDaughterBoard = 0x9,
    BaseBoardTypeMotherBoard = 0xA,
    BaseBoardTypeProcessorMemoryModule = 0xB,
    BaseBoardTypeProcessorIOModule = 0xC,
    BaseBoardTypeInterconnectBoard = 0xD,
}

///
/// Base Board (or Module) Information (Type 2).
///
/// The information in this structure defines attributes of a system baseboard -
/// for example a motherboard, planar, or server blade or other standard system module.
///
#[repr(C, packed)]
pub struct SmbiosTableType2 {
    pub hdr: SmbiosStructure,
    pub manufacturer: SmbiosTableString,
    pub product_name: SmbiosTableString,
    pub version: SmbiosTableString,
    pub serial_number: SmbiosTableString,
    pub asset_tag: SmbiosTableString,
    pub feature_flag: BaseBoardFeatureFlags,
    pub location_in_chassis: SmbiosTableString,
    pub chassis_handle: u16,
    pub board_type: u8,
    pub number_of_contained_object_handles: u8,
    pub contained_object_handles: [u16; 1],
}

///
/// System Enclosure or Chassis Types
///
pub enum MiscChassisType {
    MiscChassisTypeOther = 0x01,
    MiscChassisTypeUnknown = 0x02,
    MiscChassisTypeDeskTop = 0x03,
    MiscChassisTypeLowProfileDesktop = 0x04,
    MiscChassisTypePizzaBox = 0x05,
    MiscChassisTypeMiniTower = 0x06,
    MiscChassisTypeTower = 0x07,
    MiscChassisTypePortable = 0x08,
    MiscChassisTypeLapTop = 0x09,
    MiscChassisTypeNotebook = 0x0A,
    MiscChassisTypeHandHeld = 0x0B,
    MiscChassisTypeDockingStation = 0x0C,
    MiscChassisTypeAllInOne = 0x0D,
    MiscChassisTypeSubNotebook = 0x0E,
    MiscChassisTypeSpaceSaving = 0x0F,
    MiscChassisTypeLunchBox = 0x10,
    MiscChassisTypeMainServerChassis = 0x11,
    MiscChassisTypeExpansionChassis = 0x12,
    MiscChassisTypeSubChassis = 0x13,
    MiscChassisTypeBusExpansionChassis = 0x14,
    MiscChassisTypePeripheralChassis = 0x15,
    MiscChassisTypeRaidChassis = 0x16,
    MiscChassisTypeRackMountChassis = 0x17,
    MiscChassisTypeSealedCasePc = 0x18,
    MiscChassisMultiSystemChassis = 0x19,
    MiscChassisCompactPCI = 0x1A,
    MiscChassisAdvancedTCA = 0x1B,
    MiscChassisBlade = 0x1C,
    MiscChassisBladeEnclosure = 0x1D,
    MiscChassisTablet = 0x1E,
    MiscChassisConvertible = 0x1F,
    MiscChassisDetachable = 0x20,
    MiscChassisIoTGateway = 0x21,
    MiscChassisEmbeddedPc = 0x22,
    MiscChassisMiniPc = 0x23,
    MiscChassisStickPc = 0x24,
}

///
/// System Enclosure or Chassis States .
///
#[repr(u8)]
pub enum MiscChassisState {
    ChassisStateOther = 0x01,
    ChassisStateUnknown = 0x02,
    ChassisStateSafe = 0x03,
    ChassisStateWarning = 0x04,
    ChassisStateCritical = 0x05,
    ChassisStateNonRecoverable = 0x06,
}

///
/// System Enclosure or Chassis Security Status.
///

#[repr(u8)]
pub enum MiscChassisSecurityState {
    ChassisSecurityStatusOther = 0x01,
    ChassisSecurityStatusUnknown = 0x02,
    ChassisSecurityStatusNone = 0x03,
    ChassisSecurityStatusExternalInterfaceLockedOut = 0x04,
    ChassisSecurityStatusExternalInterfaceLockedEnabled = 0x05,
}

///
/// Contained Element record
///
#[repr(C, packed)]
pub struct ContainedElement {
    pub contained_element_type: u8,
    pub contained_element_minimum: u8,
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
    pub hdr: SmbiosStructure,
    pub manufacturer: SmbiosTableString,
    pub r#type: u8,
    pub version: SmbiosTableString,
    pub serial_number: SmbiosTableString,
    pub asset_tag: SmbiosTableString,
    pub bootup_state: u8,
    pub power_supply_state: u8,
    pub thermal_state: u8,
    pub security_status: u8,
    pub oem_defined: [u8; 4],
    pub height: u8,
    pub numberof_power_cords: u8,
    pub contained_element_count: u8,
    pub contained_element_record_length: u8,
    //
    // Can have 0 to (ContainedElementCount * ContainedElementRecordLength) contained elements
    //
    pub contained_elements: [ContainedElement; 1],
    //
    // Add for smbios 2.7
    //
    // Since ContainedElements has a variable number of entries, must not define SKUNumber in
    // the structure.  Need to reference it by starting at offset 0x15 and adding
    // (ContainedElementCount * ContainedElementRecordLength) bytes.
    //
    // SMBIOS_TABLE_STRING         SKUNumber;
}

///
/// Processor Information - Processor Type.
///
#[repr(u8)]
pub enum ProcessorTypeData {
    ProcessorOther = 0x01,
    ProcessorUnknown = 0x02,
    CentralProcessor = 0x03,
    MathProcessor = 0x04,
    DspProcessor = 0x05,
    VideoProcessor = 0x06,
}

///
/// Processor Information - Processor Family.
///
#[repr(u8)]
pub enum ProcessorFamilyData {
    ProcessorFamilyOther = 0x01,
    ProcessorFamilyUnknown = 0x02,
    ProcessorFamily8086 = 0x03,
    ProcessorFamily80286 = 0x04,
    ProcessorFamilyIntel386 = 0x05,
    ProcessorFamilyIntel486 = 0x06,
    ProcessorFamily8087 = 0x07,
    ProcessorFamily80287 = 0x08,
    ProcessorFamily80387 = 0x09,
    ProcessorFamily80487 = 0x0A,
    ProcessorFamilyPentium = 0x0B,
    ProcessorFamilyPentiumPro = 0x0C,
    ProcessorFamilyPentiumII = 0x0D,
    ProcessorFamilyPentiumMMX = 0x0E,
    ProcessorFamilyCeleron = 0x0F,
    ProcessorFamilyPentiumIIXeon = 0x10,
    ProcessorFamilyPentiumIII = 0x11,
    ProcessorFamilyM1 = 0x12,
    ProcessorFamilyM2 = 0x13,
    ProcessorFamilyIntelCeleronM = 0x14,
    ProcessorFamilyIntelPentium4Ht = 0x15,
    ProcessorFamilyAmdDuron = 0x18,
    ProcessorFamilyK5 = 0x19,
    ProcessorFamilyK6 = 0x1A,
    ProcessorFamilyK6_2 = 0x1B,
    ProcessorFamilyK6_3 = 0x1C,
    ProcessorFamilyAmdAthlon = 0x1D,
    ProcessorFamilyAmd29000 = 0x1E,
    ProcessorFamilyK6_2Plus = 0x1F,
    ProcessorFamilyPowerPC = 0x20,
    ProcessorFamilyPowerPC601 = 0x21,
    ProcessorFamilyPowerPC603 = 0x22,
    ProcessorFamilyPowerPC603Plus = 0x23,
    ProcessorFamilyPowerPC604 = 0x24,
    ProcessorFamilyPowerPC620 = 0x25,
    ProcessorFamilyPowerPCx704 = 0x26,
    ProcessorFamilyPowerPC750 = 0x27,
    ProcessorFamilyIntelCoreDuo = 0x28,
    ProcessorFamilyIntelCoreDuoMobile = 0x29,
    ProcessorFamilyIntelCoreSoloMobile = 0x2A,
    ProcessorFamilyIntelAtom = 0x2B,
    ProcessorFamilyIntelCoreM = 0x2C,
    ProcessorFamilyIntelCorem3 = 0x2D,
    ProcessorFamilyIntelCorem5 = 0x2E,
    ProcessorFamilyIntelCorem7 = 0x2F,
    ProcessorFamilyAlpha = 0x30,
    ProcessorFamilyAlpha21064 = 0x31,
    ProcessorFamilyAlpha21066 = 0x32,
    ProcessorFamilyAlpha21164 = 0x33,
    ProcessorFamilyAlpha21164PC = 0x34,
    ProcessorFamilyAlpha21164a = 0x35,
    ProcessorFamilyAlpha21264 = 0x36,
    ProcessorFamilyAlpha21364 = 0x37,
    ProcessorFamilyAmdTurionIIUltraDualCoreMobileM = 0x38,
    ProcessorFamilyAmdTurionIIDualCoreMobileM = 0x39,
    ProcessorFamilyAmdAthlonIIDualCoreM = 0x3A,
    ProcessorFamilyAmdOpteron6100Series = 0x3B,
    ProcessorFamilyAmdOpteron4100Series = 0x3C,
    ProcessorFamilyAmdOpteron6200Series = 0x3D,
    ProcessorFamilyAmdOpteron4200Series = 0x3E,
    ProcessorFamilyAmdFxSeries = 0x3F,
    ProcessorFamilyMips = 0x40,
    ProcessorFamilyMIPSR4000 = 0x41,
    ProcessorFamilyMIPSR4200 = 0x42,
    ProcessorFamilyMIPSR4400 = 0x43,
    ProcessorFamilyMIPSR4600 = 0x44,
    ProcessorFamilyMIPSR10000 = 0x45,
    ProcessorFamilyAmdCSeries = 0x46,
    ProcessorFamilyAmdESeries = 0x47,
    ProcessorFamilyAmdASeries = 0x48, // SMBIOS spec 2.8.0 updated the name
    ProcessorFamilyAmdGSeries = 0x49,
    ProcessorFamilyAmdZSeries = 0x4A,
    ProcessorFamilyAmdRSeries = 0x4B,
    ProcessorFamilyAmdOpteron4300 = 0x4C,
    ProcessorFamilyAmdOpteron6300 = 0x4D,
    ProcessorFamilyAmdOpteron3300 = 0x4E,
    ProcessorFamilyAmdFireProSeries = 0x4F,
    ProcessorFamilySparc = 0x50,
    ProcessorFamilySuperSparc = 0x51,
    ProcessorFamilymicroSparcII = 0x52,
    ProcessorFamilymicroSparcIIep = 0x53,
    ProcessorFamilyUltraSparc = 0x54,
    ProcessorFamilyUltraSparcII = 0x55,
    ProcessorFamilyUltraSparcIii = 0x56,
    ProcessorFamilyUltraSparcIII = 0x57,
    ProcessorFamilyUltraSparcIIIi = 0x58,
    ProcessorFamily68040 = 0x60,
    ProcessorFamily68xxx = 0x61,
    ProcessorFamily68000 = 0x62,
    ProcessorFamily68010 = 0x63,
    ProcessorFamily68020 = 0x64,
    ProcessorFamily68030 = 0x65,
    ProcessorFamilyAmdAthlonX4QuadCore = 0x66,
    ProcessorFamilyAmdOpteronX1000Series = 0x67,
    ProcessorFamilyAmdOpteronX2000Series = 0x68,
    ProcessorFamilyAmdOpteronASeries = 0x69,
    ProcessorFamilyAmdOpteronX3000Series = 0x6A,
    ProcessorFamilyAmdZen = 0x6B,
    ProcessorFamilyHobbit = 0x70,
    ProcessorFamilyCrusoeTM5000 = 0x78,
    ProcessorFamilyCrusoeTM3000 = 0x79,
    ProcessorFamilyEfficeonTM8000 = 0x7A,
    ProcessorFamilyWeitek = 0x80,
    ProcessorFamilyItanium = 0x82,
    ProcessorFamilyAmdAthlon64 = 0x83,
    ProcessorFamilyAmdOpteron = 0x84,
    ProcessorFamilyAmdSempron = 0x85,
    ProcessorFamilyAmdTurion64Mobile = 0x86,
    ProcessorFamilyDualCoreAmdOpteron = 0x87,
    ProcessorFamilyAmdAthlon64X2DualCore = 0x88,
    ProcessorFamilyAmdTurion64X2Mobile = 0x89,
    ProcessorFamilyQuadCoreAmdOpteron = 0x8A,
    ProcessorFamilyThirdGenerationAmdOpteron = 0x8B,
    ProcessorFamilyAmdPhenomFxQuadCore = 0x8C,
    ProcessorFamilyAmdPhenomX4QuadCore = 0x8D,
    ProcessorFamilyAmdPhenomX2DualCore = 0x8E,
    ProcessorFamilyAmdAthlonX2DualCore = 0x8F,
    ProcessorFamilyPARISC = 0x90,
    ProcessorFamilyPaRisc8500 = 0x91,
    ProcessorFamilyPaRisc8000 = 0x92,
    ProcessorFamilyPaRisc7300LC = 0x93,
    ProcessorFamilyPaRisc7200 = 0x94,
    ProcessorFamilyPaRisc7100LC = 0x95,
    ProcessorFamilyPaRisc7100 = 0x96,
    ProcessorFamilyV30 = 0xA0,
    ProcessorFamilyQuadCoreIntelXeon3200Series = 0xA1,
    ProcessorFamilyDualCoreIntelXeon3000Series = 0xA2,
    ProcessorFamilyQuadCoreIntelXeon5300Series = 0xA3,
    ProcessorFamilyDualCoreIntelXeon5100Series = 0xA4,
    ProcessorFamilyDualCoreIntelXeon5000Series = 0xA5,
    ProcessorFamilyDualCoreIntelXeonLV = 0xA6,
    ProcessorFamilyDualCoreIntelXeonULV = 0xA7,
    ProcessorFamilyDualCoreIntelXeon7100Series = 0xA8,
    ProcessorFamilyQuadCoreIntelXeon5400Series = 0xA9,
    ProcessorFamilyQuadCoreIntelXeon = 0xAA,
    ProcessorFamilyDualCoreIntelXeon5200Series = 0xAB,
    ProcessorFamilyDualCoreIntelXeon7200Series = 0xAC,
    ProcessorFamilyQuadCoreIntelXeon7300Series = 0xAD,
    ProcessorFamilyQuadCoreIntelXeon7400Series = 0xAE,
    ProcessorFamilyMultiCoreIntelXeon7400Series = 0xAF,
    ProcessorFamilyPentiumIIIXeon = 0xB0,
    ProcessorFamilyPentiumIIISpeedStep = 0xB1,
    ProcessorFamilyPentium4 = 0xB2,
    ProcessorFamilyIntelXeon = 0xB3,
    ProcessorFamilyAS400 = 0xB4,
    ProcessorFamilyIntelXeonMP = 0xB5,
    ProcessorFamilyAMDAthlonXP = 0xB6,
    ProcessorFamilyAMDAthlonMP = 0xB7,
    ProcessorFamilyIntelItanium2 = 0xB8,
    ProcessorFamilyIntelPentiumM = 0xB9,
    ProcessorFamilyIntelCeleronD = 0xBA,
    ProcessorFamilyIntelPentiumD = 0xBB,
    ProcessorFamilyIntelPentiumEx = 0xBC,
    ProcessorFamilyIntelCoreSolo = 0xBD, // SMBIOS spec 2.6 updated this value
    ProcessorFamilyReserved = 0xBE,
    ProcessorFamilyIntelCore2 = 0xBF,
    ProcessorFamilyIntelCore2Solo = 0xC0,
    ProcessorFamilyIntelCore2Extreme = 0xC1,
    ProcessorFamilyIntelCore2Quad = 0xC2,
    ProcessorFamilyIntelCore2ExtremeMobile = 0xC3,
    ProcessorFamilyIntelCore2DuoMobile = 0xC4,
    ProcessorFamilyIntelCore2SoloMobile = 0xC5,
    ProcessorFamilyIntelCoreI7 = 0xC6,
    ProcessorFamilyDualCoreIntelCeleron = 0xC7,
    ProcessorFamilyIBM390 = 0xC8,
    ProcessorFamilyG4 = 0xC9,
    ProcessorFamilyG5 = 0xCA,
    ProcessorFamilyG6 = 0xCB,
    ProcessorFamilyzArchitecture = 0xCC,
    ProcessorFamilyIntelCoreI5 = 0xCD,
    ProcessorFamilyIntelCoreI3 = 0xCE,
    ProcessorFamilyIntelCoreI9 = 0xCF,
    ProcessorFamilyViaC7M = 0xD2,
    ProcessorFamilyViaC7D = 0xD3,
    ProcessorFamilyViaC7 = 0xD4,
    ProcessorFamilyViaEden = 0xD5,
    ProcessorFamilyMultiCoreIntelXeon = 0xD6,
    ProcessorFamilyDualCoreIntelXeon3Series = 0xD7,
    ProcessorFamilyQuadCoreIntelXeon3Series = 0xD8,
    ProcessorFamilyViaNano = 0xD9,
    ProcessorFamilyDualCoreIntelXeon5Series = 0xDA,
    ProcessorFamilyQuadCoreIntelXeon5Series = 0xDB,
    ProcessorFamilyDualCoreIntelXeon7Series = 0xDD,
    ProcessorFamilyQuadCoreIntelXeon7Series = 0xDE,
    ProcessorFamilyMultiCoreIntelXeon7Series = 0xDF,
    ProcessorFamilyMultiCoreIntelXeon3400Series = 0xE0,
    ProcessorFamilyAmdOpteron3000Series = 0xE4,
    ProcessorFamilyAmdSempronII = 0xE5,
    ProcessorFamilyEmbeddedAmdOpteronQuadCore = 0xE6,
    ProcessorFamilyAmdPhenomTripleCore = 0xE7,
    ProcessorFamilyAmdTurionUltraDualCoreMobile = 0xE8,
    ProcessorFamilyAmdTurionDualCoreMobile = 0xE9,
    ProcessorFamilyAmdAthlonDualCore = 0xEA,
    ProcessorFamilyAmdSempronSI = 0xEB,
    ProcessorFamilyAmdPhenomII = 0xEC,
    ProcessorFamilyAmdAthlonII = 0xED,
    ProcessorFamilySixCoreAmdOpteron = 0xEE,
    ProcessorFamilyAmdSempronM = 0xEF,
    ProcessorFamilyi860 = 0xFA,
    ProcessorFamilyi960 = 0xFB,
    ProcessorFamilyIndicatorFamily2 = 0xFE,
    ProcessorFamilyReserved1 = 0xFF,
}

///
/// Processor Information2 - Processor Family2.
///
pub enum ProcessorFamily2Data {
    ProcessorFamilyARMv7 = 0x0100,
    ProcessorFamilyARMv8 = 0x0101,
    ProcessorFamilyARMv9 = 0x0102,
    ProcessorFamilySH3 = 0x0104,
    ProcessorFamilySH4 = 0x0105,
    ProcessorFamilyARM = 0x0118,
    ProcessorFamilyStrongARM = 0x0119,
    ProcessorFamily6x86 = 0x012C,
    ProcessorFamilyMediaGX = 0x012D,
    ProcessorFamilyMII = 0x012E,
    ProcessorFamilyWinChip = 0x0140,
    ProcessorFamilyDSP = 0x015E,
    ProcessorFamilyVideoProcessor = 0x01F4,
    ProcessorFamilyRiscvRV32 = 0x0200,
    ProcessorFamilyRiscVRV64 = 0x0201,
    ProcessorFamilyRiscVRV128 = 0x0202,
    ProcessorFamilyLoongArch = 0x0258,
    ProcessorFamilyLoongson1 = 0x0259,
    ProcessorFamilyLoongson2 = 0x025A,
    ProcessorFamilyLoongson3 = 0x025B,
    ProcessorFamilyLoongson2K = 0x025C,
    ProcessorFamilyLoongson3A = 0x025D,
    ProcessorFamilyLoongson3B = 0x025E,
    ProcessorFamilyLoongson3C = 0x025F,
    ProcessorFamilyLoongson3D = 0x0260,
    ProcessorFamilyLoongson3E = 0x0261,
    ProcessorFamilyDualCoreLoongson2K = 0x0262,
    ProcessorFamilyQuadCoreLoongson3A = 0x026C,
    ProcessorFamilyMultiCoreLoongson3A = 0x026D,
    ProcessorFamilyQuadCoreLoongson3B = 0x026E,
    ProcessorFamilyMultiCoreLoongson3B = 0x026F,
    ProcessorFamilyMultiCoreLoongson3C = 0x0270,
    ProcessorFamilyMultiCoreLoongson3D = 0x0271,
    ProcessorFamilyIntelCore3 = 0x0300,
    ProcessorFamilyIntelCore5 = 0x0301,
    ProcessorFamilyIntelCore7 = 0x0302,
    ProcessorFamilyIntelCore9 = 0x0303,
    ProcessorFamilyIntelCoreUltra3 = 0x0304,
    ProcessorFamilyIntelCoreUltra5 = 0x0305,
    ProcessorFamilyIntelCoreUltra7 = 0x0306,
    ProcessorFamilyIntelCoreUltra9 = 0x0307,
}

///
/// Processor Information - Voltage.
///
bitfield! {
    pub struct ProcessorVoltage (u8);
    impl Debug;
    pub processor_voltage_capability_5v, set_processor_voltage_capability_5v: 0;
    pub processor_voltage_capability_3_3v, set_processor_voltage_capability_3_3v: 1;
    pub processor_voltage_capability_2_9v, set_processor_voltage_capability_2_9v: 2;
    pub processor_voltage_capability_reserved, set_processor_voltage_capability_reserved: 3;
    pub processor_voltage_reserved, set_processor_voltage_reserved: 6, 4;
    pub processor_voltage_indicate_legacy, set_processor_voltage_indicate_legacy: 7;
}

///
/// Processor Information - Processor Upgrade.
///
pub enum ProcessorUpgrade {
    Other = 0x01,
    Unknown = 0x02,
    DaughterBoard = 0x03,
    ZIFSocket = 0x04,
    PiggyBack = 0x05, //  Replaceable.
    None = 0x06,
    LIFSocket = 0x07,
    Slot1 = 0x08,
    Slot2 = 0x09,
    Pin370Socket = 0x0A,
    SlotA = 0x0B,
    SlotM = 0x0C,
    Socket423 = 0x0D,
    SocketA = 0x0E, //  Socket 462.
    Socket478 = 0x0F,
    Socket754 = 0x10,
    Socket940 = 0x11,
    Socket939 = 0x12,
    SocketmPGA604 = 0x13,
    SocketLGA771 = 0x14,
    SocketLGA775 = 0x15,
    SocketS1 = 0x16,
    AM2 = 0x17,
    F1207 = 0x18,
    SocketLGA1366 = 0x19,
    SocketG34 = 0x1A,
    SocketAM3 = 0x1B,
    SocketC32 = 0x1C,
    SocketLGA1156 = 0x1D,
    SocketLGA1567 = 0x1E,
    SocketPGA988A = 0x1F,
    SocketBGA1288 = 0x20,
    SocketrPGA988B = 0x21,
    SocketBGA1023 = 0x22,
    SocketBGA1224 = 0x23,
    SocketLGA1155 = 0x24, //  SMBIOS spec 2.8.0 updated the name
    SocketLGA1356 = 0x25,
    SocketLGA2011 = 0x26,
    SocketFS1 = 0x27,
    SocketFS2 = 0x28,
    SocketFM1 = 0x29,
    SocketFM2 = 0x2A,
    SocketLGA2011_3 = 0x2B,
    SocketLGA1356_3 = 0x2C,
    SocketLGA1150 = 0x2D,
    SocketBGA1168 = 0x2E,
    SocketBGA1234 = 0x2F,
    SocketBGA1364 = 0x30,
    SocketAM4 = 0x31,
    SocketLGA1151 = 0x32,
    SocketBGA1356 = 0x33,
    SocketBGA1440 = 0x34,
    SocketBGA1515 = 0x35,
    SocketLGA3647_1 = 0x36,
    SocketSP3 = 0x37,
    SocketSP3r2 = 0x38,
    SocketLGA2066 = 0x39,
    SocketBGA1392 = 0x3A,
    SocketBGA1510 = 0x3B,
    SocketBGA1528 = 0x3C,
    SocketLGA4189 = 0x3D,
    SocketLGA1200 = 0x3E,
    SocketLGA4677 = 0x3F,
    SocketLGA1700 = 0x40,
    SocketBGA1744 = 0x41,
    SocketBGA1781 = 0x42,
    SocketBGA1211 = 0x43,
    SocketBGA2422 = 0x44,
    SocketLGA1211 = 0x45,
    SocketLGA2422 = 0x46,
    SocketLGA5773 = 0x47,
    SocketBGA5773 = 0x48,
    SocketAM5 = 0x49,
    SocketSP5 = 0x4A,
    SocketSP6 = 0x4B,
    SocketBGA883 = 0x4C,
    SocketBGA1190 = 0x4D,
    SocketBGA4129 = 0x4E,
    SocketLGA4710 = 0x4F,
    SocketLGA7529 = 0x50,
    SocketBGA1964 = 0x51,
    SocketBGA1792 = 0x52,
    SocketBGA2049 = 0x53,
    SocketBGA2551 = 0x54,
    SocketLGA1851 = 0x55,
    SocketBGA2114 = 0x56,
    SocketBGA2833 = 0x57,
}

///
/// Processor ID Field Description
///
bitfield! {
    pub struct ProcessorSignature (u32);
    impl Debug;
    pub processor_stepping_id, set_processor_stepping_id: 3, 0;
    pub processor_model, set_processor_model: 7, 4;
    pub processor_family, set_processor_family: 11, 8;
    pub processor_type, set_processor_type: 13, 12;
    pub processor_reserved1, set_processor_reserved1: 15, 14;
    pub processor_x_model, set_processor_x_model: 19, 16;
    pub processor_x_family, set_processor_x_family: 27, 20;
    pub processor_reserved2, set_processor_reserved2: 31, 28;
}

// PROCESSOR_FEATURE_FLAGS
bitfield! {
    pub struct ProcessorFeatureFlags (u32);
    impl Debug;
    pub processor_fpu, set_processor_fpu: 0;
    pub processor_vme, set_processor_vme: 1;
    pub processor_de, set_processor_de: 2;
    pub processor_pse, set_processor_pse: 3;
    pub processor_tsc, set_processor_tsc: 4;
    pub processor_msr, set_processor_msr: 5;
    pub processor_pae, set_processor_pae: 6;
    pub processor_mce, set_processor_mce: 7;
    pub processor_cx8, set_processor_cx8: 8;
    pub processor_apic, set_processor_apic: 9;
    pub processor_reserved1, set_processor_reserved1: 10;
    pub processor_sep, set_processor_sep: 11;
    pub processor_mtrr, set_processor_mtrr: 12;
    pub processor_pge, set_processor_pge: 13;
    pub processor_mca, set_processor_mca: 14;
    pub processor_cmov, set_processor_cmov: 15;
    pub processor_pat, set_processor_pat: 16;
    pub processor_pse36, set_processor_pse36: 17;
    pub processor_psn, set_processor_psn: 18;
    pub processor_clfsh, set_processor_clfsh: 19;
    pub processor_reserved2, set_processor_reserved2: 20;
    pub processor_ds, set_processor_ds: 21;
    pub processor_acpi, set_processor_acpi: 22;
    pub processor_mmx, set_processor_mmx: 23;
    pub processor_fxsr, set_processor_fxsr: 24;
    pub processor_sse, set_processor_sse: 25;
    pub processor_sse2, set_processor_sse2: 26;
    pub processor_ss, set_processor_ss: 27;
    pub processor_reserved3, set_processor_reserved3: 28;
    pub processor_tm, set_processor_tm: 29;
    pub processor_reserved4, set_processor_reserved4: 31, 30;
}

// PROCESSOR_CHARACTERISTIC_FLAGS
bitfield! {
    pub struct ProcessorCharacteristicFlags (u16);
    impl Debug;
    pub processor_reserved1, set_processor_reserved1: 0;
    pub processor_unknown, set_processor_unknown: 1;
    pub processor_64bit_capable, set_processor_64bit_capable: 2;
    pub processor_multi_core, set_processor_multi_core: 3;
    pub processor_hardware_thread, set_processor_hardware_thread: 4;
    pub processor_execute_protection, set_processor_execute_protection: 5;
    pub processor_enhanced_virtualization, set_processor_enhanced_virtualization: 6;
    pub processor_power_performance_ctrl, set_processor_power_performance_ctrl: 7;
    pub processor_128bit_capable, set_processor_128bit_capable: 8;
    pub processor_arm64_soc_id, set_processor_arm64_soc_id: 9;
    pub processor_reserved2, set_processor_reserved2: 15, 10;
}

///
/// Processor Information - Status
///
bitfield! {
    pub struct ProcessorStatusBits(u8);
    impl Debug;
    pub cpu_status, set_cpu_status: 2, 0;       //< Indicates the status of the processor.
    pub reserved1, set_reserved1: 5, 3;        //< Reserved for future use. Must be set to zero.
    pub socket_populated, set_socket_populated: 6; //< Indicates if the processor socket is populated or not.
    pub reserved2, set_reserved2: 7;        //< Reserved for future use. Must be set to zero.
}

pub union ProcessorStatusData {
    pub bits: ProcessorStatusBits,
    pub data: u8,
}

#[repr(C, packed)]
pub struct ProcessorIdData {
    pub signature: ProcessorSignature,
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
#[repr(C, packed)]
pub struct SmbiosTableType4 {
    pub hdr: SmbiosStructure,
    pub socket: SmbiosTableString,
    pub processor_type: u8, //  The enumeration value from PROCESSOR_TYPE_DATA.
    pub processor_family: u8, //  The enumeration value from PROCESSOR_FAMILY_DATA.
    pub processor_manufacturer: SmbiosTableString,
    pub processor_id: ProcessorIdData,
    pub processor_version: SmbiosTableString,
    pub voltage: ProcessorVoltage,
    pub external_clock: u16,
    pub max_speed: u16,
    pub current_speed: u16,
    pub status: u8,
    pub processor_upgrade: u8, //  The enumeration value from PROCESSOR_UPGRADE.
    pub l1_cache_handle: u16,
    pub l2_cache_handle: u16,
    pub l3_cache_handle: u16,
    pub serial_number: SmbiosTableString,
    pub asset_tag: SmbiosTableString,
    pub part_number: SmbiosTableString,
    //
    // Add for smbios 2.5
    //
    pub core_count: u8,
    pub enabled_core_count: u8,
    pub thread_count: u8,
    pub processor_characteristics: u16,
    //
    // Add for smbios 2.6
    //
    pub processor_family2: u16,
    //
    // Add for smbios 3.0
    //
    pub core_count2: u16,
    pub enabled_core_count2: u16,
    pub thread_count2: u16,
    //
    // Add for smbios 3.6
    //
    pub thread_enabled: u16,
    //
    // Add for smbios 3.8
    //
    pub socket_type: SmbiosTableString,
}

///
/// Memory Controller Error Detecting Method.
///
pub enum MemoryErrorDetectMethod {
    Other = 0x01,
    Unknown = 0x02,
    None = 0x03,
    Parity = 0x04,
    Ecc32 = 0x05,
    Ecc64 = 0x06,
    Ecc128 = 0x07,
    Crc = 0x08,
}

///
/// Memory Controller Error Correcting Capability.
///
bitfield! {
    pub struct MemoryErrorCorrectCapability(u8);
    impl Debug;
    pub other, set_other: 0;
    pub unknown, set_unknown: 1;
    pub none, set_none: 2;
    pub single_bit_error_correct, set_single_bit_error_correct: 3;
    pub double_bit_error_correct, set_double_bit_error_correct: 4;
    pub error_scrubbing, set_error_scrubbing: 5;
    pub reserved, set_reserved: 7, 6;
}

///
/// Memory Controller Information - Interleave Support.
///
pub enum MemorySupportInterleaveType {
    MemoryInterleaveOther = 0x01,
    MemoryInterleaveUnknown = 0x02,
    MemoryInterleaveOneWay = 0x03,
    MemoryInterleaveTwoWay = 0x04,
    MemoryInterleaveFourWay = 0x05,
    MemoryInterleaveEightWay = 0x06,
    MemoryInterleaveSixteenWay = 0x07,
}

///
/// Memory Controller Information - Memory Speeds.
///
bitfield! {
    pub struct MemorySpeedType(u16);
    impl Debug;
    pub other, set_other: 0;
    pub unknown, set_unknown: 1;
    pub seventy_ns, set_seventy_ns: 2;
    pub sixty_ns, set_sixty_ns: 3;
    pub fifty_ns, set_fifty_ns: 4;
    pub reserved, set_reserved: 15, 5;
}

///
/// Memory Module Information - Memory Types
///
bitfield! {
    pub struct MemoryCurrentType(u16);
    impl Debug;
    pub other, set_other: 0,
    pub unknown, set_unknown: 1,
    pub standard:, set_standard 2,
    pub fast_page_mode, set_fast_page_mode: 3,
    pub edo, set_edo: 4,
    pub parity, set_parity: 5,
    pub ecc, set_ecc: 6,
    pub simm, set_simm: 7,
    pub dimm, set_dimm: 8,
    pub burst_edo, set_burst_edo: 9,
    pub sdram, set_sdram: 10,
    pub reserved, set_reserved: 15, 11,
}

///
/// Memory Module Information - Memory Size.
///
bitfield! {
    pub struct MemoryInstalledEnabledSize(u8);
    impl Debug;
    pub installed_or_enabled_size, set_installed_or_enabled_size: 0, 6;
    pub single_or_double_bank, set_single_or_double_bank: 7;
}

///
/// Cache Information - SRAM Type.
///
bitfield! {
    pub struct CacheSramTypeData(u16);
    impl Debug;
    pub other, set_other: 0;
    pub unknown, set_unknown: 1;
    pub non_burst, set_non_burst: 2;
    pub burst, set_burst: 3;
    pub pipeline_burst, set_pipeline_burst: 4;
    pub synchronous, set_synchronous: 5;
    pub asynchronous, set_asynchronous: 6;
    pub reserved, set_reserved: 15, 7;
}

///
/// Cache Information - Error Correction Type.
///
pub enum CacheErrorTypeData {
    CacheErrorOther = 0x01,
    CacheErrorUnknown = 0x02,
    CacheErrorNone = 0x03,
    CacheErrorParity = 0x04,
    CacheErrorSingleBit = 0x05, // ECC
    CacheErrorMultiBit = 0x06,  // ECC
}

///
/// Cache Information - System Cache Type.
///
pub enum CacheTypeData {
    CacheTypeOther = 0x01,
    CacheTypeUnknown = 0x02,
    CacheTypeInstruction = 0x03,
    CacheTypeData = 0x04,
    CacheTypeUnified = 0x05,
}

///
/// Cache Information - Associativity.
///
pub enum CacheAssociativityData {
    CacheAssociativityOther = 0x01,
    CacheAssociativityUnknown = 0x02,
    CacheAssociativityDirectMapped = 0x03,
    CacheAssociativityWay2 = 0x04,
    CacheAssociativityWay4 = 0x05,
    CacheAssociativityFully = 0x06,
    CacheAssociativityWay8 = 0x07,
    CacheAssociativityWay16 = 0x08,
    CacheAssociativityWay12 = 0x09,
    CacheAssociativityWay24 = 0x0A,
    CacheAssociativityWay32 = 0x0B,
    CacheAssociativityWay48 = 0x0C,
    CacheAssociativityWay64 = 0x0D,
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
    pub hdr: SmbiosStructure,
    pub socket_designation: SmbiosTableString,
    pub cache_configuration: u16,
    pub maximum_cache_size: u16,
    pub installed_size: u16,
    pub supported_sram_type: CacheSramTypeData,
    pub current_sram_type: CacheSramTypeData,
    pub cache_speed: u8,
    pub error_correction_type: u8, //  The enumeration value from CACHE_ERROR_TYPE_DATA.
    pub system_cache_type: u8,     //  The enumeration value from CACHE_TYPE_DATA.
    pub associativity: u8,         //  The enumeration value from CACHE_ASSOCIATIVITY_DATA.
    //
    // Add for smbios 3.1.0
    //
    pub maximum_cache_size2: u32,
    pub installed_size2: u32,
}

///
/// Port Connector Information - Connector Types.
///
pub enum MiscPortConnectorType {
    PortConnectorTypeNone = 0x00,
    PortConnectorTypeCentronics = 0x01,
    PortConnectorTypeMiniCentronics = 0x02,
    PortConnectorTypeProprietar = 0x03,
    PortConnectorTypeDB25Male = 0x04,
    PortConnectorTypeDB25Female = 0x05,
    PortConnectorTypeDB15Male = 0x06,
    PortConnectorTypeDB15Female = 0x07,
    PortConnectorTypeDB9Male = 0x08,
    PortConnectorTypeDB9Female = 0x09,
    PortConnectorTypeRJ11 = 0x0A,
    PortConnectorTypeRJ45 = 0x0B,
    PortConnectorType50PinMiniScsi = 0x0C,
    PortConnectorTypeMiniDin = 0x0D,
    PortConnectorTypeMicroDin = 0x0E,
    PortConnectorTypePS2 = 0x0F,
    PortConnectorTypeInfrared = 0x10,
    PortConnectorTypeHpHil = 0x11,
    PortConnectorTypeUsb = 0x12,
    PortConnectorTypeSsaScsi = 0x13,
    PortConnectorTypeCircularDin8Male = 0x14,
    PortConnectorTypeCircularDin8Female = 0x15,
    PortConnectorTypeOnboardIde = 0x16,
    PortConnectorTypeOnboardFloppy = 0x17,
    PortConnectorType9PinDualInline = 0x18,
    PortConnectorType25PinDualInline = 0x19,
    PortConnectorType50PinDualInline = 0x1A,
    PortConnectorType68PinDualInline = 0x1B,
    PortConnectorTypeOnboardSoundInput = 0x1C,
    PortConnectorTypeMiniCentronicsType14 = 0x1D,
    PortConnectorTypeMiniCentronicsType26 = 0x1E,
    PortConnectorTypeHeadPhoneMiniJack = 0x1F,
    PortConnectorTypeBNC = 0x20,
    PortConnectorType1394 = 0x21,
    PortConnectorTypeSasSata = 0x22,
    PortConnectorTypeUsbTypeC = 0x23,
    PortConnectorTypePC98 = 0xA0,
    PortConnectorTypePC98Hireso = 0xA1,
    PortConnectorTypePCH98 = 0xA2,
    PortConnectorTypePC98Note = 0xA3,
    PortConnectorTypePC98Full = 0xA4,
    PortConnectorTypeOther = 0xFF,
}

///
/// Port Connector Information - Port Types
///
#[repr(u8)]
pub enum MiscPortType {
    None = 0x00,
    ParallelXtAtCompatible = 0x01,
    ParallelPortPs2 = 0x02,
    ParallelPortEcp = 0x03,
    ParallelPortEpp = 0x04,
    ParallelPortEcpEpp = 0x05,
    SerialXtAtCompatible = 0x06,
    Serial16450Compatible = 0x07,
    Serial16550Compatible = 0x08,
    Serial16550ACompatible = 0x09,
    Scsi = 0x0A,
    Midi = 0x0B,
    JoyStick = 0x0C,
    Keyboard = 0x0D,
    Mouse = 0x0E,
    SsaScsi = 0x0F,
    Usb = 0x10,
    FireWire = 0x11,
    PcmciaTypeI = 0x12,
    PcmciaTypeII = 0x13,
    PcmciaTypeIII = 0x14,
    CardBus = 0x15,
    AccessBusPort = 0x16,
    ScsiII = 0x17,
    ScsiWide = 0x18,
    Pc98 = 0x19,
    Pc98Hireso = 0x1A,
    Pch98 = 0x1B,
    VideoPort = 0x1C,
    AudioPort = 0x1D,
    ModemPort = 0x1E,
    NetworkPort = 0x1F,
    Sata = 0x20,
    Sas = 0x21,
    Mfdp = 0x22, // < Multi-Function Display Port
    Thunderbolt = 0x23,
    Compatible8251 = 0xA0,
    Compatible8251Fifo = 0xA1,
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
    pub hdr: SmbiosStructure,
    pub internal_reference_designator: SmbiosTableString,
    pub internal_connector_type: u8, // < The enumeration value from MISC_PORT_CONNECTOR_TYPE.
    pub external_reference_designator: SmbiosTableString,
    pub external_connector_type: u8, // < The enumeration value from MISC_PORT_CONNECTOR_TYPE.
    pub port_type: u8,               // < The enumeration value from MISC_PORT_TYPE.
}

///
/// System Slots - Slot Type
///
#[repr(u8)]
pub enum MiscSlotType {
    Other = 0x01,
    Unknown = 0x02,
    Isa = 0x03,
    Mca = 0x04,
    Eisa = 0x05,
    Pci = 0x06,
    Pcmcia = 0x07,
    VlVesa = 0x08,
    Proprietary = 0x09,
    ProcessorCardSlot = 0x0A,
    ProprietaryMemoryCardSlot = 0x0B,
    IORiserCardSlot = 0x0C,
    NuBus = 0x0D,
    Pci66MhzCapable = 0x0E,
    Agp = 0x0F,
    Apg2X = 0x10,
    Agp4X = 0x11,
    PciX = 0x12,
    Agp8X = 0x13,
    M2Socket1Dp = 0x14,
    M2Socket1Sd = 0x15,
    M2Socket2 = 0x16,
    M2Socket3 = 0x17,
    MxmTypeI = 0x18,
    MxmTypeII = 0x19,
    MxmTypeIIIStandard = 0x1A,
    MxmTypeIIIHe = 0x1B,
    MxmTypeIV = 0x1C,
    Mxm30TypeA = 0x1D,
    Mxm30TypeB = 0x1E,
    PciExpressGen2Sff8639 = 0x1F,
    PciExpressGen3Sff8639 = 0x20,
    PciExpressMini52pinWithBsko = 0x21, // < PCI Express Mini 52-pin (CEM spec. 2.0) with bottom-side keep-outs.
    PciExpressMini52pinWithoutBsko = 0x22, // < PCI Express Mini 52-pin (CEM spec. 2.0) without bottom-side keep-outs.
    PciExpressMini76pin = 0x23, // < PCI Express Mini 76-pin (CEM spec. 2.0) Corresponds to Display-Mini card.
    PciExpressGen4Sff8639 = 0x24, // < U.2
    PciExpressGen5Sff8639 = 0x25, // < U.2
    OcpNic30SmallFormFactor = 0x26, // < SFF
    OcpNic30LargeFormFactor = 0x27, // < LFF
    OcpNicPriorto30 = 0x28,
    CxlFlexbus10 = 0x30,
    Pc98C20 = 0xA0,
    Pc98C24 = 0xA1,
    Pc98E = 0xA2,
    Pc98LocalBus = 0xA3,
    Pc98Card = 0xA4,
    PciExpress = 0xA5,
    PciExpressX1 = 0xA6,
    PciExpressX2 = 0xA7,
    PciExpressX4 = 0xA8,
    PciExpressX8 = 0xA9,
    PciExpressX16 = 0xAA,
    PciExpressGen2 = 0xAB,
    PciExpressGen2X1 = 0xAC,
    PciExpressGen2X2 = 0xAD,
    PciExpressGen2X4 = 0xAE,
    PciExpressGen2X8 = 0xAF,
    PciExpressGen2X16 = 0xB0,
    PciExpressGen3 = 0xB1,
    PciExpressGen3X1 = 0xB2,
    PciExpressGen3X2 = 0xB3,
    PciExpressGen3X4 = 0xB4,
    PciExpressGen3X8 = 0xB5,
    PciExpressGen3X16 = 0xB6,
    PciExpressGen4 = 0xB8,
    PciExpressGen4X1 = 0xB9,
    PciExpressGen4X2 = 0xBA,
    PciExpressGen4X4 = 0xBB,
    PciExpressGen4X8 = 0xBC,
    PciExpressGen4X16 = 0xBD,
    PciExpressGen5 = 0xBE,
    PciExpressGen5X1 = 0xBF,
    PciExpressGen5X2 = 0xC0,
    PciExpressGen5X4 = 0xC1,
    PciExpressGen5X8 = 0xC2,
    PciExpressGen5X16 = 0xC3,
    PciExpressGen6andBeyond = 0xC4,
    EnterpriseandDatacenter1UE1FormFactorSlot = 0xC5,
    EnterpriseandDatacenter3E3FormFactorSlot = 0xC6,
}

///
/// System Slots - Slot Data Bus Width.
///
#[repr(u8)]
pub enum MiscSlotDataBusWidth {
    Other = 0x01,
    Unknown = 0x02,
    Width8Bit = 0x03,
    Width16Bit = 0x04,
    Width32Bit = 0x05,
    Width64Bit = 0x06,
    Width128Bit = 0x07,
    Width1X = 0x08,  // < Or X1
    Width2X = 0x09,  // < Or X2
    Width4X = 0x0A,  // < Or X4
    Width8X = 0x0B,  // < Or X8
    Width12X = 0x0C, // < Or X12
    Width16X = 0x0D, // < Or X16
    Width32X = 0x0E, // < Or X32
}

///
/// System Slots - Slot Physical Width.
///
#[repr(u8)]
pub enum MiscSlotPhysicalWidth {
    Other = 0x01,
    Unknown = 0x02,
    Width8Bit = 0x03,
    Width16Bit = 0x04,
    Width32Bit = 0x05,
    Width64Bit = 0x06,
    Width128Bit = 0x07,
    Width1X = 0x08,  // < Or X1
    Width2X = 0x09,  // < Or X2
    Width4X = 0x0A,  // < Or X4
    Width8X = 0x0B,  // < Or X8
    Width12X = 0x0C, // < Or X12
    Width16X = 0x0D, // < Or X16
    Width32X = 0x0E, // < Or X32
}

///
/// System Slots - Slot Information.
///
#[repr(u8)]
pub enum MiscSlotInformation {
    Others = 0x00,
    Gen1 = 0x01,
    Gen2 = 0x02, //AT: originally 0x01
    Gen3 = 0x03,
    Gen4 = 0x04,
    Gen5 = 0x05,
    Gen6 = 0x06,
}

///
/// System Slots - Current Usage.
///
#[repr(u8)]
pub enum MiscSlotUsage {
    Other = 0x01,
    Unknown = 0x02,
    Available = 0x03,
    InUse = 0x04,
    Unavailable = 0x05,
}

///
/// System Slots - Slot Length.
///
#[repr(u8)]
pub enum MiscSlotLength {
    Other = 0x01,
    Unknown = 0x02,
    Short = 0x03,
    Long = 0x04,
}

///
/// System Slots - Slot Characteristics 1.
///
bitfield! {
    pub struct MiscSlotCharacteristics1(u8);
    impl Debug;
    pub characteristics_unknown, set_characteristics_unknown: 0;
    pub provides_50_volts, set_provides_50_volts: 1;
    pub provides_33_volts, set_provides_33_volts: 2;
    pub shared_slot, set_shared_slot: 3;
    pub pc_card_16_supported, set_pc_card_16_supported: 4;
    pub card_bus_supported, set_card_bus_supported: 5;
    pub zoom_video_supported, set_zoom_video_supported: 6;
    pub modem_ring_resume_supported, set_modem_ring_resume_supported: 7;
}

///
/// System Slots - Slot Characteristics 2.
///
bitfield! {
    pub struct MiscSlotCharacteristics2(u8);
    impl Debug;
    pub pme_signal_supported, set_pme_signal_supported: 0;
    pub hot_plug_devices_supported, set_hot_plug_devices_supported: 1;
    pub smbus_signal_supported, sset_mbus_signal_supported: 2;
    pub bifurcation_supported, set_bifurcation_supported: 3;
    pub async_surprise_removal, set_async_surprise_removal: 4;
    pub flexbus_slot_cxl_10_capable, set_flexbus_slot_cxl_10_capable: 5;
    pub flexbus_slot_cxl_20_capable, set_flexbus_slot_cxl_20_capable: 6;
    pub flexbus_slot_cxl_30_capable, set_flexbus_slot_cxl_30_capable: 7; //  SMBIOS spec 3.7.0 updated CXL 3.0 support
}

///
/// System Slots - Slot Height
///
#[repr(u8)]
pub enum MiscSlotHeight {
    None = 0x00,
    Other = 0x01,
    Unknown = 0x02,
    FullHeight = 0x03,
    LowProfile = 0x04,
}

///
/// System Slots - Peer Segment/Bus/Device/Function/Width Groups
///
#[repr(C, packed)]
pub struct MiscSlotPeerGroup {
    pub segment_group_num: u16,
    pub bus_num: u8,
    pub dev_func_num: u8,
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
    pub hdr: SmbiosStructure,
    pub slot_designation: SmbiosTableString,
    pub slot_type: u8,           // < The enumeration value from MISC_SLOT_TYPE.
    pub slot_data_bus_width: u8, // < The enumeration value from MISC_SLOT_DATA_BUS_WIDTH.
    pub current_usage: u8,       // < The enumeration value from MISC_SLOT_USAGE.
    pub slot_length: u8,         // < The enumeration value from MISC_SLOT_LENGTH.
    pub slot_id: u16,
    pub slot_characteristics1: MiscSlotCharacteristics1,
    pub slot_characteristics2: MiscSlotCharacteristics2,
    //
    // Add for smbios 2.6
    //
    pub segment_group_num: u16,
    pub bus_num: u8,
    pub dev_func_num: u8,
    //
    // Add for smbios 3.2
    //
    pub data_bus_width: u8,
    pub peer_grouping_count: u8,
    pub peer_groups: [MiscSlotPeerGroup; 1],
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
    pub slot_information: u8,
    pub slot_physical_width: u8,
    pub slot_pitch: u16,
    //
    // Add for smbios 3.5
    //
    pub slot_height: u8, // < The enumeration value from MISC_SLOT_HEIGHT.
}

///
/// On Board Devices Information - Device Types.
///
#[repr(u8)]
pub enum MiscOnboardDeviceType {
    Other = 0x01,
    Unknown = 0x02,
    Video = 0x03,
    ScsiController = 0x04,
    Ethernet = 0x05,
    TokenRing = 0x06,
    Sound = 0x07,
    PataController = 0x08,
    SataController = 0x09,
    SasController = 0x0A,
}

///
/// Device Item Entry
///
#[repr(C, packed)]
pub struct DeviceStruct {
    pub device_type: u8, // < Bit [6:0] - enumeration type of device from MISC_ONBOARD_DEVICE_TYPE.
    // < Bit 7     - 1 : device enabled, 0 : device disabled.
    pub description_string: SmbiosTableString,
}

///
/// OEM Strings (Type 11).
/// This structure contains free form strings defined by the OEM. Examples of this are:
/// Part Numbers for Reference Documents for the system, contact information for the manufacturer, etc.
///
#[repr(C, packed)]
pub struct SmbiosTableType11 {
    pub hdr: SmbiosStructure,
    pub string_count: u8,
}

///
/// System Configuration Options (Type 12).
///
/// This structure contains information required to configure the base board's Jumpers and Switches.
///
#[repr(C, packed)]
pub struct SmbiosTableType12 {
    pub hdr: SmbiosStructure,
    pub string_count: u8,
}

///
/// BIOS Language Information (Type 13).
///
/// The information in this structure defines the installable language attributes of the BIOS.
///
#[repr(C, packed)]
pub struct SmbiosTableType13 {
    pub hdr: SmbiosStructure,
    pub installable_languages: u8,
    pub flags: u8,
    pub reserved: [u8; 15],
    pub current_languages: SmbiosTableString,
}

///
/// Group Item Entry
///
#[repr(C, packed)]
pub struct GroupStruct {
    pub item_type: u8,
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
pub struct SmbiosTableType14 {
    pub hdr: SmbiosStructure,
    pub group_name: SmbiosTableString,
    pub group: [GroupStruct; 1],
}

///
/// System Event Log - Event Log Types.
///
#[repr(u8)]
pub enum EventLogTypeData {
    Reserved = 0x00,
    SingleBitEcc = 0x01,
    MultiBitEcc = 0x02,
    ParityMemErr = 0x03,
    BusTimeOut = 0x04,
    IoChannelCheck = 0x05,
    SoftwareNmi = 0x06,
    PostMemResize = 0x07,
    PostErr = 0x08,
    PciParityErr = 0x09,
    PciSystemErr = 0x0A,
    CpuFailure = 0x0B,
    EisaTimeOut = 0x0C,
    MemLogDisabled = 0x0D,
    LoggingDisabled = 0x0E,
    SysLimitExce = 0x10,
    AsyncHwTimer = 0x11,
    SysConfigInfo = 0x12,
    HdInfo = 0x13,
    SysReconfig = 0x14,
    UncorrectCpuErr = 0x15,
    AreaResetAndClr = 0x16,
    SystemBoot = 0x17,
    Unused = 0x18,      // < 0x18 - 0x7F
    AvailForSys = 0x80, // < 0x80 - 0xFE
    EndOfLog = 0xFF,
}

///
/// System Event Log - Variable Data Format Types.
///
#[repr(u8)]
pub enum EventLogVariableData {
    None = 0x00,
    Handle = 0x01,
    MutilEvent = 0x02,
    MutilEventHandle = 0x03,
    PostResultBitmap = 0x04,
    SysManagementType = 0x05,
    MutliEventSysManagmentType = 0x06,
    Unused = 0x07,
    OemAssigned = 0x80,
}

///
/// Event Log Type Descriptors
///
#[repr(C, packed)]
pub struct EventLogType {
    pub log_type: u8, // < The enumeration value from EVENT_LOG_TYPE_DATA.
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
    pub hdr: SmbiosStructure,
    pub log_area_length: u16,
    pub log_header_start_offset: u16,
    pub log_data_start_offset: u16,
    pub access_method: u8,
    pub log_status: u8,
    pub log_change_token: u32,
    pub access_method_address: u32,
    pub log_header_format: u8,
    pub number_of_supported_log_type_descriptors: u8,
    pub length_of_log_type_descriptor: u8,
    pub event_log_type_descriptors: [EventLogType; 1],
}

///
/// Physical Memory Array - Location.
///
#[repr(u8)]
pub enum MemoryArrayLocation {
    Other = 0x01,
    Unknown = 0x02,
    SystemBoard = 0x03,
    IsaAddonCard = 0x04,
    EisaAddonCard = 0x05,
    PciAddonCard = 0x06,
    McaAddonCard = 0x07,
    PcmciaAddonCard = 0x08,
    ProprietaryAddonCard = 0x09,
    NuBus = 0x0A,
    Pc98C20AddonCard = 0xA0,
    Pc98C24AddonCard = 0xA1,
    Pc98EAddonCard = 0xA2,
    Pc98LocalBusAddonCard = 0xA3,
    CxlAddonCard = 0xA4,
}

///
/// Physical Memory Array - Use.
///
#[repr(u8)]
pub enum MemoryArrayUse {
    Other = 0x01,
    Unknown = 0x02,
    SystemMemory = 0x03,
    VideoMemory = 0x04,
    FlashMemory = 0x05,
    NonVolatileRam = 0x06,
    CacheMemory = 0x07,
}

///
/// Physical Memory Array - Error Correction Types.
///
#[repr(u8)]
pub enum MemoryErrorCorrection {
    Other = 0x01,
    Unknown = 0x02,
    None = 0x03,
    Parity = 0x04,
    SingleBitEcc = 0x05,
    MultiBitEcc = 0x06,
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
    pub hdr: SmbiosStructure,
    pub location: u8, // < The enumeration value from MEMORY_ARRAY_LOCATION.
    pub use_: u8,     // < The enumeration value from MEMORY_ARRAY_USE.
    pub memory_error_correction: u8, // < The enumeration value from MEMORY_ERROR_CORRECTION.
    pub maximum_capacity: u32,
    pub memory_error_information_handle: u16,
    pub number_of_memory_devices: u16,
    //
    // Add for smbios 2.7
    //
    pub extended_maximum_capacity: u64,
}

///
/// Memory Device - Form Factor.
///
#[repr(u8)]
pub enum MemoryFormFactor {
    Other = 0x01,
    Unknown = 0x02,
    Simm = 0x03,
    Sip = 0x04,
    Chip = 0x05,
    Dip = 0x06,
    Zip = 0x07,
    ProprietaryCard = 0x08,
    Dimm = 0x09,
    Tsop = 0x0A,
    RowOfChips = 0x0B,
    Rimm = 0x0C,
    Sodimm = 0x0D,
    Srimm = 0x0E,
    FbDimm = 0x0F,
    Die = 0x10,
}

///
/// Memory Device - Type
///
#[repr(u8)]
pub enum MemoryDeviceType {
    Other = 0x01,
    Unknown = 0x02,
    Dram = 0x03,
    Edram = 0x04,
    Vram = 0x05,
    Sram = 0x06,
    Ram = 0x07,
    Rom = 0x08,
    Flash = 0x09,
    Eeprom = 0x0A,
    Feprom = 0x0B,
    Eprom = 0x0C,
    Cdram = 0x0D,
    ThreeDram = 0x0E,
    Sdram = 0x0F,
    Sgram = 0x10,
    Rdram = 0x11,
    Ddr = 0x12,
    Ddr2 = 0x13,
    Ddr2FbDimm = 0x14,
    Ddr3 = 0x18,
    Fbd2 = 0x19,
    Ddr4 = 0x1A,
    Lpddr = 0x1B,
    Lpddr2 = 0x1C,
    Lpddr3 = 0x1D,
    Lpddr4 = 0x1E,
    LogicalNonVolatileDevice = 0x1F,
    Hbm = 0x20,
    Hbm2 = 0x21,
    Ddr5 = 0x22,
    Lpddr5 = 0x23,
    Hbm3 = 0x24,
}

///
/// Memory Device - Type Detail
///
bitfield! {
    pub struct MemoryDeviceTypeDetails(u16);
    impl Debug;
    pub reserved, set_reserved: 0;
    pub other, set_other: 1;
    pub unknown, set_unknown: 2;
    pub fast_paged, set_fast_paged: 3;
    pub static_column, set_static_column: 4;
    pub pseudo_static, set_pseudo_static: 5;
    pub rambus, set_rambus: 6;
    pub synchronous, set_synchronous: 7;
    pub cmos, set_cmos: 8;
    pub edo, set_edo: 9;
    pub window_dram, set_window_dram: 10;
    pub cache_dram, set_cache_dram: 11;
    pub nonvolatile, set_nonvolatile: 12;
    pub registered, set_registered: 13;
    pub unbuffered, set_unbuffered: 14;
    pub lr_dimm, set_lr_dimm: 15;
}

///
/// Memory Device - Memory Technology
///
#[repr(u8)]
pub enum MemoryDeviceTechnology {
    Other = 0x01,
    Unknown = 0x02,
    Dram = 0x03,
    NvdimmN = 0x04,
    NvdimmF = 0x05,
    NvdimmP = 0x06,
    //
    // This definition is updated to represent Intel
    // Optane DC Persistent Memory in SMBIOS spec 3.4.0
    //
    IntelOptanePersistentMemory = 0x07,
}

///
/// Memory Device - Memory Operating Mode Capability
///
bitfield! {
    pub struct MemoryDeviceOperatingModeCapabilityBits(u16);
    impl Debug;
    pub reserved, set_reserved: 0;
    pub other, set_other: 1;
    pub unknown, set_unknown: 2;
    pub volatile_memory, set_volatile_memory: 3;
    pub byte_accessible_persistent_memory, set_byte_accessible_persistent_memory: 4;
    pub block_accessible_persistent_memory, set_block_accessible_persistent_memory: 5;
    pub reserved2, set_reserved2: 15, 6;
}

pub union MemoryDeviceOperatingModeCapability {
    pub bits: MemoryDeviceOperatingModeCapabilityBits,
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
    pub hdr: SmbiosStructure,
    pub memory_array_handle: u16,
    pub memory_error_information_handle: u16,
    pub total_width: u16,
    pub data_width: u16,
    pub size: u16,
    pub form_factor: u8, // < The enumeration value from MEMORY_FORM_FACTOR.
    pub device_set: u8,
    pub device_locator: SmbiosTableString,
    pub bank_locator: SmbiosTableString,
    pub memory_type: u8, // < The enumeration value from MEMORY_DEVICE_TYPE.
    pub type_detail: MemoryDeviceTypeDetails,
    pub speed: u16,
    pub manufacturer: SmbiosTableString,
    pub serial_number: SmbiosTableString,
    pub asset_tag: SmbiosTableString,
    pub part_number: SmbiosTableString,
    //
    // Add for smbios 2.6
    //
    pub attributes: u8,
    //
    // Add for smbios 2.7
    //
    pub extended_size: u32,
    //
    // Keep using name "ConfiguredMemoryClockSpeed" for compatibility
    // although this field is renamed from "Configured Memory Clock Speed"
    // to "Configured Memory Speed" in smbios 3.2.0.
    //
    pub configured_memory_clock_speed: u16,
    //
    // Add for smbios 2.8.0
    //
    pub minimum_voltage: u16,
    pub maximum_voltage: u16,
    pub configured_voltage: u16,
    //
    // Add for smbios 3.2.0
    //
    pub memory_technology: u8, // < The enumeration value from MEMORY_DEVICE_TECHNOLOGY
    pub memory_operating_mode_capability: MemoryDeviceOperatingModeCapability,
    pub firmware_version: SmbiosTableString,
    pub module_manufacturer_id: u16,
    pub module_product_id: u16,
    pub memory_subsystem_controller_manufacturer_id: u16,
    pub memory_subsystem_controller_product_id: u16,
    pub non_volatile_size: u64,
    pub volatile_size: u64,
    pub cache_size: u64,
    pub logical_size: u64,
    //
    // Add for smbios 3.3.0
    //
    pub extended_speed: u32,
    pub extended_configured_memory_speed: u32,
    //
    // Add for smbios 3.7.0
    //
    pub pmic0_manufacturer_id: u16,
    pub pmic0_revision_number: u16,
    pub rcd_manufacturer_id: u16,
    pub rcd_revision_number: u16,
}

///
/// 32-bit Memory Error Information - Error Type.
///
#[repr(u8)]
pub enum MemoryErrorType {
    Other = 0x01,
    Unknown = 0x02,
    Ok = 0x03,
    BadRead = 0x04,
    Parity = 0x05,
    SigleBit = 0x06,
    DoubleBit = 0x07,
    MultiBit = 0x08,
    Nibble = 0x09,
    Checksum = 0x0A,
    Crc = 0x0B,
    CorrectSingleBit = 0x0C,
    Corrected = 0x0D,
    UnCorrectable = 0x0E,
}

///
/// 32-bit Memory Error Information - Error Granularity.
///
#[repr(u8)]
pub enum MemoryErrorGranularity {
    Other = 0x01,
    OtherUnknown = 0x02,
    DeviceLevel = 0x03,
    MemPartitionLevel = 0x04,
}

///
/// 32-bit Memory Error Information - Error Operation.
///
#[repr(u8)]
pub enum MemoryErrorOperation {
    Other = 0x01,
    Unknown = 0x02,
    Read = 0x03,
    Write = 0x04,
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
    pub hdr: SmbiosStructure,
    pub error_type: u8,        // < The enumeration value from MEMORY_ERROR_TYPE.
    pub error_granularity: u8, // < The enumeration value from MEMORY_ERROR_GRANULARITY.
    pub error_operation: u8,   // < The enumeration value from MEMORY_ERROR_OPERATION.
    pub vendor_syndrome: u32,
    pub memory_array_error_address: u32,
    pub device_error_address: u32,
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
    pub hdr: SmbiosStructure,
    pub starting_address: u32,
    pub ending_address: u32,
    pub memory_array_handle: u16,
    pub partition_width: u8,
    //
    // Add for smbios 2.7
    //
    pub extended_starting_address: u64,
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
    pub hdr: SmbiosStructure,
    pub starting_address: u32,
    pub ending_address: u32,
    pub memory_device_handle: u16,
    pub memory_array_mapped_address_handle: u16,
    pub partition_row_position: u8,
    pub interleave_position: u8,
    pub interleaved_data_depth: u8,
    //
    // Add for smbios 2.7
    //
    pub extended_starting_address: u64,
    pub extended_ending_address: u64,
}

///
/// Built-in Pointing Device - Type
///
#[repr(u8)]
pub enum BuiltinPointingDeviceType {
    Other = 0x01,
    Unknown = 0x02,
    Mouse = 0x03,
    TrackBall = 0x04,
    TrackPoint = 0x05,
    GlidePoint = 0x06,
    TouchPad = 0x07,
    TouchScreen = 0x08,
    OpticalSensor = 0x09,
}

///
/// Built-in Pointing Device - Interface.
///
#[repr(u8)]
pub enum BuiltinPointingDeviceInterface {
    Other = 0x01,
    Unknown = 0x02,
    Serial = 0x03,
    Ps2 = 0x04,
    Infrared = 0x05,
    HpHil = 0x06,
    BusMouse = 0x07,
    Adb = 0x08,
    BusMouseDb9 = 0xA0,
    BusMouseMicroDin = 0xA1,
    Usb = 0xA2,
    I2c = 0xA3,
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
    pub hdr: SmbiosStructure,
    pub type_: u8,     // < The enumeration value from BUILTIN_POINTING_DEVICE_TYPE.
    pub interface: u8, // < The enumeration value from BUILTIN_POINTING_DEVICE_INTERFACE.
    pub number_of_buttons: u8,
}

///
/// Portable Battery - Device Chemistry
///
#[repr(u8)]
pub enum PortableBatteryDeviceChemistry {
    Other = 0x01,
    Unknown = 0x02,
    LeadAcid = 0x03,
    NickelCadmium = 0x04,
    NickelMetalHydride = 0x05,
    LithiumIon = 0x06,
    ZincAir = 0x07,
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
    pub hdr: SmbiosStructure,
    pub location: SmbiosTableString,
    pub manufacturer: SmbiosTableString,
    pub manufacture_date: SmbiosTableString,
    pub serial_number: SmbiosTableString,
    pub device_name: SmbiosTableString,
    pub device_chemistry: u8, // < The enumeration value from PORTABLE_BATTERY_DEVICE_CHEMISTRY.
    pub device_capacity: u16,
    pub design_voltage: u16,
    pub sbds_version_number: SmbiosTableString,
    pub maximum_error_in_battery_data: u8,
    pub sbds_serial_number: u16,
    pub sbds_manufacture_date: u16,
    pub sbds_device_chemistry: SmbiosTableString,
    pub design_capacity_multiplier: u8,
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
    pub hdr: SmbiosStructure,
    pub capabilities: u8,
    pub reset_count: u16,
    pub reset_limit: u16,
    pub timer_interval: u16,
    pub timeout: u16,
}

///
/// Hardware Security (Type 24).
///
/// This structure describes the system-wide hardware security settings.
///
#[repr(C, packed)]
pub struct SmbiosTableType24 {
    pub hdr: SmbiosStructure,
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
    pub hdr: SmbiosStructure,
    pub next_scheduled_power_on_month: u8,
    pub next_scheduled_power_on_day_of_month: u8,
    pub next_scheduled_power_on_hour: u8,
    pub next_scheduled_power_on_minute: u8,
    pub next_scheduled_power_on_second: u8,
}

///
/// Voltage Probe - Location and Status.
///
#[repr(C, packed)]
pub struct MiscVoltageProbeLocation {
    pub voltage_probe_site: u8,   // 5 bits
    pub voltage_probe_status: u8, // 3 bits
}

///
/// Voltage Probe (Type 26)
///
/// This describes the attributes for a voltage probe in the system.
/// Each structure describes a single voltage probe.
///
#[repr(C, packed)]
pub struct SmbiosTableType26 {
    pub hdr: SmbiosStructure,
    pub description: SmbiosTableString,
    pub location_and_status: MiscVoltageProbeLocation,
    pub maximum_value: u16,
    pub minimum_value: u16,
    pub resolution: u16,
    pub tolerance: u16,
    pub accuracy: u16,
    pub oem_defined: u32,
    pub nominal_value: u16,
}

///
/// Cooling Device - Device Type and Status.
///
#[repr(C, packed)]
pub struct MiscCoolingDeviceType {
    pub cooling_device: u8,        // 5 bits
    pub cooling_device_status: u8, // 3 bits
}

///
/// Cooling Device (Type 27)
///
/// This structure describes the attributes for a cooling device in the system.
/// Each structure describes a single cooling device.
///
#[repr(C, packed)]
pub struct SmbiosTableType27 {
    pub hdr: SmbiosStructure,
    pub temperature_probe_handle: u16,
    pub device_type_and_status: MiscCoolingDeviceType,
    pub cooling_unit_group: u8,
    pub oem_defined: u32,
    pub nominal_speed: u16,
    //
    // Add for smbios 2.7
    //
    pub description: SmbiosTableString,
}

///
/// Temperature Probe - Location and Status.
///
bitfield! {
    pub struct MiscTemperatureProbeLocation(u8);
    impl Debug;
    pub temperature_probe_site, set_temperature_probe_site: 4, 0;
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
    pub hdr: SmbiosStructure,
    pub description: SmbiosTableString,
    pub location_and_status: MiscTemperatureProbeLocation,
    pub maximum_value: u16,
    pub minimum_value: u16,
    pub resolution: u16,
    pub tolerance: u16,
    pub accuracy: u16,
    pub oem_defined: u32,
    pub nominal_value: u16,
}

///
/// Electrical Current Probe - Location and Status.
///
bitfield! {
    pub struct MiscElectricalCurrentProbeLocation(u8);
    impl Debug;
    pub electrical_current_probe_site, set_electrical_current_probe_site: 4, 0;
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
    pub hdr: SmbiosStructure,
    pub description: SmbiosTableString,
    pub location_and_status: MiscElectricalCurrentProbeLocation,
    pub maximum_value: u16,
    pub minimum_value: u16,
    pub resolution: u16,
    pub tolerance: u16,
    pub accuracy: u16,
    pub oem_defined: u32,
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
    pub hdr: SmbiosStructure,
    pub manufacturer_name: SmbiosTableString,
    pub connections: u8,
}

///
/// Boot Integrity Services (BIS) Entry Point (Type 31).
///
/// Structure type 31 (decimal) is reserved for use by the Boot Integrity Services (BIS).
///
#[repr(C, packed)]
pub struct SmbiosTableType31 {
    pub hdr: SmbiosStructure,
    pub checksum: u8,
    pub reserved1: u8,
    pub reserved2: u16,
    pub bis_entry16: u32,
    pub bis_entry32: u32,
    pub reserved3: u64,
    pub reserved4: u32,
}

///
/// System Boot Information - System Boot Status.
///
#[repr(u8)]
pub enum MiscBootInformationStatusDataType {
    NoError = 0x00,
    NoBootableMedia = 0x01,
    NormalOsFailedLoading = 0x02,
    FirmwareDetectedFailure = 0x03,
    OsDetectedFailure = 0x04,
    UserRequestedBoot = 0x05,
    SystemSecurityViolation = 0x06,
    PreviousRequestedImage = 0x07,
    WatchdogTimerExpired = 0x08,
    StartReserved = 0x09,
    StartOemSpecific = 0x80,
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
    pub hdr: SmbiosStructure,
    pub reserved: [u8; 6],
    pub boot_status: u8, // < The enumeration value from MISC_BOOT_INFORMATION_STATUS_DATA_TYPE.
}

///
/// 64-bit Memory Error Information (Type 33).
///
/// This structure describes an error within a Physical Memory Array,
/// when the error address is above 4G (0xFFFFFFFF).
///
#[repr(C, packed)]
pub struct SmbiosTableType33 {
    pub hdr: SmbiosStructure,
    pub error_type: u8,        // < The enumeration value from MEMORY_ERROR_TYPE.
    pub error_granularity: u8, // < The enumeration value from MEMORY_ERROR_GRANULARITY.
    pub error_operation: u8,   // < The enumeration value from MEMORY_ERROR_OPERATION.
    pub vendor_syndrome: u32,
    pub memory_array_error_address: u64,
    pub device_error_address: u64,
    pub error_resolution: u32,
}

///
/// Management Device -  Type.
///
#[repr(u8)]
pub enum MiscManagementDeviceType {
    Other = 0x01,
    Unknown = 0x02,
    Lm75 = 0x03,
    Lm78 = 0x04,
    Lm79 = 0x05,
    Lm80 = 0x06,
    Lm81 = 0x07,
    Adm9240 = 0x08,
    Ds1780 = 0x09,
    Maxim1617 = 0x0A,
    Gl518Sm = 0x0B,
    W83781D = 0x0C,
    Ht82H791 = 0x0D,
}

///
/// Management Device -  Address Type.
///
#[repr(u8)]
pub enum MiscManagementDeviceAddressType {
    Other = 0x01,
    Unknown = 0x02,
    IoPort = 0x03,
    Memory = 0x04,
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
    pub hdr: SmbiosStructure,
    pub description: SmbiosTableString,
    pub type_: u8, // < The enumeration value from MISC_MANAGEMENT_DEVICE_TYPE.
    pub address: u32,
    pub address_type: u8, // < The enumeration value from MISC_MANAGEMENT_DEVICE_ADDRESS_TYPE.
}

///
/// Management Device Component (Type 35)
///
/// This structure associates a cooling device or environmental probe with structures
/// that define the controlling hardware device and (optionally) the component's thresholds.
///
#[repr(C, packed)]
pub struct SmbiosTableType35 {
    pub hdr: SmbiosStructure,
    pub description: SmbiosTableString,
    pub management_device_handle: u16,
    pub component_handle: u16,
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
    pub hdr: SmbiosStructure,
    pub lower_threshold_non_critical: u16,
    pub upper_threshold_non_critical: u16,
    pub lower_threshold_critical: u16,
    pub upper_threshold_critical: u16,
    pub lower_threshold_non_recoverable: u16,
    pub upper_threshold_non_recoverable: u16,
}

///
/// Memory Channel Entry.
///
#[repr(C, packed)]
pub struct MemoryDevice {
    pub device_load: u8,
    pub device_handle: u16,
}

///
/// Memory Channel - Channel Type.
///
#[repr(u8)]
pub enum MemoryChannelType {
    Other = 0x01,
    Unknown = 0x02,
    Rambus = 0x03,
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
    pub hdr: SmbiosStructure,
    pub channel_type: u8,
    pub maximum_channel_load: u8,
    pub memory_device_count: u8,
    pub memory_device: [MemoryDevice; 1],
}

///
/// IPMI Device Information - BMC Interface Type
///
#[repr(u8)]
pub enum BmcInterfaceType {
    Unknown = 0x00,
    Kcs = 0x01,  // < The Keyboard Controller Style.
    Smic = 0x02, // < The Server Management Interface Chip.
    Bt = 0x03,   // < The Block Transfer
    Ssif = 0x04, // < SMBus System Interface
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
    pub struct SysPowerSupplyCharacteristics(u16);
    impl Debug;
    pub power_supply_hot_replaceable, set_power_supply_hot_replaceable: 0;
    pub power_supply_present, set_power_supply_present: 1;
    pub power_supply_unplugged, set_power_supply_unplugged: 2;
    pub input_voltage_range_switch, set_input_voltage_range_switch: 6, 3;
    pub power_supply_status, set_power_supply_status: 9, 7;
    pub power_supply_type, set_power_supply_type: 13, 10;
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
    pub hdr: SmbiosStructure,
    pub power_unit_group: u8,
    pub location: SmbiosTableString,
    pub device_name: SmbiosTableString,
    pub manufacturer: SmbiosTableString,
    pub serial_number: SmbiosTableString,
    pub asset_tag_number: SmbiosTableString,
    pub model_part_number: SmbiosTableString,
    pub revision_level: SmbiosTableString,
    pub max_power_capacity: u16,
    pub power_supply_characteristics: SysPowerSupplyCharacteristics,
    pub input_voltage_probe_handle: u16,
    pub cooling_device_handle: u16,
    pub input_current_probe_handle: u16,
}

///
/// Additional Information Entry Format.
///
#[repr(C, packed)]
pub struct AdditionalInformationEntry {
    pub entry_length: u8,
    pub referenced_handle: u16,
    pub referenced_offset: u8,
    pub entry_string: SmbiosTableString,
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
    pub hdr: SmbiosStructure,
    pub number_of_additional_information_entries: u8,
    pub additional_info_entries: [AdditionalInformationEntry; 1],
}

///
/// Onboard Devices Extended Information - Onboard Device Types.
///
#[repr(u8)]
pub enum OnboardDeviceExtendedInfoType {
    Other = 0x01,
    Unknown = 0x02,
    Video = 0x03,
    ScsiController = 0x04,
    Ethernet = 0x05,
    TokenRing = 0x06,
    Sound = 0x07,
    PataController = 0x08,
    SataController = 0x09,
    SasController = 0x0A,
    WirelessLan = 0x0B,
    Bluetooth = 0x0C,
    Wwan = 0x0D,
    EMmc = 0x0E,
    Nvme = 0x0F,
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
    pub hdr: SmbiosStructure,
    pub reference_designation: SmbiosTableString,
    pub device_type: u8, // < The enumeration value from ONBOARD_DEVICE_EXTENDED_INFO_TYPE
    pub device_type_instance: u8,
    pub segment_group_num: u16,
    pub bus_num: u8,
    pub dev_func_num: u8,
}

///
///  Management Controller Host Interface - Protocol Record Data Format.
///
#[repr(C, packed)]
pub struct McHostInterfaceProtocolRecord {
    pub protocol_type: u8,
    pub protocol_type_data_len: u8,
    pub protocol_type_data: [u8; 1],
}

///
/// Management Controller Host Interface - Interface Types.
/// 00h - 3Fh: MCTP Host Interfaces
///
#[repr(u8)]
pub enum McHostInterfaceType {
    NetworkHostInterface = 0x40,
    OemDefined = 0xF0,
}

///
/// Management Controller Host Interface - Protocol Types.
///
#[repr(u8)]
pub enum McHostInterfaceProtocolType {
    Ipmi = 0x02,
    Mctp = 0x03,
    RedfishOverIp = 0x04,
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
    pub hdr: SmbiosStructure,
    pub interface_type: u8, // < The enumeration value from MC_HOST_INTERFACE_TYPE
    pub interface_type_specific_data_length: u8,
    pub interface_type_specific_data: [u8; 4], // < This field has a minimum of four bytes
}

///
/// Processor Specific Block - Processor Architecture Type
///
#[repr(u8)]
pub enum ProcessorSpecificBlockArchType {
    Reserved = 0x00,
    Ia32 = 0x01,
    X64 = 0x02,
    Itanium = 0x03,
    Aarch32 = 0x04,
    Aarch64 = 0x05,
    RiscVRv32 = 0x06,
    RiscVRv64 = 0x07,
    RiscVRv128 = 0x08,
    LoongArch32 = 0x09,
    LoongArch64 = 0x0A,
}

///
/// Processor Specific Block is the standard container of processor-specific data.
///
#[repr(C, packed)]
pub struct ProcessorSpecificBlock {
    pub length: u8,
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
    pub hdr: SmbiosStructure,
    pub ref_handle: SmbiosHandle, // < This field refer to associated SMBIOS type 4
    //
    //  Below followed by Processor-specific block
    //
    pub processor_specific_block: ProcessorSpecificBlock,
}

///
/// TPM Device (Type 43).
///
#[repr(C, packed)]
pub struct SmbiosTableType43 {
    pub hdr: SmbiosStructure,
    pub vendor_id: [u8; 4],
    pub major_spec_version: u8,
    pub minor_spec_version: u8,
    pub firmware_version1: u32,
    pub firmware_version2: u32,
    pub description: SmbiosTableString,
    pub characteristics: u64,
    pub oem_defined: u32,
}

///
/// Firmware Inventory Version Format Type (Type 45).
///
#[repr(u8)]
pub enum FirmwareInventoryVersionFormatType {
    FreeForm = 0x00,
    MajorMinor = 0x01,
    ThirtyTwoBitHex = 0x02,
    SixtyFourBitHex = 0x03,
    Reserved = 0x04, //  0x04 - 0x7F are reserved
    Oem = 0x80,      //  0x80 - 0xFF are BIOS Vendor/OEM-specific
}

///
/// Firmware Inventory Firmware Id Format Type (Type 45).
///
#[repr(u8)]
pub enum FirmwareInventoryFirmwareIdFormatType {
    FreeForm = 0x00,
    Uuid = 0x01,
    Reserved = 0x04, //  0x04 - 0x7F are reserved
    Oem = 0x80,      //  0x80 - 0xFF are BIOS Vendor/OEM-specific
}

///
/// Firmware Inventory Firmware Characteristics (Type 45).
///
bitfield! {
    pub struct FirmwareCharacteristics(u16);
    impl Debug;
    pub updatable, set_updatable: 0;
    pub write_protected, set_write_protected: 1;
    pub reserved, set_reserved: 15, 3;
}

///
/// Firmware Inventory State Information (Type 45).
///
#[repr(u8)]
pub enum FirmwareInventoryState {
    Other = 0x01,
    Unknown = 0x02,
    Disabled = 0x03,
    Enabled = 0x04,
    Absent = 0x05,
    StandbyOffline = 0x06,
    StandbySpare = 0x07,
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
    pub hdr: SmbiosStructure,
    pub firmware_component_name: SmbiosTableString,
    pub firmware_version: SmbiosTableString,
    pub firmware_version_format: u8, // < The enumeration value from FIRMWARE_INVENTORY_VERSION_FORMAT_TYPE
    pub firmware_id: SmbiosTableString,
    pub firmware_id_format: u8, // < The enumeration value from FIRMWARE_INVENTORY_FIRMWARE_ID_FORMAT_TYPE.
    pub release_date: SmbiosTableString,
    pub manufacturer: SmbiosTableString,
    pub lowest_supported_version: SmbiosTableString,
    pub image_size: u64,
    pub characteristics: FirmwareCharacteristics,
    pub state: u8, // < The enumeration value from FIRMWARE_INVENTORY_STATE.
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
    None = 0x0000,
    DevicePath = 0x0001,
    Reserved = 0x0002,   //  Reserved    0x0002 - 0x7FFF
    BiosVendor = 0x8000, //  BIOS vendor 0x8000 - 0xBFFF
    Oem = 0xC000,        //  OEM range   0xC000 - 0xFFFF
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
    pub hdr: SmbiosStructure,
    pub string_property_id: u16, // < The enumeration value from STRING_PROPERTY_ID.
    pub string_property_value: SmbiosTableString,
    pub parent_handle: SmbiosHandle,
}

///
/// Inactive (Type 126)
///
#[repr(C, packed)]
pub struct SmbiosTableType126 {
    pub hdr: SmbiosStructure,
}

///
/// End-of-Table (Type 127)
///
#[repr(C, packed)]
pub struct SmbiosTableType127 {
    pub hdr: SmbiosStructure,
}

///
/// Union of all the possible SMBIOS record types.
///
#[repr(C, packed)]
pub enum SmbiosStructurePointer {
    Hdr(*mut SmbiosStructure),
    Type0(*mut SmbiosTableType0),
    Type1(*mut SmbiosTableType1),
    Type2(*mut SmbiosTableType2),
    Type3(*mut SmbiosTableType3),
    Type4(*mut SmbiosTableType4),
    Type5(*mut SmbiosTableType5),
    Type6(*mut SmbiosTableType6),
    Type7(*mut SmbiosTableType7),
    Type8(*mut SmbiosTableType8),
    Type9(*mut SmbiosTableType9),
    Type10(*mut SmbiosTableType10),
    Type11(*mut SmbiosTableType11),
    Type12(*mut SmbiosTableType12),
    Type13(*mut SmbiosTableType13),
    Type14(*mut SmbiosTableType14),
    Type15(*mut SmbiosTableType15),
    Type16(*mut SmbiosTableType16),
    Type17(*mut SmbiosTableType17),
    Type18(*mut SmbiosTableType18),
    Type19(*mut SmbiosTableType19),
    Type20(*mut SmbiosTableType20),
    Type21(*mut SmbiosTableType21),
    Type22(*mut SmbiosTableType22),
    Type23(*mut SmbiosTableType23),
    Type24(*mut SmbiosTableType24),
    Type25(*mut SmbiosTableType25),
    Type26(*mut SmbiosTableType26),
    Type27(*mut SmbiosTableType27),
    Type28(*mut SmbiosTableType28),
    Type29(*mut SmbiosTableType29),
    Type30(*mut SmbiosTableType30),
    Type31(*mut SmbiosTableType31),
    Type32(*mut SmbiosTableType32),
    Type33(*mut SmbiosTableType33),
    Type34(*mut SmbiosTableType34),
    Type35(*mut SmbiosTableType35),
    Type36(*mut SmbiosTableType36),
    Type37(*mut SmbiosTableType37),
    Type38(*mut SmbiosTableType38),
    Type39(*mut SmbiosTableType39),
    Type40(*mut SmbiosTableType40),
    Type41(*mut SmbiosTableType41),
    Type42(*mut SmbiosTableType42),
    Type43(*mut SmbiosTableType43),
    Type44(*mut SmbiosTableType44),
    Type45(*mut SmbiosTableType45),
    Type46(*mut SmbiosTableType46),
    Type126(*mut SmbiosTableType126),
    Type127(*mut SmbiosTableType127),
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

pub type UIntn = usize;

/// SMBIOS instance structure
#[repr(C, packed)]
pub struct SmbiosInstance {
    pub signature: u32,
    pub handle: core::ffi::c_void,
    pub smbios: Option<i32>, //TODO Dell: protocol implementation later
    pub data_lock: tpl_lock::TplMutex<Option<SmbiosInstance>>,
    pub data_list_head: list_entry::Entry,
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
#[repr(C, packed)]
pub struct EfiSmbiosRecordHeader {
    pub version: u16,
    pub header_size: u16,
    pub record_size: UIntn,
    pub producer_handle: core::ffi::c_void,
    pub number_of_strings: UIntn,
}

/// Private data structure to contain the SMBIOS record. One record per
/// structure. SmbiosRecord is a copy of the data passed in and follows RecordHeader.
#[repr(C, packed)]
pub struct EfiSmbiosEntry {
    pub signature: u32,
    pub link: list_entry::Entry,
    pub record_header: Option<EfiSmbiosRecordHeader>,
    pub record_size: UIntn,
    /// Indicate which table this record is added to
    pub smbios_32bit_table: bool,
    pub smbios_64bit_table: bool,
}

/// Private data to contain the Smbios handle that already allocated.
#[repr(C, packed)]
pub struct SmbiosHandleEntry {
    pub signature: u32,
    pub link: list_entry::Entry,
    /// Filter driver will register what record guid filter should be used
    pub smbios_handle: core::ffi::c_void, //TODO Dell: protocol implementation later
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
pub struct EfiSmbiosTableEndStructure {
    pub header: EfiSmbiosTableHeader,
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
    pub guid: efi::Guid,
    pub is_valid: IsSmbiosTableValid,
}
