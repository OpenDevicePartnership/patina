//! Spec-defined device path node types defined in this module.

use core::{
    fmt::{Display, Write},
    iter::Iterator,
};

use alloc::{
    boxed::Box,
    string::{String, ToString},
};

use scroll::{
    ctx::{TryFromCtx, TryIntoCtx},
    Pread, Pwrite,
};

use super::device_path_node::{DevicePathNode, UnknownDevicePathNode};

use crate::device_path_node;

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum DevicePathType {
    Hardware = 1,
    Acpi = 2,
    Messaging = 3,
    Media = 4,
    Bios = 5,
    End = 0x7F,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum HardwareSubType {
    Pci = 1,
    Pccard = 2,
    MemoryMapped = 3,
    Vendor = 4,
    Controller = 5,
    Bmc = 6,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum AcpiSubType {
    Acpi = 1,
    ExtendedAcpi = 2,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum MessagingSubType {
    Atapi = 1,
    Scsi = 2,
    FiberChannel = 3,
    FiberChannelEx = 21,
    _1394 = 4,
    Usb = 5,
    Sata = 18,
    UsbWwid = 16,
    DeviceLogicalUnit = 17,
    UsbClass = 15,
    I2oRandomBlockStorageClass = 6,
    MacAddress = 11,
    IpV4 = 12,
    IpV6 = 13,
    Vlan = 20,
    InfiniBand = 9,
    Uart = 14,
    Vendor = 10,
    SasEx = 22,
    Iscsi = 19,
    NvmExpress = 23,
    Uri = 24,
    Ufs = 25,
    Sd = 26,
    Bluetooth = 27,
    WiFi = 28,
    /// Embedded Multi-Media Card
    Emmc = 29,
    BluetoothLE = 30,
    Dns = 31,
    Nvdimm = 32,
    RestService = 33,
    /// MVMe over Fabric
    NvmeOf = 34,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum MediaSubType {
    HardDrive = 1,
    CdRom = 2,
    Vendor = 3,
    FilePath = 4,
    MediaProtocol = 5,
    PiwgFirmwareFile = 6,
    PiwgFirmwareVolume = 7,
    RelativeOffsetRange = 8,
    RamDisk = 9,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum BiosSubType {
    BiosBootSpecification = 1,
}

pub enum EndSubType {
    Entire = 0xFF,
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
        // Media nodes.
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
        // EFI memory type.
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
        // Controller Number.
        pub number: u32,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#controller-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Bmc {
        pub interface_type: u8,
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
    pub const PCI_ROOT_HID: u32 = Acpi::eisa_id("PNP0A03");
    pub const PCIE_ROOT_HID: u32 = Acpi::eisa_id("PNP0A08");

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
        pub device_type: u16,
        pub status_flag: u16,
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
