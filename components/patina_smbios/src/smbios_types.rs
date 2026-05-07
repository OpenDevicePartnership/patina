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

extern crate alloc;

use bitfield_struct::bitfield;
use zerocopy::{Immutable, IntoBytes, KnownLayout};

/// BIOS Characteristics (Type 0, offset 0x0A) - 8 bytes
#[bitfield(u64)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct BiosCharacteristics {
    /// Reserved bits.
    #[bits(2)]
    pub reserved: u8,
    /// Unknown.
    pub unknown: bool,
    /// BIOS characteristics are not supported.
    pub bios_characteristics_unsupported: bool,
    /// ISA is supported.
    pub isa_supported: bool,
    /// MCA is supported.
    pub mca_supported: bool,
    /// EISA is supported.
    pub eisa_supported: bool,
    /// PCI is supported.
    pub pci_supported: bool,
    /// PC Card (PCMCIA) is supported.
    pub pcmcia_supported: bool,
    /// Plug and Play is supported.
    pub plug_play_supported: bool,
    /// APM is supported.
    pub apm_supported: bool,
    /// BIOS is upgradeable (Flash).
    pub bios_is_upgradable: bool,
    /// BIOS shadowing is allowed.
    pub bios_shadowing_allowed: bool,
    /// VL-VESA is supported.
    pub vlvesa_supported: bool,
    /// ESCD support is available.
    pub escd_supported: bool,
    /// Boot from CD is supported.
    pub cd_boot_supported: bool,
    /// Selectable boot is supported.
    pub selectable_boot_supported: bool,
    /// BIOS ROM is socketed.
    pub bios_rom_socketed: bool,
    /// Boot from PC Card (PCMCIA) is supported.
    pub pc_card_boot_supported: bool,
    /// EDD specification is supported.
    pub edd_spec_supported: bool,
    /// Japanese floppy for NEC 9800 1.2 MB (3.5", 1K bytes/sector, 360 RPM) is supported.
    pub japanese_nec_9800_supported: bool,
    /// Japanese floppy for Toshiba 1.2 MB (3.5", 360 RPM) is supported.
    pub japanese_toshiba_supported: bool,
    /// 5.25" / 360 KB floppy services are supported.
    pub kb_525_360_supported: bool,
    /// 5.25" / 1.2 MB floppy services are supported.
    pub mb_525_12_supported: bool,
    /// 3.5" / 720 KB floppy services are supported.
    pub mb_35_720_supported: bool,
    /// 3.5" / 2.88 MB floppy services are supported.
    pub mb_35_288_supported: bool,
    /// Print Screen service is supported.
    pub print_screen_supported: bool,
    /// 8042 keyboard services are supported.
    pub keyboard_8042_supported: bool,
    /// Serial services are supported.
    pub serial_services_supported: bool,
    /// Printer services are supported.
    pub printer_services_supported: bool,
    /// CGA/Mono video services are supported.
    pub cga_mono_video_supported: bool,
    /// NEC PC-98 system.
    pub nec_pc_98: bool,
    /// Reserved for BIOS vendor.
    #[bits(16)]
    pub reserved_bios_vendor: u16,
    /// Reserved for system vendor.
    #[bits(16)]
    pub reserved_system_vendor: u16,
}

/// BIOS Characteristics Extension Byte 1 (Type 0, offset 0x12)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct BiosCharacteristicsExt1 {
    /// ACPI is supported.
    pub acpi_supported: bool,
    /// USB Legacy is supported.
    pub usb_legacy_supported: bool,
    /// AGP is supported.
    pub agp_supported: bool,
    /// I2O boot is supported.
    pub i2o_supported: bool,
    /// LS-120 SuperDisk boot is supported.
    pub superdisk_boot_supported: bool,
    /// ATAPI ZIP drive boot is supported.
    pub zip_drive_boot_supported: bool,
    /// 1394 boot is supported.
    pub boot_1394_supported: bool,
    /// Smart battery is supported.
    pub smart_battery_supported: bool,
}

/// BIOS Characteristics Extension Byte 2 (Type 0, offset 0x13)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct BiosCharacteristicsExt2 {
    /// BIOS Boot Specification is supported.
    pub bios_boot_specification_supported: bool,
    /// Function-key initiated network service boot is supported.
    pub fn_network_service_boot_supported: bool,
    /// Enable Targeted Content Distribution.
    pub enable_targeted_content_distribution: bool,
    /// UEFI Specification is supported.
    pub uefi_spec_supported: bool,
    /// SMBIOS table describes a virtual machine.
    pub smbios_describes_vm: bool,
    /// Reserved bits.
    #[bits(3)]
    pub reserved: u8,
}

/// Extended BIOS ROM Size (Type 0, offset 0x18) - 2 bytes
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ExtendedBiosRomSize {
    /// ROM size value.
    #[bits(14)]
    pub size: u16,
    /// Unit encoding: 0b00 = megabytes, 0b01 = gigabytes.
    #[bits(2)]
    pub unit: u8,
}

/// Wake-Up Type (Type 1, offset 0x18)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum WakeUpType {
    /// Reserved.
    Reserved = 0x00,
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// APM timer.
    ApmTimer = 0x03,
    /// Modem ring.
    ModemRing = 0x04,
    /// LAN remote.
    LanRemote = 0x05,
    /// Power switch.
    PowerSwitch = 0x06,
    /// PCI PME#.
    PciPme = 0x07,
    /// AC power restored.
    AcPowerRestored = 0x08,
}

/// Baseboard Feature Flags (Type 2, offset 0x09)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct FeatureFlags {
    /// Board is a hosting board (e.g. motherboard).
    pub hosting_board: bool,
    /// Board requires at least one daughter board / auxiliary card.
    pub require_aux_board: bool,
    /// Board is removable.
    pub removable_board: bool,
    /// Board is replaceable.
    pub replaceable_board: bool,
    /// Board is hot-swappable.
    pub hot_swappable_board: bool,
    /// Reserved bits.
    #[bits(3)]
    pub reserved: u8,
}

/// Baseboard Type (Type 2, offset 0x0D)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum BoardType {
    /// Unknown.
    Unknown = 0x01,
    /// Other.
    Other = 0x02,
    /// Server blade.
    ServerBlade = 0x03,
    /// Connectivity switch.
    ConnectivitySwitch = 0x04,
    /// System management module.
    SystemManagementModule = 0x05,
    /// Processor module.
    ProcessorModule = 0x06,
    /// I/O module.
    IoModule = 0x07,
    /// Memory module.
    MemoryModule = 0x08,
    /// Daughter board.
    DaughterBoard = 0x09,
    /// Motherboard (includes processor, memory, and I/O).
    Motherboard = 0x0A,
    /// Processor/memory module.
    ProcessorMemoryModule = 0x0B,
    /// Processor/I/O module.
    ProcessorIoModule = 0x0C,
    /// Interconnect board.
    InterconnectBoard = 0x0D,
}

/// System Enclosure Boot-Up State (Type 3, offset 0x09).
///
/// Per SMBIOS spec Table 17 (System — Boot Up State).
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum BootUpState {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Safe.
    Safe = 0x03,
    /// Warning.
    Warning = 0x04,
    /// Critical.
    Critical = 0x05,
    /// Non-recoverable.
    NonRecoverable = 0x06,
}

/// System Enclosure Power Supply State (Type 3, offset 0x0A).
///
/// Per SMBIOS spec Table 17 (Power Supply State).
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum PowerSupplyState {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Safe.
    Safe = 0x03,
    /// Warning.
    Warning = 0x04,
    /// Critical.
    Critical = 0x05,
    /// Non-recoverable.
    NonRecoverable = 0x06,
}

/// System Enclosure Thermal State (Type 3, offset 0x0B)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ThermalState {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Safe.
    Safe = 0x03,
    /// Warning.
    Warning = 0x04,
    /// Critical.
    Critical = 0x05,
    /// Non-recoverable.
    NonRecoverable = 0x06,
}

/// System Enclosure Security Status (Type 3, offset 0x0C).
///
/// `NoneStatus` is named to avoid the reserved `None` keyword.
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SecurityStatus {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// No security status; named `NoneStatus` to avoid the reserved `None` keyword.
    NoneStatus = 0x03,
    /// External interface locked out.
    ExternalInterfaceLockedOut = 0x04,
    /// External interface enabled.
    ExternalInterfaceEnabled = 0x05,
}

/// Contained Element Type (Type 3 element record, byte 0)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ContainedElementType {
    /// Type value (SMBIOS record type if `type_select == true`, otherwise baseboard type).
    #[bits(7)]
    pub r#type: u8,
    /// Type selector: `false` = baseboard type (Type 2 enumeration), `true` = SMBIOS record type.
    pub type_select: bool,
}

/// Contained Element (Type 3 element record - 3 bytes)
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub struct ContainedElements {
    /// Type of contained element.
    pub contained_element_type: ContainedElementType,
    /// Minimum number of this contained element type.
    pub contained_element_minimum: u8,
    /// Maximum number of this contained element type.
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
    /// Other.
    ProcessorOther = 0x01,
    /// Unknown.
    ProcessorUnknown = 0x02,
    /// Central processor.
    CentralProcessor = 0x03,
    /// Math processor.
    MathProcessor = 0x04,
    /// DSP processor.
    DspProcessor = 0x05,
    /// Video processor.
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
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// 8086.
    Processor8086 = 0x03,
    /// 80286.
    Processor80286 = 0x04,
    /// Intel386™ processor.
    Intel386 = 0x05,
    /// Intel486™ processor.
    Intel486 = 0x06,
    /// 8087.
    Processor8087 = 0x07,
    /// 80287.
    Processor80287 = 0x08,
    /// 80387.
    Processor80387 = 0x09,
    /// 80487.
    Processor80487 = 0x0A,
    /// Intel® Pentium® processor.
    Pentium = 0x0B,
    /// Pentium® Pro processor.
    PentiumPro = 0x0C,
    /// Pentium® II processor.
    PentiumII = 0x0D,
    /// Pentium® processor with MMX™ technology.
    PentiumMMX = 0x0E,
    /// Intel® Celeron® processor.
    Celeron = 0x0F,
    /// Pentium® II Xeon™ processor.
    PentiumIIXeon = 0x10,
    /// Pentium® III processor.
    PentiumIII = 0x11,
    /// M1 family.
    M1Family = 0x12,
    /// M2 family.
    M2Family = 0x13,
    /// Intel® Celeron® M processor.
    IntelCeleronM = 0x14,
    /// Intel® Pentium® 4 HT processor.
    IntelPentium4Ht = 0x15,
    /// Intel® processor.
    IntelProcessor = 0x16,
    /// AMD Duron™ processor family.
    AmdDuron = 0x18,
    /// K5 family.
    K5Family = 0x19,
    /// K6 family.
    K6Family = 0x1A,
    /// K6-2.
    K6_2 = 0x1B,
    /// K6-3.
    K6_3 = 0x1C,
    /// AMD Athlon™ processor family.
    AmdAthlon = 0x1D,
    /// AMD29000 family.
    Amd29000 = 0x1E,
    /// K6-2+.
    K6_2Plus = 0x1F,
    /// Power PC family.
    PowerPC = 0x20,
    /// Power PC 601.
    PowerPC601 = 0x21,
    /// Power PC 603.
    PowerPC603 = 0x22,
    /// Power PC 603+.
    PowerPC603Plus = 0x23,
    /// Power PC 604.
    PowerPC604 = 0x24,
    /// Power PC 620.
    PowerPC620 = 0x25,
    /// Power PC x704.
    PowerPCx704 = 0x26,
    /// Power PC 750.
    PowerPC750 = 0x27,
    /// Intel® Core™ Duo processor.
    IntelCoreDuo = 0x28,
    /// Intel® Core™ Duo mobile processor.
    IntelCoreDuoMobile = 0x29,
    /// Intel® Core™ Solo mobile processor.
    IntelCoreSoloMobile = 0x2A,
    /// Intel® Atom™ processor.
    IntelAtom = 0x2B,
    /// Intel® Core™ M processor.
    IntelCoreM = 0x2C,
    /// Intel® Core™ m3 processor.
    IntelCorem3 = 0x2D,
    /// Intel® Core™ m5 processor.
    IntelCorem5 = 0x2E,
    /// Intel® Core™ m7 processor.
    IntelCorem7 = 0x2F,
    /// Alpha family.
    Alpha = 0x30,
    /// Alpha 21064.
    Alpha21064 = 0x31,
    /// Alpha 21066.
    Alpha21066 = 0x32,
    /// Alpha 21164.
    Alpha21164 = 0x33,
    /// Alpha 21164PC.
    Alpha21164PC = 0x34,
    /// Alpha 21164a.
    Alpha21164a = 0x35,
    /// Alpha 21264.
    Alpha21264 = 0x36,
    /// Alpha 21364.
    Alpha21364 = 0x37,
    /// AMD Turion™ II Ultra Dual-Core Mobile M processor family.
    AmdTurionIIUltraDualCoreMobileM = 0x38,
    /// AMD Turion™ II Dual-Core Mobile M processor family.
    AmdTurionIIDualCoreMobileM = 0x39,
    /// AMD Athlon™ II Dual-Core M processor family.
    AmdAthlonIIDualCoreM = 0x3A,
    /// AMD Opteron™ 6100 Series processor.
    AmdOpteron6100Series = 0x3B,
    /// AMD Opteron™ 4100 Series processor.
    AmdOpteron4100Series = 0x3C,
    /// AMD Opteron™ 6200 Series processor.
    AmdOpteron6200Series = 0x3D,
    /// AMD Opteron™ 4200 Series processor.
    AmdOpteron4200Series = 0x3E,
    /// AMD FX™ Series processor.
    AmdFxSeries = 0x3F,
    /// MIPS family.
    MipsFamily = 0x40,
    /// MIPS R4000.
    MipsR4000 = 0x41,
    /// MIPS R4200.
    MipsR4200 = 0x42,
    /// MIPS R4400.
    MipsR4400 = 0x43,
    /// MIPS R4600.
    MipsR4600 = 0x44,
    /// MIPS R10000.
    MipsR10000 = 0x45,
    /// AMD C-Series processor.
    AmdCSeries = 0x46,
    /// AMD E-Series processor.
    AmdESeries = 0x47,
    /// AMD A-Series processor.
    AmdASeries = 0x48,
    /// AMD G-Series processor.
    AmdGSeries = 0x49,
    /// AMD Z-Series processor.
    AmdZSeries = 0x4A,
    /// AMD R-Series processor.
    AmdRSeries = 0x4B,
    /// AMD Opteron™ 4300 Series processor.
    AmdOpteron4300 = 0x4C,
    /// AMD Opteron™ 6300 Series processor.
    AmdOpteron6300 = 0x4D,
    /// AMD Opteron™ 3300 Series processor.
    AmdOpteron3300 = 0x4E,
    /// AMD FirePro™ Series processor.
    AmdFireProSeries = 0x4F,
    /// SPARC family.
    Sparc = 0x50,
    /// SuperSPARC.
    SuperSparc = 0x51,
    /// microSPARC II.
    MicroSparcII = 0x52,
    /// microSPARC IIep.
    MicroSparcIIep = 0x53,
    /// UltraSPARC.
    UltraSparc = 0x54,
    /// UltraSPARC II.
    UltraSparcII = 0x55,
    /// UltraSPARC IIi.
    UltraSparcIii = 0x56,
    /// UltraSPARC III.
    UltraSparcIII = 0x57,
    /// UltraSPARC IIIi.
    UltraSparcIIIi = 0x58,
    /// 68040.
    Processor68040 = 0x60,
    /// 68xxx.
    Processor68xxx = 0x61,
    /// 68000.
    Processor68000 = 0x62,
    /// 68010.
    Processor68010 = 0x63,
    /// 68020.
    Processor68020 = 0x64,
    /// 68030.
    Processor68030 = 0x65,
    /// AMD Athlon™ X4 Quad-Core processor.
    AmdAthlonX4QuadCore = 0x66,
    /// AMD Opteron™ X1000 Series processor.
    AmdOpteronX1000Series = 0x67,
    /// AMD Opteron™ X2000 Series APU.
    AmdOpteronX2000Series = 0x68,
    /// AMD Opteron™ A-Series processor.
    AmdOpteronASeries = 0x69,
    /// AMD Opteron™ X3000 Series APU.
    AmdOpteronX3000Series = 0x6A,
    /// AMD Zen processor family.
    AmdZen = 0x6B,
    /// Hobbit family.
    HobbitFamily = 0x70,
    /// Crusoe™ TM5000 family.
    CrusoeTM5000 = 0x78,
    /// Crusoe™ TM3000 family.
    CrusoeTM3000 = 0x79,
    /// Efficeon™ TM8000 family.
    EfficeonTM8000 = 0x7A,
    /// Weitek.
    Weitek = 0x80,
    /// Itanium™ processor.
    Itanium = 0x82,
    /// AMD Athlon™ 64 processor family.
    AmdAthlon64 = 0x83,
    /// AMD Opteron™ processor family.
    AmdOpteron = 0x84,
    /// AMD Sempron™ processor family.
    AmdSempron = 0x85,
    /// AMD Turion™ 64 Mobile Technology.
    AmdTurion64Mobile = 0x86,
    /// Dual-Core AMD Opteron™ processor family.
    DualCoreAmdOpteron = 0x87,
    /// AMD Athlon™ 64 X2 Dual-Core processor family.
    AmdAthlon64X2DualCore = 0x88,
    /// AMD Turion™ 64 X2 Mobile Technology.
    AmdTurion64X2Mobile = 0x89,
    /// Quad-Core AMD Opteron™ processor family.
    QuadCoreAmdOpteron = 0x8A,
    /// Third-Generation AMD Opteron™ processor family.
    ThirdGenerationAmdOpteron = 0x8B,
    /// AMD Phenom™ FX Quad-Core processor family.
    AmdPhenomFxQuadCore = 0x8C,
    /// AMD Phenom™ X4 Quad-Core processor family.
    AmdPhenomX4QuadCore = 0x8D,
    /// AMD Phenom™ X2 Dual-Core processor family.
    AmdPhenomX2DualCore = 0x8E,
    /// AMD Athlon™ X2 Dual-Core processor family.
    AmdAthlonX2DualCore = 0x8F,
    /// PA-RISC family.
    Parisc = 0x90,
    /// PA-RISC 8500.
    PaRisc8500 = 0x91,
    /// PA-RISC 8000.
    PaRisc8000 = 0x92,
    /// PA-RISC 7300LC.
    PaRisc7300LC = 0x93,
    /// PA-RISC 7200.
    PaRisc7200 = 0x94,
    /// PA-RISC 7100LC.
    PaRisc7100LC = 0x95,
    /// PA-RISC 7100.
    PaRisc7100 = 0x96,
    /// V30 family.
    V30Family = 0xA0,
    /// Quad-Core Intel® Xeon® processor 3200 Series.
    QuadCoreIntelXeon3200Series = 0xA1,
    /// Dual-Core Intel® Xeon® processor 3000 Series.
    DualCoreIntelXeon3000Series = 0xA2,
    /// Quad-Core Intel® Xeon® processor 5300 Series.
    QuadCoreIntelXeon5300Series = 0xA3,
    /// Dual-Core Intel® Xeon® processor 5100 Series.
    DualCoreIntelXeon5100Series = 0xA4,
    /// Dual-Core Intel® Xeon® processor 5000 Series.
    DualCoreIntelXeon5000Series = 0xA5,
    /// Dual-Core Intel® Xeon® processor LV.
    DualCoreIntelXeonLV = 0xA6,
    /// Dual-Core Intel® Xeon® processor ULV.
    DualCoreIntelXeonULV = 0xA7,
    /// Dual-Core Intel® Xeon® processor 7100 Series.
    DualCoreIntelXeon7100Series = 0xA8,
    /// Quad-Core Intel® Xeon® processor 5400 Series.
    QuadCoreIntelXeon5400Series = 0xA9,
    /// Quad-Core Intel® Xeon® processor.
    QuadCoreIntelXeon = 0xAA,
    /// Dual-Core Intel® Xeon® processor 5200 Series.
    DualCoreIntelXeon5200Series = 0xAB,
    /// Dual-Core Intel® Xeon® processor 7200 Series.
    DualCoreIntelXeon7200Series = 0xAC,
    /// Quad-Core Intel® Xeon® processor 7300 Series.
    QuadCoreIntelXeon7300Series = 0xAD,
    /// Quad-Core Intel® Xeon® processor 7400 Series.
    QuadCoreIntelXeon7400Series = 0xAE,
    /// Multi-Core Intel® Xeon® processor 7400 Series.
    MultiCoreIntelXeon7400Series = 0xAF,
    /// Pentium® III Xeon™ processor.
    PentiumIIIXeon = 0xB0,
    /// Pentium® III processor with Intel® SpeedStep™ technology.
    PentiumIIISpeedStep = 0xB1,
    /// Pentium® 4 processor.
    Pentium4 = 0xB2,
    /// Intel® Xeon® processor.
    IntelXeon = 0xB3,
    /// AS400 family.
    As400 = 0xB4,
    /// Intel® Xeon™ processor MP.
    IntelXeonMP = 0xB5,
    /// AMD Athlon™ XP processor family.
    AMDAthlonXP = 0xB6,
    /// AMD Athlon™ MP processor family.
    AMDAthlonMP = 0xB7,
    /// Intel® Itanium® 2 processor.
    IntelItanium2 = 0xB8,
    /// Intel® Pentium® M processor.
    IntelPentiumM = 0xB9,
    /// Intel® Celeron® D processor.
    IntelCeleronD = 0xBA,
    /// Intel® Pentium® D processor.
    IntelPentiumD = 0xBB,
    /// Intel® Pentium® Processor Extreme Edition.
    IntelPentiumEx = 0xBC,
    /// Intel® Core™ Solo Processor.
    IntelCoreSolo = 0xBD,
    /// Reserved.
    Reserved = 0xBE,
    /// Intel® Core™ 2 Processor.
    IntelCore2 = 0xBF,
    /// Intel® Core™ 2 Solo processor.
    IntelCore2Solo = 0xC0,
    /// Intel® Core™ 2 Extreme processor.
    IntelCore2Extreme = 0xC1,
    /// Intel® Core™ 2 Quad processor.
    IntelCore2Quad = 0xC2,
    /// Intel® Core™ 2 Extreme mobile processor.
    IntelCore2ExtremeMobile = 0xC3,
    /// Intel® Core™ 2 Duo mobile processor.
    IntelCore2DuoMobile = 0xC4,
    /// Intel® Core™ 2 Solo mobile processor.
    IntelCore2SoloMobile = 0xC5,
    /// Intel® Core™ i7 processor.
    IntelCoreI7 = 0xC6,
    /// Dual-Core Intel® Celeron® processor.
    DualCoreIntelCeleron = 0xC7,
    /// IBM390 family.
    Ibm390 = 0xC8,
    /// G4.
    G4 = 0xC9,
    /// G5.
    G5 = 0xCA,
    /// ESA/390 G6.
    EsaG6 = 0xCB,
    /// z/Architecture base.
    ZArchitecture = 0xCC,
    /// Intel® Core™ i5 processor.
    IntelCoreI5 = 0xCD,
    /// Intel® Core™ i3 processor.
    IntelCoreI3 = 0xCE,
    /// Intel® Core™ i9 processor.
    IntelCoreI9 = 0xCF,
    /// Intel® Xeon® D processor.
    IntelXeonD = 0xD0,
    /// VIA C7™-M processor family.
    ViaC7M = 0xD2,
    /// VIA C7™-D processor family.
    ViaC7D = 0xD3,
    /// VIA C7™ processor family.
    ViaC7 = 0xD4,
    /// VIA Eden™ processor family.
    ViaEden = 0xD5,
    /// Multi-Core Intel® Xeon® processor.
    MultiCoreIntelXeon = 0xD6,
    /// Dual-Core Intel® Xeon® processor 3xxx Series.
    DualCoreIntelXeon3Series = 0xD7,
    /// Quad-Core Intel® Xeon® processor 3xxx Series.
    QuadCoreIntelXeon3Series = 0xD8,
    /// VIA Nano™ processor family.
    ViaNano = 0xD9,
    /// Dual-Core Intel® Xeon® processor 5xxx Series.
    DualCoreIntelXeon5Series = 0xDA,
    /// Quad-Core Intel® Xeon® processor 5xxx Series.
    QuadCoreIntelXeon5Series = 0xDB,
    /// Dual-Core Intel® Xeon® processor 7xxx Series.
    DualCoreIntelXeon7Series = 0xDD,
    /// Quad-Core Intel® Xeon® processor 7xxx Series.
    QuadCoreIntelXeon7Series = 0xDE,
    /// Multi-Core Intel® Xeon® processor 7xxx Series.
    MultiCoreIntelXeon7Series = 0xDF,
    /// Multi-Core Intel® Xeon® processor 3400 Series.
    MultiCoreIntelXeon3400Series = 0xE0,
    /// AMD Opteron™ 3000 Series processor.
    AmdOpteron3000Series = 0xE4,
    /// AMD Sempron™ II processor.
    AmdSempronII = 0xE5,
    /// Embedded AMD Opteron™ Quad-Core processor family.
    EmbeddedAmdOpteronQuadCore = 0xE6,
    /// AMD Phenom™ Triple-Core processor family.
    AmdPhenomTripleCore = 0xE7,
    /// AMD Turion™ Ultra Dual-Core Mobile processor family.
    AmdTurionUltraDualCoreMobile = 0xE8,
    /// AMD Turion™ Dual-Core Mobile processor family.
    AmdTurionDualCoreMobile = 0xE9,
    /// AMD Athlon™ Dual-Core processor family.
    AmdAthlonDualCore = 0xEA,
    /// AMD Sempron™ SI processor family.
    AmdSempronSI = 0xEB,
    /// AMD Phenom™ II processor family.
    AmdPhenomII = 0xEC,
    /// AMD Athlon™ II processor family.
    AmdAthlonII = 0xED,
    /// Six-Core AMD Opteron™ processor family.
    SixCoreAmdOpteron = 0xEE,
    /// AMD Sempron™ M processor family.
    AmdSempronM = 0xEF,
    /// i860.
    I860 = 0xFA,
    /// i960.
    I960 = 0xFB,
    /// Use this u8 marker (0xFE) in `processor_family` to indicate that the
    /// real value is in `processor_family2`.
    IndicatorFamily2 = 0xFE,
    /// Reserved.
    Reserved1 = 0xFF,
    /// ARMv7.
    ARMv7 = 0x0100,
    /// ARMv8.
    ARMv8 = 0x0101,
    /// ARMv9.
    ARMv9 = 0x0102,
    /// SH-3.
    Sh3 = 0x0103,
    /// SH-4.
    Sh4 = 0x0104,
    /// ARM.
    Arm = 0x0118,
    /// StrongARM.
    StrongARM = 0x0119,
    /// 6x86.
    Processor6x86 = 0x012C,
    /// MediaGX.
    MediaGX = 0x012D,
    /// MII.
    Mii = 0x012E,
    /// WinChip.
    WinChip = 0x0140,
    /// DSP.
    Dsp = 0x015E,
    /// Video processor.
    VideoProcessor = 0x01F4,
    /// RISC-V RV32.
    RiscvRV32 = 0x0200,
    /// RISC-V RV64.
    RiscVRV64 = 0x0201,
    /// RISC-V RV128.
    RiscVRV128 = 0x0202,
    /// LoongArch.
    LoongArch = 0x0258,
    /// Loongson™ 1 processor.
    Loongson1 = 0x0259,
    /// Loongson™ 2 processor.
    Loongson2 = 0x025A,
    /// Loongson™ 3 processor.
    Loongson3 = 0x025B,
    /// Loongson™ 2K processor.
    Loongson2K = 0x025C,
    /// Loongson™ 3A processor.
    Loongson3A = 0x025D,
    /// Loongson™ 3B processor.
    Loongson3B = 0x025E,
    /// Loongson™ 3C processor.
    Loongson3C = 0x025F,
    /// Loongson™ 3D processor.
    Loongson3D = 0x0260,
    /// Loongson™ 3E processor.
    Loongson3E = 0x0261,
    /// Dual-Core Loongson™ 2K processor 2xxx Series.
    DualCoreLoongson2K = 0x0262,
    /// Quad-Core Loongson™ 3A processor 5xxx Series.
    QuadCoreLoongson3A = 0x026C,
    /// Multi-Core Loongson™ 3A processor 5xxx Series.
    MultiCoreLoongson3A = 0x026D,
    /// Quad-Core Loongson™ 3B processor 5xxx Series.
    QuadCoreLoongson3B = 0x026E,
    /// Multi-Core Loongson™ 3B processor 5xxx Series.
    MultiCoreLoongson3B = 0x026F,
    /// Multi-Core Loongson™ 3C processor 5xxx Series.
    MultiCoreLoongson3C = 0x0270,
    /// Multi-Core Loongson™ 3D processor 5xxx Series.
    MultiCoreLoongson3D = 0x0271,
    /// Intel® Core™ 3 processor.
    IntelCore3 = 0x0300,
    /// Intel® Core™ 5 processor.
    IntelCore5 = 0x0301,
    /// Intel® Core™ 7 processor.
    IntelCore7 = 0x0302,
    /// Intel® Core™ 9 processor.
    IntelCore9 = 0x0303,
    /// Intel® Core™ Ultra 3 processor.
    IntelCoreUltra3 = 0x0304,
    /// Intel® Core™ Ultra 5 processor.
    IntelCoreUltra5 = 0x0305,
    /// Intel® Core™ Ultra 7 processor.
    IntelCoreUltra7 = 0x0306,
    /// Intel® Core™ Ultra 9 processor.
    IntelCoreUltra9 = 0x0307,
}

/// Processor Upgrade (Type 4, offset 0x19)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum ProcessorUpgrade {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Daughter board.
    DaughterBoard = 0x03,
    /// ZIF socket.
    ZIFSocket = 0x04,
    /// Replaceable piggy back.
    ReplaceablePiggyBack = 0x05,
    /// No upgrade.
    NoUpgrade = 0x06,
    /// LIF socket.
    LIFSocket = 0x07,
    /// Slot 1.
    Slot1 = 0x08,
    /// Slot 2.
    Slot2 = 0x09,
    /// 370-pin socket.
    Pin370Socket = 0x0A,
    /// Slot A.
    SlotA = 0x0B,
    /// Slot M.
    SlotM = 0x0C,
    /// Socket 423.
    Socket423 = 0x0D,
    /// Socket A (Socket 462).
    SocketA = 0x0E,
    /// Socket 478.
    Socket478 = 0x0F,
    /// Socket 754.
    Socket754 = 0x10,
    /// Socket 940.
    Socket940 = 0x11,
    /// Socket 939.
    Socket939 = 0x12,
    /// Socket mPGA604.
    SocketmPGA604 = 0x13,
    /// Socket LGA771.
    SocketLGA771 = 0x14,
    /// Socket LGA775.
    SocketLGA775 = 0x15,
    /// Socket S1.
    SocketS1 = 0x16,
    /// Socket AM2.
    SocketAM2 = 0x17,
    /// Socket F (1207).
    SocketF1207 = 0x18,
    /// Socket LGA1366.
    SocketLGA1366 = 0x19,
    /// Socket G34.
    SocketG34 = 0x1A,
    /// Socket AM3.
    SocketAM3 = 0x1B,
    /// Socket C32.
    SocketC32 = 0x1C,
    /// Socket LGA1156.
    SocketLGA1156 = 0x1D,
    /// Socket LGA1567.
    SocketLGA1567 = 0x1E,
    /// Socket PGA988A.
    SocketPGA988A = 0x1F,
    /// Socket BGA1288.
    SocketBGA1288 = 0x20,
    /// Socket rPGA988B.
    SocketrPGA988B = 0x21,
    /// Socket BGA1023.
    SocketBGA1023 = 0x22,
    /// Socket BGA1224.
    SocketBGA1224 = 0x23,
    /// Socket LGA1155.
    SocketLGA1155 = 0x24,
    /// Socket LGA1356.
    SocketLGA1356 = 0x25,
    /// Socket LGA2011.
    SocketLGA2011 = 0x26,
    /// Socket FS1.
    SocketFS1 = 0x27,
    /// Socket FS2.
    SocketFS2 = 0x28,
    /// Socket FM1.
    SocketFM1 = 0x29,
    /// Socket FM2.
    SocketFM2 = 0x2A,
    /// Socket LGA2011-3.
    SocketLGA2011_3 = 0x2B,
    /// Socket LGA1356-3.
    SocketLGA1356_3 = 0x2C,
    /// Socket LGA1150.
    SocketLGA1150 = 0x2D,
    /// Socket BGA1168.
    SocketBGA1168 = 0x2E,
    /// Socket BGA1234.
    SocketBGA1234 = 0x2F,
    /// Socket BGA1364.
    SocketBGA1364 = 0x30,
    /// Socket AM4.
    SocketAM4 = 0x31,
    /// Socket LGA1151.
    SocketLGA1151 = 0x32,
    /// Socket BGA1356.
    SocketBGA1356 = 0x33,
    /// Socket BGA1440.
    SocketBGA1440 = 0x34,
    /// Socket BGA1515.
    SocketBGA1515 = 0x35,
    /// Socket LGA3647-1.
    SocketLGA3647_1 = 0x36,
    /// Socket SP3.
    SocketSP3 = 0x37,
    /// Socket SP3r2.
    SocketSP3r2 = 0x38,
    /// Socket LGA2066.
    SocketLGA2066 = 0x39,
    /// Socket BGA1392.
    SocketBGA1392 = 0x3A,
    /// Socket BGA1510.
    SocketBGA1510 = 0x3B,
    /// Socket BGA1528.
    SocketBGA1528 = 0x3C,
    /// Socket LGA4189.
    SocketLGA4189 = 0x3D,
    /// Socket LGA1200.
    SocketLGA1200 = 0x3E,
    /// Socket LGA4677.
    SocketLGA4677 = 0x3F,
    /// Socket LGA1700.
    SocketLGA1700 = 0x40,
    /// Socket BGA1744.
    SocketBGA1744 = 0x41,
    /// Socket BGA1781.
    SocketBGA1781 = 0x42,
    /// Socket BGA1211.
    SocketBGA1211 = 0x43,
    /// Socket BGA2422.
    SocketBGA2422 = 0x44,
    /// Socket LGA1211.
    SocketLGA1211 = 0x45,
    /// Socket LGA2422.
    SocketLGA2422 = 0x46,
    /// Socket LGA5773.
    SocketLGA5773 = 0x47,
    /// Socket BGA5773.
    SocketBGA5773 = 0x48,
    /// Socket AM5.
    SocketAM5 = 0x49,
    /// Socket SP5.
    SocketSP5 = 0x4A,
    /// Socket SP6.
    SocketSP6 = 0x4B,
    /// Socket BGA883.
    SocketBGA883 = 0x4C,
    /// Socket BGA1190.
    SocketBGA1190 = 0x4D,
    /// Socket BGA4129.
    SocketBGA4129 = 0x4E,
    /// Socket LGA4710.
    SocketLGA4710 = 0x4F,
    /// Socket LGA7529.
    SocketLGA7529 = 0x50,
    /// Socket BGA1964.
    SocketBGA1964 = 0x51,
    /// Socket BGA1792.
    SocketBGA1792 = 0x52,
    /// Socket BGA2049.
    SocketBGA2049 = 0x53,
    /// Socket BGA2551.
    SocketBGA2551 = 0x54,
    /// Socket LGA1851.
    SocketLGA1851 = 0x55,
    /// Socket BGA2114.
    SocketBGA2114 = 0x56,
    /// Socket BGA2833.
    SocketBGA2833 = 0x57,
    /// Not available.
    NotAvailable = 0xFF,
}

/// Processor Voltage (Type 4, offset 0x11)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ProcessorVoltage {
    /// 5 V voltage capability.
    pub processor_voltage_capability_5v: bool,
    /// 3.3 V voltage capability.
    pub processor_voltage_capability_3_3v: bool,
    /// 2.9 V voltage capability.
    pub processor_voltage_capability_2_9v: bool,
    /// Reserved capability bit (must be zero).
    pub processor_voltage_capability_reserved: bool,
    /// Reserved bits.
    #[bits(3)]
    pub processor_voltage_reserved: u8,
    /// Set when the byte holds a legacy mode voltage (bits 0-3 are capabilities); clear when bits 0-6 hold the current voltage in tenths of a volt.
    pub processor_voltage_indicate_legacy: bool,
}

/// Processor Information Status (Type 4, offset 0x18)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ProcessorInformationStatus {
    /// CPU status: 0=unknown, 1=enabled, 2=disabled by user via BIOS, 3=disabled by BIOS (POST error), 4=idle, 7=other.
    #[bits(3)]
    pub cpu_status: u8,
    /// Reserved bits.
    #[bits(3)]
    pub reserved: u8,
    /// CPU socket populated.
    pub cpu_socket_populated: bool,
    /// Reserved bit.
    pub reserved2: bool,
}

/// Processor Characteristics (Type 4, offset 0x26)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct ProcessorCharacteristics {
    /// Reserved.
    pub reserved: bool,
    /// Unknown.
    pub unknown: bool,
    /// 64-bit capable.
    pub capable_64bit: bool,
    /// Multi-core.
    pub multi_core: bool,
    /// Hardware thread.
    pub hardware_thread: bool,
    /// Execute Protection.
    pub execute_protection: bool,
    /// Enhanced Virtualization.
    pub enhanced_virtualization: bool,
    /// Power/Performance Control.
    pub performance_control: bool,
    /// 128-bit capable.
    pub capable_128bit: bool,
    /// Arm64 SoC ID.
    pub arm64_soc_id: bool,
    /// Reserved bits.
    #[bits(6)]
    pub reserved2: u8,
}

/// Cache Configuration (Type 7, offset 0x05)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheConfiguration {
    /// Cache level: 0 = L1, 1 = L2, etc.
    #[bits(3)]
    pub cache_level: u8,
    /// Cache socketed.
    pub cache_socketed: bool,
    /// Reserved.
    pub reserved: bool,
    /// Cache location: 0 = internal, 1 = external, 3 = unknown.
    #[bits(2)]
    pub location: u8,
    /// Enabled (true) / disabled (false).
    pub enabled_disabled: bool,
    /// Operational mode: 0 = write-through, 1 = write-back, 2 = varies, 3 = unknown.
    #[bits(2)]
    pub operational_mode: u8,
    /// Reserved bits.
    #[bits(6)]
    pub reserved2: u8,
}

/// Cache Size (Type 7, offset 0x07 / 0x09)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheSize {
    /// Max cache size value (in units determined by `granularity`).
    #[bits(15)]
    pub max_size: u16,
    /// Granularity: false = 1 KB, true = 64 KB.
    pub granularity: bool,
}

/// Cache Size 2 (Type 7, offset 0x13 / 0x17)
#[bitfield(u32)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheSize2 {
    /// Max cache size value (in units determined by `granularity`).
    #[bits(31)]
    pub max_size: u32,
    /// Granularity: false = 1 KB, true = 64 KB.
    pub granularity: bool,
}

/// Cache SRAM Type (Type 7, offset 0x0B / 0x0D)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct CacheSramTypeData {
    /// Other.
    pub other: bool,
    /// Unknown.
    pub unknown: bool,
    /// Non-burst.
    pub non_burst: bool,
    /// Burst.
    pub burst: bool,
    /// Pipeline burst.
    pub pipeline_burst: bool,
    /// Synchronous.
    pub synchronous: bool,
    /// Asynchronous.
    pub asynchronous: bool,
    /// Reserved bits.
    #[bits(9)]
    pub reserved: u16,
}

/// Cache Error Correction Type (Type 7, offset 0x10)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum CacheErrorCorrectionType {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// No ECC.
    NoEcc = 0x03,
    /// Parity.
    Parity = 0x04,
    /// Single-bit ECC.
    SingleBitEcc = 0x05,
    /// Multi-bit ECC.
    MultiBitEcc = 0x06,
}

/// System Cache Type (Type 7, offset 0x11)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SystemCacheType {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Instruction cache.
    Instruction = 0x03,
    /// Data cache.
    Data = 0x04,
    /// Unified cache.
    Unified = 0x05,
}

/// Cache Associativity Field (Type 7, offset 0x12)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum AssociativityField {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Direct-mapped.
    DirectMapped = 0x03,
    /// 2-way set-associative.
    SetAssociative2Way = 0x04,
    /// 4-way set-associative.
    SetAssociative4Way = 0x05,
    /// Fully associative.
    FullyAssociative = 0x06,
    /// 8-way set-associative.
    SetAssociative8Way = 0x07,
    /// 16-way set-associative.
    SetAssociative16Way = 0x08,
    /// 12-way set-associative.
    SetAssociative12Way = 0x09,
    /// 24-way set-associative.
    SetAssociative24Way = 0x0A,
    /// 32-way set-associative.
    SetAssociative32Way = 0x0B,
    /// 48-way set-associative.
    SetAssociative48Way = 0x0C,
    /// 64-way set-associative.
    SetAssociative64Way = 0x0D,
    /// 20-way set-associative.
    SetAssociative20Way = 0x0E,
}

/// System Slot Type (Type 9, offset 0x05) - BYTE per spec
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SlotType {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// ISA.
    Isa = 0x03,
    /// MCA.
    Mca = 0x04,
    /// EISA.
    Eisa = 0x05,
    /// PCI.
    Pci = 0x06,
    /// PC Card (PCMCIA).
    PcCard = 0x07,
    /// VL-VESA.
    VlVesa = 0x08,
    /// Proprietary.
    Proprietary = 0x09,
    /// Processor card slot.
    ProcessorCardSlot = 0x0A,
    /// Proprietary memory card slot.
    ProprietaryMemoryCardSlot = 0x0B,
    /// I/O riser card slot.
    IoRiserCardSlot = 0x0C,
    /// NuBus.
    NuBus = 0x0D,
    /// PCI - 66 MHz capable.
    Pci66mhz = 0x0E,
    /// AGP.
    Agp = 0x0F,
    /// AGP 2X.
    Agp2x = 0x10,
    /// AGP 4X.
    Agp4x = 0x11,
    /// PCI-X.
    PciX = 0x12,
    /// AGP 8X.
    Agp8x = 0x13,
    /// M.2 Socket 1-DP (Mechanical Key A).
    M2Socket1DP = 0x14,
    /// M.2 Socket 1-SD (Mechanical Key E).
    M2Socket1SD = 0x15,
    /// M.2 Socket 2 (Mechanical Key B).
    M2Socket2 = 0x16,
    /// M.2 Socket 3 (Mechanical Key M).
    M2Socket3 = 0x17,
    /// MXM Type I.
    MxmTypeI = 0x18,
    /// MXM Type II.
    MxmTypeII = 0x19,
    /// MXM Type III (standard connector).
    MxmTypeIIIStandard = 0x1A,
    /// MXM Type III-HE.
    MxmTypeIIIHe = 0x1B,
    /// MXM Type IV.
    MxmTypeIV = 0x1C,
    /// MXM 3.0 Type A.
    Mxm3TypeA = 0x1D,
    /// MXM 3.0 Type B.
    Mxm3TypeB = 0x1E,
    /// PCI Express Gen 2 SFF-8639 (U.2).
    PciExpressGen2Sff8629 = 0x1F,
    /// PCI Express Gen 3 SFF-8639 (U.2).
    PciExpressGen3Sff8629 = 0x20,
    /// PCI Express Mini 52-pin (CEM spec. 2.0) with bottom-side keep-outs.
    PciExpressMini52PinBottomSideKeepOuts = 0x21,
    /// PCI Express Mini 52-pin (CEM spec. 2.0) without bottom-side keep-outs.
    PciExpressMini52Pin = 0x22,
    /// PCI Express Mini 76-pin (CEM spec. 2.0).
    PciExpressMini76Pin = 0x23,
    /// PCI Express Gen 4 SFF-8639 (U.2).
    PciExpressGen4Sff8639 = 0x24,
    /// PCI Express Gen 5 SFF-8639 (U.2).
    PciExpressGen5Sff8639 = 0x25,
    /// OCP NIC 3.0 Small Form Factor (SFF).
    OcpNic3SFF = 0x26,
    /// OCP NIC 3.0 Large Form Factor (LFF).
    OcpNic3LFF = 0x27,
    /// OCP NIC Prior to 3.0.
    OcpNicPrior = 0x28,
    /// PC-98/C20.
    Pc98C20 = 0xA0,
    /// PC-98/C24.
    Pc98C24 = 0xA1,
    /// PC-98/E.
    Pc98E = 0xA2,
    /// PC-98/Local Bus.
    Pc98LocalBus = 0xA3,
    /// PC-98/Card.
    Pc98Card = 0xA4,
    /// PCI Express (one of the undefined/general slot types).
    PciExpress = 0xA5,
    /// PCI Express x1.
    PciExpressx1 = 0xA6,
    /// PCI Express x2.
    PciExpressx2 = 0xA7,
    /// PCI Express x4.
    PciExpressx4 = 0xA8,
    /// PCI Express x8.
    PciExpressx8 = 0xA9,
    /// PCI Express x16.
    PciExpressx16 = 0xAA,
    /// PCI Express Gen 2.
    PciExpressGen2 = 0xAB,
    /// PCI Express Gen 2 x1.
    PciExpressGen2x1 = 0xAC,
    /// PCI Express Gen 2 x2.
    PciExpressGen2x2 = 0xAD,
    /// PCI Express Gen 2 x4.
    PciExpressGen2x4 = 0xAE,
    /// PCI Express Gen 2 x8.
    PciExpressGen2x8 = 0xAF,
    /// PCI Express Gen 2 x16.
    PciExpressGen2x16 = 0xB0,
    /// PCI Express Gen 3.
    PciExpressGen3 = 0xB1,
    /// PCI Express Gen 3 x1.
    PciExpressGen3x1 = 0xB2,
    /// PCI Express Gen 3 x2.
    PciExpressGen3x2 = 0xB3,
    /// PCI Express Gen 3 x4.
    PciExpressGen3x4 = 0xB4,
    /// PCI Express Gen 3 x8.
    PciExpressGen3x8 = 0xB5,
    /// PCI Express Gen 3 x16.
    PciExpressGen3x16 = 0xB6,
    /// PCI Express Gen 4.
    PciExpressGen4 = 0xB8,
    /// PCI Express Gen 4 x1.
    PciExpressGen4x1 = 0xB9,
    /// PCI Express Gen 4 x2.
    PciExpressGen4x2 = 0xBA,
    /// PCI Express Gen 4 x4.
    PciExpressGen4x4 = 0xBB,
    /// PCI Express Gen 4 x8.
    PciExpressGen4x8 = 0xBC,
    /// PCI Express Gen 4 x16.
    PciExpressGen4x16 = 0xBD,
    /// PCI Express Gen 5.
    PciExpressGen5 = 0xBE,
    /// PCI Express Gen 5 x1.
    PciExpressGen5x1 = 0xBF,
    /// PCI Express Gen 5 x2.
    PciExpressGen5x2 = 0xC0,
    /// PCI Express Gen 5 x4.
    PciExpressGen5x4 = 0xC1,
    /// PCI Express Gen 5 x8.
    PciExpressGen5x8 = 0xC2,
    /// PCI Express Gen 5 x16.
    PciExpressGen5x16 = 0xC3,
    /// PCI Express Gen 6 and beyond.
    PciExpressGen6 = 0xC4,
    /// EDSFF E1.S, E1.L.
    EdsffE1SE1L = 0xC5,
    /// EDSFF E3.S, E3.L.
    EdsffE3SE3L = 0xC6,
}

/// System Slot Data Bus Width (Type 9, offset 0x06)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SlotWidth {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// 8-bit.
    Bit8 = 0x03,
    /// 16-bit.
    Bit16 = 0x04,
    /// 32-bit.
    Bit32 = 0x05,
    /// 64-bit.
    Bit64 = 0x06,
    /// 128-bit.
    Bit128 = 0x07,
    /// 1x or x1.
    X1 = 0x08,
    /// 2x or x2.
    X2 = 0x09,
    /// 4x or x4.
    X4 = 0x0A,
    /// 8x or x8.
    X8 = 0x0B,
    /// 12x or x12.
    X12 = 0x0C,
    /// 16x or x16.
    X16 = 0x0D,
    /// 32x or x32.
    X32 = 0x0E,
}

/// System Slot Current Usage (Type 9, offset 0x07)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum CurrentUsage {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Available.
    Available = 0x03,
    /// In use.
    InUse = 0x04,
    /// Unavailable.
    Unavailable = 0x05,
}

/// System Slot Length (Type 9, offset 0x08)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum SlotLength {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// Short length.
    ShortLength = 0x03,
    /// Long length.
    LongLength = 0x04,
    /// 2.5" drive form factor.
    DriveFF25 = 0x05,
    /// 3.5" drive form factor.
    DriveFF35 = 0x06,
}

/// System Slot Characteristics 1 (Type 9, offset 0x0B)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct SlotCharacteristics1 {
    /// Characteristics unknown.
    pub characteristics_unknown: bool,
    /// Provides 5.0 V.
    pub provides_5_volts: bool,
    /// Provides 3.3 V.
    pub provides_3_volts: bool,
    /// Slot's opening is shared with another slot (e.g. PCI/EISA shared opening).
    pub shared_slot: bool,
    /// PC Card slot supports PC Card-16.
    pub pc_supports_pccard16: bool,
    /// PC Card slot supports CardBus.
    pub pc_supports_cardbus: bool,
    /// PC Card slot supports Zoom Video.
    pub pc_supports_zoomvideo: bool,
    /// PC Card slot supports Modem Ring Resume.
    pub pc_supports_modemringresume: bool,
}

/// System Slot Characteristics 2 (Type 9, offset 0x0C)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct SlotCharacteristics2 {
    /// PCI slot supports Power Management Event (PME#) signal.
    pub pci_supports_pme: bool,
    /// Slot supports hot-plug devices.
    pub supports_hotplug: bool,
    /// PCI slot supports SMBus signal.
    pub pci_supports_smbus: bool,
    /// PCIe slot supports bifurcation.
    pub pcie_supports_bifurcation: bool,
    /// Slot supports async/surprise removal.
    pub supports_async_removal: bool,
    /// Flexbus slot: CXL 1.0 capable.
    pub flexbus_slot1: bool,
    /// Flexbus slot: CXL 2.0 capable.
    pub flexbus_slot2: bool,
    /// Flexbus slot: CXL 3.0 capable (reserved per spec; vendor-defined).
    pub flexbus_slot3: bool,
}

/// System Slot Device/Function Number (Type 9, offset 0x10)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct DeviceFunctionNumber {
    /// PCI function number.
    #[bits(3)]
    pub function_number: u8,
    /// PCI device number.
    #[bits(5)]
    pub device_number: u8,
}

/// System Slot Peer Group entry (5 bytes)
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub struct MiscSlotPeerGroup {
    /// PCI segment group number.
    pub segment_group_num: u16,
    /// PCI bus number for the peer slot.
    pub bus_num: u8,
    /// PCI device/function number for the peer slot.
    pub dev_func_num: DeviceFunctionNumber,
    /// Data bus width of the peer slot (matches `SlotWidth` byte values).
    pub data_bus_width: u8,
}

/// Memory Array Location (Type 16, offset 0x04)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryArrayLocation {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// System board or motherboard.
    SystemBoard = 0x03,
    /// ISA add-on card.
    IsaAddOn = 0x04,
    /// EISA add-on card.
    EisaAddOn = 0x05,
    /// PCI add-on card.
    PciAddOn = 0x06,
    /// MCA add-on card.
    McaAddOn = 0x07,
    /// PCMCIA add-on card.
    PcmciaAddOn = 0x08,
    /// Proprietary add-on card.
    ProprietaryAddOn = 0x09,
    /// NuBus.
    NuBus = 0x0A,
    /// PC-98/C20 add-on card.
    Pc98C20AddOn = 0xA0,
    /// PC-98/C24 add-on card.
    Pc98C24AddOn = 0xA1,
    /// PC-98/E add-on card.
    Pc98EAddOn = 0xA2,
    /// PC-98/Local Bus add-on card.
    Pc98LocalAddOn = 0xA3,
    /// CXL add-on card.
    CxlAddOn = 0xA4,
}

/// Memory Array Use (Type 16, offset 0x05)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryArrayUse {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// System memory.
    SystemMemory = 0x03,
    /// Video memory.
    VideoMemory = 0x04,
    /// Flash memory.
    FlashMemory = 0x05,
    /// Non-volatile RAM.
    NonVolatileRam = 0x06,
    /// Cache memory.
    CacheMemory = 0x07,
}

/// Memory Array Error Correction Types (Type 16, offset 0x06)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryArrayErrorCorrectionType {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// No ECC.
    NoEcc = 0x03,
    /// Parity.
    Parity = 0x04,
    /// Single-bit ECC.
    SingleBitEcc = 0x05,
    /// Multi-bit ECC.
    MultiBitEcc = 0x06,
    /// CRC.
    Crc = 0x07,
}

/// Memory Device Form Factor (Type 17, offset 0x0E)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryFormFactor {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// SIMM.
    Simm = 0x03,
    /// SIP.
    Sip = 0x04,
    /// Chip.
    Chip = 0x05,
    /// DIP.
    Dip = 0x06,
    /// ZIP.
    Zip = 0x07,
    /// Proprietary card.
    ProprietaryCard = 0x08,
    /// DIMM.
    Dimm = 0x09,
    /// TSOP.
    Tsop = 0x0A,
    /// Row of chips.
    RowOfChips = 0x0B,
    /// RIMM.
    Rimm = 0x0C,
    /// SODIMM.
    Sodimm = 0x0D,
    /// SRIMM.
    Srimm = 0x0E,
    /// FB-DIMM.
    FbDimm = 0x0F,
    /// Die.
    Die = 0x10,
    /// CAMM.
    Camm = 0x11,
    /// CUDIMM.
    Cudimm = 0x12,
    /// CSODIMM.
    Csodimm = 0x13,
}

/// Memory Device Memory Type (Type 17, offset 0x12)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryDeviceType {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// DRAM.
    Dram = 0x03,
    /// EDRAM.
    Edram = 0x04,
    /// VRAM.
    Vram = 0x05,
    /// SRAM.
    Sram = 0x06,
    /// RAM.
    Ram = 0x07,
    /// ROM.
    Rom = 0x08,
    /// FLASH.
    Flash = 0x09,
    /// EEPROM.
    Eeprom = 0x0A,
    /// FEPROM.
    Feprom = 0x0B,
    /// EPROM.
    Eprom = 0x0C,
    /// CDRAM.
    Cdram = 0x0D,
    /// 3DRAM.
    ThreeDram = 0x0E,
    /// SDRAM.
    Sdram = 0x0F,
    /// SGRAM.
    Sgram = 0x10,
    /// RDRAM.
    Rdram = 0x11,
    /// DDR.
    Ddr = 0x12,
    /// DDR2.
    Ddr2 = 0x13,
    /// DDR2 FB-DIMM.
    Ddr2FbDimm = 0x14,
    /// DDR3.
    Ddr3 = 0x18,
    /// FBD2.
    Fbd2 = 0x19,
    /// DDR4.
    Ddr4 = 0x1A,
    /// LPDDR.
    Lpddr = 0x1B,
    /// LPDDR2.
    Lpddr2 = 0x1C,
    /// LPDDR3.
    Lpddr3 = 0x1D,
    /// LPDDR4.
    Lpddr4 = 0x1E,
    /// Logical non-volatile device.
    LogicalNonVolatileDevice = 0x1F,
    /// HBM (High Bandwidth Memory).
    Hbm = 0x20,
    /// HBM2.
    Hbm2 = 0x21,
    /// DDR5.
    Ddr5 = 0x22,
    /// LPDDR5.
    Lpddr5 = 0x23,
    /// HBM3.
    Hbm3 = 0x24,
    /// MRDIMM.
    Mrdimm = 0x25,
}

/// Memory Device Type Detail (Type 17, offset 0x13)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct MemoryDeviceTypeDetails {
    /// Reserved.
    pub reserved: bool,
    /// Other.
    pub other: bool,
    /// Unknown.
    pub unknown: bool,
    /// Fast-paged.
    pub fast_paged: bool,
    /// Static column.
    pub static_column: bool,
    /// Pseudo-static.
    pub pseudo_static: bool,
    /// RAMBUS.
    pub rambus: bool,
    /// Synchronous.
    pub synchronous: bool,
    /// CMOS.
    pub cmos: bool,
    /// EDO.
    pub edo: bool,
    /// Window DRAM.
    pub window_dram: bool,
    /// Cache DRAM.
    pub cache_dram: bool,
    /// Non-volatile.
    pub nonvolatile: bool,
    /// Registered (buffered).
    pub registered: bool,
    /// Unbuffered (unregistered).
    pub unbuffered: bool,
    /// LRDIMM.
    pub lr_dimm: bool,
}

/// Memory Device Memory Technology (Type 17, offset 0x28)
#[repr(u8)]
#[derive(Copy, Clone, Debug, IntoBytes, Immutable, KnownLayout)]
pub enum MemoryDeviceTechnology {
    /// Other.
    Other = 0x01,
    /// Unknown.
    Unknown = 0x02,
    /// DRAM.
    Dram = 0x03,
    /// NVDIMM-N.
    NvdimmN = 0x04,
    /// NVDIMM-F.
    NvdimmF = 0x05,
    /// NVDIMM-P.
    NvdimmP = 0x06,
    /// Intel® Optane™ persistent memory.
    IntelOptanePersistentMemory = 0x07,
}

/// Memory Device Attributes (Type 17, offset 0x1B)
#[bitfield(u8)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct MemoryDeviceAttributes {
    /// Memory rank.
    #[bits(4)]
    pub rank: u8,
    /// Reserved bits.
    #[bits(4)]
    pub reserved: u8,
}

/// Memory Device Operating Mode Capability (Type 17, offset 0x29)
#[bitfield(u16)]
#[derive(IntoBytes, Immutable, KnownLayout)]
pub struct MemoryCapability {
    /// Reserved.
    pub reserved: bool,
    /// Other.
    pub other: bool,
    /// Unknown.
    pub unknown: bool,
    /// Volatile memory.
    pub volatile_memory: bool,
    /// Byte-accessible persistent memory.
    pub byte_persistent_memory: bool,
    /// Block-accessible persistent memory.
    pub block_persistent_memory: bool,
    /// Reserved bits.
    #[bits(10)]
    pub reserved2: u16,
}
