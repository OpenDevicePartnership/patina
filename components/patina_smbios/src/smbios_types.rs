//! SMBIOS Types
//!
//! Defines the types used in SMBIOS Records 
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use bitfield::bitfield;
extern crate alloc;


bitfield! {
    ///
    /// BIOS Characteristics
    /// 
    pub struct BiosCharacteristics (u64);
    impl Debug;
    pub reserved, set_reserved: 1, 0;
    pub unknown, set_unknown: 2;
    pub bios_characteristics_unsupported, set_bios_characteristics_unsupported: 3;
    pub isa_supported, set_isa_supported: 4;
    pub mca_supported, set_mca_supported: 5;
    pub eisa_supported, set_eisa_supported: 6;
    pub pci_supported, set_pci_supported: 7;
    pub pcmcia_supported, set_pcmcia_supported: 8;
    pub plug_play_supported, set_plug_play_supported: 9;
    pub apm_supported, set_apm_supported: 10;
    pub bios_is_upgradable, set_bios_is_upgradable: 11;
    pub bios_shadowing_allowed, set_bios_shadowing_allowed: 12;
    pub vlvesa_supported, set_vlvesa_supported: 13;
    pub escd_supported, set_escd_supported: 14;
    pub cd_boot_supported, set_cd_boot_supported: 15;
    pub selectable_boot_supported, set_selectable_boot_supported: 16;
    pub bios_rom_socketed, set_bios_rom_socketed: 17;
    pub pc_card_boot_supported, set_pc_card_boot_supported: 18;
    pub edd_spec_supported, set_edd_spec_supported: 19;
    pub japanese_nec_9800_supported, set_japanese_nec_9800_supported: 20;
    pub japanese_toshiba_supported, set_japanese_toshiba_supported: 21;
    pub kb_525_360_supported, set_kb_525_360_supported: 22;
    pub mb_535_12_supported, set_mb_535_12_supported: 23;
    pub mb_35_720_supported, set_mb_35_720_supported: 24;
    pub mb_35_288_supported, set_mb_35_288_supported: 25;
    pub print_screen_supported, set_print_screen_supported: 26;
    pub keyboard_8042_supported, set_keyboard_8042_supported: 27;
    pub serial_services_supported, set_serial_services_supported: 28;
    pub printer_services_supported, set_printer_services_supported: 29;
    pub cga_mono_video_supported, set_cga_mono_video_supported: 30;
    pub nec_pc_98, set_nec_pc_98: 31;
    pub reserved_bios_vendor, set_reserved_bios_vendor: 47, 32;
    pub reserved_system_vendor, set_reserved_system_vendor: 63, 48;
}

impl BiosCharacteristics {
    pub fn new(
        unknown: bool,
        bios_characteristics_unsupported: bool,
        isa_supported: bool,
        mca_supported: bool,
        eisa_supported: bool,
        pci_supported: bool,
        pcmcia_supported: bool,
        plug_play_supported: bool,
        apm_supported: bool,
        bios_is_upgradable: bool,
        bios_shadowing_allowed: bool,
        vlvesa_supported: bool,
        escd_supported: bool,
        cd_boot_supported: bool,
        selectable_boot_supported: bool,
        bios_rom_socketed: bool,
        pc_card_boot_supported: bool,
        edd_spec_supported: bool,
        japanese_nec_9800_supported: bool,
        japanese_toshiba_supported: bool,
        kb_525_360_supported: bool,
        mb_535_12_supported: bool,
        mb_35_720_supported: bool,
        mb_35_288_supported: bool,
        print_screen_supported: bool,
        keyboard_8042_supported: bool,
        serial_services_supported: bool,
        printer_services_supported: bool,
        cga_mono_video_supported: bool,
        nec_pc_98: bool,
        reserved_bios_vendor: u64,
        reserved_system_vendor:u64,
    ) -> Self{
        let mut config = BiosCharacteristics(0);
        config.set_reserved(0);
        config.set_unknown(unknown);
        config.set_bios_characteristics_unsupported(bios_characteristics_unsupported);
        config.set_isa_supported(isa_supported);
        config.set_mca_supported(mca_supported);
        config.set_eisa_supported(eisa_supported);
        config.set_pci_supported(pci_supported);
        config.set_pcmcia_supported(pcmcia_supported);
        config.set_plug_play_supported(plug_play_supported);
        config.set_apm_supported(apm_supported);
        config.set_bios_is_upgradable(bios_is_upgradable);
        config.set_bios_shadowing_allowed(bios_shadowing_allowed);
        config.set_vlvesa_supported(vlvesa_supported);
        config.set_escd_supported(escd_supported);
        config.set_cd_boot_supported(cd_boot_supported);
        config.set_selectable_boot_supported(selectable_boot_supported);
        config.set_bios_rom_socketed(bios_rom_socketed);
        config.set_pc_card_boot_supported(pc_card_boot_supported);
        config.set_edd_spec_supported(edd_spec_supported);
        config.set_japanese_nec_9800_supported(japanese_nec_9800_supported);
        config.set_japanese_toshiba_supported(japanese_toshiba_supported);
        config.set_kb_525_360_supported(kb_525_360_supported);
        config.set_mb_535_12_supported(mb_535_12_supported);
        config.set_mb_35_720_supported(mb_35_720_supported);
        config.set_mb_35_288_supported(mb_35_288_supported);
        config.set_print_screen_supported(print_screen_supported);
        config.set_keyboard_8042_supported(keyboard_8042_supported);
        config.set_serial_services_supported(serial_services_supported);
        config.set_printer_services_supported(printer_services_supported);
        config.set_cga_mono_video_supported(cga_mono_video_supported);
        config.set_nec_pc_98(nec_pc_98);
        config.set_reserved_bios_vendor(reserved_bios_vendor);
        config.set_reserved_system_vendor(reserved_system_vendor);
        config
    }
}

bitfield! {
    ///
    /// BIOS Characteristics Extension Byte 1
    /// 
    pub struct BiosCharacteristicsExt1 (u8);
    impl Debug;
    pub acpi_supported, set_acpi_supported: 0;
    pub usb_legacy_supported, set_usb_legacy_supported: 1;
    pub agp_supported, set_agp_supported: 2;
    pub i20_supported, set_i20_supported: 3;
    pub superdisk_boot_supported, set_superdisk_boot_supported: 4;
    pub zip_drive_boot_supported, set_zip_drive_boot_supported: 5;
    pub boot_1394_supported, set_boot_1394_supported: 6;
    pub smart_battery_supported, set_smart_battery_supported: 7;
}

impl BiosCharacteristicsExt1 {
    pub fn new(
        acpi_supported: bool,
        usb_legacy_supported: bool,
        agp_supported: bool,
        i20_supported: bool,
        superdisk_boot_supported: bool,
        zip_drive_boot_supported: bool,
        boot_1394_supported: bool,
        smart_battery_supported: bool,
    ) -> Self{
        let mut config = BiosCharacteristicsExt1(0);
        config.set_acpi_supported(acpi_supported);
        config.set_usb_legacy_supported(usb_legacy_supported);
        config.set_agp_supported(agp_supported);
        config.set_i20_supported(i20_supported);
        config.set_superdisk_boot_supported(superdisk_boot_supported);
        config.set_zip_drive_boot_supported(zip_drive_boot_supported);
        config.set_boot_1394_supported(boot_1394_supported);
        config.set_smart_battery_supported(smart_battery_supported);
        config
    }
}

bitfield! {
    ///
    /// BIOS Characteristics Extension Byte 2
    /// 
    pub struct BiosCharacteristicsExt2 (u8);
    impl Debug;
    pub bios_boot_specification_supported, set_bios_boot_specification_supported: 0;
    pub fn_network_service_boot_supported, set_fn_network_service_boot_supported: 1;
    pub enable_targeted_content_distribution, set_enable_targeted_content_distribution: 2;
    pub uefi_spec_supported, set_uefi_spec_supported: 3;
    pub smbios_describes_vm, set_smbios_describes_vm: 4;
    pub reserved, set_reserved: 7, 5;
}

impl BiosCharacteristicsExt2 {
    pub fn new(
        bios_boot_specification_supported: bool,
        fn_network_service_boot_supported: bool,
        enable_targeted_content_distribution: bool,
        uefi_spec_supported: bool,
        smbios_describes_vm: bool,
    ) -> Self{
        let mut config = BiosCharacteristicsExt2(0);
        config.set_bios_boot_specification_supported(bios_boot_specification_supported);
        config.set_fn_network_service_boot_supported(fn_network_service_boot_supported);
        config.set_enable_targeted_content_distribution(enable_targeted_content_distribution);
        config.set_uefi_spec_supported(uefi_spec_supported);
        config.set_smbios_describes_vm(smbios_describes_vm);
        config.set_reserved(0);
        config
    }
}

bitfield! {
    ///
    /// Extended BIOS Rom Size
    /// 
    pub struct ExtendedBiosRomSize (u16);
    impl Debug;
    pub size, set_size: 13, 0;
    pub unit, set_unit: 15, 14;
}

impl ExtendedBiosRomSize {
    pub fn new(
        size: u16,
        unit: u16,
    ) -> Self{
        let mut config = ExtendedBiosRomSize(0);
        config.set_size(size);
        config.set_unit(unit);
        config
    }
}



///
/// Wake Up Type
/// 
#[derive(Copy, Clone, Debug)]
pub enum WakeUpType {
    Reserved = 0x0,
    Other,
    Unknown,
    ApmTimer,
    ModemRing,
    LanRemote,
    PowerSwitch,
    PciPme,
    AcPowerRestored
}

bitfield! {
    ///
    /// Feature Flags
    /// 
    pub struct FeatureFlags (u8);
    impl Debug;
    pub hosting_board, set_hosting_board: 0;
    pub require_aux_board, set_require_aux_board: 1;
    pub removable_board, set_removable_board: 2;
    pub replaceable_board, set_replaceable_board: 3;
    pub hot_swappable_board, set_hot_swappable_board: 4;
    pub reserved, set_reserved: 7, 5;
}

impl FeatureFlags {
    pub fn new(
        hosting_board: bool,
        require_aux_board: bool,
        removable_board: bool,
        replaceable_board: bool,
        hot_swappable_board: bool,
    ) -> Self{
        let mut config = FeatureFlags(0);
        config.set_hosting_board(hosting_board);
        config.set_require_aux_board(require_aux_board);
        config.set_removable_board(removable_board);
        config.set_replaceable_board(replaceable_board);
        config.set_hot_swappable_board(hot_swappable_board);
        config.set_reserved(0);
        config
    }
}

///
/// Board Type
/// 
#[derive(Copy, Clone, Debug)]
pub enum BoardType {
    Unknown = 0x01,
    Other,
    ServerBlade,
    ConnectivitySwitch,
    SystemManagementModule,
    ProcessorModule,
    IoModule,
    MemoryModule,
    DaughterBoard,
    Motherboard,
    ProcessorMemoryModule,
    ProcessorIoModule,
    InterconnectBoard
}

///
/// Boot Up State
///
#[derive(Copy, Clone, Debug)]
pub enum BootUpState {
    Other = 0x01,
    Unknown,
    Desktop,
    LowProfileDesktop,
    Pizzabox,
    MiniTower,
    Tower,
    Portable,
    Laptop,
    Notebook,
    HandHeld,
    DockingStation,
    AllInOne,
    SubNotebook,
    SpaceSaving,
    LunchBox,
    MainServerChassis,
    ExpansionChassis,
    SubChassis,
    BusExpansionChassis,
    PeripheralChassis,
    RaidChassis,
    RackMountChassis,
    SealedCasePc,
    MultiSystemChassis,
    CompactPci,
    AdvancedTca,
    Blade,
    BladeEnclosure,
    Tablet,
    Convertible,
    Detachable,
    IotGateway,
    EmbeddedPc,
    MiniPc,
    StickPc
}

///
/// Power Supply State
///
#[derive(Copy, Clone, Debug)]
pub enum PowerSupplyState {
    ProcessorOther = 0x01,
    ProcessorUnknown,
    CentralProcessor,
    MathProcessor,
    DspProcessor,
    VideoProcessor,
}

///
/// Thermal State
///
#[derive(Copy, Clone, Debug)]
pub enum ThermalState {
    Other = 0x01,
    Unknown,
    Safe,
    Warning,
    Critical,
    NonRecoverable
}

///
/// Security Status
///
#[derive(Copy, Clone, Debug)]
pub enum SecurityStatus {
    Other = 0x01,
    Unknown,
    _None,
    ExternalInterfaceLockedOut,
    ExternalInterfaceEnabled
}

bitfield! {
    ///
    /// Contained Elements - Contained Element Type
    ///
    #[derive(Copy, Clone)] 
    pub struct  ContainedElementType(u8);
    impl Debug;
    pub _type, set_type : 6, 0;
    pub type_select, set_type_select : 7;
}

impl ContainedElementType {
    pub fn new(
        _type: u8,
        type_select: bool,
    ) -> Self{
        let mut config = ContainedElementType(0);
        config.set_type(_type);
        config.set_type_select(type_select);
        config
    }
}

///
/// Contained Elements
///
#[derive(Copy, Clone, Debug)]
pub struct ContainedElements {
    pub contained_element_type: ContainedElementType,
    pub contained_element_minimum: u8,
    pub contained_element_maximum: u8,
}

impl Default for ContainedElements {
    fn default() -> Self {
        Self {
            contained_element_type: ContainedElementType::new(0, false),
            contained_element_minimum: 0,
            contained_element_maximum: 0
        }
    }
}

///
/// Processor Information
///
#[derive(Copy, Clone, Debug)]
pub enum ProcessorTypeData {
    ProcessorOther = 0x01,
    ProcessorUnknown,
    CentralProcessor,
    MathProcessor,
    DspProcessor,
    VideoProcessor,
}

///
/// Processor Information - Processor Family
///
#[derive(Copy, Clone, Debug)]
pub enum ProcessorFamilyData {
    Other = 0x01,
    Unknown,
    Processor8086,
    Processor80286,
    Intel386,
    Intel486,
    Processor8087,
    Processor80287,
    Processor80387,
    Processor80487,
    Pentium,
    PentiumPro,
    PentiumII,
    PentiumMMX,
    Celeron,
    PentiumIIXeon,
    PentiumIII,
    M1Family,
    M2Family,
    IntelCeleronM,
    IntelPentium4Ht,
    IntelProcessor,
    AmdDuron = 0x18,
    K5Family,
    K6Family,
    K6_2,
    K6_3,
    AmdAthlon,
    Amd29000,
    K6_2Plus,
    PowerPC,
    PowerPC601,
    PowerPC603,
    PowerPC603Plus,
    PowerPC604,
    PowerPC620,
    PowerPCx704,
    PowerPC750,
    IntelCoreDuo,
    IntelCoreDuoMobile,
    IntelCoreSoloMobile,
    IntelAtom,
    IntelCoreM,
    IntelCorem3,
    IntelCorem5,
    IntelCorem7,
    Alpha,
    Alpha21064,
    Alpha21066,
    Alpha21164,
    Alpha21164PC,
    Alpha21164a,
    Alpha21264,
    Alpha21364,
    AmdTurionIIUltraDualCoreMobileM,
    AmdTurionIIDualCoreMobileM,
    AmdAthlonIIDualCoreM,
    AmdOpteron6100Series,
    AmdOpteron4100Series,
    AmdOpteron6200Series,
    AmdOpteron4200Series,
    AmdFxSeries,
    MipsFamily,
    MipsR4000,
    MipsR4200,
    MipsR4400,
    MipsR4600,
    MipsR10000,
    AmdCSeries,
    AmdESeries,
    AmdASeries,
    AmdGSeries,
    AmdZSeries,
    AmdRSeries,
    AmdOpteron4300,
    AmdOpteron6300,
    AmdOpteron3300,
    AmdFireProSeries,
    Sparc,
    SuperSparc,
    MicroSparcII,
    MicroSparcIIep,
    UltraSparc,
    UltraSparcII,
    UltraSparcIii,
    UltraSparcIII,
    UltraSparcIIIi,
    Processor68040 = 0x60,
    Processor68xxx,
    Processor68000,
    Processor68010,
    Processor68020,
    Processor68030,
    AmdAthlonX4QuadCore,
    AmdOpteronX1000Series,
    AmdOpteronX2000Series,
    AmdOpteronASeries,
    AmdOpteronX3000Series,
    AmdZen,
    HobbitFamily = 0x70,
    CrusoeTM5000 = 0x78,
    CrusoeTM3000,
    EfficeonTM8000,
    Weitek = 0x80,
    Itanium = 0x82,
    AmdAthlon64,
    AmdOpteron,
    AmdSempron,
    AmdTurion64Mobile,
    DualCoreAmdOpteron,
    AmdAthlon64X2DualCore,
    AmdTurion64X2Mobile,
    QuadCoreAmdOpteron,
    ThirdGenerationAmdOpteron,
    AmdPhenomFxQuadCore,
    AmdPhenomX4QuadCore,
    AmdPhenomX2DualCore,
    AmdAthlonX2DualCore,
    Parisc,
    PaRisc8500,
    PaRisc8000,
    PaRisc7300LC,
    PaRisc7200,
    PaRisc7100LC,
    PaRisc7100,
    V30Family = 0xA0,
    QuadCoreIntelXeon3200Series,
    DualCoreIntelXeon3000Series,
    QuadCoreIntelXeon5300Series,
    DualCoreIntelXeon5100Series,
    DualCoreIntelXeon5000Series,
    DualCoreIntelXeonLV,
    DualCoreIntelXeonULV,
    DualCoreIntelXeon7100Series,
    QuadCoreIntelXeon5400Series,
    QuadCoreIntelXeon,
    DualCoreIntelXeon5200Series,
    DualCoreIntelXeon7200Series,
    QuadCoreIntelXeon7300Series,
    QuadCoreIntelXeon7400Series,
    MultiCoreIntelXeon7400Series,
    PentiumIIIXeon,
    PentiumIIISpeedStep,
    Pentium4,
    IntelXeon,
    As400,
    IntelXeonMP,
    AMDAthlonXP,
    AMDAthlonMP,
    IntelItanium2,
    IntelPentiumM,
    IntelCeleronD,
    IntelPentiumD,
    IntelPentiumEx,
    IntelCoreSolo,
    Reserved,
    IntelCore2,
    IntelCore2Solo,
    IntelCore2Extreme,
    IntelCore2Quad,
    IntelCore2ExtremeMobile,
    IntelCore2DuoMobile,
    IntelCore2SoloMobile,
    IntelCoreI7,
    DualCoreIntelCeleron,
    Ibm390,
    G4,
    G5,
    EsaG6,
    ZArchitecture,
    IntelCoreI5,
    IntelCoreI3,
    IntelCoreI9,
    IntelXeonD,
    ViaC7M = 0xD2,
    ViaC7D,
    ViaC7,
    ViaEden,
    MultiCoreIntelXeon,
    DualCoreIntelXeon3Series,
    QuadCoreIntelXeon3Series,
    ViaNano,
    DualCoreIntelXeon5Series,
    QuadCoreIntelXeon5Series,
    DualCoreIntelXeon7Series = 0xDD,
    QuadCoreIntelXeon7Series,
    MultiCoreIntelXeon7Series,
    MultiCoreIntelXeon3400Series,
    AmdOpteron3000Series = 0xE4,
    AmdSempronII,
    EmbeddedAmdOpteronQuadCore,
    AmdPhenomTripleCore,
    AmdTurionUltraDualCoreMobile,
    AmdTurionDualCoreMobile,
    AmdAthlonDualCore,
    AmdSempronSI,
    AmdPhenomII,
    AmdAthlonII,
    SixCoreAmdOpteron,
    AmdSempronM,
    I860 = 0xFA,
    I960,
    IndicatorFamily2 = 0xFE,
    Reserved1,
    ARMv7 = 0x0100,
    ARMv8,
    ARMv9,
    Sh3,
    Sh4,
    Arm = 0x0118,
    StrongARM,
    Processor6x86 = 0x012C,
    MediaGX,
    Mii,
    WinChip = 0x0140,
    Dsp = 0x015E,
    VideoProcessor = 0x01F4,
    RiscvRV32 = 0x0200,
    RiscVRV64,
    RiscVRV128,
    LoongArch = 0x0258,
    Loongson1,
    Loongson2,
    Loongson3,
    Loongson2K,
    Loongson3A,
    Loongson3B,
    Loongson3C,
    Loongson3D,
    Loongson3E,
    DualCoreLoongson2K,
    QuadCoreLoongson3A = 0x026C,
    MultiCoreLoongson3A,
    QuadCoreLoongson3B,
    MultiCoreLoongson3B,
    MultiCoreLoongson3C,
    MultiCoreLoongson3D,
    IntelCore3 = 0x0300,
    IntelCore5,
    IntelCore7,
    IntelCore9,
    IntelCoreUltra3,
    IntelCoreUltra5,
    IntelCoreUltra7,
    IntelCoreUltra9,
}

///
/// Processor Information - Processor Upgrade
///
#[derive(Copy, Clone, Debug)]
pub enum ProcessorUpgrade {
    Other = 0x01,
    Unknown,
    DaughterBoard,
    ZIFSocket,
    ReplaceablePiggyBack,
    None,
    LIFSocket,
    Slot1,
    Slot2,
    Pin370Socket,
    SlotA,
    SlotM,
    Socket423,
    SocketA, //  Socket 462
    Socket478,
    Socket754,
    Socket940,
    Socket939,
    SocketmPGA604,
    SocketLGA771,
    SocketLGA775,
    SocketS1,
    SocketAM2,
    SocketF1207,
    SocketLGA1366,
    SocketG34,
    SocketAM3,
    SocketC32,
    SocketLGA1156,
    SocketLGA1567,
    SocketPGA988A,
    SocketBGA1288,
    SocketrPGA988B,
    SocketBGA1023,
    SocketBGA1224,
    SocketLGA1155,
    SocketLGA1356,
    SocketLGA2011,
    SocketFS1,
    SocketFS2,
    SocketFM1,
    SocketFM2,
    SocketLGA2011_3,
    SocketLGA1356_3,
    SocketLGA1150,
    SocketBGA1168,
    SocketBGA1234,
    SocketBGA1364,
    SocketAM4,
    SocketLGA1151,
    SocketBGA1356,
    SocketBGA1440,
    SocketBGA1515,
    SocketLGA3647_1,
    SocketSP3,
    SocketSP3r2,
    SocketLGA2066,
    SocketBGA1392,
    SocketBGA1510,
    SocketBGA1528,
    SocketLGA4189,
    SocketLGA1200,
    SocketLGA4677,
    SocketLGA1700,
    SocketBGA1744,
    SocketBGA1781,
    SocketBGA1211,
    SocketBGA2422,
    SocketLGA1211,
    SocketLGA2422,
    SocketLGA5773,
    SocketBGA5773,
    SocketAM5,
    SocketSP5,
    SocketSP6,
    SocketBGA883,
    SocketBGA1190,
    SocketBGA4129,
    SocketLGA4710,
    SocketLGA7529,
    SocketBGA1964, //ANS: check for the rest of list, not on 3.7
    SocketBGA1792,
    SocketBGA2049,
    SocketBGA2551,
    SocketLGA1851,
    SocketBGA2114,
    SocketBGA2833,
    // Use this when no other valid enumeration is available.
    // When this enumeration is used, Socket Type at offset 32h must be non-null.
    NotAvailable = 0xFF,
}

bitfield! {
    ///
    /// Processor Information - Voltage
    ///
    pub struct ProcessorVoltage (u8);
    impl Debug;
    pub processor_voltage_capability_5v, set_processor_voltage_capability_5v: 0;
    pub processor_voltage_capability_3_3v, set_processor_voltage_capability_3_3v: 1;
    pub processor_voltage_capability_2_9v, set_processor_voltage_capability_2_9v: 2;
    pub processor_voltage_capability_reserved, set_processor_voltage_capability_reserved: 3;
    pub processor_voltage_reserved, set_processor_voltage_reserved: 6, 4;
    pub processor_voltage_indicate_legacy, set_processor_voltage_indicate_legacy: 7;
}

impl ProcessorVoltage {
    pub fn new(
        processor_voltage_capability_5v: bool,
        processor_voltage_capability_3_3v: bool,
        processor_voltage_capability_2_9v: bool,
        processor_voltage_capability_reserved: bool,
        processor_voltage_reserved: u8,
        processor_voltage_indicate_legacy: bool,
    ) -> Self{
        let mut config = ProcessorVoltage(0);
        config.set_processor_voltage_capability_5v(processor_voltage_capability_5v);
        config.set_processor_voltage_capability_3_3v(processor_voltage_capability_3_3v);
        config.set_processor_voltage_capability_2_9v(processor_voltage_capability_2_9v);
        config.set_processor_voltage_capability_reserved(processor_voltage_capability_reserved);
        config.set_processor_voltage_reserved(processor_voltage_reserved);
        config.set_processor_voltage_indicate_legacy(processor_voltage_indicate_legacy);
        config
    }
}

bitfield! {
    ///
    /// Processor Information - Status
    ///
    pub struct ProcessorInformationStatus (u8);
    impl Debug;
    pub cpu_status, set_cpu_status: 2, 0;
    // reserved must be zero
    pub reserved, set_reserved: 5, 3;
    pub cpu_socket_populated, set_cpu_socket_populated: 6;
    // reserved2 must be zero
    pub reserved2, set_reserved2: 7;
}

impl ProcessorInformationStatus {
    pub fn new(
        cpu_status: u8,
        cpu_socket_populated: bool,
    ) -> Self{
        let mut config = ProcessorInformationStatus(0);
        config.set_cpu_status(cpu_status);
        config.set_reserved(0);
        config.set_cpu_socket_populated(cpu_socket_populated);
        config.set_reserved2(false);
        config
    }
}

bitfield! {
    ///
    /// Processor Information - Status
    ///
    pub struct ProcessorCharacteristics (u16);
    impl Debug;
    pub reserved, set_reserved : 0;
    pub unknown, set_unknown : 1;
    pub _64bit_capable, set_64bit_capable : 2;
    pub multi_core, set_multi_core : 3;
    pub hardware_thread, set_hardware_thread : 4;
    pub execute_protection, set_execute_protection : 5;
    pub enhanced_virtualization, set_enhanced_virtualization : 6;
    pub performance_control, set_performance_control : 7;
    pub _128bit_capable, set_128bit_capable : 8;
    pub arm64_soc_id, set_arm64_soc_id : 9;
    pub reserved2, set_reserved2 : 15, 10;
}

impl ProcessorCharacteristics {
    pub fn new(
        unknown: bool,
        _64bit_capable: bool,
        multi_core: bool,
        hardware_thread: bool,
        execute_protection: bool,
        enhanced_virtualization: bool,
        performance_control: bool,
        _128bit_capable: bool,
        arm64_soc_id: bool,
    ) -> Self{
        let mut config = ProcessorCharacteristics(0);
        config.set_reserved(false);
        config.set_unknown(unknown);
        config.set_64bit_capable(_64bit_capable);
        config.set_multi_core(multi_core);
        config.set_hardware_thread(hardware_thread);
        config.set_execute_protection(execute_protection);
        config.set_enhanced_virtualization(enhanced_virtualization);
        config.set_performance_control(performance_control);
        config.set_128bit_capable(_128bit_capable);
        config.set_arm64_soc_id(arm64_soc_id);
        config.set_reserved2(0);
        config
    }
}

bitfield! {
    ///
    /// Cache Information - Cache Configuration
    ///
    pub struct CacheConfiguration (u16);
    impl Debug;
    pub cache_level, set_cache_level: 2, 0;
    pub cache_socketed, set_cache_socketed: 3;
    // reserved must be zero
    pub reserved, set_reserved: 4;
    pub location, set_location: 6, 5;
    pub enabled_disabled, set_enabled_disabled: 7;
    pub operational_mode, set_operational_mode: 9, 8;
    // reserved2 must be zero
    pub reserved2, set_reserved2: 15, 10;
}

impl CacheConfiguration {
    pub fn new(
        cache_level: u16,
        cache_socketed: bool,
        location: u16,
        enabled_disabled: bool,
        operational_mode: u16,
    ) -> Self {
        let mut config = CacheConfiguration(0);
        config.set_cache_level(cache_level);
        config.set_cache_socketed(cache_socketed);
        config.set_reserved(false); // reserved must be zero
        config.set_location(location);
        config.set_enabled_disabled(enabled_disabled);
        config.set_operational_mode(operational_mode);
        config.set_reserved2(0); // reserved2 must be zero
        config
    }
}

bitfield! {
    ///
    /// Cache Information - Cache Size
    ///
    pub struct CacheSize (u16);
    impl Debug;
    pub max_size, set_max_size : 14, 0;
    pub granularity, set_granularity : 15;
}

impl CacheSize {
    pub fn new(
        max_size: u16,
        granularity: bool,
    ) -> Self {
        let mut config = CacheSize(0);
        config.set_max_size(max_size);
        config.set_granularity(granularity);
        config
    }
}

bitfield! {
    ///
    /// Cache Information - Cache Size 2
    ///
    pub struct CacheSize2 (u32);
    impl Debug;
    pub max_size, set_max_size : 30, 0;
    pub granularity, set_granularity : 31;
}

impl CacheSize2 {
    pub fn new(
        max_size: u32,
        granularity: bool,
    ) -> Self {
        let mut config = CacheSize2(0);
        config.set_max_size(max_size);
        config.set_granularity(granularity);
        config
    }
}

bitfield! {
    ///
    /// Cache Information - SRAM Type
    ///
    pub struct CacheSramTypeData(u16);
    impl Debug;
    pub other, set_other: 0;
    pub unknown, set_unknown: 1;
    pub non_burst, set_non_burst: 2;
    pub burst, set_burst: 3;
    pub pipeline_burst, set_pipeline_burst: 4;
    pub synchronous, set_synchronous: 5;
    pub asynchronous, set_asynchronous: 6;
    // reserved must be zero
    pub reserved, set_reserved: 15, 7;
}

impl CacheSramTypeData {
    pub fn new(
        other: bool,
        unknown: bool,
        non_burst: bool,
        burst: bool,
        pipeline_burst: bool,
        synchronous: bool,
        asynchronous: bool,
    ) -> Self {
        let mut config = CacheSramTypeData(0);
        config.set_other(other);
        config.set_unknown(unknown);
        config.set_non_burst(non_burst);
        config.set_burst(burst);
        config.set_pipeline_burst(pipeline_burst);
        config.set_synchronous(synchronous);
        config.set_asynchronous(asynchronous);
        config.set_reserved(0);
        config
    }
}

///
/// Cache Information - Error Correction Type
///
#[derive(Copy, Clone, Debug)]
pub enum ErrorCorrectionType {
    Other = 0x01,
    Unknown,
    None,
    Parity,
    SingleBitEcc,
    MutliBitEcc,
}

///
/// Cache Information - System Cache Type
///
#[derive(Copy, Clone, Debug)]
pub enum SystemCacheType {
    Other = 0x01,
    Unknown,
    Instruction,
    Data,
    Unified,
}

///
/// Cache Information - Error Correction Type
///
#[derive(Copy, Clone, Debug)]
pub enum AssociativityField {
    Other = 0x01,
    Unknown,
    DirectMapped,
    SetAssociative2Way,
    SetAssociative4Way,
    FullyAssociative,
    SetAssociative8Way,
    SetAssociative16Way,
    SetAssociative12Way,
    SetAssociative24Way,
    SetAssociative32Way,
    SetAssociative48Way,
    SetAssociative64Way,
    SetAssociative20Way,
}

///
/// System Slots - Slot Type
///
#[derive(Copy, Clone, Debug)]
pub enum SlotType {
    Other = 0x01,
    Unknown,
    Isa,
    Mca,
    Eisa,
    Pci,
    PcCard,
    VlVesa,
    Proprietary,
    ProcessorCardSlot,
    ProprietaryMemroyCardSlot,
    IoRiserCardSlot,
    NuBus,
    // 66MHz Capable PCI
    Pci66mhz,
    Agp,
    Agp2x,
    Agp4x,
    PciX,
    Agp8x,
    M2Socket1DP,
    M2Socket1SD,
    M2Socket2,
    M2Socket3,
    MxmTypeI,
    MxmTypeII,
    MxmTypeIIIStandard,
    MxmTypeIIIHe,
    MxmTypeIV,
    Mxm3TypeA,
    Mxm3TypeB,
    // PCI Express Gen 2 SFF-8639 (U.2)
    PciExpressGen2Sff8629,
    // PCI Express Gen 3 SFF-8639 (U.2)
    PciExpressGen3Sff8629,
    // PCI Express Mini 52-pin (CEM spec. 2.0) with bottom-side keep-outs
    PciExpressMini52PinBottomSideKeepOuts,
    // PCI Express Mini 52-pin (CEM spec. 2.0) without bottom-side keep-outs.
    PciExpressMini52Pin,
    PciExpressMini76Pin,
    // PCI Express Gen 4 SFF-8639 (U.2)
    PciExpressGen4Sff8639,
    // PCI Express Gen 5 SFF-8639 (U.2)
    PciExpressGen5Sff8639,
    OcpNic3SFF,
    OcpNic3LFF,
    // OCP NIC Prior to 3.0
    OcpNicPrior,
    Pc98C20 = 0xA0,
    Pc98C24,
    Pc98E,
    Pc98LocalBus,
    Pc98Card,
    PciExpress,
    PciExpressx1,
    PciExpressx2,
    PciExpressx4,
    PciExpressx8,
    PciExpressx16,
    PciExpressGen2,
    PciExpressGen2x1,
    PciExpressGen2x2,
    PciExpressGen2x4,
    PciExpressGen2x8,
    PciExpressGen2x16,
    PciExpressGen3,
    PciExpressGen3x1,
    PciExpressGen3x2,
    PciExpressGen3x4,
    PciExpressGen3x8,
    PciExpressGen3x16,
    PciExpressGen4 = 0xB8,
    PciExpressGen4x1,
    PciExpressGen4x2,
    PciExpressGen4x4,
    PciExpressGen4x8,
    PciExpressGen4x16,
    PciExpressGen5,
    PciExpressGen5x1,
    PciExpressGen5x2,
    PciExpressGen5x4,
    PciExpressGen5x8,
    PciExpressGen5x16,
    PciExpressGen6,
    // Enterprise and Datacenter 1U E1 Form Factor Slot (EDSFF E1.S, E1.L)
    EdsffE1SE1L,
    // Enterprise and Datacenter 3" E3 Form Factor Slot (EDSFF E3.S, E3.L)
    EdsffE3SE3L,
}

///
/// System Slots - Slot Data Bus Width
///
#[derive(Copy, Clone, Debug)]
pub enum SlotWidth {
    Other = 0x01,
    Unknown,
    Bit8,
    Bit16,
    Bit32,
    Bit64,
    Bit128,
    X1,
    X2,
    X4,
    X8,
    X12,
    X16,
    X32,
}

///
/// System Slots - Current Usage
///
#[derive(Copy, Clone, Debug)]
pub enum CurrentUsage {
    Other = 0x01,
    Unknown,
    Available,
    InUse,
    Unavailable,
}

///
/// System Slots - Slot Length
///
#[derive(Copy, Clone, Debug)]
pub enum SlotLength {
    Other = 0x01,
    Unknown,
    ShortLength,
    LongLength,
    // 2.5" drive form factor
    DriveFF25,
    // 3.5" drive form factor
    DriveFF35,
}

bitfield! {
    ///
    /// System Slots - Slot Charcteristics 1
    ///
    pub struct SlotCharacteristics1(u8);
    impl Debug;
    pub characteristics_unknown, set_characteristics_unknown: 0;
    pub provides_5_volts, set_provides_5_volts: 1;
    pub provides_3_volts, set_provides_3_volts: 2;
    pub shared_slot, set_shared_slot: 3;
    pub pc_supports_pccard16, set_pc_supports_pccard16: 4;
    pub pc_supports_cardbus, set_pc_supports_cardbus: 5;
    pub pc_supports_zoomvideo, set_pc_supports_zoomvideo: 6;
    pub pc_supports_modemringresume, set_pc_supports_modemringresume: 7;
}

impl SlotCharacteristics1 {
    pub fn new(
        characteristics_unknown: bool,
        provides_5_volts: bool,
        provides_3_volts: bool,
        shared_slot: bool,
        pc_supports_pccard16: bool,
        pc_supports_cardbus: bool,
        pc_supports_zoomvideo: bool,
        pc_supports_modemringresume: bool,
    ) -> Self{
        let mut config = SlotCharacteristics1(0);
        config.set_characteristics_unknown(characteristics_unknown);
        config.set_provides_5_volts(provides_5_volts);
        config.set_provides_3_volts(provides_3_volts);
        config.set_shared_slot(shared_slot);
        config.set_pc_supports_pccard16(pc_supports_pccard16);
        config.set_pc_supports_cardbus(pc_supports_cardbus);
        config.set_pc_supports_zoomvideo(pc_supports_zoomvideo);
        config.set_pc_supports_modemringresume(pc_supports_modemringresume);
        config
    }
}

bitfield! {
    ///
    /// System Slots - Slot Charcteristics 2
    ///
    /// Various fields were added through SMBIOS Version 3. For not-applicable fields use '0'.
    ///
    pub struct  SlotCharacteristics2(u8);
    impl Debug;
    pub pci_supports_pme, set_pci_supports_pme: 0;
    pub supports_hotplug, set_supports_hotplug: 1;
    pub pci_supports_smbus, set_pci_supports_smbus: 2;
    pub pcie_supports_bifurcation, set_pcie_supports_bifurcation: 3;
    pub supports_async_removal, set_supports_async_removal: 4;
    // Flexbus slot, CXL 1.0 capable
    pub flexbus_slot1, set_flexbus_slot1: 5;
    // Flexbus slot, CXL 2.0 capable
    pub flexbus_slot2, set_flexbus_slot2: 6;
    // Flexbus slot, CXL 3.0 capable
    pub flexbus_slot3, set_flexbus_slot3: 7;
}

impl SlotCharacteristics2 {
    pub fn new(
        pci_supports_pme: bool,
        supports_hotplug: bool,
        pci_supports_smbus: bool,
        pcie_supports_bifurcation: bool,
        supports_async_removal: bool,
        flexbus_slot1: bool,
        flexbus_slot2: bool,
        flexbus_slot3: bool,
    ) -> Self{
        let mut config = SlotCharacteristics2(0);
        config.set_pci_supports_pme(pci_supports_pme);
        config.set_supports_hotplug(supports_hotplug);
        config.set_pci_supports_smbus(pci_supports_smbus);
        config.set_pcie_supports_bifurcation(pcie_supports_bifurcation);
        config.set_supports_async_removal(supports_async_removal);
        config.set_flexbus_slot1(flexbus_slot1);
        config.set_flexbus_slot2(flexbus_slot2);
        config.set_flexbus_slot3(flexbus_slot3);
        config
    }
}

bitfield! {
    ///
    /// System Slots - Device/Function Number
    ///
    #[derive(Copy, Clone)] 
    pub struct  DeviceFunctionNumber(u8);
    impl Debug;
    pub function_number, set_function_number : 2, 0;
    pub device_number, set_device_number : 7, 3;
}

impl DeviceFunctionNumber {
    pub fn new(
        function_number: u8,
        device_number: u8
    ) -> Self{
        let mut config = DeviceFunctionNumber(0);
        config.set_function_number(function_number);
        config.set_device_number(device_number);
        config
    }
}

///
/// System Slots - Peer Segment/Bus/Device/Function/Width Groups
///
#[derive(Copy, Clone, Debug)]
pub struct MiscSlotPeerGroup {
    pub segment_group_num: u16,
    pub bus_num: u8,
    pub dev_func_num: DeviceFunctionNumber,
    pub data_bus_width: u8,
}

///
/// Memory Array - Location
///
#[derive(Copy, Clone, Debug)]
pub enum MemoryArrayLocation {
    Other = 0x01,
    Unknown,
    SystemBoard,
    IsaAddOn,
    EisaAddOn,
    PciAddOn,
    McaAddOn,
    PcmciaAddOn,
    ProprietaryAddOn,
    NuBus,
    Pc98C20AddOn = 0xA0,
    Pc98C24AddOn,
    Pc98EAddOn,
    Pc98LocalAddOn,
    CxlAddOn,
}

///
/// Memory Array - Use
///
#[derive(Copy, Clone, Debug)]
pub enum MemoryArrayUse {
    Other = 0x01,
    Unknown,
    SystemMemory,
    VideoMemory,
    FlashMemory,
    NonVolatileRam,
    CacheMemory,
}

///
/// Memory Array - Error Correction Types
///
#[derive(Copy, Clone, Debug)]
pub enum ErrorCorrectionTypes {
    Other = 0x01,
    Unknown,
    None,
    Parity,
    SingleBitEcc,
    MultiBitEcc,
    Crc,
}

///
/// Memory Device - Form Factor
///
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum MemoryFormFactor {
    Other = 0x01,
    Unknown,
    Simm,
    Sip,
    Chip,
    Dip,
    Zip,
    ProprietaryCard,
    Dimm,
    Tsop,
    RowOfChips,
    Rimm,
    Sodimm,
    Srimm,
    FbDimm,
    Die,
    Camm,
    Cudimm,
    Csodimm,
}

///
/// Memory Device - Type
///
#[derive(Copy, Clone, Debug)]
pub enum MemoryDeviceType {
    Other = 0x01,
    Unknown,
    Dram,
    Edram,
    Vram,
    Sram,
    Ram,
    Rom,
    Flash,
    Eeprom,
    Feprom,
    Eprom,
    Cdram,
    ThreeDram,
    Sdram,
    Sgram,
    Rdram,
    Ddr,
    Ddr2,
    Ddr2FbDimm,
    Ddr3 = 0x18,
    Fbd2,
    Ddr4,
    Lpddr,
    Lpddr2,
    Lpddr3,
    Lpddr4,
    LogicalNonVolatileDevice,
    Hbm,
    Hbm2,
    Ddr5,
    Lpddr5,
    Hbm3,
    Mrdimm,
}

bitfield! {
    ///
    /// Memory Device - Type Detail
    ///
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

impl MemoryDeviceTypeDetails {
    pub fn new(
        other: bool,
        unknown: bool,
        fast_paged: bool,
        static_column: bool,
        pseudo_static: bool,
        rambus: bool,
        synchronous: bool,
        cmos: bool,
        edo: bool,
        window_dram: bool,
        cache_dram: bool,
        nonvolatile: bool,
        registered: bool,
        unbuffered: bool,
        lr_dimm: bool
    ) -> Self {
        let mut config = MemoryDeviceTypeDetails(0);
        config.set_reserved(false);
        config.set_other(other);
        config.set_unknown(unknown);
        config.set_fast_paged(fast_paged);
        config.set_static_column(static_column);
        config.set_pseudo_static(pseudo_static);
        config.set_rambus(rambus);
        config.set_synchronous(synchronous);
        config.set_cmos(cmos);
        config.set_edo(edo);
        config.set_window_dram(window_dram);
        config.set_cache_dram(cache_dram);
        config.set_nonvolatile(nonvolatile);
        config.set_registered(registered);
        config.set_unbuffered(unbuffered);
        config.set_lr_dimm(lr_dimm);        
        config
    }
}

///
/// Memory Device - Memory Technology
///
#[derive(Copy, Clone, Debug)]
pub enum MemoryDeviceTechnology {
    Other = 0x01,
    Unknown,
    Dram,
    NvdimmN,
    NvdimmF,
    NvdimmP,
    IntelOptanePersistentMemory,
}

bitfield! {
    ///
    /// Memory Device - Attributes
    ///
    pub struct MemoryDeviceAttributes(u8);
    impl Debug;
    pub rank, set_rank: 3, 0;
    pub reserved, set_reserved: 7, 4;
}

impl MemoryDeviceAttributes {
    pub fn new(
        rank: u8
    ) -> Self{
        let mut config = MemoryDeviceAttributes(0);
        config.set_rank(rank);
        config.set_reserved(0);
        config
    }
}

bitfield! {
    ///
    /// Memory Device - Operating Mode Capability
    ///
    pub struct MemoryCapability(u16);
    impl Debug;
    // reserved is set to zero
    pub reserved, set_reserved : 0;
    pub other, set_other : 1;
    pub unknown, set_unknown : 2;
    pub volatile_memory, set_volatile_memory : 3;
    // Byte-accessible persistent memory
    pub byte_persistent_memory, set_byte_persistent_memory : 4;
    // Block-accessible persistent memory
    pub block_persistent_memory, set_block_persistent_memory : 5;
    // reserved2 is set to zero
    pub reserved2, set_reserved2 : 15, 6;
}

impl MemoryCapability {
    pub fn new(
        other: bool,
        unknown: bool,
        volatile_memory: bool,
        byte_persistent_memory: bool,
        block_persistent_memory: bool,
    ) -> Self {
        let mut config = MemoryCapability(0);
        config.set_reserved(false);
        config.set_other(other);
        config.set_unknown(unknown);
        config.set_volatile_memory(volatile_memory);
        config.set_byte_persistent_memory(byte_persistent_memory);
        config.set_block_persistent_memory(block_persistent_memory);
        config.set_reserved2(0);
        config
    }
}

macro_rules! impl_to_le_bytes_u8 {
    ($($t:ty),*) => {
        $(
            impl $t {
                pub fn to_le_bytes(&self) -> alloc::vec::Vec<u8> {
                    alloc::vec![*self as u8]
                }
            }
        )*
    };
}

macro_rules! impl_to_le_bytes_u16 {
    ($($t:ty),*) => {
        $(
            impl $t {
                pub fn to_le_bytes(&self) -> alloc::vec::Vec<u8> {
                    (*self as u16).to_le_bytes().to_vec()
                }
            }
        )*
    };
}

macro_rules! impl_to_le_bytes_bitfield {
    ($($t:ty),*) => {
        $(
            impl $t {
                pub fn to_le_bytes(&self) -> alloc::vec::Vec<u8> {
                    self.0.to_le_bytes().to_vec()
                }
            }
        )*
    };
}

// Use the macros
impl_to_le_bytes_u8!(
    ProcessorTypeData,
    ProcessorUpgrade,
    ErrorCorrectionType,
    SystemCacheType,
    AssociativityField,
    SlotWidth,
    CurrentUsage,
    SlotLength,
    MemoryArrayLocation,
    MemoryArrayUse,
    ErrorCorrectionTypes,
    MemoryFormFactor,
    MemoryDeviceType,
    MemoryDeviceTechnology,
    BoardType,
    WakeUpType,
    BootUpState,
    PowerSupplyState,
    ThermalState,
    SecurityStatus
);

impl_to_le_bytes_u16!(
    ProcessorFamilyData,
    SlotType
);

impl_to_le_bytes_bitfield!(
    ProcessorCharacteristics,
    CacheConfiguration,
    CacheSize,
    CacheSramTypeData,
    MemoryDeviceTypeDetails,
    MemoryCapability,
    ExtendedBiosRomSize,
    CacheSize2,
    BiosCharacteristics,
    ProcessorVoltage,
    ProcessorInformationStatus,
    SlotCharacteristics1,
    SlotCharacteristics2,
    DeviceFunctionNumber,
    MemoryDeviceAttributes,
    BiosCharacteristicsExt1,
    BiosCharacteristicsExt2,
    ContainedElementType,
    FeatureFlags
);

// special to_le_bytes for structs that use bitfields
impl MiscSlotPeerGroup {
    pub fn to_le_bytes(&self) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec::Vec::new();
        bytes.extend_from_slice(&self.segment_group_num.to_le_bytes());
        bytes.push(self.bus_num);
        bytes.push(self.dev_func_num.0);
        bytes.push(self.data_bus_width);
        bytes
    }
}

impl ContainedElements {
    pub fn to_le_bytes(&self) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec::Vec::new();
        bytes.push(self.contained_element_type.0);
        bytes.push(self.contained_element_minimum);
        bytes.push(self.contained_element_maximum);
        bytes
    }
}