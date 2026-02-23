//! Spec-defined device path node types defined in this module.
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

use alloc::{
    boxed::Box,
    string::{String, ToString},
};

use scroll::{
    Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

use crate::{
    device_path::parse_node::{self, DevicePathNode, UnknownDevicePathNode},
    device_path_node,
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
    /// WiFi device path.
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
    /// NVMe over Fabric device path.
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

/// Function used to cast an unknown device path to a known one based on the type and sub type in the header.
pub fn cast_to_dyn_device_path_node(unknown: UnknownDevicePathNode<'_>) -> Box<dyn DevicePathNode + '_> {
    macro_rules! cast {
        ($unknown:expr, $($ty:ty),*) => {
            match unknown.header {
                $(
                    h if <$ty>::is_type(h.r#type, h.sub_type) => {
                        match unknown.data.pread_with::<$ty>(0, scroll::LE) {
                            Ok(n) => Some(Box::new(n) as Box<dyn DevicePathNode>),
                            Err(_) => {
                                debug_assert!(false);
                                None
                            }
                        }
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
        // Media nodes.
        FilePath,
        // BIOS nodes.
        Bios,
        // End nodes
        EndEntire,
        EndInstance
    ) {
        Some(n) => n,
        None => Box::new(unknown),
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
    /// PCIe Root Bridge HID (PNP0A08).
    pub const PCIE_ROOT_HID: u32 = Acpi::eisa_id("PNP0A08");

    /// Create a new PCI root bridge ACPI device path node.
    pub fn new_pci_root(uid: u32) -> Self {
        Self { hid: Acpi::PCI_ROOT_HID, uid }
    }

    /// Converts and compresses the 7-character text argument into its corresponding 4-byte numeric EISA ID encoding.
    /// <https://uefi.org/specs/ACPI/6.5_A/19_ASL_Reference.html#asl-macros>
    pub const fn eisa_id(hid: &str) -> u32 {
        let bytes = hid.as_bytes();

        let c1 = (bytes[0] - 0x40) & 0x1F;
        let c2 = (bytes[1] - 0x40) & 0x1F;
        let c3 = (bytes[2] - 0x40) & 0x1F;

        let h1 = (bytes[3] as char).to_digit(16).unwrap() as u8;
        let h2 = (bytes[4] as char).to_digit(16).unwrap() as u8;
        let h3 = (bytes[5] as char).to_digit(16).unwrap() as u8;
        let h4 = (bytes[6] as char).to_digit(16).unwrap() as u8;

        let byte_0 = (c1 << 2) | (c2 >> 3);
        let byte_1 = (c2 << 5) | c3;
        let byte_2 = (h1 << 4) | h2;
        let byte_3 = (h3 << 4) | h4;

        u32::from_le_bytes([byte_3, byte_2, byte_1, byte_0])
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

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bios-boot-specification-device-path>
    @[DevicePathNode(DevicePathType::Bios, BiosSubType::BiosBootSpecification)]
    @[DevicePathNodeDerive(Debug, Display)]
    pub struct Bios {
        /// BIOS device type.
        pub device_type: u16,
        /// Status flags.
        pub status_flag: u16,
        /// Description string.
        pub description_str: String,
    }
}

impl TryIntoCtx<scroll::Endian> for Bios {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let mut offset = 0;
        dest.gwrite_with(self.device_type, &mut offset, ctx)?;
        dest.gwrite_with(self.status_flag, &mut offset, ctx)?;
        dest.gwrite_with(self.description_str.as_bytes(), &mut offset, ())?;
        dest.gwrite_with(0, &mut offset, ctx)?; // End of string
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Bios {
    type Error = scroll::Error;

    fn try_from_ctx(buffer: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let device_type = buffer.gread_with::<u16>(&mut offset, ctx)?;
        let status_flag = buffer.gread_with::<u16>(&mut offset, ctx)?;
        let end_str_idx = &buffer[offset..]
            .iter()
            .position(|c| c == &0)
            .ok_or(scroll::Error::TooBig { size: buffer.len() + 1, len: buffer.len() })?;
        let description_str = String::from_utf8_lossy(&buffer[offset..offset + end_str_idx]).to_string();
        Ok((Self { device_type, status_flag, description_str }, offset))
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

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
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
        Ok((Self { hba_port, port_multiplier_port, lun }, offset))
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
    /// The on-wire size of NvmExpress data (without header): 4 bytes NSID + 8 bytes EUI64.
    const DATA_SIZE: usize = 4 + 8;

    /// Create a new NvmExpress device path node.
    ///
    /// # Arguments
    /// * `namespace_id` - The namespace identifier (typically 1 for the first namespace)
    /// * `eui64` - The IEEE Extended Unique Identifier (can be 0 if not specified)
    pub fn new(namespace_id: u32, eui64: u64) -> Self {
        Self { namespace_id, eui64 }
    }
}

impl parse_node::DevicePathNode for NvmExpress {
    fn header(&self) -> crate::device_path::parse_node::Header {
        parse_node::Header {
            r#type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::NvmExpress as u8,
            length: parse_node::Header::size_of_header() + Self::DATA_SIZE,
        }
    }

    fn is_type(r#type: u8, sub_type: u8) -> bool {
        r#type == DevicePathType::Messaging as u8 && sub_type == MessagingSubType::NvmExpress as u8
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
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
        Ok((Self { namespace_id, eui64 }, offset))
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
    /// The on-wire size of HardDrive data (without header).
    const DATA_SIZE: usize = 4 + 8 + 8 + 16 + 1 + 1; // 38 bytes

    /// Partition format: GPT
    pub const FORMAT_GPT: u8 = 0x02;
    /// Partition format: MBR
    pub const FORMAT_MBR: u8 = 0x01;

    /// Signature type: GUID
    pub const SIGNATURE_TYPE_GUID: u8 = 0x02;
    /// Signature type: MBR 32-bit signature
    pub const SIGNATURE_TYPE_MBR: u8 = 0x01;

    /// Create a new HardDrive device path node for a GPT partition.
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

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
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
        Ok((
            Self {
                partition_number,
                partition_start,
                partition_size,
                partition_signature,
                partition_format,
                signature_type,
            },
            offset,
        ))
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
    /// Create a new FilePath device path node from a path string.
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

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        let header = self.header();
        let mut offset = 0;
        buffer.gwrite_with(header, &mut offset, scroll::Endian::Little)?;
        // Write the path as UTF-16LE (UCS-2)
        for c in self.path.chars() {
            let code_point = c as u16;
            buffer.gwrite_with(code_point, &mut offset, scroll::LE)?;
        }
        // Write null terminator (UTF-16)
        buffer.gwrite_with(0u16, &mut offset, scroll::LE)?;
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
        let mut offset = 0;
        // Write the path as UTF-16LE (UCS-2)
        for c in self.path.chars() {
            let code_point = c as u16;
            dest.gwrite_with(code_point, &mut offset, scroll::LE)?;
        }
        // Write null terminator (UTF-16)
        dest.gwrite_with(0u16, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for FilePath {
    type Error = scroll::Error;

    fn try_from_ctx(buffer: &[u8], _ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let mut path = String::new();

        // Read UTF-16LE characters until null terminator
        loop {
            if offset + 2 > buffer.len() {
                break;
            }
            let code_point: u16 = buffer.gread_with(&mut offset, scroll::LE)?;
            if code_point == 0 {
                break;
            }
            // Convert UTF-16 code point to char
            if let Some(c) = char::from_u32(code_point as u32) {
                path.push(c);
            }
        }

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
        let display = std::format!("{}", sata);
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
        let display = std::format!("{}", nvme);
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
        let display = std::format!("{}", hd);
        assert!(display.contains("HD"));
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
        let display = std::format!("{}", fp);
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
