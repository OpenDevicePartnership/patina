//! Messaging device path nodes (UEFI 2.10 Section 10.3.4, type `0x03`).
//!
//! A messaging device path describes the connection of devices outside the resource
//! domain of the system. It can describe physical messaging information, such as a
//! SCSI ID, or abstract information, such as networking protocol IP addresses.
//!
//! Messaging nodes are used for buses that have a standard enumeration algorithm but
//! are outside the coherency domain of the system, such as SCSI and Fibre Channel,
//! and for virtual connections over network-style devices (Section 10.4.4).
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#messaging-device-path>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, MessagingSubType, invalid};
use crate::{
    device_path_node,
    uefi::device_path::{
        encoding::{AsciiNul, Utf8Nul, Vector},
        parse_node::{MeasureSize, Validate},
    },
};
use alloc::{string::String, vec::Vec};
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

device_path_node! {
    /// ATAPI device path node (UEFI 2.10 Table 10.13).
    ///
    /// The primary or secondary channel, master or slave mode, and logical unit number
    /// of an ATAPI device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#atapi-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Atapi)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Atapi {
        /// Zero for the primary channel, one for the secondary channel.
        pub primary_secondary: u8,
        /// Zero for master mode, one for slave mode.
        pub slave_master: u8,
        /// Logical unit number.
        pub lun: u16,
    }
}

impl Validate for Atapi {
    fn validate(&self) -> Result<(), &'static str> {
        if self.primary_secondary > 1 || self.slave_master > 1 {
            return Err("invalid ATAPI channel or master/slave selector");
        }
        Ok(())
    }
}

device_path_node! {
    /// SCSI device path node (UEFI 2.10 Table 10.14).
    ///
    /// The target ID (PUN) and logical unit number on a SCSI bus.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#scsi-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Scsi)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Scsi {
        /// Target ID (physical unit number) on the SCSI bus.
        pub pun: u16,
        /// Logical unit number.
        pub lun: u16,
    }
}

device_path_node! {
    /// Fibre Channel device path node (UEFI 2.10 Table 10.15).
    ///
    /// The Fibre Channel World Wide Name and logical unit number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#fibre-channel-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::FiberChannel)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct FiberChannel {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Fibre Channel world wide name.
        pub wwn: u64,
        /// Fibre Channel logical unit number.
        pub lun: u64,
    }
}

device_path_node! {
    /// Fibre Channel Ex device path node (UEFI 2.10 Table 10.16).
    ///
    /// The World Wide Name and logical unit number as 8-byte arrays. The logical unit
    /// number follows the SCSI Architecture Model 4 (SAM-4).
    ///
    /// The 24-byte layout includes the reserved field, as specified in Table 10.16 and
    /// EDK II, rather than the inconsistent 20-byte example in Table 10.17.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#fibre-channel-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::FiberChannelEx)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct FiberChannelEx {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Eight port-name bytes in wire order, without integer byte swapping.
        pub wwn: [u8; 8],
        /// Eight SCSI REPORT LUNS bytes in wire order.
        pub lun: [u8; 8],
    }
}

device_path_node! {
    /// 1394 device path node (UEFI 2.10 Table 10.18).
    ///
    /// The 64-bit IEEE 1394 global unique ID, which is not an EFI GUID.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::_1394)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Ieee1394 {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// The 64-bit IEEE 1394 identifier, not an EFI GUID.
        pub guid: u64,
    }
}

device_path_node! {
    /// USB device path node (UEFI 2.10 Table 10.19).
    ///
    /// The USB parent port number and interface number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#usb-device-paths>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Usb)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Usb {
        /// USB parent port number.
        pub parent_port_number: u8,
        /// USB interface number.
        pub interface_number: u8,
    }
}

device_path_node! {
    /// I2O device path node (UEFI 2.10 Table 10.26).
    ///
    /// The target ID (TID) of an I2O random block storage device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#i-2-o-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::I2oRandomBlockStorageClass)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct I2oRandomBlockStorageClass {
        /// Target identifier.
        pub tid: u32,
    }
}

device_path_node! {
    /// MAC address device path node (UEFI 2.10 Table 10.27).
    ///
    /// The MAC address of a network interface, padded with zeros, and its interface
    /// type.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#mac-address-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::MacAddress)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct MacAddress {
        /// Address bytes in wire order; producers pad unused bytes with zero.
        pub mac_address: [u8; 32],
        /// Network interface type, as assigned by IANA.
        pub if_type: u8,
    }
}

device_path_node! {
    /// IPv6 device path node (UEFI 2.10 Table 10.29).
    ///
    /// Local and remote IPv6 addresses and ports, protocol, address origin, prefix
    /// length, and gateway address.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ipv6-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::IpV6)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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

impl Validate for IpV6 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.ip_address_origin > 2 || self.prefix_length > 128 {
            return Err("invalid IPv6 address origin or prefix length");
        }
        Ok(())
    }
}

device_path_node! {
    /// VLAN device path node (UEFI 2.10 Table 10.30).
    ///
    /// An IEEE 802.1Q VLAN identifier.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vlan-device-path-node>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Vlan)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Vlan {
        /// VLAN identifier, from 0 through 4094.
        pub vlan_id: u16,
    }
}

impl Validate for Vlan {
    fn validate(&self) -> Result<(), &'static str> {
        if self.vlan_id > 4094 {
            return Err("invalid VLAN identifier");
        }
        Ok(())
    }
}

device_path_node! {
    /// InfiniBand device path node (UEFI 2.10 Table 10.31).
    ///
    /// The remote fabric port, IOC or service, target port, and device of an InfiniBand
    /// connection.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#infiniband-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::InfiniBand)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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

device_path_node! {
    /// UART device path node (UEFI 2.10 Table 10.32).
    ///
    /// The baud rate, data bits, parity, and stop bits of a UART. A value of zero
    /// selects the device default.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#uart-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Uart)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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

impl Validate for Uart {
    fn validate(&self) -> Result<(), &'static str> {
        if self.parity > 5 || self.stop_bits > 3 {
            return Err("invalid UART parity or stop bits");
        }
        Ok(())
    }
}

device_path_node! {
    /// SAS extended device path node (UEFI 2.10 Table 10.36).
    ///
    /// The SAS target port address and logical unit number as 8-byte arrays, with
    /// device topology information and the relative target port.
    ///
    /// Uses EDK II's packed 24-byte layout: address, LUN, topology, and relative port
    /// follow the header without gaps. Table 10.36 inconsistently gives a 32-byte length
    /// and leaves an eight-byte hole before the LUN.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#serial-attached-scsi-sas-extended-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::SasEx)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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

device_path_node! {
    /// UFS device path node (UEFI 2.10 Table 10.42).
    ///
    /// The target ID (PUN) and logical unit number of a Universal Flash Storage device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ufs-universal-flash-storage-device-messaging-devicepath-node>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Ufs)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Ufs {
        /// Target ID; UFS 2.0 producers use zero.
        pub pun: u8,
        /// Logical unit number, including well-known LUN encodings.
        pub lun: u8,
    }
}

device_path_node! {
    /// SD device path node (UEFI 2.10 Table 10.43).
    ///
    /// The slot number of a Secure Digital card.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#sd-secure-digital-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Sd)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Sd {
        /// Slot number.
        pub slot_number: u8,
    }
}

device_path_node! {
    /// Bluetooth device path node (UEFI 2.10 Table 10.44).
    ///
    /// The 48-bit address of a Bluetooth device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-bluetooth-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Bluetooth)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Bluetooth {
        /// Six Bluetooth address bytes in wire order.
        pub address: [u8; 6],
    }
}

device_path_node! {
    /// Wi-Fi device path node (UEFI 2.10 Table 10.45).
    ///
    /// The SSID of a wireless network as a 32-byte octet string.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#wireless-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::WiFi)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct WiFi {
        /// The 32 SSID octets; this field is not a NUL-terminated string.
        pub ssid: [u8; 32],
    }
}

device_path_node! {
    /// eMMC device path node (UEFI 2.10 Table 10.46).
    ///
    /// The slot number of an embedded Multi-Media Card.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#emmc-embedded-multi-media-card-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Emmc)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Emmc {
        /// Slot number.
        pub slot_number: u8,
    }
}

device_path_node! {
    /// Bluetooth LE device path node (UEFI 2.10 Table 10.47).
    ///
    /// The 48-bit address of a Bluetooth Low Energy device and its address type.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-bluetoothle-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::BluetoothLE)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct BluetoothLE {
        /// Six Bluetooth address bytes in wire order.
        pub address: [u8; 6],
        /// Address type: zero for public, one for random.
        pub address_type: u8,
    }
}

impl Validate for BluetoothLE {
    fn validate(&self) -> Result<(), &'static str> {
        if self.address_type > 1 {
            return Err("invalid Bluetooth LE address type");
        }
        Ok(())
    }
}

device_path_node! {
    /// NVDIMM namespace device path node (UEFI 2.10 Table 10.49).
    ///
    /// A bootable NVDIMM namespace identified by its label UUID. This is distinct from
    /// the ACPI [`NvdimmAcpi`](super::NvdimmAcpi) device node.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvdimm-namespace-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Nvdimm)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Nvdimm {
        /// Namespace label UUID as 16 EFI GUID wire bytes, without additional byte swapping.
        pub uuid: [u8; 16],
    }
}

device_path_node! {
    /// Device logical unit device path node (UEFI 2.10 Table 10.24).
    ///
    /// The logical unit number, appended to the device path of a device with several
    /// logical units, such as a USB mass storage device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-logical-unit>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::DeviceLogicalUnit)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct DeviceLogicalUnit {
        /// Logical unit number for the interface.
        pub lun: u8,
    }
}

device_path_node! {
    /// USB class device path node (UEFI 2.10 Table 10.25).
    ///
    /// Matches USB devices by vendor ID, product ID, class, subclass, and protocol. A
    /// value of `0xFFFF` or `0xFF` matches any value.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-logical-unit>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::UsbClass)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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

device_path_node! {
    /// Vendor-defined messaging device path node (UEFI 2.10 Table 10.33).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-messaging-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Vendor)]
    @[DevicePathNodeDerive(Debug, Display, Validate)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct MessagingVendor {
        /// Vendor GUID in EFI wire byte order.
        pub guid: [u8; 16],
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Vector)]
        pub data: Vec<u8>,
    }
}

impl MeasureSize for MessagingVendor {
    fn measure_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

device_path_node! {
    /// USB WWID device path node (UEFI 2.10 Table 10.23).
    ///
    /// A USB device identified by its interface number, vendor and product IDs, and the
    /// last 64 or fewer UTF-16 characters of its serial number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#usb-device-paths-wwid>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::UsbWwid)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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
        #[scroll(ctx = Vector)]
        pub serial_number: Vec<u16>,
    }
}

impl MeasureSize for UsbWwid {
    fn measure_size(&self) -> usize {
        size_of_val(&self.interface)
            + size_of_val(&self.vendor_id)
            + size_of_val(&self.product_id)
            + size_of_val(self.serial_number.as_slice())
    }
}

impl Validate for UsbWwid {
    fn validate(&self) -> Result<(), &'static str> {
        if self.serial_number.len() > 64
            || core::char::decode_utf16(self.serial_number.iter().copied()).any(|c| c.is_err())
        {
            return Err("USB WWID requires at most 64 valid UTF-16 code units");
        }
        Ok(())
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

device_path_node! {
    /// IPv4 device path node (UEFI 2.10 Table 10.28).
    ///
    /// Local and remote IPv4 addresses and ports, protocol, and address origin, with the
    /// gateway and subnet mask. Earlier specification versions defined a 19-byte node
    /// without the gateway and subnet mask; both forms are supported.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ipv4-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::IpV4)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq)]
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
}

impl MeasureSize for IpV4 {
    fn measure_size(&self) -> usize {
        size_of_val(&self.local_ip)
            + size_of_val(&self.remote_ip)
            + size_of_val(&self.local_port)
            + size_of_val(&self.remote_port)
            + size_of_val(&self.protocol)
            + size_of_val(&self.static_ip)
            + self
                .network
                .as_ref()
                .map_or(0, |network| size_of_val(&network.gateway) + size_of_val(&network.subnet_mask))
    }
}

impl Validate for IpV4 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.static_ip > 1 {
            return Err("invalid IPv4 address origin");
        }
        Ok(())
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
        Ok((Self { local_ip, remote_ip, local_port, remote_port, protocol, static_ip, network }, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for IpV4 {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
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

device_path_node! {
    /// iSCSI device path node (UEFI 2.10 Table 10.37).
    ///
    /// The network protocol, login options, logical unit number, target portal group
    /// tag, and target name of an iSCSI target.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#iscsi-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Iscsi)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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
        #[scroll(ctx = AsciiNul)]
        pub target_name: String,
    }
}

impl MeasureSize for Iscsi {
    fn measure_size(&self) -> usize {
        size_of_val(&self.protocol)
            + size_of_val(&self.options)
            + size_of_val(&self.lun)
            + size_of_val(&self.portal_group)
            + self.target_name.len()
            + size_of::<u8>() // NUL terminator
    }
}

impl Validate for Iscsi {
    fn validate(&self) -> Result<(), &'static str> {
        if self.target_name.is_empty() || self.target_name.len() > 223 || self.protocol != 0 {
            return Err("invalid iSCSI target name or network protocol");
        }
        Ok(())
    }
}

device_path_node! {
    /// URI device path node (UEFI 2.10 Table 10.41).
    ///
    /// A URI as defined by RFC 3986. The node length bounds the URI; there is no
    /// terminator, and an empty URI is allowed.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#uniform-resource-identifiers-uri-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Uri)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct Uri {
        /// URI text, including an empty URI when appropriate.
        pub uri: String,
    }
}

impl MeasureSize for Uri {
    fn measure_size(&self) -> usize {
        self.uri.len()
    }
}

impl Validate for Uri {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.uri.is_ascii() || self.uri.contains('\0') {
            return Err("URI must be ASCII without embedded NUL");
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for Uri {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let uri = core::str::from_utf8(data).map_err(|_| invalid("URI must be ASCII"))?;
        Ok((Self { uri: String::from(uri) }, data.len()))
    }
}

impl TryIntoCtx<scroll::Endian> for Uri {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        data.pwrite_with(self.uri.as_bytes(), 0, ())
    }
}

device_path_node! {
    /// DNS device path node (UEFI 2.10 Table 10.48).
    ///
    /// One or more DNS server addresses, all IPv4 or all IPv6, each stored as a 16-byte
    /// `EFI_IP_ADDRESS`.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#dns-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Dns)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct Dns {
        /// True for IPv6 addresses; false for IPv4 addresses in the first four bytes.
        pub is_ipv6: bool,
        /// One or more 16-byte addresses, with unused IPv4 bytes preserved.
        pub addresses: Vec<[u8; 16]>,
    }
}

impl MeasureSize for Dns {
    fn measure_size(&self) -> usize {
        size_of_val(&self.is_ipv6) + size_of_val(self.addresses.as_slice())
    }
}

impl Validate for Dns {
    fn validate(&self) -> Result<(), &'static str> {
        if self.addresses.is_empty() {
            return Err("DNS node requires at least one address");
        }
        Ok(())
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
        Ok((Self { is_ipv6: family == 1, addresses }, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for Dns {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        let mut offset = 0;
        data.gwrite_with(u8::from(self.is_ipv6), &mut offset, scroll::LE)?;
        for address in &self.addresses {
            data.gwrite_with(address.as_slice(), &mut offset, ())?;
        }
        Ok(offset)
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

device_path_node! {
    /// REST service device path node (UEFI 2.10 Tables 10.50 and 10.51).
    ///
    /// A Redfish, `OData`, or vendor-specific REST service and its in-band or out-of-band
    /// access mode. The vendor-specific form adds a GUID and vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#rest-service-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::RestService)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct RestService {
        /// Standard service or vendor payload.
        pub service: RestServiceType,
        /// One for in-band access, two for out-of-band access.
        pub access_mode: u8,
    }
}

impl MeasureSize for RestService {
    fn measure_size(&self) -> usize {
        let vendor = match &self.service {
            RestServiceType::Vendor { guid, data } => size_of_val(guid) + size_of_val(data.as_slice()),
            _ => 0,
        };
        size_of::<u8>() // service type, encoded as one byte
            + size_of_val(&self.access_mode)
            + vendor
    }
}

impl Validate for RestService {
    fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.access_mode, 1 | 2) {
            return Err("invalid REST access mode");
        }
        Ok(())
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
        Ok((Self { service, access_mode }, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for RestService {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
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

device_path_node! {
    /// `NVMe` over Fabrics namespace device path node (UEFI 2.10 Table 10.52).
    ///
    /// A bootable NVMe-oF namespace identified by its namespace identifier type and
    /// value and the NVM subsystem NQN.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvme-over-fabric-nvme-of-namespace-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::NvmeOf)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct NvmeOf {
        /// Namespace identifier type: 1 for EUI-64, 2 for NGUID, 3 for UUID.
        pub identifier_type: u8,
        /// Identifier bytes; EUI-64 uses the first eight bytes, UUID uses RFC byte order.
        pub identifier: [u8; 16],
        /// UTF-8 subsystem NQN without its wire NUL terminator, at most 223 bytes.
        #[scroll(ctx = Utf8Nul)]
        pub subsystem_nqn: String,
    }
}

impl MeasureSize for NvmeOf {
    fn measure_size(&self) -> usize {
        size_of_val(&self.identifier_type) + size_of_val(&self.identifier) + self.subsystem_nqn.len() + size_of::<u8>() // NUL terminator
    }
}

impl Validate for NvmeOf {
    fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.identifier_type, 1..=3)
            || self.subsystem_nqn.is_empty()
            || self.subsystem_nqn.len() > 223
            || (self.identifier_type == 1 && self.identifier[8..].iter().any(|byte| *byte != 0))
        {
            return Err("invalid NVMe over Fabrics namespace identity");
        }
        Ok(())
    }
}

device_path_node! {
    /// SATA device path node (UEFI 2.10 Table 10.22).
    ///
    /// The HBA port, port multiplier port, and logical unit number of a SATA device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#sata-device-path>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Sata)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone)]
    pub struct Sata {
        /// HBA Port Number.
        pub hba_port: u16,
        /// Port Multiplier Port Number.
        pub port_multiplier_port: u16,
        /// Logical Unit Number.
        pub lun: u16,
    }
}

impl Sata {
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

impl Validate for Sata {
    fn validate(&self) -> Result<(), &'static str> {
        if self.hba_port == u16::MAX {
            return Err("SATA HBA port 0xffff is reserved");
        }
        Ok(())
    }
}

device_path_node! {
    /// NVM Express namespace device path node (UEFI 2.10 Table 10.40).
    ///
    /// An NVM Express namespace identifier (NSID) and IEEE Extended Unique Identifier
    /// (EUI-64).
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvm-express-namespace-messaging-device-path-node>
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::NvmExpress)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pread, Pwrite, Clone)]
    pub struct NvmExpress {
        /// Namespace Identifier (NSID).
        pub namespace_id: u32,
        /// IEEE Extended Unique Identifier (EUI-64).
        pub eui64: u64,
    }
}

impl NvmExpress {
    /// Create a new `NvmExpress` device path node.
    ///
    /// # Arguments
    /// * `namespace_id` - The namespace identifier (typically 1 for the first namespace)
    /// * `eui64` - The IEEE Extended Unique Identifier (can be 0 if not specified)
    pub fn new(namespace_id: u32, eui64: u64) -> Self {
        Self { namespace_id, eui64 }
    }
}

impl Validate for NvmExpress {
    fn validate(&self) -> Result<(), &'static str> {
        if matches!(self.namespace_id, 0 | u32::MAX) {
            return Err("NVMe namespace ID is reserved");
        }
        Ok(())
    }
}
