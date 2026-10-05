//! Device path text representation (UEFI 2.10 Section 10.6).
//!
//! This module implements the conversion side of the
//! `EFI_DEVICE_PATH_TO_TEXT_PROTOCOL`. Standalone end nodes display as their
//! conventional separators (`EndInstance` as `,`, `EndEntire` as an empty
//! string); at path level those are handled as an instance separator and final
//! terminator.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// cspell:ignore sassata nguid
use super::{
    DevicePath,
    node::{
        Acpi, AcpiAdr, Atapi, Bios, Bluetooth, BluetoothLE, Bmc, CdRom, Controller, DeviceLogicalUnit, DevicePathNode,
        Dns, Emmc, EndEntire, EndInstance, ExtendedAcpi, FiberChannel, FiberChannelEx, FilePath, GenericDevicePathNode,
        HardDrive, HardwareVendor, I2oRandomBlockStorageClass, Ieee1394, InfiniBand, IpV4, IpV6, Iscsi, MacAddress,
        MediaProtocol, MediaVendor, MemoryMapped, MessagingVendor, Nvdimm, NvdimmAcpi, NvmExpress, NvmeOf, PcCard, Pci,
        PiwgFirmwareFile, PiwgFirmwareVolume, RamDisk, RelativeOffsetRange, RestService, RestServiceType, Sas, SasEx,
        Sata, Scsi, Sd, Uart, UartFlowControl, Ufs, Uri, Usb, UsbClass, UsbWwid, Vlan, WiFi,
    },
};
use crate::BinaryGuid;
use alloc::{format, string::String, string::ToString};
use core::fmt;

/// Options controlling UEFI device-path-to-text conversion.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TextOptions {
    /// Use display-only abbreviations that are not guaranteed to be parseable.
    pub display_only: bool,
    /// Use shortcut node names when a GUID, EISA ID, or class code identifies one.
    pub allow_shortcuts: bool,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            // Match the common ConvertDevicePathToText(..., FALSE, TRUE) usage: retain
            // the parseable form while still using standardized shortcut spellings.
            display_only: false,
            allow_shortcuts: true,
        }
    }
}

/// Object-safe conversion of one device path node to UEFI text.
pub trait DevicePathText {
    /// Format this node using UEFI Section 10.6 text rules.
    fn fmt_text(&self, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result;
}

/// Format a borrowed device path into UEFI text.
pub fn fmt_device_path(path: &DevicePath, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    let mut need_separator = false;
    for generic in path.iter() {
        if EndEntire::is_type(generic.header.r#type, generic.header.sub_type) {
            break;
        }
        if EndInstance::is_type(generic.header.r#type, generic.header.sub_type) {
            f.write_str(",")?;
            need_separator = false;
            continue;
        }
        if need_separator {
            f.write_str("/")?;
        }
        fmt_generic_node(generic, f, options)?;
        need_separator = true;
    }
    Ok(())
}

impl DevicePath {
    /// Convert this device path to UEFI text with the supplied options.
    ///
    /// [UEFI Spec Documentation: 10.6.4. EFI_DEVICE_PATH_TO_TEXT_PROTOCOL.ConvertDevicePathToText()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-to-text-protocol-convertdevicepathtotext)
    #[must_use]
    pub fn to_text(&self, options: TextOptions) -> String {
        struct TextDisplay<'a> {
            path: &'a DevicePath,
            options: TextOptions,
        }
        impl fmt::Display for TextDisplay<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt_device_path(self.path, f, self.options)
            }
        }
        TextDisplay { path: self, options }.to_string()
    }
}

/// Convert a device path to UEFI text with the default [`TextOptions`].
///
/// [UEFI Spec Documentation: 10.6.4. EFI_DEVICE_PATH_TO_TEXT_PROTOCOL.ConvertDevicePathToText()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-to-text-protocol-convertdevicepathtotext)
impl From<&DevicePath> for String {
    fn from(path: &DevicePath) -> Self {
        path.to_text(TextOptions::default())
    }
}

macro_rules! impl_text {
    ($ty:ty, $func:ident) => {
        impl DevicePathText for $ty {
            fn fmt_text(&self, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
                $func(self, f, options)
            }
        }
    };
}

macro_rules! impl_display_text {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl fmt::Display for $ty {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    self.fmt_text(f, TextOptions::default())
                }
            }
        )+
    };
}

macro_rules! try_fmt_node {
    ($generic:expr, $f:expr, $options:expr, $($ty:ty),+ $(,)?) => {
        $(
            if let Ok(node) = <$ty>::try_from($generic) {
                return node.fmt_text($f, $options);
            }
        )+
    };
}

fn fmt_generic_node(node: &GenericDevicePathNode, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    try_fmt_node!(
        node,
        f,
        options,
        Pci,
        PcCard,
        MemoryMapped,
        HardwareVendor,
        Controller,
        Bmc,
        Acpi,
        ExtendedAcpi,
        AcpiAdr,
        NvdimmAcpi,
        Atapi,
        Scsi,
        FiberChannel,
        FiberChannelEx,
        Ieee1394,
        Usb,
        I2oRandomBlockStorageClass,
        MacAddress,
        IpV4,
        IpV6,
        Vlan,
        InfiniBand,
        Uart,
        UartFlowControl,
        Sas,
        MessagingVendor,
        UsbWwid,
        UsbClass,
        DeviceLogicalUnit,
        Sata,
        Iscsi,
        SasEx,
        NvmExpress,
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
        MediaVendor,
        CdRom,
        MediaProtocol,
        PiwgFirmwareFile,
        PiwgFirmwareVolume,
        RelativeOffsetRange,
        RamDisk,
        HardDrive,
        FilePath,
        Bios,
        EndInstance,
        EndEntire,
    );
    node.fmt_text(f, options)
}

fn write_hex_bytes(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

fn write_dash_hex_bytes(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for (index, byte) in bytes.iter().enumerate() {
        if index != 0 {
            f.write_str("-")?;
        }
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

fn write_hex_u8_data(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(f, ",0x{byte:x}")?;
    }
    Ok(())
}

fn write_guid(f: &mut fmt::Formatter<'_>, guid: &BinaryGuid) -> fmt::Result {
    write!(f, "{guid}")
}

fn write_eisa_id(f: &mut fmt::Formatter<'_>, value: u32) -> fmt::Result {
    if value == 0 {
        return f.write_str("0");
    }
    let vendor = value & 0x7fff;
    let product = (value >> 16) & 0xffff;
    let a = char::from_u32(((vendor >> 10) & 0x1f) + u32::from(b'A') - 1).unwrap_or('?');
    let b = char::from_u32(((vendor >> 5) & 0x1f) + u32::from(b'A') - 1).unwrap_or('?');
    let c = char::from_u32((vendor & 0x1f) + u32::from(b'A') - 1).unwrap_or('?');
    write!(f, "{a}{b}{c}{product:04X}")
}

fn eisa_string(value: u32) -> String {
    struct Eisa(u32);
    impl fmt::Display for Eisa {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_eisa_id(f, self.0)
        }
    }
    Eisa(value).to_string()
}

fn write_vendor(f: &mut fmt::Formatter<'_>, name: &str, guid: &BinaryGuid, data: &[u8]) -> fmt::Result {
    write!(f, "{name}(")?;
    write_guid(f, guid)?;
    if !data.is_empty() {
        f.write_str(",")?;
        write_hex_bytes(f, data)?;
    }
    f.write_str(")")
}

fn write_generic(f: &mut fmt::Formatter<'_>, node: &GenericDevicePathNode) -> fmt::Result {
    let name = match node.header.r#type {
        1 => "HardwarePath",
        2 => "AcpiPath",
        3 => "Msg",
        4 => "MediaPath",
        5 => "BbsPath",
        _ => "Path",
    };
    if name == "Path" {
        write!(f, "Path({},{},", node.header.r#type, node.header.sub_type)?;
    } else {
        write!(f, "{name}({},", node.header.sub_type)?;
    }
    write_hex_bytes(f, &node.data)?;
    f.write_str(")")
}

impl DevicePathText for &GenericDevicePathNode {
    fn fmt_text(&self, f: &mut fmt::Formatter<'_>, _options: TextOptions) -> fmt::Result {
        write_generic(f, self)
    }
}

fn pci_text(node: &Pci, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Pci(0x{:x},0x{:x})", node.device, node.function)
}
impl_text!(Pci, pci_text);

fn pc_card_text(node: &PcCard, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "PcCard(0x{:x})", node.function_number)
}
impl_text!(PcCard, pc_card_text);

fn memory_mapped_text(node: &MemoryMapped, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "MemoryMapped(0x{:x},0x{:x},0x{:x})", node.memory_type, node.start_address, node.end_address)
}
impl_text!(MemoryMapped, memory_mapped_text);

fn hardware_vendor_text(node: &HardwareVendor, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write_vendor(f, "VenHw", &node.guid, &node.data)
}
impl_text!(HardwareVendor, hardware_vendor_text);

fn controller_text(node: &Controller, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Ctrl(0x{:x})", node.number)
}
impl_text!(Controller, controller_text);

fn bmc_text(node: &Bmc, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "BMC(0x{:x},0x{:x})", node.interface_type, node.base_address)
}
impl_text!(Bmc, bmc_text);

const PNP0604: u32 = Acpi::eisa_id("PNP0604");
const PNP0301: u32 = Acpi::eisa_id("PNP0301");
const PNP0501: u32 = Acpi::eisa_id("PNP0501");
const PNP0401: u32 = Acpi::eisa_id("PNP0401");

fn acpi_shortcut(hid: u32) -> Option<&'static str> {
    match hid {
        Acpi::PCI_ROOT_HID => Some("PciRoot"),
        Acpi::PCIE_ROOT_HID => Some("PcieRoot"),
        PNP0604 => Some("Floppy"),
        PNP0301 => Some("Keyboard"),
        PNP0501 => Some("Serial"),
        PNP0401 => Some("ParallelPort"),
        _ => None,
    }
}

fn acpi_text(node: &Acpi, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.allow_shortcuts
        && let Some(name) = acpi_shortcut(node.hid)
    {
        return write!(f, "{name}(0x{:x})", node.uid);
    }
    write!(f, "Acpi(")?;
    write_eisa_id(f, node.hid)?;
    write!(f, ",0x{:x})", node.uid)
}
impl_text!(Acpi, acpi_text);

fn extended_acpi_text(node: &ExtendedAcpi, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.display_only {
        if node.hid == Acpi::PCI_ROOT_HID || (node.cid == Acpi::PCI_ROOT_HID && node.hid != Acpi::PCIE_ROOT_HID) {
            return write_root_shortcut(f, "PciRoot", node);
        }
        if node.hid == Acpi::PCIE_ROOT_HID || node.cid == Acpi::PCIE_ROOT_HID {
            return write_root_shortcut(f, "PcieRoot", node);
        }
    }

    let hid = eisa_string(node.hid);
    let cid = eisa_string(node.cid);
    if node.hid_string.is_empty() && node.cid_string.is_empty() && !node.uid_string.is_empty() {
        if node.cid == 0 {
            write!(f, "AcpiExp({hid},0,{})", node.uid_string)
        } else {
            write!(f, "AcpiExp({hid},{cid},{})", node.uid_string)
        }
    } else if options.display_only {
        write!(
            f,
            "AcpiEx({},{},{})",
            if node.hid_string.is_empty() { hid.as_str() } else { &node.hid_string },
            if node.cid_string.is_empty() { cid.as_str() } else { &node.cid_string },
            if node.uid_string.is_empty() {
                return write!(
                    f,
                    "AcpiEx({},{},0x{:x})",
                    if node.hid_string.is_empty() { hid.as_str() } else { &node.hid_string },
                    if node.cid_string.is_empty() { cid.as_str() } else { &node.cid_string },
                    node.uid
                );
            } else {
                &node.uid_string
            },
        )
    } else {
        write!(f, "AcpiEx({hid},{cid},0x{:x},{},{},{})", node.uid, node.hid_string, node.cid_string, node.uid_string)
    }
}

fn write_root_shortcut(f: &mut fmt::Formatter<'_>, name: &str, node: &ExtendedAcpi) -> fmt::Result {
    if node.uid_string.is_empty() {
        write!(f, "{name}(0x{:x})", node.uid)
    } else {
        write!(f, "{name}({})", node.uid_string)
    }
}
impl_text!(ExtendedAcpi, extended_acpi_text);

fn acpi_adr_text(node: &AcpiAdr, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str("AcpiAdr(")?;
    for (index, address) in node.addresses.iter().enumerate() {
        if index != 0 {
            f.write_str(",")?;
        }
        write!(f, "0x{address:x}")?;
    }
    f.write_str(")")
}
impl_text!(AcpiAdr, acpi_adr_text);

fn nvdimm_acpi_text(node: &NvdimmAcpi, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "NvdimmAcpiAdr(0x{:x})", node.nfit_device_handle)
}
impl_text!(NvdimmAcpi, nvdimm_acpi_text);

fn atapi_text(node: &Atapi, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.display_only {
        write!(f, "Ata(0x{:x})", node.lun)
    } else {
        write!(
            f,
            "Ata({},{},0x{:x})",
            if node.primary_secondary == 1 { "Secondary" } else { "Primary" },
            if node.slave_master == 1 { "Slave" } else { "Master" },
            node.lun
        )
    }
}
impl_text!(Atapi, atapi_text);

fn scsi_text(node: &Scsi, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Scsi(0x{:x},0x{:x})", node.pun, node.lun)
}
impl_text!(Scsi, scsi_text);
fn fiber_channel_text(node: &FiberChannel, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Fibre(0x{:x},0x{:x})", node.wwn, node.lun)
}
impl_text!(FiberChannel, fiber_channel_text);
fn fiber_channel_ex_text(node: &FiberChannelEx, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str("FibreEx(0x")?;
    write_hex_bytes(f, &node.wwn)?;
    f.write_str(",0x")?;
    write_hex_bytes(f, &node.lun)?;
    f.write_str(")")
}
impl_text!(FiberChannelEx, fiber_channel_ex_text);
fn ieee1394_text(node: &Ieee1394, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    // The 1394 GUID is an 8-byte integer (UEFI 2.10 Table 10.18), so it is written as a hexadecimal
    // integer (Table 10.66) that parses back to the same value.
    write!(f, "I1394(0x{:016x})", node.guid)
}
impl_text!(Ieee1394, ieee1394_text);
fn usb_text(node: &Usb, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "USB(0x{:x},0x{:x})", node.parent_port_number, node.interface_number)
}
impl_text!(Usb, usb_text);
fn i2o_text(node: &I2oRandomBlockStorageClass, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "I2O(0x{:x})", node.tid)
}
impl_text!(I2oRandomBlockStorageClass, i2o_text);

fn mac_text(node: &MacAddress, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    let length = if matches!(node.if_type, 0 | 1) { 6 } else { node.mac_address.addr.len() };
    f.write_str("MAC(")?;
    write_hex_bytes(f, node.mac_address.addr.get(..length).ok_or(fmt::Error)?)?;
    write!(f, ",0x{:x})", node.if_type)
}
impl_text!(MacAddress, mac_text);

fn protocol_name(protocol: u16) -> ProtocolDisplay {
    ProtocolDisplay(protocol)
}
struct ProtocolDisplay(u16);
impl fmt::Display for ProtocolDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            6 => f.write_str("TCP"),
            17 => f.write_str("UDP"),
            value => write!(f, "0x{value:x}"),
        }
    }
}

fn ip_v4_text(node: &IpV4, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    write!(f, "IPv4({}", Ipv4Endpoint(node.remote_ip, node.remote_port))?;
    if options.display_only {
        return f.write_str(")");
    }
    write!(f, ",{},{}", protocol_name(node.protocol), if node.static_ip { "Static" } else { "DHCP" })?;
    write!(f, ",{}", Ipv4Endpoint(node.local_ip, node.local_port))?;
    if let Some(network) = &node.network {
        write!(f, ",{},{}", network.gateway, network.subnet_mask)?;
    }
    f.write_str(")")
}
impl_text!(IpV4, ip_v4_text);

/// An IPv4 address followed by `:port` when the port is not zero (UEFI 2.10 Table 10.66).
struct Ipv4Endpoint(core::net::Ipv4Addr, u16);

impl fmt::Display for Ipv4Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.1 {
            0 => write!(f, "{}", self.0),
            port => write!(f, "{}:{port}", self.0),
        }
    }
}

/// An IPv6 address, written `[address]:port` when the port is not zero (UEFI 2.10 Table 10.66).
struct Ipv6Endpoint(core::net::Ipv6Addr, u16);

impl fmt::Display for Ipv6Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.1 {
            0 => write!(f, "{}", self.0),
            port => write!(f, "[{}]:{port}", self.0),
        }
    }
}

fn ip_v6_text(node: &IpV6, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    write!(f, "IPv6({}", Ipv6Endpoint(node.remote_ip_address, node.remote_port))?;
    if options.display_only {
        return f.write_str(")");
    }
    let origin = match node.ip_address_origin {
        0 => "Static",
        1 => "StatelessAutoConfigure",
        _ => "StatefulAutoConfigure",
    };
    write!(
        f,
        ",{},{},{},0x{:x},{}",
        protocol_name(node.protocol),
        origin,
        Ipv6Endpoint(node.local_ip_address, node.local_port),
        node.prefix_length,
        node.gateway_ip_address
    )?;
    f.write_str(")")
}
impl_text!(IpV6, ip_v6_text);

fn vlan_text(node: &Vlan, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Vlan({})", node.vlan_id)
}
impl_text!(Vlan, vlan_text);
fn infiniband_text(node: &InfiniBand, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(
        f,
        "Infiniband(0x{:x},{},0x{:x},0x{:x},0x{:x})",
        node.resource_flags,
        BinaryGuid::from_bytes(&node.port_gid),
        node.service_id,
        node.target_port_id,
        node.device_id
    )
}
impl_text!(InfiniBand, infiniband_text);

fn uart_text(node: &Uart, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    let baud = if node.baud_rate == 0 { "DEFAULT".to_string() } else { node.baud_rate.to_string() };
    let data = if node.data_bits == 0 { "DEFAULT".to_string() } else { node.data_bits.to_string() };
    let parity = match node.parity {
        0 => "D",
        1 => "N",
        2 => "E",
        3 => "O",
        4 => "M",
        5 => "S",
        _ => "x",
    };
    let stop = match node.stop_bits {
        0 => "D",
        1 => "1",
        2 => "1.5",
        3 => "2",
        _ => "x",
    };
    write!(f, "Uart({baud},{data},{parity},{stop})")
}
impl_text!(Uart, uart_text);

/// Write the `SASSATA`, `Location`, `Connect`, and `DriveBay` arguments of a SAS node, then
/// the optional `reserved` argument.
///
/// Following UEFI 2.10 Table 10.67, `Location`, `Connect`, and `DriveBay` are left out when
/// the topology is `NoTopology` or an integer, and `DriveBay` is left out when the topology
/// does not include it. Arguments that are left out before a written one stay as empty
/// placeholders, and trailing ones are dropped.
fn topology_text(f: &mut fmt::Formatter<'_>, topology: u16, reserved: Option<u32>) -> fmt::Result {
    let information = topology & 0x0f;
    let keywords = topology & 0x80 == 0 && (information == 2 || (information == 1 && topology >> 8 == 0));
    let mut arguments: [String; 5] = Default::default();
    if topology == 0 {
        arguments[0] = "NoTopology".to_string();
    } else if keywords {
        arguments[0] = (if topology & 0x10 != 0 { "SATA" } else { "SAS" }).to_string();
        arguments[1] = (if topology & 0x20 != 0 { "External" } else { "Internal" }).to_string();
        arguments[2] = (if topology & 0x40 != 0 { "Expanded" } else { "Direct" }).to_string();
        if information == 2 {
            arguments[3] = format!("0x{:x}", (topology >> 8) + 1);
        }
    } else {
        arguments[0] = format!("0x{topology:x}");
    }
    if let Some(reserved) = reserved.filter(|reserved| *reserved != 0) {
        arguments[4] = format!("0x{reserved:x}");
    }
    let count = arguments.iter().rposition(|argument| !argument.is_empty()).map_or(0, |last| last + 1);
    f.write_str(&arguments.get(..count).unwrap_or_default().join(","))
}

fn uart_flow_control_text(node: &UartFlowControl, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.allow_shortcuts {
        let value = match node.flow_control_map {
            0 => "None",
            1 => "Hardware",
            2 => "XonXoff",
            other => return write!(f, "UartFlowCtrl(0x{other:x})"),
        };
        write!(f, "UartFlowCtrl({value})")
    } else {
        write!(f, "VenMsg({},", UartFlowControl::GUID)?;
        write_hex_bytes(f, &node.flow_control_map.to_le_bytes())?;
        f.write_str(")")
    }
}
impl_text!(UartFlowControl, uart_flow_control_text);

fn sas_text(node: &Sas, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.allow_shortcuts {
        write!(f, "SAS(0x{:x},0x{:x},0x{:x},", node.sas_address, node.lun, node.relative_target_port)?;
        topology_text(f, node.device_topology, Some(node.reserved))?;
        f.write_str(")")
    } else {
        write!(f, "VenMsg({},", Sas::GUID)?;
        write_hex_bytes(f, &node.reserved.to_le_bytes())?;
        write_hex_bytes(f, &node.sas_address.to_le_bytes())?;
        write_hex_bytes(f, &node.lun.to_le_bytes())?;
        write_hex_bytes(f, &node.device_topology.to_le_bytes())?;
        write_hex_bytes(f, &node.relative_target_port.to_le_bytes())?;
        f.write_str(")")
    }
}
impl_text!(Sas, sas_text);

fn messaging_vendor_text(node: &MessagingVendor, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.allow_shortcuts && node.data.is_empty() {
        if node.guid == BinaryGuid::from_string("E0C14753-F9BE-11D2-9A0C-0090273FC14D") {
            return f.write_str("VenPcAnsi()");
        }
        if node.guid == BinaryGuid::from_string("DFA66065-B419-11D3-9A2D-0090273FC14D") {
            return f.write_str("VenVt100()");
        }
        if node.guid == BinaryGuid::from_string("7BAEC70B-57E0-4C76-8E87-2F9E28088343") {
            return f.write_str("VenVt100Plus()");
        }
        if node.guid == BinaryGuid::from_string("AD15A0D6-8BEC-4ACF-A073-D01DE77E2D88") {
            return f.write_str("VenUtf8()");
        }
        if node.guid == BinaryGuid::from_string("EBA4E8D2-3858-41EC-A281-2647BA9660D0") {
            return f.write_str("DebugPort()");
        }
    }
    write_vendor(f, "VenMsg", &node.guid, &node.data)
}
impl_text!(MessagingVendor, messaging_vendor_text);

fn usb_wwid_text(node: &UsbWwid, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    let serial = String::from_utf16_lossy(&node.serial_number);
    write!(f, "UsbWwid(0x{:x},0x{:x},0x{:x},\"{}\")", node.vendor_id, node.product_id, node.interface, serial)
}
impl_text!(UsbWwid, usb_wwid_text);

fn usb_class_shortcut(node: &UsbClass) -> Option<(&'static str, bool)> {
    let name = match (node.device_class, node.device_subclass) {
        (1, _) => ("UsbAudio", false),
        (2, _) => ("UsbCDCControl", false),
        (3, _) => ("UsbHID", false),
        (6, _) => ("UsbImage", false),
        (7, _) => ("UsbPrinter", false),
        (8, _) => ("UsbMassStorage", false),
        (9, _) => ("UsbHub", false),
        (10, _) => ("UsbCDCData", false),
        (11, _) => ("UsbSmartCard", false),
        (14, _) => ("UsbVideo", false),
        (220, _) => ("UsbDiagnostic", false),
        (224, _) => ("UsbWireless", false),
        (254, 1) => ("UsbDeviceFirmwareUpdate", true),
        (254, 2) => ("UsbIrdaBridge", true),
        (254, 3) => ("UsbTestAndMeasurement", true),
        _ => return None,
    };
    Some(name)
}

fn usb_class_text(node: &UsbClass, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.allow_shortcuts
        && let Some((name, fixed_subclass)) = usb_class_shortcut(node)
    {
        if fixed_subclass {
            return write!(f, "{name}(0x{:x},0x{:x},0x{:x})", node.vendor_id, node.product_id, node.device_protocol);
        }
        return write!(
            f,
            "{name}(0x{:x},0x{:x},0x{:x},0x{:x})",
            node.vendor_id, node.product_id, node.device_subclass, node.device_protocol
        );
    }
    write!(
        f,
        "UsbClass(0x{:x},0x{:x},0x{:x},0x{:x},0x{:x})",
        node.vendor_id, node.product_id, node.device_class, node.device_subclass, node.device_protocol
    )
}
impl_text!(UsbClass, usb_class_text);

fn unit_text(node: &DeviceLogicalUnit, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Unit(0x{:x})", node.lun)
}
impl_text!(DeviceLogicalUnit, unit_text);
fn sata_text(node: &Sata, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Sata(0x{:x},0x{:x},0x{:x})", node.hba_port, node.port_multiplier_port, node.lun)
}
impl_text!(Sata, sata_text);

fn iscsi_text(node: &Iscsi, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    let header = if node.options & (1 << 1) != 0 { "CRC32C" } else { "None" };
    let data = if node.options & (1 << 3) != 0 { "CRC32C" } else { "None" };
    let auth = if node.options & (1 << 11) != 0 {
        "None"
    } else if node.options & (1 << 12) != 0 {
        "CHAP_UNI"
    } else {
        "CHAP_BI"
    };
    write!(f, "iSCSI({},0x{:x},0x", node.target_name, node.portal_group)?;
    write_hex_bytes(f, &node.lun)?;
    write!(f, ",{header},{data},{auth},TCP)")
}
impl_text!(Iscsi, iscsi_text);

fn sas_ex_text(node: &SasEx, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str("SasEx(0x")?;
    write_hex_bytes(f, &node.sas_address)?;
    f.write_str(",0x")?;
    write_hex_bytes(f, &node.lun)?;
    write!(f, ",0x{:x},", node.relative_target_port)?;
    topology_text(f, node.device_topology, None)?;
    f.write_str(")")
}
impl_text!(SasEx, sas_ex_text);

fn nvm_express_text(node: &NvmExpress, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "NVMe(0x{:x},", node.namespace_id)?;
    write_dash_hex_bytes(f, &node.eui64.to_be_bytes())?;
    f.write_str(")")
}
impl_text!(NvmExpress, nvm_express_text);
fn uri_text(node: &Uri, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Uri({})", node.uri)
}
impl_text!(Uri, uri_text);
fn ufs_text(node: &Ufs, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "UFS(0x{:x},0x{:x})", node.pun, node.lun)
}
impl_text!(Ufs, ufs_text);
fn sd_text(node: &Sd, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "SD(0x{:x})", node.slot_number)
}
impl_text!(Sd, sd_text);
fn bluetooth_text(node: &Bluetooth, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str("Bluetooth(")?;
    write_hex_bytes(f, &node.address)?;
    f.write_str(")")
}
impl_text!(Bluetooth, bluetooth_text);
fn wifi_text(node: &WiFi, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    let len = node.ssid.iter().position(|b| *b == 0).unwrap_or(node.ssid.len());
    write!(f, "Wi-Fi({})", String::from_utf8_lossy(node.ssid.get(..len).ok_or(fmt::Error)?))
}
impl_text!(WiFi, wifi_text);
fn emmc_text(node: &Emmc, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "eMMC(0x{:x})", node.slot_number)
}
impl_text!(Emmc, emmc_text);
fn bluetooth_le_text(node: &BluetoothLE, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str("BluetoothLE(")?;
    write_hex_bytes(f, &node.address)?;
    write!(f, ",0x{:02x})", node.address_type)
}
impl_text!(BluetoothLE, bluetooth_le_text);
fn dns_text(node: &Dns, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str("Dns(")?;
    for (index, addr) in node.addresses.iter().enumerate() {
        if index != 0 {
            f.write_str(",")?;
        }
        write!(f, "{addr}")?;
    }
    f.write_str(")")
}
impl_text!(Dns, dns_text);
fn nvdimm_text(node: &Nvdimm, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "NVDIMM({})", node.uuid)
}
impl_text!(Nvdimm, nvdimm_text);

fn rest_service_text(node: &RestService, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    match &node.service {
        RestServiceType::Redfish => write!(f, "RestService(0x1,0x{:x})", node.access_mode),
        RestServiceType::OData => write!(f, "RestService(0x2,0x{:x})", node.access_mode),
        RestServiceType::Vendor { guid, data } => {
            write!(f, "RestService(0xff,0x{:x},{}", node.access_mode, guid)?;
            write_hex_u8_data(f, data)?;
            f.write_str(")")
        }
    }
}
impl_text!(RestService, rest_service_text);

fn nvme_of_text(node: &NvmeOf, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "NVMEoF({},", node.subsystem_nqn)?;
    match node.identifier_type {
        1 => {
            f.write_str("eui.")?;
            write_hex_bytes(f, &node.identifier[..8])?;
        }
        2 => {
            f.write_str("nguid.")?;
            write_hex_bytes(f, &node.identifier)?;
        }
        3 => {
            f.write_str("urn:uuid:")?;
            write_uuid_bytes(f, &node.identifier)?;
        }
        other => write!(f, "unknown{other}.")?,
    }
    f.write_str(")")
}
impl_text!(NvmeOf, nvme_of_text);

fn write_uuid_bytes(f: &mut fmt::Formatter<'_>, b: &[u8; 16]) -> fmt::Result {
    write!(
        f,
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]
    )?;
    write_hex_bytes(f, &b[10..])
}

fn media_vendor_text(node: &MediaVendor, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write_vendor(f, "VenMedia", &node.guid, &node.data)
}
impl_text!(MediaVendor, media_vendor_text);
fn cdrom_text(node: &CdRom, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    if options.display_only {
        write!(f, "CDROM(0x{:x})", node.boot_entry)
    } else {
        write!(f, "CDROM(0x{:x},0x{:x},0x{:x})", node.boot_entry, node.partition_start, node.partition_size)
    }
}
impl_text!(CdRom, cdrom_text);
fn media_protocol_text(node: &MediaProtocol, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Media({})", node.protocol)
}
impl_text!(MediaProtocol, media_protocol_text);
fn piwg_file_text(node: &PiwgFirmwareFile, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "FvFile({})", node.fv_file_name)
}
impl_text!(PiwgFirmwareFile, piwg_file_text);
fn piwg_volume_text(node: &PiwgFirmwareVolume, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Fv({})", node.fv_name)
}
impl_text!(PiwgFirmwareVolume, piwg_volume_text);
fn offset_text(node: &RelativeOffsetRange, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    write!(f, "Offset(0x{:x},0x{:x})", node.starting_offset, node.ending_offset)
}
impl_text!(RelativeOffsetRange, offset_text);

const VIRTUAL_DISK_GUID: BinaryGuid = BinaryGuid::from_string("77AB535A-45FC-624B-5560-F7B281D1F96E");
const VIRTUAL_CD_GUID: BinaryGuid = BinaryGuid::from_string("3D5ABD30-4175-87CE-6D64-D2ADE523C4BB");
const PERSISTENT_VIRTUAL_DISK_GUID: BinaryGuid = BinaryGuid::from_string("5CEA02C9-4D07-69D3-269F-4496FBE096F9");
const PERSISTENT_VIRTUAL_CD_GUID: BinaryGuid = BinaryGuid::from_string("08018188-42CD-BB48-100F-5387D53DED3D");

fn ram_disk_text(node: &RamDisk, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    let name = if options.allow_shortcuts {
        if node.type_guid == VIRTUAL_DISK_GUID {
            Some("VirtualDisk")
        } else if node.type_guid == VIRTUAL_CD_GUID {
            Some("VirtualCD")
        } else if node.type_guid == PERSISTENT_VIRTUAL_DISK_GUID {
            Some("PersistentVirtualDisk")
        } else if node.type_guid == PERSISTENT_VIRTUAL_CD_GUID {
            Some("PersistentVirtualCD")
        } else {
            None
        }
    } else {
        None
    };
    if let Some(name) = name {
        write!(f, "{name}(0x{:x},0x{:x},{})", node.starting_address, node.ending_address, node.instance)
    } else {
        write!(
            f,
            "RamDisk(0x{:x},0x{:x},{},{})",
            node.starting_address, node.ending_address, node.instance, node.type_guid
        )
    }
}
impl_text!(RamDisk, ram_disk_text);

fn hard_drive_text(node: &HardDrive, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    write!(f, "HD({},", node.partition_number)?;
    match node.signature_type {
        HardDrive::SIGNATURE_TYPE_MBR => write!(
            f,
            "MBR,0x{:08x}",
            u32::from_le_bytes(node.partition_signature[..4].try_into().map_err(|_| fmt::Error)?)
        )?,
        HardDrive::SIGNATURE_TYPE_GUID => {
            f.write_str("GPT,")?;
            write_guid(f, &BinaryGuid::from_bytes(&node.partition_signature))?;
        }
        other => write!(f, "{other},0")?,
    }
    if !options.display_only {
        write!(f, ",0x{:x},0x{:x}", node.partition_start, node.partition_size)?;
    }
    f.write_str(")")
}
impl_text!(HardDrive, hard_drive_text);

fn file_path_text(node: &FilePath, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str(&node.path)
}
impl_text!(FilePath, file_path_text);

fn bios_text(node: &Bios, f: &mut fmt::Formatter<'_>, options: TextOptions) -> fmt::Result {
    let ty = match node.device_type {
        1 => "Floppy".to_string(),
        2 => "HD".to_string(),
        3 => "CDROM".to_string(),
        4 => "PCMCIA".to_string(),
        5 => "USB".to_string(),
        6 => "Network".to_string(),
        other => format!("0x{other:x}"),
    };
    write!(f, "BBS({ty},{}", node.description_str)?;
    if !options.display_only {
        write!(f, ",0x{:x}", node.status_flag)?;
    }
    f.write_str(")")
}
impl_text!(Bios, bios_text);

fn end_entire_text(_: &EndEntire, _: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    Ok(())
}
impl_text!(EndEntire, end_entire_text);
fn end_instance_text(_: &EndInstance, f: &mut fmt::Formatter<'_>, _: TextOptions) -> fmt::Result {
    f.write_str(",")
}
impl_text!(EndInstance, end_instance_text);

impl_display_text!(
    Pci,
    PcCard,
    MemoryMapped,
    HardwareVendor,
    Controller,
    Bmc,
    ExtendedAcpi,
    AcpiAdr,
    NvdimmAcpi,
    Atapi,
    Scsi,
    FiberChannel,
    FiberChannelEx,
    Ieee1394,
    Usb,
    I2oRandomBlockStorageClass,
    MacAddress,
    IpV4,
    IpV6,
    Vlan,
    InfiniBand,
    Uart,
    UartFlowControl,
    Sas,
    MessagingVendor,
    UsbWwid,
    UsbClass,
    DeviceLogicalUnit,
    Sata,
    Iscsi,
    SasEx,
    NvmExpress,
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
    MediaVendor,
    CdRom,
    MediaProtocol,
    PiwgFirmwareFile,
    PiwgFirmwareVolume,
    RelativeOffsetRange,
    RamDisk,
    Bios,
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devpath;
    use alloc::format;

    fn round_trip<const N: usize>(_text: &str, bytes: &[u8; N], expected: &str) {
        let path = DevicePath::try_from_bytes(bytes).expect("macro output is a valid path");
        assert_eq!(path.to_text(TextOptions::default()), expected);
    }

    #[test]
    fn test_device_path_text_common_nodes() {
        round_trip("PciRoot(0)/Pci(0x11,0)", &devpath!("PciRoot(0)/Pci(0x11,0)"), "PciRoot(0x0)/Pci(0x11,0x0)");
        round_trip(
            "USB(2,1)/HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)",
            &devpath!("USB(2,1)/HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)"),
            "USB(0x2,0x1)/HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
        );
        round_trip(r"\EFI\BOOT\BOOTX64.EFI", &devpath!(r"\EFI\BOOT\BOOTX64.EFI"), r"\EFI\BOOT\BOOTX64.EFI");
    }

    #[test]
    fn test_device_path_text_instances_and_options() {
        let bytes = devpath!("Pci(1,0),USB(2,1)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(path.to_text(TextOptions::default()), "Pci(0x1,0x0),USB(0x2,0x1)");

        let hd = DevicePath::try_from_bytes(&devpath!("HD(1,MBR,0x12345678,0x800,0x1000)")).expect("valid path");
        assert_eq!(hd.to_text(TextOptions { display_only: true, allow_shortcuts: true }), "HD(1,MBR,0x12345678)");

        let uart = UartFlowControl { flow_control_map: UartFlowControl::HARDWARE };
        assert_eq!(format!("{uart}"), "UartFlowCtrl(Hardware)");
        struct Wrapper<'a>(&'a UartFlowControl);
        impl fmt::Display for Wrapper<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt_text(f, TextOptions { display_only: false, allow_shortcuts: false })
            }
        }
        assert_eq!(format!("{}", Wrapper(&uart)), "VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,01000000)");
    }

    #[test]
    fn test_device_path_text_into_string() {
        let bytes = devpath!("PciRoot(0)/Pci(0x1D,0)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(String::from(path), "PciRoot(0x0)/Pci(0x1d,0x0)");
        let text: String = path.into();
        assert_eq!(text, path.to_text(TextOptions::default()));
    }

    #[test]
    fn test_generic_device_path_text() {
        let bytes = [9u8, 2, 6, 0, 0xaa, 0xbb, 0x7f, 0xff, 4, 0];
        let path = DevicePath::try_from_bytes(&bytes).expect("generic path is valid");
        assert_eq!(path.to_text(TextOptions::default()), "Path(9,2,aabb)");
    }

    /// Check the text of every kind of node in the three text forms, and that the parseable
    /// forms parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_round_trip() {
        macro_rules! check {
            ($input:literal, $text:literal, $display_only:literal, $no_shortcuts:literal) => {{
                let bytes = devpath!($input);
                let path = DevicePath::try_from_bytes(&bytes).expect($input);
                assert_eq!(path.to_text(TextOptions::default()), $text, "default text of {}", $input);
                assert_eq!(
                    path.to_text(TextOptions { display_only: true, allow_shortcuts: true }),
                    $display_only,
                    "display-only text of {}",
                    $input
                );
                assert_eq!(
                    path.to_text(TextOptions { display_only: false, allow_shortcuts: false }),
                    $no_shortcuts,
                    "text without shortcuts of {}",
                    $input
                );
                assert_eq!(devpath!($text), bytes, "default text of {} does not parse back", $input);
                assert_eq!(devpath!($no_shortcuts), bytes, "text without shortcuts of {} does not parse back", $input);
            }};
        }
        check!(r"Pci(0x1F,0x2)", r"Pci(0x1f,0x2)", r"Pci(0x1f,0x2)", r"Pci(0x1f,0x2)");
        check!(r"PcCard(1)", r"PcCard(0x1)", r"PcCard(0x1)", r"PcCard(0x1)");
        check!(
            r"MemoryMapped(11,0x1000,0x1FFF)",
            r"MemoryMapped(0xb,0x1000,0x1fff)",
            r"MemoryMapped(0xb,0x1000,0x1fff)",
            r"MemoryMapped(0xb,0x1000,0x1fff)"
        );
        check!(
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)"
        );
        check!(r"Ctrl(3)", r"Ctrl(0x3)", r"Ctrl(0x3)", r"Ctrl(0x3)");
        check!(r"BMC(1,0x1000)", r"BMC(0x1,0x1000)", r"BMC(0x1,0x1000)", r"BMC(0x1,0x1000)");
        check!(r"PciRoot(1)", r"PciRoot(0x1)", r"PciRoot(0x1)", r"Acpi(PNP0A03,0x1)");
        check!(r"PcieRoot(2)", r"PcieRoot(0x2)", r"PcieRoot(0x2)", r"Acpi(PNP0A08,0x2)");
        check!(r"Floppy(0)", r"Floppy(0x0)", r"Floppy(0x0)", r"Acpi(PNP0604,0x0)");
        check!(r"Keyboard(0)", r"Keyboard(0x0)", r"Keyboard(0x0)", r"Acpi(PNP0301,0x0)");
        check!(r"Serial(1)", r"Serial(0x1)", r"Serial(0x1)", r"Acpi(PNP0501,0x1)");
        check!(r"ParallelPort(0)", r"ParallelPort(0x0)", r"ParallelPort(0x0)", r"Acpi(PNP0401,0x0)");
        check!(r"Acpi(HWP0002,3)", r"Acpi(HWP0002,0x3)", r"Acpi(HWP0002,0x3)", r"Acpi(HWP0002,0x3)");
        check!(
            r"AcpiEx(HWP0002,PNP0A03,0)",
            r"AcpiEx(HWP0002,PNP0A03,0x0,,,)",
            r"PciRoot(0x0)",
            r"AcpiEx(HWP0002,PNP0A03,0x0,,,)"
        );
        check!(
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)",
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)",
            r"PciRoot(UniqueId)",
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)"
        );
        check!(
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)"
        );
        check!(r"Ata(Primary,Master,0)", r"Ata(Primary,Master,0x0)", r"Ata(0x0)", r"Ata(Primary,Master,0x0)");
        check!(r"Scsi(1,2)", r"Scsi(0x1,0x2)", r"Scsi(0x1,0x2)", r"Scsi(0x1,0x2)");
        check!(
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)"
        );
        check!(
            r"FibreEx(0102030405060708,1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)"
        );
        check!(
            r"I1394(0x0123456789ABCDEF)",
            r"I1394(0x0123456789abcdef)",
            r"I1394(0x0123456789abcdef)",
            r"I1394(0x0123456789abcdef)"
        );
        check!(r"USB(1,0)", r"USB(0x1,0x0)", r"USB(0x1,0x0)", r"USB(0x1,0x0)");
        check!(r"I2O(5)", r"I2O(0x5)", r"I2O(0x5)", r"I2O(0x5)");
        check!(
            r"Infiniband(1,00112233-4455-6677-8899-AABBCCDDEEFF,1,2,3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)"
        );
        check!(
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)"
        );
        check!(r"VenPcAnsi()", r"VenPcAnsi()", r"VenPcAnsi()", r"VenMsg(E0C14753-F9BE-11D2-9A0C-0090273FC14D)");
        check!(r"VenVt100()", r"VenVt100()", r"VenVt100()", r"VenMsg(DFA66065-B419-11D3-9A2D-0090273FC14D)");
        check!(
            r"VenVt100Plus()",
            r"VenVt100Plus()",
            r"VenVt100Plus()",
            r"VenMsg(7BAEC70B-57E0-4C76-8E87-2F9E28088343)"
        );
        check!(r"VenUtf8()", r"VenUtf8()", r"VenUtf8()", r"VenMsg(AD15A0D6-8BEC-4ACF-A073-D01DE77E2D88)");
        check!(
            r"UartFlowCtrl(XonXoff)",
            r"UartFlowCtrl(XonXoff)",
            r"UartFlowCtrl(XonXoff)",
            r"VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,02000000)"
        );
        check!(
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,2)",
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,0x2)",
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,0x2)",
            r"VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,000000003412000000000000050000000000000062010600)"
        );
        check!(r"DebugPort()", r"DebugPort()", r"DebugPort()", r"VenMsg(EBA4E8D2-3858-41EC-A281-2647BA9660D0)");
        check!(r"MAC(001122334455,0x1)", r"MAC(001122334455,0x1)", r"MAC(001122334455,0x1)", r"MAC(001122334455,0x1)");
        check!(
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,UDP,DHCP,0.0.0.0,0.0.0.0,0.0.0.0)",
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,UDP,DHCP,0.0.0.0,0.0.0.0,0.0.0.0)"
        );
        check!(
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)",
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)",
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)"
        );
        check!(
            r"IPv6(2001:db8::1)",
            r"IPv6(2001:db8::1,UDP,Static,::,0x0,::)",
            r"IPv6(2001:db8::1)",
            r"IPv6(2001:db8::1,UDP,Static,::,0x0,::)"
        );
        check!(
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080)",
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080,0.0.0.0,0.0.0.0)",
            r"IPv4(192.168.0.1:80)",
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080,0.0.0.0,0.0.0.0)"
        );
        check!(
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,64,2001:db8::ffff)",
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,0x40,2001:db8::ffff)",
            r"IPv6([2001:db8::1]:443)",
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,0x40,2001:db8::ffff)"
        );
        check!(r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)");
        check!(
            r"UsbClass(0x1234,0x5678,0x8,0x6,0x50)",
            r"UsbMassStorage(0x1234,0x5678,0x6,0x50)",
            r"UsbMassStorage(0x1234,0x5678,0x6,0x50)",
            r"UsbClass(0x1234,0x5678,0x8,0x6,0x50)"
        );
        check!(
            r"UsbMassStorage(0xFFFF,0xFFFF,0x6,0x50)",
            r"UsbMassStorage(0xffff,0xffff,0x6,0x50)",
            r"UsbMassStorage(0xffff,0xffff,0x6,0x50)",
            r"UsbClass(0xffff,0xffff,0x8,0x6,0x50)"
        );
        check!(
            r#"UsbWwid(0x1234,0x5678,0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#
        );
        check!(r"Unit(1)", r"Unit(0x1)", r"Unit(0x1)", r"Unit(0x1)");
        check!(r"Sata(0,0xFFFF,0)", r"Sata(0x0,0xffff,0x0)", r"Sata(0x0,0xffff,0x0)", r"Sata(0x0,0xffff,0x0)");
        check!(
            r"iSCSI(iqn.x,1,0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)"
        );
        check!(r"Vlan(100)", r"Vlan(100)", r"Vlan(100)", r"Vlan(100)");
        check!(
            r"SasEx(0102030405060708,1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)"
        );
        check!(
            r"SasEx(0102030405060708,1112131415161718,0x6)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)"
        );
        check!(
            r"SasEx(0102030405060708,1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)"
        );
        check!(
            r"SAS(0x1234,0,0,SATA,Internal,Direct,256)",
            r"SAS(0x1234,0x0,0x0,SATA,Internal,Direct,0x100)",
            r"SAS(0x1234,0x0,0x0,SATA,Internal,Direct,0x100)",
            r"VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,000000003412000000000000000000000000000012ff0000)"
        );
        check!(
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)"
        );
        check!(r"Uri(http://a/b)", r"Uri(http://a/b)", r"Uri(http://a/b)", r"Uri(http://a/b)");
        check!(r"UFS(0,0x3)", r"UFS(0x0,0x3)", r"UFS(0x0,0x3)", r"UFS(0x0,0x3)");
        check!(r"SD(1)", r"SD(0x1)", r"SD(0x1)", r"SD(0x1)");
        check!(
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)"
        );
        check!(r"Wi-Fi(ssid)", r"Wi-Fi(ssid)", r"Wi-Fi(ssid)", r"Wi-Fi(ssid)");
        check!(r"eMMC(0)", r"eMMC(0x0)", r"eMMC(0x0)", r"eMMC(0x0)");
        check!(
            r"BluetoothLE(001122334455,0x1)",
            r"BluetoothLE(001122334455,0x01)",
            r"BluetoothLE(001122334455,0x01)",
            r"BluetoothLE(001122334455,0x01)"
        );
        check!(
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)"
        );
        check!(r"RestService(1,1)", r"RestService(0x1,0x1)", r"RestService(0x1,0x1)", r"RestService(0x1,0x1)");
        check!(
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)"
        );
        check!(
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)"
        );
        check!(
            r"HD(2,MBR,0x12345678,0x800,0x1000)",
            r"HD(2,MBR,0x12345678,0x800,0x1000)",
            r"HD(2,MBR,0x12345678)",
            r"HD(2,MBR,0x12345678,0x800,0x1000)"
        );
        check!(r"CDROM(1,0x10,0x20)", r"CDROM(0x1,0x10,0x20)", r"CDROM(0x1)", r"CDROM(0x1,0x10,0x20)");
        check!(
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)"
        );
        check!(r"\file", r"\file", r"\file", r"\file");
        check!(
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(r"Offset(0x10,0x20)", r"Offset(0x10,0x20)", r"Offset(0x10,0x20)", r"Offset(0x10,0x20)");
        check!(
            r"RamDisk(0x1000,0x1FFF,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(
            r"VirtualDisk(0x1000,0x1FFF,0)",
            r"VirtualDisk(0x1000,0x1fff,0)",
            r"VirtualDisk(0x1000,0x1fff,0)",
            r"RamDisk(0x1000,0x1fff,0,77AB535A-45FC-624B-5560-F7B281D1F96E)"
        );
        check!(r"BBS(HD,boot,0x0)", r"BBS(HD,boot,0x0)", r"BBS(HD,boot)", r"BBS(HD,boot,0x0)");
        check!(r"Path(9,2,aabb)", r"Path(9,2,aabb)", r"Path(9,2,aabb)", r"Path(9,2,aabb)");
        check!(r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)");
        check!(r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)");
        check!(r"Msg(99,aabb)", r"Msg(99,aabb)", r"Msg(99,aabb)", r"Msg(99,aabb)");
        check!(r"MediaPath(99,aabb)", r"MediaPath(99,aabb)", r"MediaPath(99,aabb)", r"MediaPath(99,aabb)");
        check!(r"BbsPath(9,aabb)", r"BbsPath(9,aabb)", r"BbsPath(9,aabb)", r"BbsPath(9,aabb)");
    }
}
