//! Device path type and sub-type identifiers (UEFI 2.10 Section 10.3.1, Table 10.1).
//!
//! Every device path node starts with a 4-byte header: a type, a sub-type whose
//! meaning depends on the type, and the node length including the header. All
//! undefined types and sub-types are reserved. Extensions must use a vendor-defined
//! node with the vendor's own GUID (Section 10.4.6).
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

/// Device path type values as defined in UEFI specification.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum DevicePathType {
    /// Hardware device path.
    Hardware = 1,
    /// ACPI device path.
    Acpi = 2,
    /// Messaging device path.
    Messaging = 3,
    /// Media device path.
    Media = 4,
    /// BIOS Boot Specification device path.
    Bios = 5,
    /// End of hardware device path.
    End = 0x7F,
}

/// Hardware device path sub-types.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum HardwareSubType {
    /// PCI device path.
    Pci = 1,
    /// PC Card device path.
    Pccard = 2,
    /// Memory-mapped device path.
    MemoryMapped = 3,
    /// Vendor-defined device path.
    Vendor = 4,
    /// Controller device path.
    Controller = 5,
    /// BMC device path.
    Bmc = 6,
}

/// ACPI device path sub-types.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum AcpiSubType {
    /// ACPI device path.
    Acpi = 1,
    /// Extended ACPI device path.
    ExtendedAcpi = 2,
    /// ACPI address device path.
    Adr = 3,
    /// NVDIMM device identified by its NFIT device handle.
    Nvdimm = 4,
}

/// Messaging device path sub-types.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum MessagingSubType {
    /// ATAPI device path.
    Atapi = 1,
    /// SCSI device path.
    Scsi = 2,
    /// Fibre Channel device path.
    FiberChannel = 3,
    /// Fibre Channel Ex device path.
    FiberChannelEx = 21,
    /// 1394 device path.
    _1394 = 4,
    /// USB device path.
    Usb = 5,
    /// SATA device path.
    Sata = 18,
    /// USB WWID device path.
    UsbWwid = 16,
    /// Device Logical Unit device path.
    DeviceLogicalUnit = 17,
    /// USB Class device path.
    UsbClass = 15,
    /// I2O Random Block Storage Class device path.
    I2oRandomBlockStorageClass = 6,
    /// MAC Address device path.
    MacAddress = 11,
    /// IPv4 device path.
    IpV4 = 12,
    /// IPv6 device path.
    IpV6 = 13,
    /// VLAN device path.
    Vlan = 20,
    /// InfiniBand device path.
    InfiniBand = 9,
    /// UART device path.
    Uart = 14,
    /// Vendor-defined device path.
    Vendor = 10,
    /// SAS Ex device path.
    SasEx = 22,
    /// iSCSI device path.
    Iscsi = 19,
    /// NVM Express device path.
    NvmExpress = 23,
    /// URI device path.
    Uri = 24,
    /// UFS device path.
    Ufs = 25,
    /// SD device path.
    Sd = 26,
    /// Bluetooth device path.
    Bluetooth = 27,
    /// `WiFi` device path.
    WiFi = 28,
    /// Embedded Multi-Media Card device path.
    Emmc = 29,
    /// Bluetooth LE device path.
    BluetoothLE = 30,
    /// DNS device path.
    Dns = 31,
    /// NVDIMM device path.
    Nvdimm = 32,
    /// REST Service device path.
    RestService = 33,
    /// `NVMe` over Fabric device path.
    NvmeOf = 34,
}

/// Media device path sub-types.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum MediaSubType {
    /// Hard drive device path.
    HardDrive = 1,
    /// CD-ROM device path.
    CdRom = 2,
    /// Vendor-defined device path.
    Vendor = 3,
    /// File path device path.
    FilePath = 4,
    /// Media protocol device path.
    MediaProtocol = 5,
    /// PIWG firmware file device path.
    PiwgFirmwareFile = 6,
    /// PIWG firmware volume device path.
    PiwgFirmwareVolume = 7,
    /// Relative offset range device path.
    RelativeOffsetRange = 8,
    /// RAM disk device path.
    RamDisk = 9,
}

/// BIOS Boot Specification device path sub-types.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum BiosSubType {
    /// BIOS Boot Specification device path.
    BiosBootSpecification = 1,
}

/// End device path sub-types.
pub enum EndSubType {
    /// End of entire device path.
    Entire = 0xFF,
    /// End of device path instance.
    Instance = 0x01,
}
