//! SMBIOS Types
//!
//! Defines the types used in SMBIOS Records.
//!
//! Bitfield types are defined with [`bitfield_struct::bitfield`] and derive
//! [`zerocopy::IntoBytes`] so they can be serialized directly by the
//! `SmbiosRecord` derive macro. Enum types are tagged with `#[repr(u8)]` or
//! `#[repr(u16)]` per the SMBIOS specification field width and likewise
//! derive `IntoBytes`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

#![allow(missing_docs)]

extern crate alloc;

use bitfield_struct::bitfield;
use zerocopy::{Immutable, IntoBytes, KnownLayout};

/// BIOS Characteristics (Type 0, offset 0x0A) - 8 bytes
#[bitfield(u64)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct BiosCharacteristics {
    #[bits(2)]
    pub reserved: u8,
    pub unknown: bool,
    pub bios_characteristics_unsupported: bool,
    pub isa_supported: bool,
    pub mca_supported: bool,
    pub eisa_supported: bool,
    pub pci_supported: bool,
    pub pcmcia_supported: bool,
    pub plug_play_supported: bool,
    pub apm_supported: bool,
    pub bios_is_upgradable: bool,
    pub bios_shadowing_allowed: bool,
    pub vlvesa_supported: bool,
    pub escd_supported: bool,
    pub cd_boot_supported: bool,
    pub selectable_boot_supported: bool,
    pub bios_rom_socketed: bool,
    pub pc_card_boot_supported: bool,
    pub edd_spec_supported: bool,
    pub japanese_nec_9800_supported: bool,
    pub japanese_toshiba_supported: bool,
    pub kb_525_360_supported: bool,
    pub mb_535_12_supported: bool,
    pub mb_35_720_supported: bool,
    pub mb_35_288_supported: bool,
    pub print_screen_supported: bool,
    pub keyboard_8042_supported: bool,
    pub serial_services_supported: bool,
    pub printer_services_supported: bool,
    pub cga_mono_video_supported: bool,
    pub nec_pc_98: bool,
    #[bits(16)]
    pub reserved_bios_vendor: u16,
    #[bits(16)]
    pub reserved_system_vendor: u16,
}

/// BIOS Characteristics Extension Byte 1 (Type 0, offset 0x12)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct BiosCharacteristicsExt1 {
    pub acpi_supported: bool,
    pub usb_legacy_supported: bool,
    pub agp_supported: bool,
    pub i20_supported: bool,
    pub superdisk_boot_supported: bool,
    pub zip_drive_boot_supported: bool,
    pub boot_1394_supported: bool,
    pub smart_battery_supported: bool,
}

/// BIOS Characteristics Extension Byte 2 (Type 0, offset 0x13)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct BiosCharacteristicsExt2 {
    pub bios_boot_specification_supported: bool,
    pub fn_network_service_boot_supported: bool,
    pub enable_targeted_content_distribution: bool,
    pub uefi_spec_supported: bool,
    pub smbios_describes_vm: bool,
    #[bits(3)]
    pub reserved: u8,
}

/// Extended BIOS ROM Size (Type 0, offset 0x18) - 2 bytes
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ExtendedBiosRomSize {
    #[bits(14)]
    pub size: u16,
    #[bits(2)]
    pub unit: u8,
}

/// Wake-Up Type (Type 1, offset 0x18)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum WakeUpType {
    Reserved = 0x00,
    Other = 0x01,
    Unknown = 0x02,
    ApmTimer = 0x03,
    ModemRing = 0x04,
    LanRemote = 0x05,
    PowerSwitch = 0x06,
    PciPme = 0x07,
    AcPowerRestored = 0x08,
}

/// Baseboard Feature Flags (Type 2, offset 0x09)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct FeatureFlags {
    pub hosting_board: bool,
    pub require_aux_board: bool,
    pub removable_board: bool,
    pub replaceable_board: bool,
    pub hot_swappable_board: bool,
    #[bits(3)]
    pub reserved: u8,
}

/// Baseboard Type (Type 2, offset 0x0D)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum BoardType {
    Unknown = 0x01,
    Other = 0x02,
    ServerBlade = 0x03,
    ConnectivitySwitch = 0x04,
    SystemManagementModule = 0x05,
    ProcessorModule = 0x06,
    IoModule = 0x07,
    MemoryModule = 0x08,
    DaughterBoard = 0x09,
    Motherboard = 0x0A,
    ProcessorMemoryModule = 0x0B,
    ProcessorIoModule = 0x0C,
    InterconnectBoard = 0x0D,
}

/// System Enclosure Boot-Up State (Type 3, offset 0x09).
///
/// Per SMBIOS spec Table 17 (System — Boot Up State).
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum BootUpState {
    Other = 0x01,
    Unknown = 0x02,
    Safe = 0x03,
    Warning = 0x04,
    Critical = 0x05,
    NonRecoverable = 0x06,
}

/// System Enclosure Power Supply State (Type 3, offset 0x0A).
///
/// Per SMBIOS spec Table 17 (Power Supply State).
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum PowerSupplyState {
    Other = 0x01,
    Unknown = 0x02,
    Safe = 0x03,
    Warning = 0x04,
    Critical = 0x05,
    NonRecoverable = 0x06,
}

/// System Enclosure Thermal State (Type 3, offset 0x0B)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ThermalState {
    Other = 0x01,
    Unknown = 0x02,
    Safe = 0x03,
    Warning = 0x04,
    Critical = 0x05,
    NonRecoverable = 0x06,
}

/// System Enclosure Security Status (Type 3, offset 0x0C).
///
/// `NoneStatus` is named to avoid the reserved `None` keyword.
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SecurityStatus {
    Other = 0x01,
    Unknown = 0x02,
    NoneStatus = 0x03,
    ExternalInterfaceLockedOut = 0x04,
    ExternalInterfaceEnabled = 0x05,
}

/// Contained Element Type (Type 3 element record, byte 0)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ContainedElementType {
    #[bits(7)]
    pub r#type: u8,
    pub type_select: bool,
}

/// Contained Element (Type 3 element record - 3 bytes)
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub struct ContainedElements {
    pub contained_element_type: ContainedElementType,
    pub contained_element_minimum: u8,
    pub contained_element_maximum: u8,
}

impl Default for ContainedElements {
    fn default() -> Self {
        Self {
            contained_element_type: ContainedElementType::new(),
            contained_element_minimum: 0,
            contained_element_maximum: 0,
        }
    }
}

/// Processor Type (Type 4, offset 0x05)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ProcessorTypeData {
    ProcessorOther = 0x01,
    ProcessorUnknown = 0x02,
    CentralProcessor = 0x03,
    MathProcessor = 0x04,
    DspProcessor = 0x05,
    VideoProcessor = 0x06,
}

/// Processor Family / Family 2 (Type 4, offset 0x06 BYTE / 0x28 WORD).
///
/// Tagged `#[repr(u16)]` to cover the full SMBIOS extended family list.
/// The 1-byte `processor_family` field on `Type4ProcessorInformation` is a
/// raw `u8`; set it to `0xFE` (IndicatorFamily2) when using the extended
/// `processor_family2` field for values >= 0x100.
#[repr(u16)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ProcessorFamilyData {
    Other = 0x01,
    Unknown = 0x02,
    Processor8086 = 0x03,
    Processor80286 = 0x04,
    Intel386 = 0x05,
    Intel486 = 0x06,
    Processor8087 = 0x07,
    Processor80287 = 0x08,
    Processor80387 = 0x09,
    Processor80487 = 0x0A,
    Pentium = 0x0B,
    PentiumPro = 0x0C,
    PentiumII = 0x0D,
    PentiumMMX = 0x0E,
    Celeron = 0x0F,
    PentiumIIXeon = 0x10,
    PentiumIII = 0x11,
    M1Family = 0x12,
    M2Family = 0x13,
    IntelCeleronM = 0x14,
    IntelPentium4Ht = 0x15,
    IntelProcessor = 0x16,
    AmdDuron = 0x18,
    K5Family = 0x19,
    K6Family = 0x1A,
    K6_2 = 0x1B,
    K6_3 = 0x1C,
    AmdAthlon = 0x1D,
    Amd29000 = 0x1E,
    K6_2Plus = 0x1F,
    PowerPC = 0x20,
    PowerPC601 = 0x21,
    PowerPC603 = 0x22,
    PowerPC603Plus = 0x23,
    PowerPC604 = 0x24,
    PowerPC620 = 0x25,
    PowerPCx704 = 0x26,
    PowerPC750 = 0x27,
    IntelCoreDuo = 0x28,
    IntelCoreDuoMobile = 0x29,
    IntelCoreSoloMobile = 0x2A,
    IntelAtom = 0x2B,
    IntelCoreM = 0x2C,
    IntelCorem3 = 0x2D,
    IntelCorem5 = 0x2E,
    IntelCorem7 = 0x2F,
    Alpha = 0x30,
    Alpha21064 = 0x31,
    Alpha21066 = 0x32,
    Alpha21164 = 0x33,
    Alpha21164PC = 0x34,
    Alpha21164a = 0x35,
    Alpha21264 = 0x36,
    Alpha21364 = 0x37,
    AmdTurionIIUltraDualCoreMobileM = 0x38,
    AmdTurionIIDualCoreMobileM = 0x39,
    AmdAthlonIIDualCoreM = 0x3A,
    AmdOpteron6100Series = 0x3B,
    AmdOpteron4100Series = 0x3C,
    AmdOpteron6200Series = 0x3D,
    AmdOpteron4200Series = 0x3E,
    AmdFxSeries = 0x3F,
    MipsFamily = 0x40,
    MipsR4000 = 0x41,
    MipsR4200 = 0x42,
    MipsR4400 = 0x43,
    MipsR4600 = 0x44,
    MipsR10000 = 0x45,
    AmdCSeries = 0x46,
    AmdESeries = 0x47,
    AmdASeries = 0x48,
    AmdGSeries = 0x49,
    AmdZSeries = 0x4A,
    AmdRSeries = 0x4B,
    AmdOpteron4300 = 0x4C,
    AmdOpteron6300 = 0x4D,
    AmdOpteron3300 = 0x4E,
    AmdFireProSeries = 0x4F,
    Sparc = 0x50,
    SuperSparc = 0x51,
    MicroSparcII = 0x52,
    MicroSparcIIep = 0x53,
    UltraSparc = 0x54,
    UltraSparcII = 0x55,
    UltraSparcIii = 0x56,
    UltraSparcIII = 0x57,
    UltraSparcIIIi = 0x58,
    Processor68040 = 0x60,
    Processor68xxx = 0x61,
    Processor68000 = 0x62,
    Processor68010 = 0x63,
    Processor68020 = 0x64,
    Processor68030 = 0x65,
    AmdAthlonX4QuadCore = 0x66,
    AmdOpteronX1000Series = 0x67,
    AmdOpteronX2000Series = 0x68,
    AmdOpteronASeries = 0x69,
    AmdOpteronX3000Series = 0x6A,
    AmdZen = 0x6B,
    HobbitFamily = 0x70,
    CrusoeTM5000 = 0x78,
    CrusoeTM3000 = 0x79,
    EfficeonTM8000 = 0x7A,
    Weitek = 0x80,
    Itanium = 0x82,
    AmdAthlon64 = 0x83,
    AmdOpteron = 0x84,
    AmdSempron = 0x85,
    AmdTurion64Mobile = 0x86,
    DualCoreAmdOpteron = 0x87,
    AmdAthlon64X2DualCore = 0x88,
    AmdTurion64X2Mobile = 0x89,
    QuadCoreAmdOpteron = 0x8A,
    ThirdGenerationAmdOpteron = 0x8B,
    AmdPhenomFxQuadCore = 0x8C,
    AmdPhenomX4QuadCore = 0x8D,
    AmdPhenomX2DualCore = 0x8E,
    AmdAthlonX2DualCore = 0x8F,
    Parisc = 0x90,
    PaRisc8500 = 0x91,
    PaRisc8000 = 0x92,
    PaRisc7300LC = 0x93,
    PaRisc7200 = 0x94,
    PaRisc7100LC = 0x95,
    PaRisc7100 = 0x96,
    V30Family = 0xA0,
    QuadCoreIntelXeon3200Series = 0xA1,
    DualCoreIntelXeon3000Series = 0xA2,
    QuadCoreIntelXeon5300Series = 0xA3,
    DualCoreIntelXeon5100Series = 0xA4,
    DualCoreIntelXeon5000Series = 0xA5,
    DualCoreIntelXeonLV = 0xA6,
    DualCoreIntelXeonULV = 0xA7,
    DualCoreIntelXeon7100Series = 0xA8,
    QuadCoreIntelXeon5400Series = 0xA9,
    QuadCoreIntelXeon = 0xAA,
    DualCoreIntelXeon5200Series = 0xAB,
    DualCoreIntelXeon7200Series = 0xAC,
    QuadCoreIntelXeon7300Series = 0xAD,
    QuadCoreIntelXeon7400Series = 0xAE,
    MultiCoreIntelXeon7400Series = 0xAF,
    PentiumIIIXeon = 0xB0,
    PentiumIIISpeedStep = 0xB1,
    Pentium4 = 0xB2,
    IntelXeon = 0xB3,
    As400 = 0xB4,
    IntelXeonMP = 0xB5,
    AMDAthlonXP = 0xB6,
    AMDAthlonMP = 0xB7,
    IntelItanium2 = 0xB8,
    IntelPentiumM = 0xB9,
    IntelCeleronD = 0xBA,
    IntelPentiumD = 0xBB,
    IntelPentiumEx = 0xBC,
    IntelCoreSolo = 0xBD,
    Reserved = 0xBE,
    IntelCore2 = 0xBF,
    IntelCore2Solo = 0xC0,
    IntelCore2Extreme = 0xC1,
    IntelCore2Quad = 0xC2,
    IntelCore2ExtremeMobile = 0xC3,
    IntelCore2DuoMobile = 0xC4,
    IntelCore2SoloMobile = 0xC5,
    IntelCoreI7 = 0xC6,
    DualCoreIntelCeleron = 0xC7,
    Ibm390 = 0xC8,
    G4 = 0xC9,
    G5 = 0xCA,
    EsaG6 = 0xCB,
    ZArchitecture = 0xCC,
    IntelCoreI5 = 0xCD,
    IntelCoreI3 = 0xCE,
    IntelCoreI9 = 0xCF,
    IntelXeonD = 0xD0,
    ViaC7M = 0xD2,
    ViaC7D = 0xD3,
    ViaC7 = 0xD4,
    ViaEden = 0xD5,
    MultiCoreIntelXeon = 0xD6,
    DualCoreIntelXeon3Series = 0xD7,
    QuadCoreIntelXeon3Series = 0xD8,
    ViaNano = 0xD9,
    DualCoreIntelXeon5Series = 0xDA,
    QuadCoreIntelXeon5Series = 0xDB,
    DualCoreIntelXeon7Series = 0xDD,
    QuadCoreIntelXeon7Series = 0xDE,
    MultiCoreIntelXeon7Series = 0xDF,
    MultiCoreIntelXeon3400Series = 0xE0,
    AmdOpteron3000Series = 0xE4,
    AmdSempronII = 0xE5,
    EmbeddedAmdOpteronQuadCore = 0xE6,
    AmdPhenomTripleCore = 0xE7,
    AmdTurionUltraDualCoreMobile = 0xE8,
    AmdTurionDualCoreMobile = 0xE9,
    AmdAthlonDualCore = 0xEA,
    AmdSempronSI = 0xEB,
    AmdPhenomII = 0xEC,
    AmdAthlonII = 0xED,
    SixCoreAmdOpteron = 0xEE,
    AmdSempronM = 0xEF,
    I860 = 0xFA,
    I960 = 0xFB,
    /// Use this u8 marker (0xFE) in `processor_family` to indicate that the
    /// real value is in `processor_family2`.
    IndicatorFamily2 = 0xFE,
    Reserved1 = 0xFF,
    ARMv7 = 0x0100,
    ARMv8 = 0x0101,
    ARMv9 = 0x0102,
    Sh3 = 0x0103,
    Sh4 = 0x0104,
    Arm = 0x0118,
    StrongARM = 0x0119,
    Processor6x86 = 0x012C,
    MediaGX = 0x012D,
    Mii = 0x012E,
    WinChip = 0x0140,
    Dsp = 0x015E,
    VideoProcessor = 0x01F4,
    RiscvRV32 = 0x0200,
    RiscVRV64 = 0x0201,
    RiscVRV128 = 0x0202,
    LoongArch = 0x0258,
    Loongson1 = 0x0259,
    Loongson2 = 0x025A,
    Loongson3 = 0x025B,
    Loongson2K = 0x025C,
    Loongson3A = 0x025D,
    Loongson3B = 0x025E,
    Loongson3C = 0x025F,
    Loongson3D = 0x0260,
    Loongson3E = 0x0261,
    DualCoreLoongson2K = 0x0262,
    QuadCoreLoongson3A = 0x026C,
    MultiCoreLoongson3A = 0x026D,
    QuadCoreLoongson3B = 0x026E,
    MultiCoreLoongson3B = 0x026F,
    MultiCoreLoongson3C = 0x0270,
    MultiCoreLoongson3D = 0x0271,
    IntelCore3 = 0x0300,
    IntelCore5 = 0x0301,
    IntelCore7 = 0x0302,
    IntelCore9 = 0x0303,
    IntelCoreUltra3 = 0x0304,
    IntelCoreUltra5 = 0x0305,
    IntelCoreUltra7 = 0x0306,
    IntelCoreUltra9 = 0x0307,
}

/// Processor Upgrade (Type 4, offset 0x19)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ProcessorUpgrade {
    Other = 0x01,
    Unknown = 0x02,
    DaughterBoard = 0x03,
    ZIFSocket = 0x04,
    ReplaceablePiggyBack = 0x05,
    NoUpgrade = 0x06,
    LIFSocket = 0x07,
    Slot1 = 0x08,
    Slot2 = 0x09,
    Pin370Socket = 0x0A,
    SlotA = 0x0B,
    SlotM = 0x0C,
    Socket423 = 0x0D,
    SocketA = 0x0E,
    Socket478 = 0x0F,
    Socket754 = 0x10,
    Socket940 = 0x11,
    Socket939 = 0x12,
    SocketmPGA604 = 0x13,
    SocketLGA771 = 0x14,
    SocketLGA775 = 0x15,
    SocketS1 = 0x16,
    SocketAM2 = 0x17,
    SocketF1207 = 0x18,
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
    SocketLGA1155 = 0x24,
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
    NotAvailable = 0xFF,
}

/// Processor Voltage (Type 4, offset 0x11)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ProcessorVoltage {
    pub processor_voltage_capability_5v: bool,
    pub processor_voltage_capability_3_3v: bool,
    pub processor_voltage_capability_2_9v: bool,
    pub processor_voltage_capability_reserved: bool,
    #[bits(3)]
    pub processor_voltage_reserved: u8,
    pub processor_voltage_indicate_legacy: bool,
}

/// Processor Information Status (Type 4, offset 0x18)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ProcessorInformationStatus {
    #[bits(3)]
    pub cpu_status: u8,
    #[bits(3)]
    pub reserved: u8,
    pub cpu_socket_populated: bool,
    pub reserved2: bool,
}

/// Processor Characteristics (Type 4, offset 0x26)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ProcessorCharacteristics {
    pub reserved: bool,
    pub unknown: bool,
    pub capable_64bit: bool,
    pub multi_core: bool,
    pub hardware_thread: bool,
    pub execute_protection: bool,
    pub enhanced_virtualization: bool,
    pub performance_control: bool,
    pub capable_128bit: bool,
    pub arm64_soc_id: bool,
    #[bits(6)]
    pub reserved2: u8,
}

/// Cache Configuration (Type 7, offset 0x05)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheConfiguration {
    #[bits(3)]
    pub cache_level: u8,
    pub cache_socketed: bool,
    pub reserved: bool,
    #[bits(2)]
    pub location: u8,
    pub enabled_disabled: bool,
    #[bits(2)]
    pub operational_mode: u8,
    #[bits(6)]
    pub reserved2: u8,
}

/// Cache Size (Type 7, offset 0x07 / 0x09)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheSize {
    #[bits(15)]
    pub max_size: u16,
    pub granularity: bool,
}

/// Cache Size 2 (Type 7, offset 0x13 / 0x17)
#[bitfield(u32)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheSize2 {
    #[bits(31)]
    pub max_size: u32,
    pub granularity: bool,
}

/// Cache SRAM Type (Type 7, offset 0x0B / 0x0D)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheSramTypeData {
    pub other: bool,
    pub unknown: bool,
    pub non_burst: bool,
    pub burst: bool,
    pub pipeline_burst: bool,
    pub synchronous: bool,
    pub asynchronous: bool,
    #[bits(9)]
    pub reserved: u16,
}

/// Cache Error Correction Type (Type 7, offset 0x10)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ErrorCorrectionType {
    Other = 0x01,
    Unknown = 0x02,
    NoEcc = 0x03,
    Parity = 0x04,
    SingleBitEcc = 0x05,
    MutliBitEcc = 0x06,
}

/// System Cache Type (Type 7, offset 0x11)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SystemCacheType {
    Other = 0x01,
    Unknown = 0x02,
    Instruction = 0x03,
    Data = 0x04,
    Unified = 0x05,
}

/// Cache Associativity Field (Type 7, offset 0x12)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum AssociativityField {
    Other = 0x01,
    Unknown = 0x02,
    DirectMapped = 0x03,
    SetAssociative2Way = 0x04,
    SetAssociative4Way = 0x05,
    FullyAssociative = 0x06,
    SetAssociative8Way = 0x07,
    SetAssociative16Way = 0x08,
    SetAssociative12Way = 0x09,
    SetAssociative24Way = 0x0A,
    SetAssociative32Way = 0x0B,
    SetAssociative48Way = 0x0C,
    SetAssociative64Way = 0x0D,
    SetAssociative20Way = 0x0E,
}

/// System Slot Type (Type 9, offset 0x05) - BYTE per spec
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SlotType {
    Other = 0x01,
    Unknown = 0x02,
    Isa = 0x03,
    Mca = 0x04,
    Eisa = 0x05,
    Pci = 0x06,
    PcCard = 0x07,
    VlVesa = 0x08,
    Proprietary = 0x09,
    ProcessorCardSlot = 0x0A,
    ProprietaryMemroyCardSlot = 0x0B,
    IoRiserCardSlot = 0x0C,
    NuBus = 0x0D,
    Pci66mhz = 0x0E,
    Agp = 0x0F,
    Agp2x = 0x10,
    Agp4x = 0x11,
    PciX = 0x12,
    Agp8x = 0x13,
    M2Socket1DP = 0x14,
    M2Socket1SD = 0x15,
    M2Socket2 = 0x16,
    M2Socket3 = 0x17,
    MxmTypeI = 0x18,
    MxmTypeII = 0x19,
    MxmTypeIIIStandard = 0x1A,
    MxmTypeIIIHe = 0x1B,
    MxmTypeIV = 0x1C,
    Mxm3TypeA = 0x1D,
    Mxm3TypeB = 0x1E,
    PciExpressGen2Sff8629 = 0x1F,
    PciExpressGen3Sff8629 = 0x20,
    PciExpressMini52PinBottomSideKeepOuts = 0x21,
    PciExpressMini52Pin = 0x22,
    PciExpressMini76Pin = 0x23,
    PciExpressGen4Sff8639 = 0x24,
    PciExpressGen5Sff8639 = 0x25,
    OcpNic3SFF = 0x26,
    OcpNic3LFF = 0x27,
    OcpNicPrior = 0x28,
    Pc98C20 = 0xA0,
    Pc98C24 = 0xA1,
    Pc98E = 0xA2,
    Pc98LocalBus = 0xA3,
    Pc98Card = 0xA4,
    PciExpress = 0xA5,
    PciExpressx1 = 0xA6,
    PciExpressx2 = 0xA7,
    PciExpressx4 = 0xA8,
    PciExpressx8 = 0xA9,
    PciExpressx16 = 0xAA,
    PciExpressGen2 = 0xAB,
    PciExpressGen2x1 = 0xAC,
    PciExpressGen2x2 = 0xAD,
    PciExpressGen2x4 = 0xAE,
    PciExpressGen2x8 = 0xAF,
    PciExpressGen2x16 = 0xB0,
    PciExpressGen3 = 0xB1,
    PciExpressGen3x1 = 0xB2,
    PciExpressGen3x2 = 0xB3,
    PciExpressGen3x4 = 0xB4,
    PciExpressGen3x8 = 0xB5,
    PciExpressGen3x16 = 0xB6,
    PciExpressGen4 = 0xB8,
    PciExpressGen4x1 = 0xB9,
    PciExpressGen4x2 = 0xBA,
    PciExpressGen4x4 = 0xBB,
    PciExpressGen4x8 = 0xBC,
    PciExpressGen4x16 = 0xBD,
    PciExpressGen5 = 0xBE,
    PciExpressGen5x1 = 0xBF,
    PciExpressGen5x2 = 0xC0,
    PciExpressGen5x4 = 0xC1,
    PciExpressGen5x8 = 0xC2,
    PciExpressGen5x16 = 0xC3,
    PciExpressGen6 = 0xC4,
    EdsffE1SE1L = 0xC5,
    EdsffE3SE3L = 0xC6,
}

/// System Slot Data Bus Width (Type 9, offset 0x06)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SlotWidth {
    Other = 0x01,
    Unknown = 0x02,
    Bit8 = 0x03,
    Bit16 = 0x04,
    Bit32 = 0x05,
    Bit64 = 0x06,
    Bit128 = 0x07,
    X1 = 0x08,
    X2 = 0x09,
    X4 = 0x0A,
    X8 = 0x0B,
    X12 = 0x0C,
    X16 = 0x0D,
    X32 = 0x0E,
}

/// System Slot Current Usage (Type 9, offset 0x07)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum CurrentUsage {
    Other = 0x01,
    Unknown = 0x02,
    Available = 0x03,
    InUse = 0x04,
    Unavailable = 0x05,
}

/// System Slot Length (Type 9, offset 0x08)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SlotLength {
    Other = 0x01,
    Unknown = 0x02,
    ShortLength = 0x03,
    LongLength = 0x04,
    DriveFF25 = 0x05,
    DriveFF35 = 0x06,
}

/// System Slot Characteristics 1 (Type 9, offset 0x0B)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct SlotCharacteristics1 {
    pub characteristics_unknown: bool,
    pub provides_5_volts: bool,
    pub provides_3_volts: bool,
    pub shared_slot: bool,
    pub pc_supports_pccard16: bool,
    pub pc_supports_cardbus: bool,
    pub pc_supports_zoomvideo: bool,
    pub pc_supports_modemringresume: bool,
}

/// System Slot Characteristics 2 (Type 9, offset 0x0C)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct SlotCharacteristics2 {
    pub pci_supports_pme: bool,
    pub supports_hotplug: bool,
    pub pci_supports_smbus: bool,
    pub pcie_supports_bifurcation: bool,
    pub supports_async_removal: bool,
    pub flexbus_slot1: bool,
    pub flexbus_slot2: bool,
    pub flexbus_slot3: bool,
}

/// System Slot Device/Function Number (Type 9, offset 0x10)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct DeviceFunctionNumber {
    #[bits(3)]
    pub function_number: u8,
    #[bits(5)]
    pub device_number: u8,
}

/// System Slot Peer Group entry (5 bytes)
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub struct MiscSlotPeerGroup {
    pub segment_group_num: u16,
    pub bus_num: u8,
    pub dev_func_num: DeviceFunctionNumber,
    pub data_bus_width: u8,
}

/// Memory Array Location (Type 16, offset 0x04)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryArrayLocation {
    Other = 0x01,
    Unknown = 0x02,
    SystemBoard = 0x03,
    IsaAddOn = 0x04,
    EisaAddOn = 0x05,
    PciAddOn = 0x06,
    McaAddOn = 0x07,
    PcmciaAddOn = 0x08,
    ProprietaryAddOn = 0x09,
    NuBus = 0x0A,
    Pc98C20AddOn = 0xA0,
    Pc98C24AddOn = 0xA1,
    Pc98EAddOn = 0xA2,
    Pc98LocalAddOn = 0xA3,
    CxlAddOn = 0xA4,
}

/// Memory Array Use (Type 16, offset 0x05)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryArrayUse {
    Other = 0x01,
    Unknown = 0x02,
    SystemMemory = 0x03,
    VideoMemory = 0x04,
    FlashMemory = 0x05,
    NonVolatileRam = 0x06,
    CacheMemory = 0x07,
}

/// Memory Array Error Correction Types (Type 16, offset 0x06)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ErrorCorrectionTypes {
    Other = 0x01,
    Unknown = 0x02,
    NoEcc = 0x03,
    Parity = 0x04,
    SingleBitEcc = 0x05,
    MultiBitEcc = 0x06,
    Crc = 0x07,
}

/// Memory Device Form Factor (Type 17, offset 0x0E)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
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
    Camm = 0x11,
    Cudimm = 0x12,
    Csodimm = 0x13,
}

/// Memory Device Memory Type (Type 17, offset 0x12)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
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
    Mrdimm = 0x25,
}

/// Memory Device Type Detail (Type 17, offset 0x13)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct MemoryDeviceTypeDetails {
    pub reserved: bool,
    pub other: bool,
    pub unknown: bool,
    pub fast_paged: bool,
    pub static_column: bool,
    pub pseudo_static: bool,
    pub rambus: bool,
    pub synchronous: bool,
    pub cmos: bool,
    pub edo: bool,
    pub window_dram: bool,
    pub cache_dram: bool,
    pub nonvolatile: bool,
    pub registered: bool,
    pub unbuffered: bool,
    pub lr_dimm: bool,
}

/// Memory Device Memory Technology (Type 17, offset 0x28)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryDeviceTechnology {
    Other = 0x01,
    Unknown = 0x02,
    Dram = 0x03,
    NvdimmN = 0x04,
    NvdimmF = 0x05,
    NvdimmP = 0x06,
    IntelOptanePersistentMemory = 0x07,
}

/// Memory Device Attributes (Type 17, offset 0x21)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct MemoryDeviceAttributes {
    #[bits(4)]
    pub rank: u8,
    #[bits(4)]
    pub reserved: u8,
}

/// Memory Device Operating Mode Capability (Type 17, offset 0x29)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct MemoryCapability {
    pub reserved: bool,
    pub other: bool,
    pub unknown: bool,
    pub volatile_memory: bool,
    pub byte_persistent_memory: bool,
    pub block_persistent_memory: bool,
    #[bits(10)]
    pub reserved2: u16,
}
