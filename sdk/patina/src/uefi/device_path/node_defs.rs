//! Spec-defined device path node types defined in this module.
//!
//! Node definitions, binary conversions, and their unit tests are kept together
//! here. Binary conversions use [`DevicePathNode`], [`TryFromCtx`], and
//! [`TryIntoCtx`] directly. Display implementations are diagnostic and do not
//! yet provide complete canonical UEFI text conversion.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    fmt::{Display, Write},
    iter::Iterator,
};

use alloc::{boxed::Box, string::String, vec::Vec};

use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

use crate::{
    Char16Str, Char16String, device_path_node,
    uefi::device_path::parse_node::{self, DevicePathNode, Header, UnknownDevicePathNode},
};

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
    /// `InfiniBand` device path.
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

fn invalid(msg: &'static str) -> scroll::Error {
    scroll::Error::BadInput { size: 0, msg }
}

fn check_length(size: usize) -> Result<(), scroll::Error> {
    if size > usize::from(u16::MAX) - 4 {
        return Err(invalid("device path payload exceeds the 16-bit node length"));
    }
    Ok(())
}

fn ascii(value: &str) -> Result<(), scroll::Error> {
    if !value.is_ascii() || value.as_bytes().contains(&0) {
        return Err(invalid("device path string must be ASCII without embedded NUL"));
    }
    Ok(())
}

fn read_ascii(data: &[u8], offset: &mut usize) -> Result<String, scroll::Error> {
    let remaining = data.get(*offset..).ok_or_else(|| invalid("missing ASCII string"))?;
    let len = remaining.iter().position(|byte| *byte == 0).ok_or_else(|| invalid("missing ASCII terminator"))?;
    let bytes = remaining.get(..len).ok_or_else(|| invalid("invalid ASCII string bounds"))?;
    let value = core::str::from_utf8(bytes).map_err(|_| invalid("invalid ASCII string"))?;
    ascii(value)?;
    *offset += len + 1;
    Ok(String::from(value))
}

device_path_node! {
    /// An ATAPI device and its logical unit.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Atapi)]
    @[DevicePathNodeValidate(Self::validate_fields)]
    #[derive(Debug, Clone, Eq, PartialEq)]
    pub struct Atapi {
        /// Zero for the primary channel, one for the secondary channel.
        pub primary_secondary: u8,
        /// Zero for master mode, one for slave mode.
        pub slave_master: u8,
        /// Logical unit number.
        pub lun: u16,
    }
}

impl Atapi {
    fn validate_fields(&self) -> Result<(), scroll::Error> {
        let node = self;

        if node.primary_secondary > 1 || node.slave_master > 1 {
            return Err(invalid("invalid ATAPI channel or master/slave selector"));
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for Atapi {
    type Error = scroll::Error;

    fn try_from_ctx(data: &[u8], endian: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let node = Self {
            primary_secondary: data.gread_with::<u8>(&mut offset, endian)?,
            slave_master: data.gread_with::<u8>(&mut offset, endian)?,
            lun: data.gread_with::<u16>(&mut offset, endian)?,
        };
        node.validate()?;
        Ok((node, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for Atapi {
    type Error = scroll::Error;

    fn try_into_ctx(self, data: &mut [u8], endian: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.primary_secondary, &mut offset, endian)?;
        data.gwrite_with(self.slave_master, &mut offset, endian)?;
        data.gwrite_with(self.lun, &mut offset, endian)?;
        Ok(offset)
    }
}

impl Display for Atapi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A target and logical unit on a SCSI bus.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Scsi)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Scsi {
        /// Target ID (physical unit number) on the SCSI bus.
        pub pun: u16,
        /// Logical unit number.
        pub lun: u16,
    }
}

impl Display for Scsi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A Fibre Channel device using numeric WWN and LUN fields.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::FiberChannel)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct FiberChannel {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Fibre Channel world wide name.
        pub wwn: u64,
        /// Fibre Channel logical unit number.
        pub lun: u64,
    }
}

impl Display for FiberChannel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A Fibre Channel device with byte-oriented port name and SCSI LUN.
    ///
    /// The 24-byte layout includes the reserved field, as specified in UEFI 2.11
    /// Table 10.16 and EDK II, rather than the inconsistent 20-byte example.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::FiberChannelEx)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct FiberChannelEx {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Eight port-name bytes in wire order, without integer byte swapping.
        pub wwn: [u8; 8],
        /// Eight SCSI REPORT LUNS bytes in wire order.
        pub lun: [u8; 8],
    }
}

impl Display for FiberChannelEx {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An IEEE 1394 device.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::_1394)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Ieee1394 {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// The 64-bit IEEE 1394 identifier, not an EFI GUID.
        pub guid: u64,
    }
}

impl Display for Ieee1394 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A USB port and interface.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Usb)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Usb {
        /// USB parent port number.
        pub parent_port_number: u8,
        /// USB interface number.
        pub interface_number: u8,
    }
}

impl Display for Usb {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An I2O random block storage target.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::I2oRandomBlockStorageClass)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct I2oRandomBlockStorageClass {
        /// Target identifier.
        pub tid: u32,
    }
}

impl Display for I2oRandomBlockStorageClass {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A network interface's MAC address.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::MacAddress)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct MacAddress {
        /// Address bytes in wire order; producers pad unused bytes with zero.
        pub mac_address: [u8; 32],
        /// Network interface type, as assigned by IANA.
        pub if_type: u8,
    }
}

impl Display for MacAddress {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An IPv6 network connection.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::IpV6)]
    @[DevicePathNodeValidate(Self::validate_fields)]
    #[derive(Debug, Clone, Eq, PartialEq)]
    pub struct IpV6 {
        /// Local IPv6 address octets in network order.
        pub local_ip_address: [u8; 16],
        /// Remote IPv6 address octets in network order.
        pub remote_ip_address: [u8; 16],
        /// Local port number, encoded as a UEFI integer.
        pub local_port: u16,
        /// Remote port number, encoded as a UEFI integer.
        pub remote_port: u16,
        /// Network protocol number, such as 6 for TCP or 17 for UDP.
        pub protocol: u16,
        /// Address origin: 0 is manual, 1 is stateless, and 2 is stateful configuration.
        pub ip_address_origin: u8,
        /// Number of prefix bits, from 0 through 128.
        pub prefix_length: u8,
        /// Gateway IPv6 address octets in network order.
        pub gateway_ip_address: [u8; 16],
    }
}

impl IpV6 {
    fn validate_fields(&self) -> Result<(), scroll::Error> {
        let node = self;

        if node.ip_address_origin > 2 || node.prefix_length > 128 {
            return Err(invalid("invalid IPv6 address origin or prefix length"));
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for IpV6 {
    type Error = scroll::Error;

    fn try_from_ctx(data: &[u8], endian: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let node = Self {
            local_ip_address: data.gread_with::<[u8; 16]>(&mut offset, scroll::LE)?,
            remote_ip_address: data.gread_with::<[u8; 16]>(&mut offset, scroll::LE)?,
            local_port: data.gread_with::<u16>(&mut offset, endian)?,
            remote_port: data.gread_with::<u16>(&mut offset, endian)?,
            protocol: data.gread_with::<u16>(&mut offset, endian)?,
            ip_address_origin: data.gread_with::<u8>(&mut offset, endian)?,
            prefix_length: data.gread_with::<u8>(&mut offset, endian)?,
            gateway_ip_address: data.gread_with::<[u8; 16]>(&mut offset, scroll::LE)?,
        };
        node.validate()?;
        Ok((node, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for IpV6 {
    type Error = scroll::Error;

    fn try_into_ctx(self, data: &mut [u8], endian: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.local_ip_address.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.remote_ip_address.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.local_port, &mut offset, endian)?;
        data.gwrite_with(self.remote_port, &mut offset, endian)?;
        data.gwrite_with(self.protocol, &mut offset, endian)?;
        data.gwrite_with(self.ip_address_origin, &mut offset, endian)?;
        data.gwrite_with(self.prefix_length, &mut offset, endian)?;
        data.gwrite_with(self.gateway_ip_address.as_slice(), &mut offset, ())?;
        Ok(offset)
    }
}

impl Display for IpV6 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An IEEE 802.1Q virtual LAN.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Vlan)]
    @[DevicePathNodeValidate(Self::validate_fields)]
    #[derive(Debug, Clone, Eq, PartialEq)]
    pub struct Vlan {
        /// VLAN identifier, from 0 through 4094.
        pub vlan_id: u16,
    }
}

impl Vlan {
    fn validate_fields(&self) -> Result<(), scroll::Error> {
        let node = self;

        if node.vlan_id > 4094 {
            return Err(invalid("invalid VLAN identifier"));
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for Vlan {
    type Error = scroll::Error;

    fn try_from_ctx(data: &[u8], endian: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let node = Self { vlan_id: data.gread_with::<u16>(&mut offset, endian)? };
        node.validate()?;
        Ok((node, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for Vlan {
    type Error = scroll::Error;

    fn try_into_ctx(self, data: &mut [u8], endian: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.vlan_id, &mut offset, endian)?;
        Ok(offset)
    }
}

impl Display for Vlan {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An InfiniBand fabric endpoint.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::InfiniBand)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct InfiniBand {
        /// Resource flags; producers clear reserved bits above bit 4.
        pub resource_flags: u32,
        /// Remote port global identifier bytes, not an EFI GUID.
        pub port_gid: [u8; 16],
        /// Remote IOC or service identifier, selected by resource flags bit 0.
        pub service_id: u64,
        /// Persistent identifier of the remote IOC port.
        pub target_port_id: u64,
        /// Persistent identifier of the remote device.
        pub device_id: u64,
    }
}

impl Display for InfiniBand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A UART connection and its serial configuration.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Uart)]
    @[DevicePathNodeValidate(Self::validate_fields)]
    #[derive(Debug, Clone, Eq, PartialEq)]
    pub struct Uart {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Baud rate, or zero to use the device default.
        pub baud_rate: u64,
        /// Number of data bits, or zero to use the device default.
        pub data_bits: u8,
        /// Parity: 0 default, 1 none, 2 even, 3 odd, 4 mark, or 5 space.
        pub parity: u8,
        /// Stop bits: 0 default, 1 one, 2 one-and-a-half, or 3 two.
        pub stop_bits: u8,
    }
}

impl Uart {
    fn validate_fields(&self) -> Result<(), scroll::Error> {
        let node = self;

        if node.parity > 5 || node.stop_bits > 3 {
            return Err(invalid("invalid UART parity or stop bits"));
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for Uart {
    type Error = scroll::Error;

    fn try_from_ctx(data: &[u8], endian: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let node = Self {
            reserved: data.gread_with::<u32>(&mut offset, endian)?,
            baud_rate: data.gread_with::<u64>(&mut offset, endian)?,
            data_bits: data.gread_with::<u8>(&mut offset, endian)?,
            parity: data.gread_with::<u8>(&mut offset, endian)?,
            stop_bits: data.gread_with::<u8>(&mut offset, endian)?,
        };
        node.validate()?;
        Ok((node, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for Uart {
    type Error = scroll::Error;

    fn try_into_ctx(self, data: &mut [u8], endian: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.reserved, &mut offset, endian)?;
        data.gwrite_with(self.baud_rate, &mut offset, endian)?;
        data.gwrite_with(self.data_bits, &mut offset, endian)?;
        data.gwrite_with(self.parity, &mut offset, endian)?;
        data.gwrite_with(self.stop_bits, &mut offset, endian)?;
        Ok(offset)
    }
}

impl Display for Uart {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An extended serial-attached SCSI endpoint.
    ///
    /// Uses EDK II's packed 24-byte layout: address, LUN, topology, and relative
    /// port follow the header without gaps. UEFI 2.11 Table 10.36 inconsistently
    /// gives a 32-byte length and leaves an eight-byte hole before the LUN.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::SasEx)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct SasEx {
        /// Eight SAS target-port address bytes in wire order.
        pub sas_address: [u8; 8],
        /// Eight SCSI REPORT LUNS bytes in wire order.
        pub lun: [u8; 8],
        /// SAS/SATA device and topology information, including reserved bits.
        pub device_topology: u16,
        /// Relative target port.
        pub relative_target_port: u16,
    }
}

impl Display for SasEx {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A Universal Flash Storage target and logical unit.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Ufs)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Ufs {
        /// Target ID; UFS 2.0 producers use zero.
        pub pun: u8,
        /// Logical unit number, including well-known LUN encodings.
        pub lun: u8,
    }
}

impl Display for Ufs {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A Secure Digital card slot.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Sd)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Sd {
        /// Slot number.
        pub slot_number: u8,
    }
}

impl Display for Sd {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A Bluetooth device.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Bluetooth)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Bluetooth {
        /// Six Bluetooth address bytes in wire order.
        pub address: [u8; 6],
    }
}

impl Display for Bluetooth {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A wireless network identified by its SSID.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::WiFi)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct WiFi {
        /// The 32 SSID octets; this field is not a NUL-terminated string.
        pub ssid: [u8; 32],
    }
}

impl Display for WiFi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An embedded Multi-Media Card slot.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Emmc)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Emmc {
        /// Slot number.
        pub slot_number: u8,
    }
}

impl Display for Emmc {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A Bluetooth Low Energy device.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::BluetoothLE)]
    @[DevicePathNodeValidate(Self::validate_fields)]
    #[derive(Debug, Clone, Eq, PartialEq)]
    pub struct BluetoothLE {
        /// Six Bluetooth address bytes in wire order.
        pub address: [u8; 6],
        /// Address type: zero for public, one for random.
        pub address_type: u8,
    }
}

impl BluetoothLE {
    fn validate_fields(&self) -> Result<(), scroll::Error> {
        let node = self;

        if node.address_type > 1 {
            return Err(invalid("invalid Bluetooth LE address type"));
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for BluetoothLE {
    type Error = scroll::Error;

    fn try_from_ctx(data: &[u8], endian: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let node = Self {
            address: data.gread_with::<[u8; 6]>(&mut offset, scroll::LE)?,
            address_type: data.gread_with::<u8>(&mut offset, endian)?,
        };
        node.validate()?;
        Ok((node, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for BluetoothLE {
    type Error = scroll::Error;

    fn try_into_ctx(self, data: &mut [u8], endian: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.address.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.address_type, &mut offset, endian)?;
        Ok(offset)
    }
}

impl Display for BluetoothLE {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A persistent memory namespace (messaging node, not the ACPI physical device).
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Nvdimm)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct Nvdimm {
        /// Namespace label UUID as 16 EFI GUID wire bytes, without additional byte swapping.
        pub uuid: [u8; 16],
    }
}

impl Display for Nvdimm {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// An ACPI physical NVDIMM device, distinct from a messaging namespace node.
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Nvdimm)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct NvdimmAcpi {
        /// NFIT device handle corresponding to the NVDIMM's ACPI `_ADR`.
        pub nfit_device_handle: u32,
    }
}

impl Display for NvdimmAcpi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A logical unit appended to an interface's device path.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::DeviceLogicalUnit)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct DeviceLogicalUnit {
        /// Logical unit number for the interface.
        pub lun: u8,
    }
}

impl Display for DeviceLogicalUnit {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A USB class matching pattern.
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::UsbClass)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct UsbClass {
        /// USB vendor ID; `0xffff` matches any vendor.
        pub vendor_id: u16,
        /// USB product ID; `0xffff` matches any product.
        pub product_id: u16,
        /// USB class code; `0xff` matches any class.
        pub device_class: u8,
        /// USB subclass code; `0xff` matches any subclass.
        pub device_subclass: u8,
        /// USB protocol code; `0xff` matches any protocol.
        pub device_protocol: u8,
    }
}

impl Display for UsbClass {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A CD-ROM system partition.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::CdRom)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct CdRom {
        /// Boot catalog entry number; zero denotes the initial/default entry.
        pub boot_entry: u32,
        /// Starting relative logical block address.
        pub partition_start: u64,
        /// Partition size in blocks.
        pub partition_size: u64,
    }
}

impl Display for CdRom {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A protocol used at a location in a media device path.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::MediaProtocol)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct MediaProtocol {
        /// Protocol identifier as 16 EFI GUID wire bytes, without additional byte swapping.
        pub protocol: [u8; 16],
    }
}

impl Display for MediaProtocol {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A firmware file in a PI firmware volume.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareFile)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareFile {
        /// File name as 16 EFI GUID wire bytes, without additional byte swapping.
        pub fv_file_name: [u8; 16],
    }
}

impl Display for PiwgFirmwareFile {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A PI firmware volume.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareVolume)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareVolume {
        /// Volume name as 16 EFI GUID wire bytes, without additional byte swapping.
        pub fv_name: [u8; 16],
    }
}

impl Display for PiwgFirmwareVolume {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A range of offsets within a media device.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RelativeOffsetRange)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct RelativeOffsetRange {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Starting offset relative to the media device.
        pub starting_offset: u64,
        /// Ending offset relative to the media device.
        pub ending_offset: u64,
    }
}

impl Display for RelativeOffsetRange {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// A memory-backed disk.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RamDisk)]

    #[derive(Pread, Pwrite, Debug, Clone, Eq, PartialEq)]
    pub struct RamDisk {
        /// Starting physical memory address.
        pub starting_address: u64,
        /// Ending physical memory address.
        pub ending_address: u64,
        /// Disk type as 16 EFI GUID wire bytes, without additional byte swapping.
        pub type_guid: [u8; 16],
        /// Disk instance number, or zero if unsupported.
        pub instance: u16,
    }
}

impl Display for RamDisk {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// Vendor-defined hardware node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HardwareVendor {
    /// Vendor GUID in EFI wire byte order.
    pub guid: [u8; 16],
    /// Opaque vendor-defined bytes following the GUID.
    pub data: Vec<u8>,
}

impl DevicePathNode for HardwareVendor {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Hardware as u8, HardwareSubType::Vendor as u8, 20 + self.data.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Hardware as u8 && subtype == HardwareSubType::Vendor as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for HardwareVendor {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid = data.gread_with(&mut offset, scroll::LE)?;
        let payload = data.get(offset..).ok_or_else(|| invalid("missing vendor payload"))?;
        let (node, consumed) = (Self { guid, data: payload.to_vec() }, data.len());
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for HardwareVendor {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.guid.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.data.as_slice(), &mut offset, ())?;
        Ok(offset)
    }
}

impl core::fmt::Display for HardwareVendor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}
/// Vendor-defined messaging node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MessagingVendor {
    /// Vendor GUID in EFI wire byte order.
    pub guid: [u8; 16],
    /// Opaque vendor-defined bytes following the GUID.
    pub data: Vec<u8>,
}

impl DevicePathNode for MessagingVendor {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Messaging as u8, MessagingSubType::Vendor as u8, 20 + self.data.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::Vendor as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for MessagingVendor {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid = data.gread_with(&mut offset, scroll::LE)?;
        let payload = data.get(offset..).ok_or_else(|| invalid("missing vendor payload"))?;
        let (node, consumed) = (Self { guid, data: payload.to_vec() }, data.len());
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for MessagingVendor {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.guid.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.data.as_slice(), &mut offset, ())?;
        Ok(offset)
    }
}

impl core::fmt::Display for MessagingVendor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}
/// Vendor-defined media node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MediaVendor {
    /// Vendor GUID in EFI wire byte order.
    pub guid: [u8; 16],
    /// Opaque vendor-defined bytes following the GUID.
    pub data: Vec<u8>,
}

impl DevicePathNode for MediaVendor {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Media as u8, MediaSubType::Vendor as u8, 20 + self.data.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Media as u8 && subtype == MediaSubType::Vendor as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for MediaVendor {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid = data.gread_with(&mut offset, scroll::LE)?;
        let payload = data.get(offset..).ok_or_else(|| invalid("missing vendor payload"))?;
        let (node, consumed) = (Self { guid, data: payload.to_vec() }, data.len());
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for MediaVendor {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.guid.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.data.as_slice(), &mut offset, ())?;
        Ok(offset)
    }
}

impl core::fmt::Display for MediaVendor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// Expanded ACPI identifiers, with optional ASCII string overrides.
///
/// A nonempty string supersedes its corresponding numeric identifier.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ExtendedAcpi {
    /// Numeric hardware identifier.
    pub hid: u32,
    /// Numeric unique identifier.
    pub uid: u32,
    /// Numeric compatible identifier.
    pub cid: u32,
    /// Hardware identifier override, without its terminator.
    pub hid_string: String,
    /// Unique identifier override, without its terminator.
    pub uid_string: String,
    /// Compatible identifier override, without its terminator.
    pub cid_string: String,
}

impl DevicePathNode for ExtendedAcpi {
    fn header(&self) -> Header {
        Header::new(
            DevicePathType::Acpi as u8,
            AcpiSubType::ExtendedAcpi as u8,
            19 + self.hid_string.len() + self.uid_string.len() + self.cid_string.len(),
        )
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Acpi as u8 && subtype == AcpiSubType::ExtendedAcpi as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        ascii(&self.hid_string)?;
        ascii(&self.uid_string)?;
        ascii(&self.cid_string)
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for ExtendedAcpi {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let hid = data.gread_with(&mut offset, endian)?;
        let uid = data.gread_with(&mut offset, endian)?;
        let cid = data.gread_with(&mut offset, endian)?;
        let hid_string = read_ascii(data, &mut offset)?;
        let uid_string = read_ascii(data, &mut offset)?;
        let cid_string = read_ascii(data, &mut offset)?;
        let (node, consumed) = (Self { hid, uid, cid, hid_string, uid_string, cid_string }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for ExtendedAcpi {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.hid, &mut offset, endian)?;
        data.gwrite_with(self.uid, &mut offset, endian)?;
        data.gwrite_with(self.cid, &mut offset, endian)?;
        for value in [&self.hid_string, &self.uid_string, &self.cid_string] {
            data.gwrite_with(value.as_bytes(), &mut offset, ())?;
            data.gwrite_with(0u8, &mut offset, endian)?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for ExtendedAcpi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// ACPI address node containing one or more display device addresses.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AcpiAdr {
    /// ACPI `_ADR` values, in their original order.
    pub addresses: Vec<u32>,
}

impl DevicePathNode for AcpiAdr {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Acpi as u8, AcpiSubType::Adr as u8, 4 + 4 * self.addresses.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Acpi as u8 && subtype == AcpiSubType::Adr as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        if self.addresses.is_empty() {
            return Err(invalid("ACPI address node requires at least one address"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for AcpiAdr {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        if !data.len().is_multiple_of(4) {
            return Err(invalid("ACPI address payload is not a sequence of u32 values"));
        }
        let mut addresses = Vec::new();
        let mut offset = 0;
        while offset < data.len() {
            addresses.push(data.gread_with(&mut offset, endian)?);
        }
        let (node, consumed) = (Self { addresses }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for AcpiAdr {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        for address in &self.addresses {
            data.gwrite_with(*address, &mut offset, endian)?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for AcpiAdr {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// USB identity node using a serial-number suffix.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UsbWwid {
    /// USB interface number.
    pub interface: u16,
    /// USB vendor identifier.
    pub vendor_id: u16,
    /// USB product identifier.
    pub product_id: u16,
    /// Serial suffix as at most 64 UTF-16 code units, preserved verbatim.
    ///
    /// The wire length, rather than a required NUL, bounds this field.
    pub serial_number: Vec<u16>,
}

impl DevicePathNode for UsbWwid {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Messaging as u8, MessagingSubType::UsbWwid as u8, 10 + self.serial_number.len() * 2)
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::UsbWwid as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        if self.serial_number.len() > 64
            || core::char::decode_utf16(self.serial_number.iter().copied()).any(|c| c.is_err())
        {
            return Err(invalid("USB WWID requires at most 64 valid UTF-16 code units"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for UsbWwid {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        if !data.len().is_multiple_of(2) {
            return Err(invalid("USB WWID has an incomplete code unit"));
        }
        let mut offset = 0;
        let interface = data.gread_with(&mut offset, endian)?;
        let vendor_id = data.gread_with(&mut offset, endian)?;
        let product_id = data.gread_with(&mut offset, endian)?;
        let mut serial_number = Vec::new();
        while offset < data.len() {
            serial_number.push(data.gread_with(&mut offset, endian)?);
        }
        let (node, consumed) = (Self { interface, vendor_id, product_id, serial_number }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for UsbWwid {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.interface, &mut offset, endian)?;
        data.gwrite_with(self.vendor_id, &mut offset, endian)?;
        data.gwrite_with(self.product_id, &mut offset, endian)?;
        for unit in &self.serial_number {
            data.gwrite_with(*unit, &mut offset, endian)?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for UsbWwid {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// Gateway and subnet fields added to the original IPv4 node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IpV4Network {
    /// Gateway address in network byte order.
    pub gateway: [u8; 4],
    /// Subnet mask in network byte order.
    pub subnet_mask: [u8; 4],
}

/// IPv4 messaging node, preserving both the 19-byte legacy and 27-byte current form.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IpV4 {
    /// Local address in network byte order.
    pub local_ip: [u8; 4],
    /// Remote address in network byte order.
    pub remote_ip: [u8; 4],
    /// Local port, encoded as a UEFI little-endian integer.
    pub local_port: u16,
    /// Remote port, encoded as a UEFI little-endian integer.
    pub remote_port: u16,
    /// IP protocol number.
    pub protocol: u16,
    /// Zero for DHCP, one for a static address.
    pub static_ip: u8,
    /// Absent only for the original 19-byte node.
    pub network: Option<IpV4Network>,
}

impl DevicePathNode for IpV4 {
    fn header(&self) -> Header {
        Header::new(
            DevicePathType::Messaging as u8,
            MessagingSubType::IpV4 as u8,
            if self.network.is_some() { 27 } else { 19 },
        )
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::IpV4 as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        if self.static_ip > 1 {
            return Err(invalid("invalid IPv4 address origin"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for IpV4 {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let local_ip = data.gread_with(&mut offset, scroll::LE)?;
        let remote_ip = data.gread_with(&mut offset, scroll::LE)?;
        let local_port = data.gread_with(&mut offset, endian)?;
        let remote_port = data.gread_with(&mut offset, endian)?;
        let protocol = data.gread_with(&mut offset, endian)?;
        let static_ip = data.gread_with(&mut offset, endian)?;
        let network = if data.len() == offset {
            None
        } else {
            Some(IpV4Network {
                gateway: data.gread_with(&mut offset, scroll::LE)?,
                subnet_mask: data.gread_with(&mut offset, scroll::LE)?,
            })
        };
        let (node, consumed) =
            (Self { local_ip, remote_ip, local_port, remote_port, protocol, static_ip, network }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for IpV4 {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.local_ip.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.remote_ip.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.local_port, &mut offset, endian)?;
        data.gwrite_with(self.remote_port, &mut offset, endian)?;
        data.gwrite_with(self.protocol, &mut offset, endian)?;
        data.gwrite_with(self.static_ip, &mut offset, endian)?;
        if let Some(network) = &self.network {
            data.gwrite_with(network.gateway.as_slice(), &mut offset, ())?;
            data.gwrite_with(network.subnet_mask.as_slice(), &mut offset, ())?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for IpV4 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// iSCSI target and login parameters.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Iscsi {
    /// Network protocol; zero identifies TCP.
    pub protocol: u16,
    /// Login option bit field.
    pub options: u16,
    /// SCSI LUN bytes in SAM-defined order.
    pub lun: [u8; 8],
    /// Target portal group tag.
    pub portal_group: u16,
    /// ASCII target name without its NUL terminator, at most 223 bytes.
    pub target_name: String,
}

impl DevicePathNode for Iscsi {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Messaging as u8, MessagingSubType::Iscsi as u8, 19 + self.target_name.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::Iscsi as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        ascii(&self.target_name)?;
        if self.target_name.is_empty() || self.target_name.len() > 223 || self.protocol != 0 {
            return Err(invalid("invalid iSCSI target name or network protocol"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Iscsi {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let protocol = data.gread_with(&mut offset, endian)?;
        let options = data.gread_with(&mut offset, endian)?;
        let lun = data.gread_with(&mut offset, scroll::LE)?;
        let portal_group = data.gread_with(&mut offset, endian)?;
        let target_name = read_ascii(data, &mut offset)?;
        let (node, consumed) = (Self { protocol, options, lun, portal_group, target_name }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for Iscsi {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.protocol, &mut offset, endian)?;
        data.gwrite_with(self.options, &mut offset, endian)?;
        data.gwrite_with(self.lun.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.portal_group, &mut offset, endian)?;
        data.gwrite_with(self.target_name.as_bytes(), &mut offset, ())?;
        data.gwrite_with(0u8, &mut offset, endian)?;
        Ok(offset)
    }
}

impl core::fmt::Display for Iscsi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// URI node whose ASCII payload is bounded by the node length, not a terminator.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Uri {
    /// URI text, including an empty URI when appropriate.
    pub uri: String,
}

impl DevicePathNode for Uri {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Messaging as u8, MessagingSubType::Uri as u8, 4 + self.uri.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::Uri as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        ascii(&self.uri)
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Uri {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let uri = core::str::from_utf8(data).map_err(|_| invalid("URI must be ASCII"))?;
        let (node, consumed) = (Self { uri: String::from(uri) }, data.len());
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for Uri {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        data.pwrite_with(self.uri.as_bytes(), 0, ())
    }
}

impl core::fmt::Display for Uri {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// DNS server list with addresses stored as `EFI_IP_ADDRESS` byte arrays.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Dns {
    /// True for IPv6 addresses; false for IPv4 addresses in the first four bytes.
    pub is_ipv6: bool,
    /// One or more 16-byte addresses, with unused IPv4 bytes preserved.
    pub addresses: Vec<[u8; 16]>,
}

impl DevicePathNode for Dns {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Messaging as u8, MessagingSubType::Dns as u8, 5 + self.addresses.len() * 16)
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::Dns as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        if self.addresses.is_empty() {
            return Err(invalid("DNS node requires at least one address"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Dns {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let family: u8 = data.gread_with(&mut offset, scroll::LE)?;
        if family > 1 || !(data.len() - offset).is_multiple_of(16) {
            return Err(invalid("invalid DNS family or address length"));
        }
        let mut addresses = Vec::new();
        while offset < data.len() {
            addresses.push(data.gread_with(&mut offset, scroll::LE)?);
        }
        let (node, consumed) = (Self { is_ipv6: family == 1, addresses }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for Dns {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(u8::from(self.is_ipv6), &mut offset, scroll::LE)?;
        for address in &self.addresses {
            data.gwrite_with(address.as_slice(), &mut offset, ())?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for Dns {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// Service-specific data carried by a REST device path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RestServiceType {
    /// Redfish REST service.
    Redfish,
    /// `OData` REST service.
    OData,
    /// Vendor-specific REST service.
    Vendor {
        /// Vendor GUID in EFI wire byte order.
        guid: [u8; 16],
        /// Opaque vendor-defined bytes.
        data: Vec<u8>,
    },
}

/// REST service node, including its vendor-specific form.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RestService {
    /// Standard service or vendor payload.
    pub service: RestServiceType,
    /// One for in-band access, two for out-of-band access.
    pub access_mode: u8,
}

impl DevicePathNode for RestService {
    fn header(&self) -> Header {
        Header::new(
            DevicePathType::Messaging as u8,
            MessagingSubType::RestService as u8,
            match &self.service {
                RestServiceType::Vendor { data, .. } => 22 + data.len(),
                _ => 6,
            },
        )
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::RestService as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        if !matches!(self.access_mode, 1 | 2) {
            return Err(invalid("invalid REST access mode"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for RestService {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let kind: u8 = data.gread_with(&mut offset, scroll::LE)?;
        let access_mode = data.gread_with(&mut offset, scroll::LE)?;
        let service = match kind {
            1 => RestServiceType::Redfish,
            2 => RestServiceType::OData,
            0xff => {
                let guid = data.gread_with(&mut offset, scroll::LE)?;
                let vendor_data = data.get(offset..).ok_or_else(|| invalid("missing REST vendor payload"))?.to_vec();
                offset = data.len();
                RestServiceType::Vendor { guid, data: vendor_data }
            }
            _ => return Err(invalid("unknown REST service type")),
        };
        let (node, consumed) = (Self { service, access_mode }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for RestService {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        let kind: u8 = match self.service {
            RestServiceType::Redfish => 1,
            RestServiceType::OData => 2,
            RestServiceType::Vendor { .. } => 0xff,
        };
        data.gwrite_with(kind, &mut offset, scroll::LE)?;
        data.gwrite_with(self.access_mode, &mut offset, scroll::LE)?;
        if let RestServiceType::Vendor { guid, data: payload } = &self.service {
            data.gwrite_with(guid.as_slice(), &mut offset, ())?;
            data.gwrite_with(payload.as_slice(), &mut offset, ())?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for RestService {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// `NVMe` over Fabrics namespace identity.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NvmeOf {
    /// Namespace identifier type: 1 for EUI-64, 2 for NGUID, 3 for UUID.
    pub identifier_type: u8,
    /// Identifier bytes; EUI-64 uses the first eight bytes, UUID uses RFC byte order.
    pub identifier: [u8; 16],
    /// UTF-8 subsystem NQN without its wire NUL terminator, at most 223 bytes.
    pub subsystem_nqn: String,
}

impl DevicePathNode for NvmeOf {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Messaging as u8, MessagingSubType::NvmeOf as u8, 22 + self.subsystem_nqn.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Messaging as u8 && subtype == MessagingSubType::NvmeOf as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        if !matches!(self.identifier_type, 1..=3)
            || self.subsystem_nqn.is_empty()
            || self.subsystem_nqn.len() > 223
            || self.subsystem_nqn.as_bytes().contains(&0)
            || (self.identifier_type == 1 && self.identifier[8..].iter().any(|byte| *byte != 0))
        {
            return Err(invalid("invalid NVMe over Fabrics namespace identity"));
        }
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for NvmeOf {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let identifier_type = data.gread_with(&mut offset, scroll::LE)?;
        let identifier = data.gread_with(&mut offset, scroll::LE)?;
        let remaining = data.get(offset..).ok_or_else(|| invalid("missing subsystem NQN"))?;
        let length = remaining.iter().position(|byte| *byte == 0).ok_or_else(|| invalid("missing NQN terminator"))?;
        let bytes = remaining.get(..length).ok_or_else(|| invalid("invalid NQN length"))?;
        let subsystem_nqn = core::str::from_utf8(bytes).map_err(|_| invalid("invalid UTF-8 NQN"))?;
        offset += length + 1;
        let (node, consumed) =
            (Self { identifier_type, identifier, subsystem_nqn: String::from(subsystem_nqn) }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for NvmeOf {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.identifier_type, &mut offset, scroll::LE)?;
        data.gwrite_with(self.identifier.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.subsystem_nqn.as_bytes(), &mut offset, ())?;
        data.gwrite_with(0u8, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl core::fmt::Display for NvmeOf {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

/// Decode a node using its type and subtype, preserving unsupported nodes.
///
/// Malformed recognized nodes are logged and preserved for diagnostic display.
/// Use [`try_cast_to_dyn_device_path_node`] when decoding errors must be propagated.
pub fn cast_to_dyn_device_path_node(unknown: UnknownDevicePathNode<'_>) -> Box<dyn DevicePathNode + '_> {
    match try_cast_to_dyn_device_path_node(unknown.clone()) {
        Ok(node) => node,
        Err(error) => {
            log::warn!("Invalid device path node {}/{}: {error}", unknown.header.r#type, unknown.header.sub_type);
            Box::new(unknown)
        }
    }
}

/// Decode a recognized node, preserving unsupported type/subtype combinations.
///
/// # Errors
///
/// Returns an error for inconsistent lengths, invalid payloads, or trailing bytes
/// in a recognized node. Unknown payloads are retained without interpretation.
pub fn try_cast_to_dyn_device_path_node(
    unknown: UnknownDevicePathNode<'_>,
) -> Result<Box<dyn DevicePathNode + '_>, scroll::Error> {
    if unknown.header.length != 4 + unknown.data.len() {
        return Err(invalid("device path header and payload lengths differ"));
    }
    unknown.validate()?;
    macro_rules! cast {
        ($unknown:expr, $($ty:ty),*) => {
            match unknown.header {
                $(
                    h if <$ty>::is_type(h.r#type, h.sub_type) => {
                        let (node, consumed) = <$ty>::try_from_ctx(unknown.data, scroll::LE)?;
                        if consumed != unknown.data.len() {
                            return Err(invalid("unexpected trailing device path payload"));
                        }
                        node.validate()?;
                        Some(Box::new(node) as Box<dyn DevicePathNode>)
                    }
                )*,
                _ => None
            }
        };
    }

    match cast!(
        &unknown,
        // Hardware nodes.
        Pci,
        PcCard,
        MemoryMapped,
        Controller,
        Bmc,
        HardwareVendor,
        // ACPI nodes.
        Acpi,
        ExtendedAcpi,
        AcpiAdr,
        NvdimmAcpi,
        // Messaging nodes.
        NvmExpress,
        Sata,
        Atapi,
        Scsi,
        FiberChannel,
        FiberChannelEx,
        Ieee1394,
        Usb,
        UsbWwid,
        UsbClass,
        DeviceLogicalUnit,
        I2oRandomBlockStorageClass,
        MacAddress,
        IpV4,
        IpV6,
        Vlan,
        InfiniBand,
        Uart,
        MessagingVendor,
        SasEx,
        Iscsi,
        Uri,
        Ufs,
        Sd,
        Bluetooth,
        WiFi,
        Emmc,
        BluetoothLE,
        Dns,
        Nvdimm,
        RestService,
        NvmeOf,
        // Media nodes.
        FilePath,
        HardDrive,
        CdRom,
        MediaVendor,
        MediaProtocol,
        PiwgFirmwareFile,
        PiwgFirmwareVolume,
        RelativeOffsetRange,
        RamDisk,
        // BIOS nodes.
        Bios,
        // End nodes
        EndEntire,
        EndInstance
    ) {
        Some(n) => Ok(n),
        None => Ok(Box::new(unknown)),
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pci-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pci)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Pci {
        /// PCI Function Number.
        pub function: u8,
        /// PCI Device Number.
        pub device: u8,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pci-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pccard)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct PcCard {
        /// Function Number, 0 is the first one.
        pub function_number: u8,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#memory-mapped-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::MemoryMapped)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct MemoryMapped {
        /// EFI memory type.
        pub memory_type: u32,
        /// Starting memory Address.
        pub start_address: u64,
        /// Ending Memory Address.
        pub end_address: u64,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#controller-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Controller)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Controller {
        /// Controller Number.
        pub number: u32,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bmc-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Bmc {
        /// BMC interface type.
        pub interface_type: u8,
        /// BMC base address.
        pub base_address: u64,
    }
}

device_path_node! {
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Acpi)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-device-path>
    pub struct Acpi {
        /// _HID
        pub hid: u32,
        /// _UID
        pub uid: u32,
    }
}

impl Acpi {
    /// PCI Root Bridge HID (PNP0A03).
    pub const PCI_ROOT_HID: u32 = Acpi::eisa_id("PNP0A03");
    /// `PCIe` Root Bridge HID (PNP0A08).
    pub const PCIE_ROOT_HID: u32 = Acpi::eisa_id("PNP0A08");

    /// Create a new PCI root bridge ACPI device path node.
    pub fn new_pci_root(uid: u32) -> Self {
        Self { hid: Acpi::PCI_ROOT_HID, uid }
    }

    /// Compress a seven-character identifier using UEFI device path byte order.
    ///
    /// # Panics
    ///
    /// Panics for an invalid identifier. Use [`Self::try_eisa_id`] for external input.
    pub const fn eisa_id(hid: &str) -> u32 {
        match Self::try_eisa_id(hid) {
            Ok(id) => id,
            Err(_) => panic!("invalid EISA device path identifier"),
        }
    }

    /// Compress a seven-character EISA identifier in UEFI device path byte order.
    ///
    /// # Errors
    ///
    /// Rejects identifiers without three ASCII letters followed by four hex digits.
    #[allow(clippy::indexing_slicing)]
    pub const fn try_eisa_id(hid: &str) -> Result<u32, &'static str> {
        let bytes = hid.as_bytes();
        if bytes.len() != 7 {
            return Err("EISA identifier must contain seven ASCII characters");
        }
        let mut vendor = 0u32;
        let mut index = 0;
        while index < 3 {
            if !bytes[index].is_ascii_alphabetic() {
                return Err("EISA vendor must contain ASCII letters");
            }
            vendor = (vendor << 5) | (bytes[index].to_ascii_uppercase() - b'A' + 1) as u32;
            index += 1;
        }
        let mut product = 0u32;
        while index < 7 {
            let digit = match (bytes[index] as char).to_digit(16) {
                Some(digit) => digit,
                None => return Err("EISA product must contain hex digits"),
            };
            product = (product << 4) | digit;
            index += 1;
        }
        Ok((product << 16) | vendor)
    }
}

impl Display for Acpi {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.hid {
            Acpi::PCI_ROOT_HID => f.debug_tuple("PciRoot").field(&self.uid).finish(),
            _ => f.debug_tuple("Acpi").field(&self.hid).field(&self.uid).finish(),
        }
    }
}

/// BIOS Boot Specification device path with a NUL-terminated ASCII description.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Bios {
    /// BIOS device type.
    pub device_type: u16,
    /// Status flags.
    pub status_flag: u16,
    /// Description string without the wire terminator.
    pub description_str: String,
}

impl DevicePathNode for Bios {
    fn header(&self) -> Header {
        Header::new(
            DevicePathType::Bios as u8,
            BiosSubType::BiosBootSpecification as u8,
            9 + self.description_str.len(),
        )
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Bios as u8 && subtype == BiosSubType::BiosBootSpecification as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        ascii(&self.description_str)
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Bios {
    type Error = scroll::Error;
    fn try_from_ctx(buffer: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let device_type = buffer.gread_with::<u16>(&mut offset, ctx)?;
        let status_flag = buffer.gread_with::<u16>(&mut offset, ctx)?;
        let description_str = read_ascii(buffer, &mut offset)?;
        let (node, consumed) = (Self { device_type, status_flag, description_str }, offset);
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for Bios {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        dest.gwrite_with(self.device_type, &mut offset, ctx)?;
        dest.gwrite_with(self.status_flag, &mut offset, ctx)?;
        dest.gwrite_with(self.description_str.as_bytes(), &mut offset, ())?;
        dest.gwrite_with(0u8, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl core::fmt::Display for Bios {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// End of entire device path node.
    @[DevicePathNode(DevicePathType::End, EndSubType::Entire)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Copy)]
    pub struct EndEntire;
}

impl Display for EndEntire {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_char('.')
    }
}

impl TryIntoCtx<scroll::Endian> for EndEntire {
    type Error = scroll::Error;

    fn try_into_ctx(self, _: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl TryFromCtx<'_, scroll::Endian> for EndEntire {
    type Error = scroll::Error;

    fn try_from_ctx(_: &[u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        Ok((Self, 0))
    }
}

device_path_node! {
    /// End of device path instance node.
    @[DevicePathNode(DevicePathType::End, EndSubType::Instance)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Copy)]
    pub struct EndInstance;
}

impl Display for EndInstance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_char(';')
    }
}

impl TryIntoCtx<scroll::Endian> for EndInstance {
    type Error = scroll::Error;

    fn try_into_ctx(self, _: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl TryFromCtx<'_, scroll::Endian> for EndInstance {
    type Error = scroll::Error;

    fn try_from_ctx(_: &[u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        Ok((Self, 0))
    }
}

/// SATA Device Path.
///
/// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#sata-device-path>
#[derive(Clone)]
pub struct Sata {
    /// HBA Port Number.
    pub hba_port: u16,
    /// Port Multiplier Port Number.
    pub port_multiplier_port: u16,
    /// Logical Unit Number.
    pub lun: u16,
}

impl Sata {
    /// The on-wire size of Sata data (without header): 2 + 2 + 2 = 6 bytes.
    const DATA_SIZE: usize = 6;

    /// Create a new SATA device path node.
    ///
    /// # Arguments
    /// * `hba_port` - HBA port number
    /// * `port_multiplier_port` - Port multiplier port number (0xFFFF if not present)
    /// * `lun` - Logical unit number
    pub fn new(hba_port: u16, port_multiplier_port: u16, lun: u16) -> Self {
        Self { hba_port, port_multiplier_port, lun }
    }
}

impl DevicePathNode for Sata {
    fn header(&self) -> parse_node::Header {
        parse_node::Header {
            r#type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Sata as u8,
            length: parse_node::Header::size_of_header() + Self::DATA_SIZE,
        }
    }

    fn is_type(r#type: u8, sub_type: u8) -> bool {
        r#type == DevicePathType::Messaging as u8 && sub_type == MessagingSubType::Sata as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        if self.hba_port == u16::MAX {
            return Err(invalid("SATA HBA port 0xffff is reserved"));
        }
        Ok(())
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        let mut offset = 0;
        buffer.gwrite_with(header, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.hba_port, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.port_multiplier_port, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.lun, &mut offset, scroll::Endian::Little)?;
        Ok(offset)
    }
}

impl core::fmt::Debug for Sata {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sata")
            .field("hba_port", &self.hba_port)
            .field("port_multiplier_port", &self.port_multiplier_port)
            .field("lun", &self.lun)
            .finish()
    }
}

impl Display for Sata {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("Sata").field(&self.hba_port).field(&self.port_multiplier_port).field(&self.lun).finish()
    }
}

impl TryIntoCtx<scroll::Endian> for Sata {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        dest.gwrite_with(self.hba_port, &mut offset, ctx)?;
        dest.gwrite_with(self.port_multiplier_port, &mut offset, ctx)?;
        dest.gwrite_with(self.lun, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Sata {
    type Error = scroll::Error;

    fn try_from_ctx(buffer: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let hba_port = buffer.gread_with(&mut offset, ctx)?;
        let port_multiplier_port = buffer.gread_with(&mut offset, ctx)?;
        let lun = buffer.gread_with(&mut offset, ctx)?;
        let node = Self { hba_port, port_multiplier_port, lun };
        node.validate()?;
        Ok((node, offset))
    }
}

/// NVM Express Namespace Device Path.
///
/// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvme-namespace-device-path>
#[derive(Clone)]
pub struct NvmExpress {
    /// Namespace Identifier (NSID).
    pub namespace_id: u32,
    /// IEEE Extended Unique Identifier (EUI-64).
    pub eui64: u64,
}

impl NvmExpress {
    /// The on-wire size of `NvmExpress` data (without header): 4 bytes NSID + 8 bytes EUI64.
    const DATA_SIZE: usize = 4 + 8;

    /// Create a new `NvmExpress` device path node.
    ///
    /// # Arguments
    /// * `namespace_id` - The namespace identifier (typically 1 for the first namespace)
    /// * `eui64` - The IEEE Extended Unique Identifier (can be 0 if not specified)
    pub fn new(namespace_id: u32, eui64: u64) -> Self {
        Self { namespace_id, eui64 }
    }
}

impl parse_node::DevicePathNode for NvmExpress {
    fn header(&self) -> crate::uefi::device_path::parse_node::Header {
        parse_node::Header {
            r#type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::NvmExpress as u8,
            length: parse_node::Header::size_of_header() + Self::DATA_SIZE,
        }
    }

    fn is_type(r#type: u8, sub_type: u8) -> bool {
        r#type == DevicePathType::Messaging as u8 && sub_type == MessagingSubType::NvmExpress as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        if matches!(self.namespace_id, 0 | u32::MAX) {
            return Err(invalid("NVMe namespace ID is reserved"));
        }
        Ok(())
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        let mut offset = 0;
        buffer.gwrite_with(header, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.namespace_id, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.eui64, &mut offset, scroll::Endian::Little)?;
        Ok(offset)
    }
}

impl core::fmt::Debug for NvmExpress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NvmExpress").field("namespace_id", &self.namespace_id).field("eui64", &self.eui64).finish()
    }
}

impl Display for NvmExpress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("NvmExpress").field(&self.namespace_id).field(&self.eui64).finish()
    }
}

impl TryIntoCtx<scroll::Endian> for NvmExpress {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        dest.gwrite_with(self.namespace_id, &mut offset, ctx)?;
        dest.gwrite_with(self.eui64, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for NvmExpress {
    type Error = scroll::Error;

    fn try_from_ctx(buffer: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let namespace_id = buffer.gread_with(&mut offset, ctx)?;
        let eui64 = buffer.gread_with(&mut offset, ctx)?;
        let node = Self { namespace_id, eui64 };
        node.validate()?;
        Ok((node, offset))
    }
}

/// Hard Drive Media Device Path.
///
/// Represents a partition on a hard drive or similar storage device.
/// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hard-drive-media-device-path>
#[derive(Clone)]
pub struct HardDrive {
    /// Partition number (1-based).
    pub partition_number: u32,
    /// Starting LBA of the partition.
    pub partition_start: u64,
    /// Size of the partition in blocks.
    pub partition_size: u64,
    /// Partition signature (GUID for GPT, 4-byte signature for MBR).
    pub partition_signature: [u8; 16],
    /// Partition format: 0x01 = PC-AT MBR, 0x02 = GPT.
    pub partition_format: u8,
    /// Signature type: 0x00 = None, 0x01 = 32-bit MBR, 0x02 = GUID.
    pub signature_type: u8,
}

impl HardDrive {
    /// The on-wire size of `HardDrive` data (without header).
    const DATA_SIZE: usize = 4 + 8 + 8 + 16 + 1 + 1; // 38 bytes

    /// Partition format: GPT
    pub const FORMAT_GPT: u8 = 0x02;
    /// Partition format: MBR
    pub const FORMAT_MBR: u8 = 0x01;

    /// Signature type: GUID
    pub const SIGNATURE_TYPE_GUID: u8 = 0x02;
    /// Signature type: MBR 32-bit signature
    pub const SIGNATURE_TYPE_MBR: u8 = 0x01;

    /// Try to parse a `HardDrive` from a raw device path node.
    ///
    /// Returns `None` if the node's type/subtype does not identify a
    /// `HardDrive` node, or if the node data is shorter than
    /// [`HardDrive::DATA_SIZE`] and so cannot be decoded.
    ///
    /// Useful when walking a device path with
    /// [`crate::uefi::device_path::paths::DevicePath`] to recover the typed
    /// `HardDrive` payload from each node without doing the
    /// type/subtype check at the call site.
    pub fn try_from_node(node: &parse_node::UnknownDevicePathNode<'_>) -> Option<Self> {
        if !<Self as parse_node::DevicePathNode>::is_type(node.header.r#type, node.header.sub_type) {
            return None;
        }
        node.data.pread_with(0, scroll::LE).ok()
    }

    /// Create a new `HardDrive` device path node for a GPT partition.
    ///
    /// # Arguments
    /// * `partition_number` - 1-based partition number
    /// * `partition_start` - Starting LBA
    /// * `partition_size` - Size in blocks
    /// * `partition_guid` - The unique GUID of the partition
    pub fn new_gpt(partition_number: u32, partition_start: u64, partition_size: u64, partition_guid: [u8; 16]) -> Self {
        Self {
            partition_number,
            partition_start,
            partition_size,
            partition_signature: partition_guid,
            partition_format: Self::FORMAT_GPT,
            signature_type: Self::SIGNATURE_TYPE_GUID,
        }
    }
}

impl parse_node::DevicePathNode for HardDrive {
    fn header(&self) -> parse_node::Header {
        parse_node::Header {
            r#type: DevicePathType::Media as u8,
            sub_type: MediaSubType::HardDrive as u8,
            length: parse_node::Header::size_of_header() + Self::DATA_SIZE,
        }
    }

    fn is_type(r#type: u8, sub_type: u8) -> bool {
        r#type == DevicePathType::Media as u8 && sub_type == MediaSubType::HardDrive as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        if !matches!(self.partition_format, 1 | 2) || self.signature_type > 2 {
            return Err(invalid("invalid hard drive partition format or signature type"));
        }
        if (self.signature_type == 0 && self.partition_signature != [0; 16])
            || (self.signature_type == 1 && self.partition_signature[4..].iter().any(|byte| *byte != 0))
        {
            return Err(invalid("invalid hard drive signature padding"));
        }
        Ok(())
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        let mut offset = 0;
        buffer.gwrite_with(header, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.partition_number, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.partition_start, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.partition_size, &mut offset, scroll::Endian::Little)?;
        for byte in &self.partition_signature {
            buffer.gwrite_with(*byte, &mut offset, scroll::Endian::Little)?;
        }
        buffer.gwrite_with(self.partition_format, &mut offset, scroll::Endian::Little)?;
        buffer.gwrite_with(self.signature_type, &mut offset, scroll::Endian::Little)?;
        Ok(offset)
    }
}

impl core::fmt::Debug for HardDrive {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HardDrive")
            .field("partition_signature", &self.partition_signature)
            .field("partition_number", &self.partition_number)
            .field("partition_start", &self.partition_start)
            .field("partition_size", &self.partition_size)
            .field("partition_format", &self.partition_format)
            .field("signature_type", &self.signature_type)
            .finish()
    }
}

impl Display for HardDrive {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "HD({}, GPT, ...)", self.partition_number)
    }
}

impl TryIntoCtx<scroll::Endian> for HardDrive {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        dest.gwrite_with(self.partition_number, &mut offset, ctx)?;
        dest.gwrite_with(self.partition_start, &mut offset, ctx)?;
        dest.gwrite_with(self.partition_size, &mut offset, ctx)?;
        for byte in &self.partition_signature {
            dest.gwrite_with(*byte, &mut offset, ctx)?;
        }
        dest.gwrite_with(self.partition_format, &mut offset, ctx)?;
        dest.gwrite_with(self.signature_type, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for HardDrive {
    type Error = scroll::Error;

    fn try_from_ctx(buffer: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let partition_number = buffer.gread_with(&mut offset, ctx)?;
        let partition_start = buffer.gread_with(&mut offset, ctx)?;
        let partition_size = buffer.gread_with(&mut offset, ctx)?;
        let mut partition_signature = [0u8; 16];
        for byte in &mut partition_signature {
            *byte = buffer.gread_with(&mut offset, ctx)?;
        }
        let partition_format = buffer.gread_with(&mut offset, ctx)?;
        let signature_type = buffer.gread_with(&mut offset, ctx)?;
        let node = Self {
            partition_number,
            partition_start,
            partition_size,
            partition_signature,
            partition_format,
            signature_type,
        };
        node.validate()?;
        Ok((node, offset))
    }
}

/// File Path Media Device Path.
///
/// The file path is a null-terminated string that describes a file path.
/// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#file-path-media-device-path>
#[derive(Clone)]
pub struct FilePath {
    /// The file path string (stored as UTF-8, serialized as UTF-16).
    pub path: String,
}

impl FilePath {
    /// Create a new `FilePath` device path node from a path string.
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    /// Calculate the UTF-16 encoded size of the path including null terminator.
    fn utf16_size(&self) -> usize {
        // Each char becomes one UTF-16 code unit (2 bytes), plus null terminator (2 bytes)
        // Note: This assumes BMP characters only (no surrogate pairs)
        (self.path.chars().count() + 1) * 2
    }
}

impl parse_node::DevicePathNode for FilePath {
    fn header(&self) -> parse_node::Header {
        parse_node::Header {
            r#type: DevicePathType::Media as u8,
            sub_type: MediaSubType::FilePath as u8,
            length: parse_node::Header::size_of_header() + self.utf16_size(),
        }
    }

    fn is_type(r#type: u8, sub_type: u8) -> bool {
        r#type == DevicePathType::Media as u8 && sub_type == MediaSubType::FilePath as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.utf16_size())?;
        Char16String::try_from_str(&self.path).map_err(|_| invalid("file path is not valid UCS-2"))?;
        Ok(())
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        let mut offset = 0;
        buffer.gwrite_with(header, &mut offset, scroll::Endian::Little)?;
        // Write the path as UCS-2 (UEFI CHAR16), including the trailing NUL terminator.
        let encoded = Char16String::try_from_str(&self.path)
            .map_err(|_| scroll::Error::BadInput { size: offset, msg: "file path is not valid UCS-2" })?;
        for unit in encoded.as_units_with_nul() {
            buffer.gwrite_with(*unit, &mut offset, scroll::LE)?;
        }
        Ok(offset)
    }
}

impl core::fmt::Debug for FilePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FilePath").field("path", &self.path).finish()
    }
}

impl Display for FilePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.path)
    }
}

impl TryIntoCtx<scroll::Endian> for FilePath {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.validate()?;
        let mut offset = 0;
        // Write the path as UCS-2 (UEFI CHAR16), including the trailing NUL terminator.
        let encoded = Char16String::try_from_str(&self.path)
            .map_err(|_| scroll::Error::BadInput { size: offset, msg: "file path is not valid UCS-2" })?;
        for unit in encoded.as_units_with_nul() {
            dest.gwrite_with(*unit, &mut offset, scroll::LE)?;
        }
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for FilePath {
    type Error = scroll::Error;

    fn try_from_ctx(buffer: &[u8], _ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let mut units = Vec::new();

        // Read UCS-2 code units until the null terminator.
        loop {
            if offset + 2 > buffer.len() {
                return Err(invalid("file path is missing its NUL terminator"));
            }
            let code_unit: u16 = buffer.gread_with(&mut offset, scroll::LE)?;
            if code_unit == 0 {
                break;
            }
            units.push(code_unit);
        }
        units.push(0);

        // Validate the collected code units as UCS-2 before decoding.
        let path = Char16Str::from_units_with_nul(&units)
            .map_err(|_| scroll::Error::BadInput { size: offset, msg: "file path is not valid UCS-2" })?
            .chars()
            .collect();

        Ok((Self { path }, offset))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use scroll::{Pread, Pwrite};

    #[test]
    fn test_sata_new() {
        let sata = Sata::new(1, 0xFFFF, 0);
        assert_eq!(sata.hba_port, 1);
        assert_eq!(sata.port_multiplier_port, 0xFFFF);
        assert_eq!(sata.lun, 0);
    }

    #[test]
    fn test_sata_serialization_roundtrip() {
        let sata = Sata::new(2, 0, 1);
        let mut buf = [0u8; 6];
        let written = buf.pwrite_with(sata.clone(), 0, scroll::LE).unwrap();
        assert_eq!(written, 6);

        let parsed: Sata = buf.pread_with(0, scroll::LE).unwrap();
        assert_eq!(parsed.hba_port, 2);
        assert_eq!(parsed.port_multiplier_port, 0);
        assert_eq!(parsed.lun, 1);
    }

    #[test]
    fn test_sata_display() {
        let sata = Sata::new(1, 0xFFFF, 0);
        let display = std::format!("{sata}");
        assert!(display.contains("Sata"));
    }

    #[test]
    fn test_nvme_new() {
        let nvme = NvmExpress::new(1, 0x123456789ABCDEF0);
        assert_eq!(nvme.namespace_id, 1);
        assert_eq!(nvme.eui64, 0x123456789ABCDEF0);
    }

    #[test]
    fn test_nvme_serialization_roundtrip() {
        let nvme = NvmExpress::new(1, 0xDEADBEEF12345678);
        let mut buf = [0u8; 12];
        let written = buf.pwrite_with(nvme.clone(), 0, scroll::LE).unwrap();
        assert_eq!(written, 12);

        let parsed: NvmExpress = buf.pread_with(0, scroll::LE).unwrap();
        assert_eq!(parsed.namespace_id, 1);
        assert_eq!(parsed.eui64, 0xDEADBEEF12345678);
    }

    #[test]
    fn test_nvme_display() {
        let nvme = NvmExpress::new(1, 0);
        let display = std::format!("{nvme}");
        assert!(display.contains("NvmExpress"));
    }

    #[test]
    fn test_hard_drive_new_gpt() {
        let guid = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10];
        let hd = HardDrive::new_gpt(1, 2048, 1000000, guid);
        assert_eq!(hd.partition_number, 1);
        assert_eq!(hd.partition_start, 2048);
        assert_eq!(hd.partition_size, 1000000);
        assert_eq!(hd.partition_signature, guid);
        assert_eq!(hd.partition_format, HardDrive::FORMAT_GPT);
        assert_eq!(hd.signature_type, HardDrive::SIGNATURE_TYPE_GUID);
    }

    #[test]
    fn test_hard_drive_serialization_roundtrip() {
        let guid = [0xAA; 16];
        let hd = HardDrive::new_gpt(2, 4096, 500000, guid);
        // DATA_SIZE = 4 + 8 + 8 + 16 + 1 + 1 = 38 bytes (no header in TryIntoCtx)
        let mut buf = [0u8; 38];
        let written = buf.pwrite_with(hd.clone(), 0, scroll::LE).unwrap();
        assert_eq!(written, 38);

        let parsed: HardDrive = buf.pread_with(0, scroll::LE).unwrap();
        assert_eq!(parsed.partition_number, 2);
        assert_eq!(parsed.partition_start, 4096);
        assert_eq!(parsed.partition_size, 500000);
        assert_eq!(parsed.partition_signature, guid);
        assert_eq!(parsed.partition_format, HardDrive::FORMAT_GPT);
        assert_eq!(parsed.signature_type, HardDrive::SIGNATURE_TYPE_GUID);
    }

    #[test]
    fn test_hard_drive_display() {
        let hd = HardDrive::new_gpt(1, 0, 0, [0; 16]);
        let display = std::format!("{hd}");
        assert!(display.contains("HD"));
    }

    #[test]
    fn test_hard_drive_try_from_node_success() {
        let guid = [0x11u8; 16];
        let hd = HardDrive::new_gpt(3, 0x1000, 0x4000, guid);
        let mut data = [0u8; 38];
        data.pwrite_with(hd, 0, scroll::LE).unwrap();
        let node = parse_node::UnknownDevicePathNode {
            header: parse_node::Header {
                r#type: DevicePathType::Media as u8,
                sub_type: MediaSubType::HardDrive as u8,
                length: parse_node::Header::size_of_header() + 38,
            },
            data: &data,
        };
        let parsed = HardDrive::try_from_node(&node).expect("valid HD node should parse");
        assert_eq!(parsed.partition_number, 3);
        assert_eq!(parsed.partition_start, 0x1000);
        assert_eq!(parsed.partition_size, 0x4000);
        assert_eq!(parsed.partition_signature, guid);
        assert_eq!(parsed.partition_format, HardDrive::FORMAT_GPT);
        assert_eq!(parsed.signature_type, HardDrive::SIGNATURE_TYPE_GUID);
    }

    #[test]
    fn test_hard_drive_try_from_node_wrong_type() {
        let data = [0u8; 38];
        let node = parse_node::UnknownDevicePathNode {
            header: parse_node::Header {
                r#type: DevicePathType::Messaging as u8, // not Media
                sub_type: MediaSubType::HardDrive as u8,
                length: parse_node::Header::size_of_header() + 38,
            },
            data: &data,
        };
        assert!(HardDrive::try_from_node(&node).is_none());
    }

    #[test]
    fn test_hard_drive_try_from_node_short_data() {
        let data = [0u8; 16]; // shorter than HardDrive::DATA_SIZE (38)
        let node = parse_node::UnknownDevicePathNode {
            header: parse_node::Header {
                r#type: DevicePathType::Media as u8,
                sub_type: MediaSubType::HardDrive as u8,
                length: parse_node::Header::size_of_header() + 16,
            },
            data: &data,
        };
        assert!(HardDrive::try_from_node(&node).is_none());
    }

    #[test]
    fn test_file_path_new() {
        let fp = FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI");
        assert_eq!(fp.path, "\\EFI\\BOOT\\BOOTX64.EFI");
    }

    #[test]
    fn test_file_path_serialization_roundtrip() {
        let fp = FilePath::new("\\test.efi");
        // UTF-16: 9 chars (\test.efi) + null = 10 * 2 = 20 bytes
        let mut buf = [0u8; 20];
        let written = buf.pwrite_with(fp.clone(), 0, scroll::LE).unwrap();
        assert_eq!(written, 20);

        let parsed: FilePath = buf.pread_with(0, scroll::LE).unwrap();
        assert_eq!(parsed.path, "\\test.efi");
    }

    #[test]
    fn test_file_path_display() {
        let fp = FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI");
        let display = std::format!("{fp}");
        assert_eq!(display, "\\EFI\\BOOT\\BOOTX64.EFI");
    }

    #[test]
    fn test_file_path_utf16_encoding() {
        let fp = FilePath::new("A");
        // 'A' (0x0041) + null (0x0000) = 4 bytes
        let mut buf = [0u8; 4];
        let written = buf.pwrite_with(fp, 0, scroll::LE).unwrap();
        assert_eq!(written, 4);
        // UTF-16LE: 'A' = 0x41, 0x00
        assert_eq!(buf[0], 0x41);
        assert_eq!(buf[1], 0x00);
        // Null terminator
        assert_eq!(buf[2], 0x00);
        assert_eq!(buf[3], 0x00);
    }
}

#[cfg(test)]
mod fixed_tests {
    use super::*;
    use crate::uefi::device_path::{
        node_defs::try_cast_to_dyn_device_path_node,
        parse_node::{DevicePathNode, UnknownDevicePathNode},
    };
    use alloc::{format, vec};
    use scroll::ctx::{TryFromCtx, TryIntoCtx};

    fn check_fixture<T>(node: T, wire: &[u8])
    where
        T: DevicePathNode + Clone + Eq,
        T: for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        node.validate().unwrap();
        assert_eq!(node.header().length, wire.len());
        assert!(T::is_type(wire[0], wire[1]));
        assert!(!T::is_type(wire[0] ^ 0x80, wire[1]));
        assert!(!T::is_type(wire[0], wire[1] ^ 0x80));

        let mut exact = vec![0xa5; wire.len()];
        assert_eq!(node.clone().write_into(&mut exact).unwrap(), wire.len());
        assert_eq!(exact, wire);

        let mut oversized = vec![0xa5; wire.len() + 7];
        assert_eq!(node.clone().write_into(&mut oversized).unwrap(), wire.len());
        assert_eq!(&oversized[..wire.len()], wire);
        assert_eq!(&oversized[wire.len()..], &[0xa5; 7]);

        let unknown: UnknownDevicePathNode<'_> = wire.pread_with(0, scroll::LE).unwrap();
        assert_eq!(unknown.header, node.header());
        let dynamic = try_cast_to_dyn_device_path_node(unknown.clone()).unwrap();
        assert_eq!(dynamic.header(), node.header());
        let type_name = core::any::type_name::<T>().rsplit("::").next().unwrap();
        let debug = format!("{dynamic:?}");
        assert!(debug.starts_with(&format!("{type_name} {{")), "unexpected decoded type: {debug}");
        assert_eq!(debug, format!("{node:?}"));
        let decoded: T = wire.pread_with(4, scroll::LE).unwrap();
        assert_eq!(decoded, node);
        let mut offset = 0;
        let decoded: T = unknown.data.gread_with(&mut offset, scroll::LE).unwrap();
        assert_eq!(decoded, node);
        assert_eq!(offset, wire.len() - 4);
        let mut payload = vec![0xa5; wire.len() - 4];
        assert_eq!(payload.pwrite_with(node.clone(), 0, scroll::LE).unwrap(), wire.len() - 4);
        assert_eq!(payload, &wire[4..]);

        for length in 0..wire.len() {
            assert!(
                wire[..length].pread_with::<UnknownDevicePathNode<'_>>(0, scroll::LE).is_err(),
                "accepted truncated node at {length}"
            );
            let mut short = vec![0xa5; length];
            assert!(node.clone().write_into(&mut short).is_err(), "accepted short destination at {length}");
            assert!(short.iter().all(|byte| *byte == 0xa5), "modified short destination at {length}");
        }
        for length in 0..payload.len() {
            assert!(
                payload[..length].pread_with::<T>(0, scroll::LE).is_err(),
                "accepted truncated payload at {length}"
            );
            let mut short = vec![0xa5; length];
            assert!(short.pwrite_with(node.clone(), 0, scroll::LE).is_err());
        }

        // The scroll context affects scalar fields; opaque byte arrays retain wire order.
        let mut big_endian = vec![0; payload.len()];
        assert_eq!(big_endian.pwrite_with(node.clone(), 0, scroll::BE).unwrap(), payload.len());
        assert_eq!(big_endian.pread_with::<T>(0, scroll::BE).unwrap(), node);
    }

    fn check_invalid<T>(node: T, payload: &[u8])
    where
        T: DevicePathNode + Clone,
        T: for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        assert!(node.validate().is_err());
        let mut output = vec![0xa5; node.header().length];
        assert!(node.clone().write_into(&mut output).is_err());
        assert!(output.iter().all(|byte| *byte == 0xa5));
        assert!(output.pwrite_with(node.clone(), 0, scroll::LE).is_err());
        assert!(output.iter().all(|byte| *byte == 0xa5));

        assert_eq!(payload.len() + 4, node.header().length);
        assert!(payload.pread_with::<T>(0, scroll::LE).is_err());
    }

    macro_rules! fixture {
    ($test:ident, $node:expr, [$($wire:expr),* $(,)?]) => {
        #[test]
        fn $test() {
            check_fixture($node, &[$($wire),*]);
        }
    };
}

    fixture!(
        test_device_path_atapi,
        Atapi { primary_secondary: 1, slave_master: 0, lun: 0x0403 },
        [3, 1, 8, 0, 1, 0, 3, 4]
    );

    fixture!(test_device_path_scsi, Scsi { pun: 0x0201, lun: 0x0403 }, [3, 2, 8, 0, 1, 2, 3, 4]);

    fixture!(
        test_device_path_fiber_channel,
        FiberChannel { reserved: 0x04030201, wwn: 0x0c0b0a0908070605, lun: 0x14131211100f0e0d },
        [3, 3, 24, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_fiber_channel_ex,
        FiberChannelEx {
            reserved: 0x04030201,
            wwn: [5, 6, 7, 8, 9, 10, 11, 12],
            lun: [13, 14, 15, 16, 17, 18, 19, 20]
        },
        [3, 21, 24, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_ieee1394,
        Ieee1394 { reserved: 0x04030201, guid: 0x0c0b0a0908070605 },
        [3, 4, 16, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );

    fixture!(test_device_path_usb, Usb { parent_port_number: 1, interface_number: 2 }, [3, 5, 6, 0, 1, 2]);

    fixture!(test_device_path_i2o, I2oRandomBlockStorageClass { tid: 0x04030201 }, [3, 6, 8, 0, 1, 2, 3, 4]);

    fixture!(
        test_device_path_mac_address,
        MacAddress {
            mac_address: [
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
                29, 30, 31, 32,
            ],
            if_type: 33,
        },
        [
            3, 11, 37, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
            26, 27, 28, 29, 30, 31, 32, 33,
        ]
    );

    fixture!(
        test_device_path_ipv6,
        IpV6 {
            local_ip_address: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            remote_ip_address: [17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32],
            local_port: 0x2221,
            remote_port: 0x2423,
            protocol: 0x2625,
            ip_address_origin: 2,
            prefix_length: 128,
            gateway_ip_address: [41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56],
        },
        [
            3, 13, 60, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
            26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 2, 128, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52,
            53, 54, 55, 56,
        ]
    );

    fixture!(test_device_path_vlan, Vlan { vlan_id: 4094 }, [3, 20, 6, 0, 0xfe, 0x0f]);

    fixture!(
        test_device_path_infiniband,
        InfiniBand {
            resource_flags: 0x04030201,
            port_gid: [5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20],
            service_id: 0x1c1b1a1918171615,
            target_port_id: 0x24232221201f1e1d,
            device_id: 0x2c2b2a2928272625,
        },
        [
            3, 9, 48, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
            27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44,
        ]
    );

    fixture!(
        test_device_path_uart,
        Uart { reserved: 0x04030201, baud_rate: 0x0c0b0a0908070605, data_bits: 8, parity: 5, stop_bits: 3 },
        [3, 14, 19, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 8, 5, 3]
    );

    fixture!(
        test_device_path_sas_ex,
        SasEx {
            sas_address: [1, 2, 3, 4, 5, 6, 7, 8],
            lun: [9, 10, 11, 12, 13, 14, 15, 16],
            device_topology: 0x1211,
            relative_target_port: 0x1413,
        },
        [3, 22, 24, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(test_device_path_ufs, Ufs { pun: 1, lun: 0x81 }, [3, 25, 6, 0, 1, 0x81]);

    fixture!(test_device_path_sd, Sd { slot_number: 7 }, [3, 26, 5, 0, 7]);

    fixture!(test_device_path_bluetooth, Bluetooth { address: [1, 2, 3, 4, 5, 6] }, [3, 27, 10, 0, 1, 2, 3, 4, 5, 6]);

    fixture!(
        test_device_path_wifi,
        WiFi {
            ssid: [
                1, 0, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
                29, 30, 31, 255,
            ],
        },
        [
            3, 28, 36, 0, 1, 0, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
            26, 27, 28, 29, 30, 31, 255,
        ]
    );

    fixture!(test_device_path_emmc, Emmc { slot_number: 8 }, [3, 29, 5, 0, 8]);

    fixture!(
        test_device_path_bluetooth_le,
        BluetoothLE { address: [1, 2, 3, 4, 5, 6], address_type: 1 },
        [3, 30, 11, 0, 1, 2, 3, 4, 5, 6, 1]
    );

    fixture!(
        test_device_path_nvdimm,
        Nvdimm { uuid: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] },
        [3, 32, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(test_device_path_nvdimm_acpi, NvdimmAcpi { nfit_device_handle: 0x04030201 }, [2, 4, 8, 0, 1, 2, 3, 4]);

    fixture!(test_device_path_device_logical_unit, DeviceLogicalUnit { lun: 7 }, [3, 17, 5, 0, 7]);

    fixture!(
        test_device_path_usb_class,
        UsbClass { vendor_id: 0x0201, product_id: 0x0403, device_class: 5, device_subclass: 6, device_protocol: 7 },
        [3, 15, 11, 0, 1, 2, 3, 4, 5, 6, 7]
    );

    fixture!(
        test_device_path_cdrom,
        CdRom { boot_entry: 0x04030201, partition_start: 0x0c0b0a0908070605, partition_size: 0x14131211100f0e0d },
        [4, 2, 24, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_media_protocol,
        MediaProtocol { protocol: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] },
        [4, 5, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(
        test_device_path_piwg_firmware_file,
        PiwgFirmwareFile { fv_file_name: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] },
        [4, 6, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(
        test_device_path_piwg_firmware_volume,
        PiwgFirmwareVolume { fv_name: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] },
        [4, 7, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(
        test_device_path_relative_offset_range,
        RelativeOffsetRange {
            reserved: 0x04030201,
            starting_offset: 0x0c0b0a0908070605,
            ending_offset: 0x14131211100f0e0d
        },
        [4, 8, 24, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_ram_disk,
        RamDisk {
            starting_address: 0x0807060504030201,
            ending_address: 0x100f0e0d0c0b0a09,
            type_guid: [17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32],
            instance: 0x2221,
        },
        [
            4, 9, 38, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
            27, 28, 29, 30, 31, 32, 33, 34,
        ]
    );

    #[test]
    fn test_device_path_atapi_selectors() {
        for primary_secondary in 0..=1 {
            for slave_master in 0..=1 {
                Atapi { primary_secondary, slave_master, lun: u16::MAX }.validate().unwrap();
            }
        }
        for invalid in [2, u8::MAX] {
            check_invalid(Atapi { primary_secondary: invalid, slave_master: 0, lun: 0 }, &[invalid, 0, 0, 0]);
            check_invalid(Atapi { primary_secondary: 0, slave_master: invalid, lun: 0 }, &[0, invalid, 0, 0]);
        }
    }

    #[test]
    fn test_device_path_vlan_range() {
        Vlan { vlan_id: 0 }.validate().unwrap();
        Vlan { vlan_id: 4094 }.validate().unwrap();
        check_invalid(Vlan { vlan_id: 4095 }, &[0xff, 0x0f]);
        check_invalid(Vlan { vlan_id: u16::MAX }, &[0xff, 0xff]);
    }

    #[test]
    fn test_device_path_ipv6_ranges() {
        let node = IpV6 {
            local_ip_address: [0; 16],
            remote_ip_address: [0; 16],
            local_port: 0,
            remote_port: 0,
            protocol: 0,
            ip_address_origin: 0,
            prefix_length: 0,
            gateway_ip_address: [0; 16],
        };
        for ip_address_origin in 0..=2 {
            for prefix_length in [0, 64, 128] {
                IpV6 { ip_address_origin, prefix_length, ..node.clone() }.validate().unwrap();
            }
        }
        for ip_address_origin in [3, u8::MAX] {
            let mut payload = [0; 56];
            payload[38] = ip_address_origin;
            check_invalid(IpV6 { ip_address_origin, ..node.clone() }, &payload);
        }
        for prefix_length in [129, u8::MAX] {
            let mut payload = [0; 56];
            payload[39] = prefix_length;
            check_invalid(IpV6 { prefix_length, ..node.clone() }, &payload);
        }
    }

    #[test]
    fn test_device_path_uart_ranges() {
        let node = Uart { reserved: 0, baud_rate: 0, data_bits: 0, parity: 0, stop_bits: 0 };
        for parity in 0..=5 {
            for stop_bits in 0..=3 {
                Uart { parity, stop_bits, ..node.clone() }.validate().unwrap();
            }
        }
        for parity in [6, u8::MAX] {
            let mut payload = [0; 15];
            payload[13] = parity;
            check_invalid(Uart { parity, ..node.clone() }, &payload);
        }
        for stop_bits in [4, u8::MAX] {
            let mut payload = [0; 15];
            payload[14] = stop_bits;
            check_invalid(Uart { stop_bits, ..node.clone() }, &payload);
        }
    }

    #[test]
    fn test_device_path_bluetooth_le_address_type() {
        for address_type in 0..=1 {
            BluetoothLE { address: [0; 6], address_type }.validate().unwrap();
        }
        for address_type in [2, u8::MAX] {
            check_invalid(BluetoothLE { address: [0; 6], address_type }, &[0, 0, 0, 0, 0, 0, address_type]);
        }
    }

    #[test]
    fn test_device_path_usb_class_wildcards() {
        check_fixture(
            UsbClass {
                vendor_id: u16::MAX,
                product_id: u16::MAX,
                device_class: u8::MAX,
                device_subclass: u8::MAX,
                device_protocol: u8::MAX,
            },
            &[3, 15, 11, 0, 255, 255, 255, 255, 255, 255, 255],
        );
    }

    #[test]
    fn test_device_path_opaque_arrays_ignore_endian_context() {
        let node = FiberChannelEx {
            reserved: 0x04030201,
            wwn: [5, 6, 7, 8, 9, 10, 11, 12],
            lun: [13, 14, 15, 16, 17, 18, 19, 20],
        };
        let mut output = [0; 20];
        output.pwrite_with(node, 0, scroll::BE).unwrap();
        assert_eq!(output, [4, 3, 2, 1, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);

        let guid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let mut output = [0; 16];
        output.pwrite_with(MediaProtocol { protocol: guid }, 0, scroll::BE).unwrap();
        assert_eq!(output, guid);
    }
}

#[cfg(test)]
mod binary_tests {
    use super::*;
    use crate::uefi::device_path::{parse_node::Header, paths::DevicePathBuf};
    use alloc::{format, vec};

    fn check_node<T>(node: T, expected: &[u8])
    where
        T: DevicePathNode + Clone + for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
    {
        assert_eq!(node.header().length, expected.len());
        let mut encoded = vec![0xa5; expected.len() + 3];
        assert_eq!(node.clone().write_into(&mut encoded).unwrap(), expected.len());
        assert_eq!(&encoded[..expected.len()], expected);
        assert_eq!(&encoded[expected.len()..], &[0xa5; 3]);
        for len in 0..expected.len() {
            assert!(node.clone().write_into(&mut vec![0; len]).is_err(), "short output {len}");
            assert!(expected[..len].pread_with::<UnknownDevicePathNode>(0, scroll::LE).is_err());
        }
        let (parsed, consumed) = T::try_from_ctx(&expected[4..], scroll::LE).unwrap();
        assert_eq!(consumed + 4, expected.len());
        let mut roundtrip = vec![0; expected.len()];
        parsed.write_into(&mut roundtrip).unwrap();
        assert_eq!(roundtrip, expected);
        let raw = expected.pread_with::<UnknownDevicePathNode>(0, scroll::LE).unwrap();
        let decoded = try_cast_to_dyn_device_path_node(raw).unwrap();
        assert_eq!(decoded.header().length, expected.len());
        assert!(!format!("{decoded:?}").starts_with("UnknownDevicePathNode"));
    }

    #[test]
    fn test_device_path_vendor_nodes() {
        let mut expected = vec![1, 4, 22, 0];
        expected.extend_from_slice(&[0x11; 16]);
        expected.extend_from_slice(&[0xaa, 0xbb]);
        check_node(HardwareVendor { guid: [0x11; 16], data: vec![0xaa, 0xbb] }, &expected);
        expected[0] = 3;
        expected[1] = 10;
        check_node(MessagingVendor { guid: [0x11; 16], data: vec![0xaa, 0xbb] }, &expected);
        expected[0] = 4;
        expected[1] = 3;
        check_node(MediaVendor { guid: [0x11; 16], data: vec![0xaa, 0xbb] }, &expected);
    }

    #[test]
    fn test_device_path_extended_acpi() {
        check_node(
            ExtendedAcpi {
                hid: 1,
                uid: 2,
                cid: 3,
                hid_string: "A".into(),
                uid_string: "B".into(),
                cid_string: "C".into(),
            },
            &[2, 2, 22, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, b'A', 0, b'B', 0, b'C', 0],
        );
        check_node(AcpiAdr { addresses: vec![0x12345678, 9] }, &[2, 3, 12, 0, 0x78, 0x56, 0x34, 0x12, 9, 0, 0, 0]);
    }

    #[test]
    fn test_device_path_usb_wwid() {
        check_node(
            UsbWwid { interface: 1, vendor_id: 0x1234, product_id: 0x5678, serial_number: vec![0x41, 0x42] },
            &[3, 16, 14, 0, 1, 0, 0x34, 0x12, 0x78, 0x56, 0x41, 0, 0x42, 0],
        );
    }

    #[test]
    fn test_device_path_ipv4_legacy_and_current() {
        let mut node = IpV4 {
            local_ip: [1, 2, 3, 4],
            remote_ip: [5, 6, 7, 8],
            local_port: 0x1234,
            remote_port: 0x5678,
            protocol: 6,
            static_ip: 1,
            network: None,
        };
        let mut expected = vec![3, 12, 19, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0x34, 0x12, 0x78, 0x56, 6, 0, 1];
        check_node(node.clone(), &expected);
        node.network = Some(IpV4Network { gateway: [9, 10, 11, 12], subnet_mask: [255, 255, 255, 0] });
        expected[2] = 27;
        expected.extend_from_slice(&[9, 10, 11, 12, 255, 255, 255, 0]);
        check_node(node, &expected);
        for len in 16..23 {
            assert!(IpV4::try_from_ctx(&expected[4..4 + len], scroll::LE).is_err());
        }
    }

    #[test]
    fn test_device_path_iscsi_and_uri() {
        check_node(
            Iscsi {
                protocol: 0,
                options: 0x0800,
                lun: [1, 2, 3, 4, 5, 6, 7, 8],
                portal_group: 0x1234,
                target_name: "iqn".into(),
            },
            &[3, 19, 22, 0, 0, 0, 0, 8, 1, 2, 3, 4, 5, 6, 7, 8, 0x34, 0x12, b'i', b'q', b'n', 0],
        );
        check_node(
            Uri { uri: "https://a".into() },
            &[3, 24, 13, 0, b'h', b't', b't', b'p', b's', b':', b'/', b'/', b'a'],
        );
        check_node(Uri { uri: String::new() }, &[3, 24, 4, 0]);
    }

    #[test]
    fn test_device_path_dns() {
        let mut expected = vec![3, 31, 21, 0, 1];
        expected.extend_from_slice(&[0x12; 16]);
        check_node(Dns { is_ipv6: true, addresses: vec![[0x12; 16]] }, &expected);
        expected[4] = 0;
        check_node(Dns { is_ipv6: false, addresses: vec![[0x12; 16]] }, &expected);
    }

    #[test]
    fn test_device_path_rest_variants() {
        check_node(RestService { service: RestServiceType::Redfish, access_mode: 1 }, &[3, 33, 6, 0, 1, 1]);
        check_node(RestService { service: RestServiceType::OData, access_mode: 2 }, &[3, 33, 6, 0, 2, 2]);
        let mut expected = vec![3, 33, 24, 0, 255, 1];
        expected.extend_from_slice(&[0x33; 16]);
        expected.extend_from_slice(&[0x44, 0x55]);
        check_node(
            RestService {
                service: RestServiceType::Vendor { guid: [0x33; 16], data: vec![0x44, 0x55] },
                access_mode: 1,
            },
            &expected,
        );
    }

    #[test]
    fn test_device_path_nvme_of() {
        let mut expected = vec![3, 34, 25, 0, 3];
        expected.extend_from_slice(&[0x22; 16]);
        expected.extend_from_slice(b"nqn\0");
        check_node(NvmeOf { identifier_type: 3, identifier: [0x22; 16], subsystem_nqn: "nqn".into() }, &expected);
    }

    #[test]
    fn test_device_path_existing_wire_sizes() {
        check_node(Pci { function: 1, device: 2 }, &[1, 1, 6, 0, 1, 2]);
        check_node(PcCard { function_number: 3 }, &[1, 2, 5, 0, 3]);
        check_node(Controller { number: 0x12345678 }, &[1, 5, 8, 0, 0x78, 0x56, 0x34, 0x12]);
        check_node(
            MemoryMapped { memory_type: 3, start_address: 0x1234, end_address: 0x5678 },
            &[1, 3, 24, 0, 3, 0, 0, 0, 0x34, 0x12, 0, 0, 0, 0, 0, 0, 0x78, 0x56, 0, 0, 0, 0, 0, 0],
        );
        check_node(Bmc { interface_type: 1, base_address: 0x1234 }, &[1, 6, 13, 0, 1, 0x34, 0x12, 0, 0, 0, 0, 0, 0]);
        check_node(Acpi::new_pci_root(0), &[2, 1, 12, 0, 0xd0, 0x41, 3, 0x0a, 0, 0, 0, 0]);
        check_node(
            Bios { device_type: 2, status_flag: 1, description_str: "foo".into() },
            &[5, 1, 12, 0, 2, 0, 1, 0, b'f', b'o', b'o', 0],
        );
        check_node(EndEntire, &[0x7f, 0xff, 4, 0]);
        check_node(EndInstance, &[0x7f, 1, 4, 0]);
        check_node(Sata::new(1, 0xffff, 2), &[3, 18, 10, 0, 1, 0, 255, 255, 2, 0]);
        check_node(NvmExpress::new(1, 0x1234), &[3, 23, 16, 0, 1, 0, 0, 0, 0x34, 0x12, 0, 0, 0, 0, 0, 0]);
        check_node(FilePath::new("A"), &[4, 4, 8, 0, b'A', 0, 0, 0]);
        let mut expected = vec![4, 1, 42, 0, 1, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0];
        expected.extend_from_slice(&[0x11; 16]);
        expected.extend_from_slice(&[2, 2]);
        check_node(HardDrive::new_gpt(1, 2048, 4096, [0x11; 16]), &expected);
    }

    #[test]
    fn test_device_path_checked_lengths_and_dispatch_errors() {
        let mut output = [0u8; 4];
        assert!(Header::new(1, 1, 65536).try_into_ctx(&mut output, scroll::LE).is_err());
        assert!(Header::new(1, 1, 3).try_into_ctx(&mut output, scroll::LE).is_err());
        for length in 0..4 {
            assert!([1, 1, length, 0].pread_with::<UnknownDevicePathNode>(0, scroll::LE).is_err());
        }
        for data in [&[1u8][..], &[1, 2, 3][..]] {
            let raw = UnknownDevicePathNode { header: Header::new(1, 1, data.len() + 4), data };
            assert!(try_cast_to_dyn_device_path_node(raw).is_err());
        }
        let unknown = UnknownDevicePathNode { header: Header::new(6, 1, 6), data: &[1, 2] };
        assert!(
            format!("{:?}", try_cast_to_dyn_device_path_node(unknown.clone()).unwrap())
                .starts_with("UnknownDevicePathNode")
        );
        let mut encoded = [0; 6];
        unknown.write_into(&mut encoded).unwrap();
        assert_eq!(encoded, [6, 1, 6, 0, 1, 2]);
    }

    #[test]
    fn test_device_path_invalid_variable_payloads() {
        assert!(AcpiAdr { addresses: vec![] }.validate().is_err());
        assert!(Dns { is_ipv6: false, addresses: vec![] }.validate().is_err());
        assert!(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0; 65] }.validate().is_err());
        assert!(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0xd800] }.validate().is_err());
        assert!(RestService { service: RestServiceType::Redfish, access_mode: 3 }.validate().is_err());
        assert!(Uri { uri: "a\0b".into() }.validate().is_err());
        assert!(Uri { uri: "\u{e9}".into() }.validate().is_err());
        assert!(Uri { uri: "a".repeat(65532) }.validate().is_err());
        assert!(Bios { device_type: 0, status_flag: 0, description_str: "a\0b".into() }.validate().is_err());
        assert!(
            ExtendedAcpi {
                hid: 0,
                uid: 0,
                cid: 0,
                hid_string: "\u{e9}".into(),
                uid_string: "".into(),
                cid_string: "".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Iscsi { protocol: 1, options: 0, lun: [0; 8], portal_group: 0, target_name: "iqn".into() }
                .validate()
                .is_err()
        );
        assert!(NvmeOf { identifier_type: 0, identifier: [0; 16], subsystem_nqn: "nqn".into() }.validate().is_err());
        assert!(NvmeOf { identifier_type: 1, identifier: [1; 16], subsystem_nqn: "nqn".into() }.validate().is_err());
        assert!(FilePath::try_from_ctx(&[b'A', 0], scroll::LE).is_err());
        assert!(FilePath::try_from_ctx(&[b'A'], scroll::LE).is_err());
        assert!(Bios::try_from_ctx(&[0, 0, 0, 0, b'A'], scroll::LE).is_err());
        assert!(ExtendedAcpi::try_from_ctx(&[0; 14], scroll::LE).is_err());
        assert!(Dns::try_from_ctx(&[0; 16], scroll::LE).is_err());
        assert!(RestService::try_from_ctx(&[0xff, 1], scroll::LE).is_err());
        assert!(Uri::try_from_ctx(&[0xff], scroll::LE).is_err());
    }

    #[test]
    fn test_device_path_existing_validation() {
        assert!(Sata::new(u16::MAX, 0, 0).validate().is_err());
        for id in [0, u32::MAX] {
            assert!(NvmExpress::new(id, 0).validate().is_err());
        }
        assert!(FilePath::new("a\0b").validate().is_err());
        assert!(FilePath::new("\u{1f600}").validate().is_err());
        for hid in ["", "PNP", "1NP0A03", "PNPZZZZ", "\u{e9}NP0A03"] {
            assert!(Acpi::try_eisa_id(hid).is_err());
        }
        assert_eq!(Acpi::try_eisa_id("PNP0A03"), Ok(0x0a0341d0));
        assert_eq!(Acpi::try_eisa_id("pnp0a08"), Ok(0x0a0841d0));
    }

    #[test]
    fn test_device_path_builder_propagates_errors() {
        let mut path = DevicePathBuf::from_device_path_node_iter([EndEntire].into_iter()).unwrap();
        let expected = path.as_bytes().to_vec();
        assert!(path.append(Sata::new(u16::MAX, 0, 0)).is_err());
        assert_eq!(path.as_bytes(), expected);
        assert!(path.append(FilePath::new("bad\0path")).is_err());
        assert_eq!(path.as_bytes(), expected);
        assert!(DevicePathBuf::from_device_path_node_iter([Sata::new(u16::MAX, 0, 0)].into_iter()).is_err());
        assert!(DevicePathBuf::from_device_path_node_iter([EndEntire, EndEntire].into_iter()).is_err());
    }
}
