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
    Pread, Pwrite,
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
        // ACPI nodes.
        Acpi,
        // Messaging nodes.
        NvmExpress,
        Sata,
        // Media nodes.
        FilePath,
        HardDrive,
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
