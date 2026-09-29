//! Messaging device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, MessagingSubType, ascii, check_length, invalid, read_ascii};
use crate::{
    device_path_node,
    uefi::device_path::parse_node::{self, DevicePathNode, Header},
};
use alloc::{string::String, vec::Vec};
use core::fmt::Display;
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

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
