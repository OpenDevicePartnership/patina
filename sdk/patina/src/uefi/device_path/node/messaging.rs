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

// cspell:ignore NGUID nguid sassata iscsitarget iscsidisk nvmeoftarget nvmeofdisk NSUUID
use super::TextString;
use super::{DevicePathType, MessagingSubType, device_path_enum, device_path_node};
use crate::BinaryGuid;
use crate::{
    standard::efi::{self, protocols::usb_io},
    uefi::{
        boot_services::BootServices,
        device_path::encoding::{Ascii, AsciiNul, EfiMacAddress, IpAddress, Utf8Nul, Vector},
    },
};
use alloc::{string::String, vec::Vec};
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use core::{fmt, mem, ptr, slice};
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

device_path_enum! {
    /// ATAPI channel of an [`Atapi`] node ([UEFI 2.10 Table 10.13](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#atapi-device-path)).
    pub enum AtapiChannel: u8 {
        /// Primary channel.
        Primary = 0 => "Primary",
        /// Secondary channel.
        Secondary = 1 => "Secondary",
    }
}

device_path_enum! {
    /// ATAPI drive of an [`Atapi`] node, master or slave on its channel
    /// ([UEFI 2.10 Table 10.13](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#atapi-device-path)).
    pub enum AtapiDrive: u8 {
        /// Master drive.
        Master = 0 => "Master",
        /// Slave drive.
        Slave = 1 => "Slave",
    }
}

device_path_node! {
    /// ATAPI device path node (UEFI 2.10 Table 10.13).
    ///
    /// The primary or secondary channel, master or slave mode, and logical unit number
    /// of an ATAPI device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#atapi-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{Atapi, AtapiChannel, AtapiDrive};
    ///
    /// let node = Atapi { primary_secondary: AtapiChannel::Primary, slave_master: AtapiDrive::Master, lun: 0 };
    /// assert_eq!(node.to_string(), "Ata(Primary,Master,0x0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Ata(0x0)");
    /// assert_eq!(format!("{node:#}"), "Ata(Primary,Master,0x0)");
    /// assert_eq!(format!("{node:-#}"), "Ata(0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Atapi)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Atapi {
        /// Channel of the drive.
        pub primary_secondary: AtapiChannel,
        /// Master or slave drive on the channel.
        pub slave_master: AtapiDrive,
        /// Logical unit number.
        pub lun: u16,
    }
}
impl fmt::Display for Atapi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        if f.sign_minus() {
            write!(f, "Ata(0x{:x})", node.lun)
        } else {
            write!(f, "Ata({},{},0x{:x})", node.primary_secondary, node.slave_master, node.lun)
        }
    }
}

device_path_node! {
    /// SCSI device path node (UEFI 2.10 Table 10.14).
    ///
    /// The target ID (PUN) and logical unit number on a SCSI bus.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#scsi-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Scsi;
    ///
    /// let node = Scsi { pun: 1, lun: 2 };
    /// assert_eq!(node.to_string(), "Scsi(0x1,0x2)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Scsi(0x1,0x2)");
    /// assert_eq!(format!("{node:#}"), "Scsi(0x1,0x2)");
    /// assert_eq!(format!("{node:-#}"), "Scsi(0x1,0x2)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Scsi)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Scsi {
        /// Target ID (physical unit number) on the SCSI bus.
        pub pun: u16,
        /// Logical unit number.
        pub lun: u16,
    }
}
impl fmt::Display for Scsi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Scsi(0x{:x},0x{:x})", node.pun, node.lun)
    }
}

device_path_node! {
    /// Fibre Channel device path node (UEFI 2.10 Table 10.15).
    ///
    /// The Fibre Channel World Wide Name and logical unit number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#fibre-channel-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::FiberChannel;
    ///
    /// // The device of `FibreEx(0x0001020304050607,0x0001020304050607)` (UEFI 2.10 Section 10.3.4.3)
    /// // in the original Fibre Channel node, where the name and LUN are integers.
    /// let node = FiberChannel { reserved: 0, wwn: 0x0001_0203_0405_0607, lun: 0x0001_0203_0405_0607 };
    /// assert_eq!(node.to_string(), "Fibre(0x1020304050607,0x1020304050607)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Fibre(0x1020304050607,0x1020304050607)");
    /// assert_eq!(format!("{node:#}"), "Fibre(0x1020304050607,0x1020304050607)");
    /// assert_eq!(format!("{node:-#}"), "Fibre(0x1020304050607,0x1020304050607)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::FiberChannel, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct FiberChannel {
        /// Reserved; must be zero.
        pub reserved: u32,
        /// Fibre Channel world wide name.
        pub wwn: u64,
        /// Fibre Channel logical unit number.
        pub lun: u64,
    }
}
impl fmt::Display for FiberChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Fibre(0x{:x},0x{:x})", node.wwn, node.lun)
    }
}

impl FiberChannel {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.reserved != 0 {
            return Err("reserved field must be zero");
        }
        Ok(())
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::FiberChannelEx;
    ///
    /// // `PciRoot(0)/PCI(31,0)/FibreEx(0x0001020304050607,0x0001020304050607)` (UEFI 2.10 Section 10.3.4.3).
    /// let node = FiberChannelEx { reserved: 0, wwn: [0, 1, 2, 3, 4, 5, 6, 7], lun: [0, 1, 2, 3, 4, 5, 6, 7] };
    /// assert_eq!(node.to_string(), "FibreEx(0x0001020304050607,0x0001020304050607)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "FibreEx(0x0001020304050607,0x0001020304050607)");
    /// assert_eq!(format!("{node:#}"), "FibreEx(0x0001020304050607,0x0001020304050607)");
    /// assert_eq!(format!("{node:-#}"), "FibreEx(0x0001020304050607,0x0001020304050607)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::FiberChannelEx, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct FiberChannelEx {
        /// Reserved; must be zero.
        pub reserved: u32,
        /// Eight port-name bytes in wire order, without integer byte swapping.
        pub wwn: [u8; 8],
        /// Eight SCSI REPORT LUNS bytes in wire order.
        pub lun: [u8; 8],
    }
}
impl fmt::Display for FiberChannelEx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        f.write_str("FibreEx(0x")?;
        for byte in &node.wwn {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(",0x")?;
        for byte in &node.lun {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

impl FiberChannelEx {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.reserved != 0 {
            return Err("reserved field must be zero");
        }
        Ok(())
    }
}

device_path_node! {
    /// 1394 device path node (UEFI 2.10 Table 10.18).
    ///
    /// The 64-bit IEEE 1394 global unique ID, which is not an EFI GUID.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Ieee1394;
    ///
    /// let node = Ieee1394 { reserved: 0, guid: 0x0123456789abcdef };
    /// assert_eq!(node.to_string(), "I1394(0x0123456789abcdef)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "I1394(0x0123456789abcdef)");
    /// assert_eq!(format!("{node:#}"), "I1394(0x0123456789abcdef)");
    /// assert_eq!(format!("{node:-#}"), "I1394(0x0123456789abcdef)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::_1394, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Ieee1394 {
        /// Reserved; must be zero.
        pub reserved: u32,
        /// The 64-bit IEEE 1394 identifier, not an EFI GUID.
        pub guid: u64,
    }
}
impl fmt::Display for Ieee1394 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        // The 1394 GUID is an 8-byte integer (UEFI 2.10 Table 10.18), so it is written as a hexadecimal
        // integer (Table 10.66) that parses back to the same value.
        write!(f, "I1394(0x{:016x})", node.guid)
    }
}

impl Ieee1394 {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.reserved != 0 {
            return Err("reserved field must be zero");
        }
        Ok(())
    }
}

device_path_node! {
    /// USB device path node (UEFI 2.10 Table 10.19).
    ///
    /// The USB parent port number and interface number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#usb-device-paths>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Usb;
    ///
    /// // The device on port 0 of the root hub, interface 0, of `PciRoot(0)/PCI(31,2)/USB(0,0)`
    /// // (UEFI 2.10 Section 10.3.4.5.1).
    /// let node = Usb { parent_port_number: 0, interface_number: 0 };
    /// assert_eq!(node.to_string(), "USB(0x0,0x0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "USB(0x0,0x0)");
    /// assert_eq!(format!("{node:#}"), "USB(0x0,0x0)");
    /// assert_eq!(format!("{node:-#}"), "USB(0x0,0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Usb)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Usb {
        /// USB parent port number.
        pub parent_port_number: u8,
        /// USB interface number.
        pub interface_number: u8,
    }
}
impl fmt::Display for Usb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "USB(0x{:x},0x{:x})", node.parent_port_number, node.interface_number)
    }
}

device_path_node! {
    /// I2O device path node (UEFI 2.10 Table 10.26).
    ///
    /// The target ID (TID) of an I2O random block storage device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#i-2-o-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::I2oRandomBlockStorageClass;
    ///
    /// let node = I2oRandomBlockStorageClass { tid: 5 };
    /// assert_eq!(node.to_string(), "I2O(0x5)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "I2O(0x5)");
    /// assert_eq!(format!("{node:#}"), "I2O(0x5)");
    /// assert_eq!(format!("{node:-#}"), "I2O(0x5)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::I2oRandomBlockStorageClass)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct I2oRandomBlockStorageClass {
        /// Target identifier.
        pub tid: u32,
    }
}
impl fmt::Display for I2oRandomBlockStorageClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "I2O(0x{:x})", node.tid)
    }
}

device_path_node! {
    /// MAC address device path node (UEFI 2.10 Table 10.27).
    ///
    /// The MAC address of a network interface, padded with zeros, and its interface
    /// type.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#mac-address-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{standard::efi::MacAddress as EfiMacAddress, uefi::device_path::node::MacAddress};
    ///
    /// // The network interface of the iSCSI example `PciRoot(0)/Pci(0x19,0x0)/Mac(001320F5FA77,0x01)/...`
    /// // (UEFI 2.10 Section 10.3.4.20.2). The interface type 1 is Ethernet.
    /// let mut addr = [0; 32];
    /// addr[..6].copy_from_slice(&[0x00, 0x13, 0x20, 0xf5, 0xfa, 0x77]);
    /// let node = MacAddress { mac_address: EfiMacAddress { addr }, if_type: 1 };
    /// assert_eq!(node.to_string(), "MAC(001320f5fa77,0x1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "MAC(001320f5fa77,0x1)");
    /// assert_eq!(format!("{node:#}"), "MAC(001320f5fa77,0x1)");
    /// assert_eq!(format!("{node:-#}"), "MAC(001320f5fa77,0x1)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::MacAddress)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct MacAddress {
        /// Interface address; [`if_type`](Self::if_type) defines how many bytes are
        /// significant, and producers pad the rest with zeros.
        #[scroll(ctx = EfiMacAddress)]
        pub mac_address: efi::MacAddress,
        /// Network interface type, as assigned by IANA.
        pub if_type: u8,
    }
}
impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        let length = if matches!(node.if_type, 0 | 1) { 6 } else { node.mac_address.addr.len() };
        f.write_str("MAC(")?;
        for byte in node.mac_address.addr.get(..length).ok_or(fmt::Error)? {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ",0x{:x})", node.if_type)
    }
}

/// IP protocol number of an [`IpV4`] or [`IpV6`] node
/// ([UEFI 2.10 Tables 10.28 and 10.29](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ipv4-device-path)), as assigned by IANA, such as
/// [`TCP`](Self::TCP) or [`UDP`](Self::UDP).
///
/// The text form, from `Display`, is `TCP`, `UDP`, or the number in hexadecimal.
///
/// # Examples
///
/// ```
/// use patina::uefi::device_path::node::IpProtocol;
///
/// assert_eq!(IpProtocol::TCP.to_string(), "TCP");
/// assert_eq!(IpProtocol(0x2f).to_string(), "0x2f");
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct IpProtocol(pub u16);

impl IpProtocol {
    /// Transmission Control Protocol.
    pub const TCP: Self = Self(6);
    /// User Datagram Protocol.
    pub const UDP: Self = Self(17);
}

impl fmt::Display for IpProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TCP => f.write_str("TCP"),
            Self::UDP => f.write_str("UDP"),
            Self(value) => write!(f, "0x{value:x}"),
        }
    }
}

// Scroll encoding: a 16-bit value, like the `u16` it wraps.
impl<'a> TryFromCtx<'a, Endian> for IpProtocol {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], ctx: Endian) -> Result<(Self, usize), Self::Error> {
        let (value, size) = u16::try_from_ctx(src, ctx)?;
        Ok((Self(value), size))
    }
}

impl TryIntoCtx<Endian> for &IpProtocol {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

impl TryIntoCtx<Endian> for IpProtocol {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

device_path_enum! {
    /// How the local address of an [`IpV6`] node was assigned
    /// ([UEFI 2.10 Table 10.29](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ipv6-device-path)).
    pub enum Ipv6AddressOrigin: u8 {
        /// The local address is manually configured.
        Static = 0 => "Static",
        /// The local address comes from stateless auto-configuration.
        StatelessAutoConfigure = 1 => "StatelessAutoConfigure",
        /// The local address comes from stateful configuration.
        StatefulAutoConfigure = 2 => "StatefulAutoConfigure",
    }
}

device_path_node! {
    /// IPv6 device path node (UEFI 2.10 Table 10.29).
    ///
    /// Local and remote IPv6 addresses and ports, protocol, address origin, prefix
    /// length, and gateway address.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ipv6-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use core::net::Ipv6Addr;
    /// use patina::uefi::device_path::node::{IpProtocol, IpV6, Ipv6AddressOrigin};
    ///
    /// // The IPv6 node of the iSCSI example (UEFI 2.10 Section 10.3.4.20.2): the target
    /// // 2001:4898:a:1005:95a6:ee6c:bed3:4859 over TCP, with the local address 2001:4898:a:1005:215:17ff:fe21:5593
    /// // assigned by DHCPv6 (stateful auto-configuration).
    /// let node = IpV6 {
    ///     local_ip_address: Ipv6Addr::new(0x2001, 0x4898, 0xa, 0x1005, 0x215, 0x17ff, 0xfe21, 0x5593),
    ///     remote_ip_address: Ipv6Addr::new(0x2001, 0x4898, 0xa, 0x1005, 0x95a6, 0xee6c, 0xbed3, 0x4859),
    ///     local_port: 0,
    ///     remote_port: 0,
    ///     protocol: IpProtocol::TCP,
    ///     ip_address_origin: Ipv6AddressOrigin::StatefulAutoConfigure,
    ///     prefix_length: 0,
    ///     gateway_ip_address: Ipv6Addr::UNSPECIFIED,
    /// };
    /// assert_eq!(
    ///     node.to_string(),
    ///     "IPv6(2001:4898:a:1005:95a6:ee6c:bed3:4859,TCP,StatefulAutoConfigure,2001:4898:a:1005:215:17ff:fe21:5593,0x0,::)",
    /// );
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "IPv6(2001:4898:a:1005:95a6:ee6c:bed3:4859)");
    /// assert_eq!(
    ///     format!("{node:#}"),
    ///     "IPv6(2001:4898:a:1005:95a6:ee6c:bed3:4859,TCP,StatefulAutoConfigure,2001:4898:a:1005:215:17ff:fe21:5593,0x0,::)",
    /// );
    /// assert_eq!(format!("{node:-#}"), "IPv6(2001:4898:a:1005:95a6:ee6c:bed3:4859)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::IpV6)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct IpV6 {
        /// Local IPv6 address.
        #[scroll(ctx = IpAddress)]
        pub local_ip_address: Ipv6Addr,
        /// Remote IPv6 address.
        #[scroll(ctx = IpAddress)]
        pub remote_ip_address: Ipv6Addr,
        /// Local port number, encoded as a UEFI integer.
        pub local_port: u16,
        /// Remote port number, encoded as a UEFI integer.
        pub remote_port: u16,
        /// Network protocol.
        pub protocol: IpProtocol,
        /// How the local address was assigned.
        pub ip_address_origin: Ipv6AddressOrigin,
        /// Number of prefix bits.
        pub prefix_length: u8,
        /// Gateway IPv6 address.
        #[scroll(ctx = IpAddress)]
        pub gateway_ip_address: Ipv6Addr,
    }
}
impl fmt::Display for IpV6 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        // An address is written with its port, as `[address]:port`, only when the port is not zero.
        match node.remote_port {
            0 => write!(f, "IPv6({}", node.remote_ip_address)?,
            port => write!(f, "IPv6({}", SocketAddrV6::new(node.remote_ip_address, port, 0, 0))?,
        }
        if f.sign_minus() {
            return f.write_str(")");
        }
        write!(f, ",{}", node.protocol)?;
        let origin = node.ip_address_origin;
        match node.local_port {
            0 => write!(f, ",{origin},{}", node.local_ip_address)?,
            port => write!(f, ",{origin},{}", SocketAddrV6::new(node.local_ip_address, port, 0, 0))?,
        }
        write!(f, ",0x{:x},{})", node.prefix_length, node.gateway_ip_address)
    }
}

device_path_node! {
    /// VLAN device path node (UEFI 2.10 Table 10.30).
    ///
    /// An IEEE 802.1Q VLAN identifier.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vlan-device-path-node>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Vlan;
    ///
    /// let node = Vlan { vlan_id: 100 };
    /// assert_eq!(node.to_string(), "Vlan(100)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Vlan(100)");
    /// assert_eq!(format!("{node:#}"), "Vlan(100)");
    /// assert_eq!(format!("{node:-#}"), "Vlan(100)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Vlan, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Vlan {
        /// VLAN identifier, from 0 through 4094.
        pub vlan_id: u16,
    }
}
impl fmt::Display for Vlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Vlan({})", node.vlan_id)
    }
}

impl Vlan {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::InfiniBand};
    ///
    /// let node = InfiniBand {
    ///     resource_flags: 1,
    ///     port_gid: *BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF").as_bytes(),
    ///     service_id: 1,
    ///     target_port_id: 2,
    ///     device_id: 3,
    /// };
    /// assert_eq!(node.to_string(), "Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)");
    /// assert_eq!(format!("{node:#}"), "Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)");
    /// assert_eq!(format!("{node:-#}"), "Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::InfiniBand, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct InfiniBand {
        /// Resource flags; bits 5 through 31 are reserved and must be zero.
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
impl fmt::Display for InfiniBand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
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
}

impl InfiniBand {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.resource_flags & !InfiniBand::RESOURCE_FLAGS_MASK != 0 {
            return Err("InfiniBand resource flags bits 5 through 31 are reserved");
        }
        Ok(())
    }
}

impl InfiniBand {
    /// Defined resource flag bits 0 through 4; the other bits are reserved and must be zero.
    pub const RESOURCE_FLAGS_MASK: u32 = 0x1f;
}

device_path_enum! {
    /// Parity of a [`Uart`] node ([UEFI 2.10 Table 10.32](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#uart-device-path)).
    ///
    /// The text form, from `Display`, is the letter of the `Uart` text.
    pub enum UartParity: u8 {
        /// The device default.
        Default = 0 => "D",
        /// No parity.
        None = 1 => "N",
        /// Even parity.
        Even = 2 => "E",
        /// Odd parity.
        Odd = 3 => "O",
        /// Mark parity.
        Mark = 4 => "M",
        /// Space parity.
        Space = 5 => "S",
    }
}

device_path_enum! {
    /// Number of stop bits of a [`Uart`] node ([UEFI 2.10 Table 10.32](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#uart-device-path)).
    ///
    /// The text form, from `Display`, is the value of the `Uart` text.
    pub enum UartStopBits: u8 {
        /// The device default.
        Default = 0 => "D",
        /// One stop bit.
        One = 1 => "1",
        /// One and a half stop bits.
        OneAndHalf = 2 => "1.5",
        /// Two stop bits.
        Two = 3 => "2",
    }
}

device_path_node! {
    /// UART device path node (UEFI 2.10 Table 10.32).
    ///
    /// The baud rate, data bits, parity, and stop bits of a UART. A value of zero
    /// selects the device default.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#uart-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{Uart, UartParity, UartStopBits};
    ///
    /// let node = Uart { reserved: 0, baud_rate: 115200, data_bits: 8, parity: UartParity::None, stop_bits: UartStopBits::One };
    /// assert_eq!(node.to_string(), "Uart(115200,8,N,1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Uart(115200,8,N,1)");
    /// assert_eq!(format!("{node:#}"), "Uart(115200,8,N,1)");
    /// assert_eq!(format!("{node:-#}"), "Uart(115200,8,N,1)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Uart, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Uart {
        /// Reserved; must be zero.
        pub reserved: u32,
        /// Baud rate, or zero to use the device default.
        pub baud_rate: u64,
        /// Number of data bits, or zero to use the device default.
        pub data_bits: u8,
        /// Parity.
        pub parity: UartParity,
        /// Number of stop bits.
        pub stop_bits: UartStopBits,
    }
}
impl fmt::Display for Uart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        // Zero, the device default, is written as a number: the text has no keyword for it.
        write!(f, "Uart({},{},{},{})", node.baud_rate, node.data_bits, node.parity, node.stop_bits)
    }
}

impl Uart {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.reserved != 0 {
            return Err("reserved field must be zero");
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{SasEx, SasTopology};
    ///
    /// // The device of `SAS(0x31000004CF13F6BD,0)` (UEFI 2.10 Section 10.3.4.18.4) in the extended node,
    /// // where the address and LUN are byte arrays.
    /// let node = SasEx {
    ///     sas_address: [0x31, 0x00, 0x00, 0x04, 0xcf, 0x13, 0xf6, 0xbd],
    ///     lun: [0; 8],
    ///     device_topology: SasTopology(0),
    ///     relative_target_port: 0,
    /// };
    /// assert_eq!(node.to_string(), "SasEx(0x31000004cf13f6bd,0x0000000000000000,0x0,NoTopology)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "SasEx(0x31000004cf13f6bd,0x0000000000000000,0x0,NoTopology)");
    /// assert_eq!(format!("{node:#}"), "SasEx(0x31000004cf13f6bd,0x0000000000000000,0x0,NoTopology)");
    /// assert_eq!(format!("{node:-#}"), "SasEx(0x31000004cf13f6bd,0x0000000000000000,0x0,NoTopology)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::SasEx)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct SasEx {
        /// Eight SAS target-port address bytes in wire order.
        pub sas_address: [u8; 8],
        /// Eight SCSI REPORT LUNS bytes in wire order.
        pub lun: [u8; 8],
        /// SAS/SATA device and topology information, including reserved bits.
        pub device_topology: SasTopology,
        /// Relative target port.
        pub relative_target_port: u16,
    }
}
impl fmt::Display for Sas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        if f.alternate() {
            // The vendor data: the reserved field, SAS address, LUN, topology, and relative target port.
            write!(f, "VenMsg({},", Sas::GUID)?;
            let data = node
                .reserved
                .to_le_bytes()
                .into_iter()
                .chain(node.sas_address.to_le_bytes())
                .chain(node.lun.to_le_bytes())
                .chain(node.device_topology.0.to_le_bytes())
                .chain(node.relative_target_port.to_le_bytes());
            for byte in data {
                write!(f, "{byte:02x}")?;
            }
            f.write_str(")")
        } else {
            write!(f, "SAS(0x{:x},0x{:x},0x{:x},", node.sas_address, node.lun, node.relative_target_port)?;
            write!(f, "{}", node.device_topology)?;
            if node.reserved != 0 {
                // The reserved field is the fifth argument after the topology.
                for _ in node.device_topology.argument_count()..4 {
                    f.write_str(",")?;
                }
                write!(f, ",0x{:x}", node.reserved)?;
            }
            f.write_str(")")
        }
    }
}

impl fmt::Display for SasEx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        f.write_str("SasEx(0x")?;
        for byte in &node.sas_address {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(",0x")?;
        for byte in &node.lun {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ",0x{:x},{})", node.relative_target_port, node.device_topology)
    }
}

device_path_node! {
    /// UFS device path node (UEFI 2.10 Table 10.42).
    ///
    /// The target ID (PUN) and logical unit number of a Universal Flash Storage device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ufs-universal-flash-storage-device-messaging-devicepath-node>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Ufs;
    ///
    /// let node = Ufs { pun: 0, lun: 3 };
    /// assert_eq!(node.to_string(), "UFS(0x0,0x3)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "UFS(0x0,0x3)");
    /// assert_eq!(format!("{node:#}"), "UFS(0x0,0x3)");
    /// assert_eq!(format!("{node:-#}"), "UFS(0x0,0x3)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Ufs)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Ufs {
        /// Target ID; UFS 2.0 producers use zero.
        pub pun: u8,
        /// Logical unit number, including well-known LUN encodings.
        pub lun: u8,
    }
}
impl fmt::Display for Ufs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "UFS(0x{:x},0x{:x})", node.pun, node.lun)
    }
}

device_path_node! {
    /// SD device path node (UEFI 2.10 Table 10.43).
    ///
    /// The slot number of a Secure Digital card.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#sd-secure-digital-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Sd;
    ///
    /// let node = Sd { slot_number: 1 };
    /// assert_eq!(node.to_string(), "SD(0x1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "SD(0x1)");
    /// assert_eq!(format!("{node:#}"), "SD(0x1)");
    /// assert_eq!(format!("{node:-#}"), "SD(0x1)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Sd)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Sd {
        /// Slot number.
        pub slot_number: u8,
    }
}
impl fmt::Display for Sd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "SD(0x{:x})", node.slot_number)
    }
}

device_path_node! {
    /// Bluetooth device path node (UEFI 2.10 Table 10.44).
    ///
    /// The 48-bit address of a Bluetooth device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-bluetooth-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Bluetooth;
    ///
    /// let node = Bluetooth { address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55] };
    /// assert_eq!(node.to_string(), "Bluetooth(001122334455)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Bluetooth(001122334455)");
    /// assert_eq!(format!("{node:#}"), "Bluetooth(001122334455)");
    /// assert_eq!(format!("{node:-#}"), "Bluetooth(001122334455)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Bluetooth)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Bluetooth {
        /// Six Bluetooth address bytes in wire order.
        pub address: [u8; 6],
    }
}
impl fmt::Display for Bluetooth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        f.write_str("Bluetooth(")?;
        for byte in &node.address {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

device_path_node! {
    /// Wi-Fi device path node (UEFI 2.10 Table 10.45).
    ///
    /// The SSID of a wireless network as a 32-byte octet string.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#wireless-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::WiFi;
    ///
    /// let mut ssid = [0; 32];
    /// ssid[..4].copy_from_slice(b"ssid");
    /// let node = WiFi { ssid };
    /// assert_eq!(node.to_string(), "Wi-Fi(ssid)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Wi-Fi(ssid)");
    /// assert_eq!(format!("{node:#}"), "Wi-Fi(ssid)");
    /// assert_eq!(format!("{node:-#}"), "Wi-Fi(ssid)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::WiFi)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct WiFi {
        /// The 32 SSID octets; this field is not a NUL-terminated string.
        pub ssid: [u8; 32],
    }
}
impl fmt::Display for WiFi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        let len = node.ssid.iter().position(|b| *b == 0).unwrap_or(node.ssid.len());
        write!(f, "Wi-Fi({})", TextString(&String::from_utf8_lossy(node.ssid.get(..len).ok_or(fmt::Error)?)))
    }
}

device_path_node! {
    /// eMMC device path node (UEFI 2.10 Table 10.46).
    ///
    /// The slot number of an embedded Multi-Media Card.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#emmc-embedded-multi-media-card-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Emmc;
    ///
    /// let node = Emmc { slot_number: 0 };
    /// assert_eq!(node.to_string(), "eMMC(0x0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "eMMC(0x0)");
    /// assert_eq!(format!("{node:#}"), "eMMC(0x0)");
    /// assert_eq!(format!("{node:-#}"), "eMMC(0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Emmc)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Emmc {
        /// Slot number.
        pub slot_number: u8,
    }
}
impl fmt::Display for Emmc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "eMMC(0x{:x})", node.slot_number)
    }
}

device_path_enum! {
    /// Address type of a [`BluetoothLE`] node
    /// ([UEFI 2.10 Table 10.47](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-bluetoothle-device-path)).
    ///
    /// The device path text writes it as a number, such as `BluetoothLE(001122334455,0x01)`.
    pub enum BluetoothLeAddressType: u8 {
        /// Public device address.
        Public = 0,
        /// Random device address.
        Random = 1,
    }
}

device_path_node! {
    /// Bluetooth LE device path node (UEFI 2.10 Table 10.47).
    ///
    /// The 48-bit address of a Bluetooth Low Energy device and its address type.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-bluetoothle-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{BluetoothLE, BluetoothLeAddressType};
    ///
    /// let node = BluetoothLE { address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55], address_type: BluetoothLeAddressType::Random };
    /// assert_eq!(node.to_string(), "BluetoothLE(001122334455,0x01)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "BluetoothLE(001122334455,0x01)");
    /// assert_eq!(format!("{node:#}"), "BluetoothLE(001122334455,0x01)");
    /// assert_eq!(format!("{node:-#}"), "BluetoothLE(001122334455,0x01)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::BluetoothLE,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct BluetoothLE {
        /// Six Bluetooth address bytes in wire order.
        pub address: [u8; 6],
        /// Address type.
        pub address_type: BluetoothLeAddressType,
    }
}
impl fmt::Display for BluetoothLE {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        f.write_str("BluetoothLE(")?;
        for byte in &node.address {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ",0x{:02x})", u8::from(node.address_type))
    }
}

device_path_node! {
    /// NVDIMM namespace device path node (UEFI 2.10 Table 10.49).
    ///
    /// A bootable NVDIMM namespace identified by its label UUID. This is distinct from
    /// the ACPI [`NvdimmAcpi`](super::NvdimmAcpi) device node.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvdimm-namespace-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::Nvdimm};
    ///
    /// let node = Nvdimm { uuid: BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF") };
    /// assert_eq!(node.to_string(), "NVDIMM(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "NVDIMM(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:#}"), "NVDIMM(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:-#}"), "NVDIMM(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Nvdimm)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Nvdimm {
        /// Namespace label UUID.
        pub uuid: BinaryGuid,
    }
}
impl fmt::Display for Nvdimm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "NVDIMM({})", node.uuid)
    }
}

device_path_node! {
    /// Device logical unit device path node (UEFI 2.10 Table 10.24).
    ///
    /// The logical unit number, appended to the device path of a device with several
    /// logical units, such as a USB mass storage device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-logical-unit>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::DeviceLogicalUnit;
    ///
    /// let node = DeviceLogicalUnit { lun: 1 };
    /// assert_eq!(node.to_string(), "Unit(0x1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Unit(0x1)");
    /// assert_eq!(format!("{node:#}"), "Unit(0x1)");
    /// assert_eq!(format!("{node:-#}"), "Unit(0x1)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::DeviceLogicalUnit)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct DeviceLogicalUnit {
        /// Logical unit number for the interface.
        pub lun: u8,
    }
}
impl fmt::Display for DeviceLogicalUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Unit(0x{:x})", node.lun)
    }
}

device_path_node! {
    /// USB class device path node (UEFI 2.10 Table 10.25).
    ///
    /// Matches USB devices by vendor ID, product ID, class, subclass, and protocol. A
    /// value of `0xFFFF` or `0xFF` matches any value.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-logical-unit>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::UsbClass;
    ///
    /// // Any USB mass storage device using the SCSI command set over bulk-only transport (class 0x08,
    /// // subclass 0x06, protocol 0x50). 0xFFFF matches any vendor and product.
    /// let node = UsbClass {
    ///     vendor_id: 0xffff,
    ///     product_id: 0xffff,
    ///     device_class: 0x08,
    ///     device_subclass: 0x06,
    ///     device_protocol: 0x50,
    /// };
    /// assert_eq!(node.to_string(), "UsbMassStorage(0xffff,0xffff,0x6,0x50)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "UsbMassStorage(0xffff,0xffff,0x6,0x50)");
    /// assert_eq!(format!("{node:#}"), "UsbClass(0xffff,0xffff,0x8,0x6,0x50)");
    /// assert_eq!(format!("{node:-#}"), "UsbClass(0xffff,0xffff,0x8,0x6,0x50)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::UsbClass)]
    @[DevicePathNodeDerive(Debug)]
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
impl UsbClass {
    /// The shortcut text name of this node's class (UEFI 2.10 Table 10.67), and whether the
    /// name also fixes the subclass, so that the subclass is left out of the text.
    fn shortcut(&self) -> Option<(&'static str, bool)> {
        let name = match (self.device_class, self.device_subclass) {
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

    /// Return true if the USB device matches this node; a field of all ones matches any value.
    ///
    /// When the device descriptor class is zero, each interface declares its own class, so the
    /// class, subclass, and protocol come from the interface descriptor.
    pub(crate) fn matches(&self, usb_io: &mut usb_io::Protocol) -> bool {
        // SAFETY: The descriptor holds only integers, so all zero bytes is a valid value.
        let mut device: usb_io::DeviceDescriptor = unsafe { mem::zeroed() };
        // SAFETY: The protocol is installed on a USB device handle, and the descriptor is writable.
        if unsafe { (usb_io.get_device_descriptor)(usb_io, &raw mut device) } != efi::Status::SUCCESS {
            return false;
        }
        let (mut class, mut subclass, mut protocol) =
            (device.device_class, device.device_sub_class, device.device_protocol);
        if class == 0 {
            // SAFETY: The descriptor holds only integers, so all zero bytes is a valid value.
            let mut interface: usb_io::InterfaceDescriptor = unsafe { mem::zeroed() };
            // SAFETY: The protocol is installed on a USB device handle, and the descriptor is writable.
            if unsafe { (usb_io.get_interface_descriptor)(usb_io, &raw mut interface) } != efi::Status::SUCCESS {
                return false;
            }
            (class, subclass, protocol) =
                (interface.interface_class, interface.interface_sub_class, interface.interface_protocol);
        }
        (self.vendor_id == u16::MAX || self.vendor_id == device.id_vendor)
            && (self.product_id == u16::MAX || self.product_id == device.id_product)
            && (self.device_class == u8::MAX || self.device_class == class)
            && (self.device_subclass == u8::MAX || self.device_subclass == subclass)
            && (self.device_protocol == u8::MAX || self.device_protocol == protocol)
    }
}

impl fmt::Display for UsbClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        if !f.alternate()
            && let Some((name, fixed_subclass)) = node.shortcut()
        {
            if fixed_subclass {
                return write!(
                    f,
                    "{name}(0x{:x},0x{:x},0x{:x})",
                    node.vendor_id, node.product_id, node.device_protocol
                );
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
}

device_path_node! {
    /// Vendor-defined messaging device path node (UEFI 2.10 Table 10.33).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-messaging-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::MessagingVendor;
    ///
    /// // The `DebugPort()` node of
    /// // `PciRoot(0)/Pci(0x1f,0)/ACPI(PNP0501,0)/UART(115200,N,8,1)/UartFlowCtrl(2)/DebugPort()`
    /// // (UEFI 2.10 Section 10.3.4.17): a vendor node with `EFI_DEBUGPORT_PROTOCOL_GUID` and no data.
    /// let node = MessagingVendor { guid: MessagingVendor::DEBUG_PORT_GUID, data: Vec::new() };
    /// assert_eq!(node.to_string(), "DebugPort()");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "DebugPort()");
    /// assert_eq!(format!("{node:#}"), "VenMsg(EBA4E8D2-3858-41EC-A281-2647BA9660D0)");
    /// assert_eq!(format!("{node:-#}"), "VenMsg(EBA4E8D2-3858-41EC-A281-2647BA9660D0)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::Vendor,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct MessagingVendor {
        /// Vendor GUID.
        pub guid: BinaryGuid,
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Vector)]
        pub data: Vec<u8>,
    }
}
impl fmt::Display for MessagingVendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        if !f.alternate() && node.data.is_empty() {
            let shortcut = match node.guid {
                Self::PC_ANSI_GUID => Some("VenPcAnsi()"),
                Self::VT_100_GUID => Some("VenVt100()"),
                Self::VT_100_PLUS_GUID => Some("VenVt100Plus()"),
                Self::VT_UTF8_GUID => Some("VenUtf8()"),
                Self::DEBUG_PORT_GUID => Some("DebugPort()"),
                _ => None,
            };
            if let Some(shortcut) = shortcut {
                return f.write_str(shortcut);
            }
        }
        write!(f, "VenMsg({}", node.guid)?;
        if !node.data.is_empty() {
            f.write_str(",")?;
            for byte in &node.data {
                write!(f, "{byte:02x}")?;
            }
        }
        f.write_str(")")
    }
}

impl MessagingVendor {
    /// `EFI_PC_ANSI_GUID`: a PC-ANSI terminal, `VenPcAnsi()` in text
    /// ([UEFI 2.10 Section 10.3.4.16](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-messaging-device-path)).
    pub const PC_ANSI_GUID: BinaryGuid = BinaryGuid::from_string("E0C14753-F9BE-11D2-9A0C-0090273FC14D");
    /// `EFI_VT_100_GUID`: a VT-100 terminal, `VenVt100()` in text
    /// ([UEFI 2.10 Section 10.3.4.16](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-messaging-device-path)).
    pub const VT_100_GUID: BinaryGuid = BinaryGuid::from_string("DFA66065-B419-11D3-9A2D-0090273FC14D");
    /// `EFI_VT_100_PLUS_GUID`: a VT-100+ terminal, `VenVt100Plus()` in text
    /// ([UEFI 2.10 Section 10.3.4.16](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-messaging-device-path)).
    pub const VT_100_PLUS_GUID: BinaryGuid = BinaryGuid::from_string("7BAEC70B-57E0-4C76-8E87-2F9E28088343");
    /// `EFI_VT_UTF8_GUID`: a VT-UTF8 terminal, `VenUtf8()` in text
    /// ([UEFI 2.10 Section 10.3.4.16](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-messaging-device-path)).
    pub const VT_UTF8_GUID: BinaryGuid = BinaryGuid::from_string("AD15A0D6-8BEC-4ACF-A073-D01DE77E2D88");
    /// `DEVICE_PATH_MESSAGING_DEBUGPORT`, the same value as `EFI_DEBUGPORT_PROTOCOL_GUID`: a
    /// debug port, `DebugPort()` in text ([UEFI 2.10 Section 18.3.7](https://uefi.org/specs/UEFI/2.10/18_Protocols_Debugger_Support.html#debugport-device-path)).
    pub const DEBUG_PORT_GUID: BinaryGuid = BinaryGuid::from_string("EBA4E8D2-3858-41EC-A281-2647BA9660D0");

    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

device_path_node! {
    /// UART flow control messaging device path node (UEFI 2.10 Table 10.34).
    ///
    /// A vendor-defined messaging node, identified by [`UartFlowControl::GUID`], that
    /// declares the flow control of the UART node it follows.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#uart-flow-control-messaging-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::UartFlowControl;
    ///
    /// // `UartFlowCtrl(2)`: Xon/Xoff flow control, from the debug port example (UEFI 2.10 Section 10.3.4.17).
    /// let node = UartFlowControl { flow_control_map: UartFlowControl::XON_XOFF };
    /// assert_eq!(node.to_string(), "UartFlowCtrl(XonXoff)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "UartFlowCtrl(XonXoff)");
    /// assert_eq!(format!("{node:#}"), "VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,02000000)");
    /// assert_eq!(format!("{node:-#}"), "VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,02000000)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::Vendor,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct UartFlowControl {
        /// Flow control bitmap: bit 0 is hardware (RTS/CTS) flow control, bit 1 is
        /// XON/XOFF flow control, and the other bits are reserved and must be zero.
        pub flow_control_map: u32,
    }
}
/// The SAS/SATA device and topology information of a [`Sas`] or [`SasEx`] node
/// ([UEFI 2.10 Sections 10.3.4.18.1 and 10.3.4.18.2](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-and-topology-information)).
///
/// The low byte describes the device: bits 0-3 say how much information is valid (`0` none, `1`
/// the low byte, `2` both bytes), bits 4-5 give the device type (SAS or SATA, internal or
/// external), and bits 6-7 the interconnect (direct, or through expanders). When both bytes are
/// valid, the high byte is the internal drive bay, `0` to `255` for drives 1 to 256.
///
/// The text form, from `Display`, is the topology arguments of the `SAS` and `SasEx` text
/// (UEFI 2.10 Table 10.67): `NoTopology`; the keywords such as `SATA,Internal,Direct` followed by
/// the drive bay when present; or the raw value, such as `0x81`, when it has reserved bits set.
///
/// # Examples
///
/// ```
/// use patina::uefi::device_path::node::SasTopology;
///
/// assert_eq!(SasTopology(0).to_string(), "NoTopology");
/// // A SATA device in internal drive bay 256, connected directly to the HBA.
/// assert_eq!(SasTopology(0xff12).to_string(), "SATA,Internal,Direct,0x100");
/// assert_eq!(SasTopology(0x81).to_string(), "0x81");
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct SasTopology(pub u16);

impl SasTopology {
    /// Return true if the value is written as keywords rather than as a number: some information
    /// is valid, no reserved bit is set, and the high byte is zero unless it is valid.
    fn has_keywords(self) -> bool {
        let information = self.0 & 0x0f;
        self.0 & 0x80 == 0 && (information == 2 || (information == 1 && self.0 >> 8 == 0))
    }

    /// Number of comma-separated arguments of the text form.
    fn argument_count(self) -> usize {
        match self.0 & 0x0f {
            2 if self.has_keywords() => 4,
            _ if self.has_keywords() => 3,
            _ => 1,
        }
    }
}

impl fmt::Display for SasTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        if value == 0 {
            return f.write_str("NoTopology");
        }
        if !self.has_keywords() {
            return write!(f, "0x{value:x}");
        }
        f.write_str(if value & 0x10 != 0 { "SATA" } else { "SAS" })?;
        f.write_str(if value & 0x20 != 0 { ",External" } else { ",Internal" })?;
        f.write_str(if value & 0x40 != 0 { ",Expanded" } else { ",Direct" })?;
        if value & 0x0f == 2 {
            write!(f, ",0x{:x}", (value >> 8) + 1)?;
        }
        Ok(())
    }
}

// Scroll encoding: a 16-bit value, like the `u16` it wraps.
impl<'a> TryFromCtx<'a, Endian> for SasTopology {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], ctx: Endian) -> Result<(Self, usize), Self::Error> {
        let (value, size) = u16::try_from_ctx(src, ctx)?;
        Ok((Self(value), size))
    }
}

impl TryIntoCtx<Endian> for &SasTopology {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

impl TryIntoCtx<Endian> for SasTopology {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

impl fmt::Display for UartFlowControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        // `UartFlowCtrl` takes only 0, 1, or 2 (UEFI 2.10 Table 10.67), so any other map, such as
        // both bits set, is written in the vendor form.
        let shortcut = match node.flow_control_map {
            _ if f.alternate() => None,
            0 => Some("None"),
            1 => Some("Hardware"),
            2 => Some("XonXoff"),
            _ => None,
        };
        if let Some(value) = shortcut {
            return write!(f, "UartFlowCtrl({value})");
        }
        write!(f, "VenMsg({},", UartFlowControl::GUID)?;
        for byte in node.flow_control_map.to_le_bytes() {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

impl UartFlowControl {
    /// Vendor GUID `DEVICE_PATH_MESSAGING_UART_FLOW_CONTROL` that identifies this node.
    pub const GUID: BinaryGuid = BinaryGuid::from_string("37499A9D-542F-4C89-A026-35DA142094E4");
    /// Hardware (RTS/CTS) flow control bit.
    pub const HARDWARE: u32 = 1 << 0;
    /// XON/XOFF flow control bit.
    pub const XON_XOFF: u32 = 1 << 1;
}

impl UartFlowControl {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of::<BinaryGuid>() + size_of_val(&self.flow_control_map)
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.flow_control_map & !(UartFlowControl::HARDWARE | UartFlowControl::XON_XOFF) != 0 {
            return Err("UART flow control map bits 2 through 31 are reserved");
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for UartFlowControl {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid: BinaryGuid = data.gread_with(&mut offset, scroll::LE)?;
        if guid != Self::GUID {
            return Err(scroll::Error::BadInput { size: 0, msg: "vendor GUID does not match the node" });
        }
        let flow_control_map = data.gread_with(&mut offset, endian)?;
        Ok((Self { flow_control_map }, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for UartFlowControl {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        let mut offset = 0;
        data.gwrite_with(Self::GUID, &mut offset, scroll::LE)?;
        data.gwrite_with(self.flow_control_map, &mut offset, endian)?;
        Ok(offset)
    }
}

device_path_node! {
    /// Serial Attached SCSI (SAS) messaging device path node (UEFI 2.10 Table 10.35).
    ///
    /// A vendor-defined messaging node, identified by [`Sas::GUID`], with the SAS
    /// address, logical unit number, device topology, and relative target port.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#serial-attached-scsi-sas-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{Sas, SasTopology};
    ///
    /// // `SAS(0x31000004CF13F6BD,0)`: LUN 0 of a SAS device reached through SSP, without additional
    /// // topology information (UEFI 2.10 Section 10.3.4.18.4).
    /// let node = Sas { reserved: 0, sas_address: 0x3100_0004_cf13_f6bd, lun: 0, device_topology: SasTopology(0), relative_target_port: 0 };
    /// assert_eq!(node.to_string(), "SAS(0x31000004cf13f6bd,0x0,0x0,NoTopology)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "SAS(0x31000004cf13f6bd,0x0,0x0,NoTopology)");
    /// assert_eq!(
    ///     format!("{node:#}"),
    ///     "VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,00000000bdf613cf04000031000000000000000000000000)",
    /// );
    /// assert_eq!(
    ///     format!("{node:-#}"),
    ///     "VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,00000000bdf613cf04000031000000000000000000000000)",
    /// );
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::Vendor,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct Sas {
        /// Reserved; must be zero.
        pub reserved: u32,
        /// SAS address.
        pub sas_address: u64,
        /// SAS logical unit number.
        pub lun: u64,
        /// SAS/SATA device and topology information.
        pub device_topology: SasTopology,
        /// Relative target port.
        pub relative_target_port: u16,
    }
}

impl Sas {
    /// Vendor GUID `DEVICE_PATH_MESSAGING_SAS` that identifies this node.
    pub const GUID: BinaryGuid = BinaryGuid::from_string("D487DDB4-008B-11D9-AFDC-001083FFCA4D");
}

impl Sas {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of::<BinaryGuid>()
            + size_of_val(&self.reserved)
            + size_of_val(&self.sas_address)
            + size_of_val(&self.lun)
            + size_of_val(&self.device_topology)
            + size_of_val(&self.relative_target_port)
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.reserved != 0 {
            return Err("reserved field must be zero");
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for Sas {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid: BinaryGuid = data.gread_with(&mut offset, scroll::LE)?;
        if guid != Self::GUID {
            return Err(scroll::Error::BadInput { size: 0, msg: "vendor GUID does not match the node" });
        }
        let node = Self {
            reserved: data.gread_with(&mut offset, endian)?,
            sas_address: data.gread_with(&mut offset, endian)?,
            lun: data.gread_with(&mut offset, endian)?,
            device_topology: data.gread_with(&mut offset, endian)?,
            relative_target_port: data.gread_with(&mut offset, endian)?,
        };
        Ok((node, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for Sas {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        let mut offset = 0;
        data.gwrite_with(Self::GUID, &mut offset, scroll::LE)?;
        data.gwrite_with(self.reserved, &mut offset, endian)?;
        data.gwrite_with(self.sas_address, &mut offset, endian)?;
        data.gwrite_with(self.lun, &mut offset, endian)?;
        data.gwrite_with(self.device_topology, &mut offset, endian)?;
        data.gwrite_with(self.relative_target_port, &mut offset, endian)?;
        Ok(offset)
    }
}

device_path_node! {
    /// USB WWID device path node (UEFI 2.10 Table 10.23).
    ///
    /// A USB device identified by its interface number, vendor and product IDs, and the
    /// last 64 or fewer UTF-16 characters of its serial number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#usb-device-paths-wwid>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::UsbWwid;
    ///
    /// let node = UsbWwid {
    ///     interface: 0,
    ///     vendor_id: 0x1234,
    ///     product_id: 0x5678,
    ///     serial_number: vec![0x0053, 0x004e, 0x0031, 0x0032, 0x0033],
    /// };
    /// assert_eq!(node.to_string(), r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#);
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#);
    /// assert_eq!(format!("{node:#}"), r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#);
    /// assert_eq!(format!("{node:-#}"), r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#);
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::UsbWwid,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
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
impl fmt::Display for IpV4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        // An address is written with its port, as `address:port`, only when the port is not zero.
        match node.remote_port {
            0 => write!(f, "IPv4({}", node.remote_ip)?,
            port => write!(f, "IPv4({}", SocketAddrV4::new(node.remote_ip, port))?,
        }
        if f.sign_minus() {
            return f.write_str(")");
        }
        write!(f, ",{}", node.protocol)?;
        f.write_str(if node.static_ip { ",Static" } else { ",DHCP" })?;
        match node.local_port {
            0 => write!(f, ",{}", node.local_ip)?,
            port => write!(f, ",{}", SocketAddrV4::new(node.local_ip, port))?,
        }
        if let Some(network) = &node.network {
            write!(f, ",{},{}", network.gateway, network.subnet_mask)?;
        }
        f.write_str(")")
    }
}

impl UsbWwid {
    /// Maximum number of UTF-16 code units read from a USB string descriptor, which holds at
    /// most 126 characters.
    const MAX_USB_STRING_LENGTH: usize = 127;

    /// Return true if the USB device matches this node.
    ///
    /// The vendor ID, product ID, and interface number must be equal, and the device serial
    /// number string, in one of the languages the device supports, must end with this node's
    /// serial number.
    pub(crate) fn matches<B: BootServices>(&self, boot_services: &B, usb_io: &mut usb_io::Protocol) -> bool {
        // SAFETY: The descriptor holds only integers, so all zero bytes is a valid value.
        let mut device: usb_io::DeviceDescriptor = unsafe { mem::zeroed() };
        // SAFETY: The protocol is installed on a USB device handle, and the descriptor is writable.
        if unsafe { (usb_io.get_device_descriptor)(usb_io, &raw mut device) } != efi::Status::SUCCESS {
            return false;
        }
        if device.id_vendor != self.vendor_id || device.id_product != self.product_id || device.str_serial_number == 0 {
            return false;
        }
        // SAFETY: The descriptor holds only integers, so all zero bytes is a valid value.
        let mut interface: usb_io::InterfaceDescriptor = unsafe { mem::zeroed() };
        // SAFETY: The protocol is installed on a USB device handle, and the descriptor is writable.
        if unsafe { (usb_io.get_interface_descriptor)(usb_io, &raw mut interface) } != efi::Status::SUCCESS
            || u16::from(interface.interface_number) != self.interface
        {
            return false;
        }

        let mut serial_suffix = self.serial_number.as_slice();
        if let [rest @ .., 0] = serial_suffix {
            serial_suffix = rest;
        }
        let mut languages: *mut u16 = ptr::null_mut();
        let mut table_size = 0u16;
        // SAFETY: The protocol is installed on a USB device handle, and both outputs are writable.
        if unsafe { (usb_io.get_supported_languages)(usb_io, &raw mut languages, &raw mut table_size) }
            != efi::Status::SUCCESS
            || languages.is_null()
        {
            return false;
        }
        // SAFETY: The protocol returned a language table of `table_size` bytes, which it owns.
        let languages = unsafe { slice::from_raw_parts(languages, usize::from(table_size) / 2) };
        for &language in languages {
            let mut serial: *mut u16 = ptr::null_mut();
            // SAFETY: The protocol is installed on a USB device handle, and the output is writable.
            if unsafe { (usb_io.get_string_descriptor)(usb_io, language, device.str_serial_number, &raw mut serial) }
                != efi::Status::SUCCESS
                || serial.is_null()
            {
                continue;
            }
            // SAFETY: The protocol returned a NUL-terminated string, read up to the longest USB string.
            let length = (0..Self::MAX_USB_STRING_LENGTH).find(|&index| unsafe { serial.add(index).read() } == 0);
            let matches = length.is_some_and(|length| {
                // SAFETY: The first `length` code units of the string were just read.
                let serial = unsafe { slice::from_raw_parts(serial, length) };
                serial.ends_with(serial_suffix)
            });
            // SAFETY: The string was allocated from pool memory by the protocol, and is not used after this.
            if unsafe { boot_services.free_pool(serial.cast::<u8>()) }.is_err() {
                log::warn!("Cannot free a USB serial number string");
            }
            if matches {
                return true;
            }
        }
        false
    }
}

impl fmt::Display for UsbWwid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        let serial = String::from_utf16_lossy(&node.serial_number);
        write!(f, "UsbWwid(0x{:x},0x{:x},0x{:x},\"{}\")", node.vendor_id, node.product_id, node.interface, serial)
    }
}

impl UsbWwid {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.interface)
            + size_of_val(&self.vendor_id)
            + size_of_val(&self.product_id)
            + size_of_val(self.serial_number.as_slice())
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.serial_number.len() > 64 || char::decode_utf16(self.serial_number.iter().copied()).any(|c| c.is_err()) {
            return Err("USB WWID requires at most 64 valid UTF-16 code units");
        }
        Ok(())
    }
}

/// Gateway and subnet fields added to the original IPv4 node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IpV4Network {
    /// Gateway address.
    pub gateway: Ipv4Addr,
    /// Subnet mask.
    pub subnet_mask: Ipv4Addr,
}

device_path_node! {
    /// IPv4 device path node (UEFI 2.10 Table 10.28).
    ///
    /// Local and remote IPv4 addresses and ports, protocol, and address origin, with the
    /// gateway and subnet mask. Earlier specification versions defined a 19-byte node
    /// without the gateway and subnet mask; both forms are supported.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ipv4-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use core::net::Ipv4Addr;
    /// use patina::uefi::device_path::node::{IpProtocol, IpV4};
    ///
    /// // `IPv4(192.168.0.100,TCP,Static,192.168.0.1)` of the iSCSI example (UEFI 2.10 Section 10.3.4.20.2):
    /// // the target is 192.168.0.100 and the static local address is 192.168.0.1.
    /// let node = IpV4 {
    ///     local_ip: Ipv4Addr::new(192, 168, 0, 1),
    ///     remote_ip: Ipv4Addr::new(192, 168, 0, 100),
    ///     local_port: 0,
    ///     remote_port: 0,
    ///     protocol: IpProtocol::TCP,
    ///     static_ip: true,
    ///     network: None,
    /// };
    /// assert_eq!(node.to_string(), "IPv4(192.168.0.100,TCP,Static,192.168.0.1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "IPv4(192.168.0.100)");
    /// assert_eq!(format!("{node:#}"), "IPv4(192.168.0.100,TCP,Static,192.168.0.1)");
    /// assert_eq!(format!("{node:-#}"), "IPv4(192.168.0.100)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::IpV4,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct IpV4 {
        /// Local address.
        pub local_ip: Ipv4Addr,
        /// Remote address.
        pub remote_ip: Ipv4Addr,
        /// Local port, encoded as a UEFI little-endian integer.
        pub local_port: u16,
        /// Remote port, encoded as a UEFI little-endian integer.
        pub remote_port: u16,
        /// Network protocol.
        pub protocol: IpProtocol,
        /// True for a static address, false for an address assigned through DHCP.
        pub static_ip: bool,
        /// Absent only for the original 19-byte node.
        pub network: Option<IpV4Network>,
    }
}

impl IpV4 {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
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

impl TryFromCtx<'_, scroll::Endian> for IpV4 {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let local_ip = data.gread_with(&mut offset, IpAddress)?;
        let remote_ip = data.gread_with(&mut offset, IpAddress)?;
        let local_port = data.gread_with(&mut offset, endian)?;
        let remote_port = data.gread_with(&mut offset, endian)?;
        let protocol = data.gread_with(&mut offset, endian)?;
        let static_ip = match data.gread_with::<u8>(&mut offset, endian)? {
            0 => false,
            1 => true,
            _ => return Err(scroll::Error::BadInput { size: 0, msg: "invalid IPv4 address origin" }),
        };
        let network = if data.len() == offset {
            None
        } else {
            Some(IpV4Network {
                gateway: data.gread_with(&mut offset, IpAddress)?,
                subnet_mask: data.gread_with(&mut offset, IpAddress)?,
            })
        };
        Ok((Self { local_ip, remote_ip, local_port, remote_port, protocol, static_ip, network }, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for IpV4 {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        let mut offset = 0;
        data.gwrite_with(&self.local_ip, &mut offset, IpAddress)?;
        data.gwrite_with(&self.remote_ip, &mut offset, IpAddress)?;
        data.gwrite_with(self.local_port, &mut offset, endian)?;
        data.gwrite_with(self.remote_port, &mut offset, endian)?;
        data.gwrite_with(self.protocol, &mut offset, endian)?;
        data.gwrite_with(u8::from(self.static_ip), &mut offset, endian)?;
        if let Some(network) = &self.network {
            data.gwrite_with(&network.gateway, &mut offset, IpAddress)?;
            data.gwrite_with(&network.subnet_mask, &mut offset, IpAddress)?;
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Iscsi;
    ///
    /// // `iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0,None,None,None,TCP)`
    /// // (UEFI 2.10 Section 10.3.4.20.2): portal group 1, LUN 0, no digests and no authentication.
    /// let node = Iscsi {
    ///     protocol: 0,
    ///     options: 1 << 11,
    ///     lun: [0; 8],
    ///     portal_group: 1,
    ///     target_name: "iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target".into(),
    /// };
    /// assert_eq!(
    ///     node.to_string(),
    ///     "iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)",
    /// );
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(
    ///     format!("{node:-}"),
    ///     "iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)",
    /// );
    /// assert_eq!(
    ///     format!("{node:#}"),
    ///     "iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)",
    /// );
    /// assert_eq!(
    ///     format!("{node:-#}"),
    ///     "iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)",
    /// );
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::Iscsi,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
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
impl fmt::Display for Iscsi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        let header = if node.options & (1 << 1) != 0 { "CRC32C" } else { "None" };
        let data = if node.options & (1 << 3) != 0 { "CRC32C" } else { "None" };
        let auth = if node.options & (1 << 11) != 0 {
            "None"
        } else if node.options & (1 << 12) != 0 {
            "CHAP_UNI"
        } else {
            "CHAP_BI"
        };
        write!(f, "iSCSI({},0x{:x},0x", TextString(&node.target_name), node.portal_group)?;
        for byte in &node.lun {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ",{header},{data},{auth},TCP)")
    }
}

impl Iscsi {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.protocol)
            + size_of_val(&self.options)
            + size_of_val(&self.lun)
            + size_of_val(&self.portal_group)
            + self.target_name.len()
            + size_of::<u8>() // NUL terminator
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.target_name.len() > 223 || self.protocol != 0 {
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Uri;
    ///
    /// // The boot file of an HTTP boot server.
    /// let node = Uri { uri: "http://192.168.0.1/boot/bootx64.efi".into() };
    /// assert_eq!(node.to_string(), "Uri(http://192.168.0.1/boot/bootx64.efi)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Uri(http://192.168.0.1/boot/bootx64.efi)");
    /// assert_eq!(format!("{node:#}"), "Uri(http://192.168.0.1/boot/bootx64.efi)");
    /// assert_eq!(format!("{node:-#}"), "Uri(http://192.168.0.1/boot/bootx64.efi)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::Uri,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Uri {
        /// URI text, including an empty URI when appropriate.
        #[scroll(ctx = Ascii)]
        pub uri: String,
    }
}
impl fmt::Display for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Uri({})", TextString(&node.uri))
    }
}

impl Uri {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        self.uri.len()
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if !self.uri.is_ascii() || self.uri.contains('\0') {
            return Err("URI must be ASCII without embedded NUL");
        }
        Ok(())
    }
}

device_path_node! {
    /// DNS device path node (UEFI 2.10 Table 10.48).
    ///
    /// One or more DNS server addresses, all IPv4 or all IPv6, each stored as a 16-byte
    /// `EFI_IP_ADDRESS`. An IPv4 address uses the first four bytes; the others are zero
    /// when written and ignored when read.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#dns-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use core::net::{IpAddr, Ipv4Addr};
    /// use patina::uefi::device_path::node::Dns;
    ///
    /// let node = Dns { addresses: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))] };
    /// assert_eq!(node.to_string(), "Dns(192.168.0.1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Dns(192.168.0.1)");
    /// assert_eq!(format!("{node:#}"), "Dns(192.168.0.1)");
    /// assert_eq!(format!("{node:-#}"), "Dns(192.168.0.1)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::Dns,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct Dns {
        /// One or more addresses of the same family.
        pub addresses: Vec<IpAddr>,
    }
}
impl fmt::Display for Dns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        f.write_str("Dns(")?;
        for (index, addr) in node.addresses.iter().enumerate() {
            if index != 0 {
                f.write_str(",")?;
            }
            write!(f, "{addr}")?;
        }
        f.write_str(")")
    }
}

impl Dns {
    /// Size of each `EFI_IP_ADDRESS` entry on the wire.
    const ADDRESS_SIZE: usize = 16;

    /// Return true if the addresses are IPv6, which is the node's `IsIPv6` field.
    pub fn is_ipv6(&self) -> bool {
        self.addresses.first().is_some_and(IpAddr::is_ipv6)
    }
}

impl Dns {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        1 + self.addresses.len() * Dns::ADDRESS_SIZE
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.addresses.is_empty() {
            return Err("DNS node requires at least one address");
        }
        if self.addresses.iter().any(|address| address.is_ipv6() != self.is_ipv6()) {
            return Err("DNS node addresses must all be IPv4 or all be IPv6");
        }
        Ok(())
    }
}

impl TryFromCtx<'_, scroll::Endian> for Dns {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let is_ipv6 = match data.gread_with::<u8>(&mut offset, scroll::LE)? {
            0 => false,
            1 => true,
            _ => return Err(scroll::Error::BadInput { size: 0, msg: "invalid DNS address family" }),
        };
        if !(data.len() - offset).is_multiple_of(Dns::ADDRESS_SIZE) {
            return Err(scroll::Error::BadInput { size: 0, msg: "invalid DNS address length" });
        }
        let mut addresses = Vec::new();
        while offset < data.len() {
            let address: Ipv6Addr = data.pread_with(offset, IpAddress)?;
            addresses.push(if is_ipv6 {
                IpAddr::V6(address)
            } else {
                let octets = address.octets();
                IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]))
            });
            offset += Dns::ADDRESS_SIZE;
        }
        Ok((Self { addresses }, offset))
    }
}

impl TryIntoCtx<scroll::Endian> for Dns {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        let mut offset = 0;
        data.gwrite_with(u8::from(self.is_ipv6()), &mut offset, scroll::LE)?;
        for address in &self.addresses {
            let mut entry = [0u8; Dns::ADDRESS_SIZE];
            match address {
                IpAddr::V4(address) => entry[..4].copy_from_slice(&address.octets()),
                IpAddr::V6(address) => entry = address.octets(),
            }
            data.gwrite_with(entry.as_slice(), &mut offset, ())?;
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
        /// Vendor GUID.
        guid: BinaryGuid,
        /// Opaque vendor-defined bytes.
        data: Vec<u8>,
    },
}

device_path_enum! {
    /// Access mode of a [`RestService`] node ([UEFI 2.10 Table 10.50](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#rest-service-device-path)).
    ///
    /// The device path text writes it as a number, such as `RestService(0x1,0x2)`.
    pub enum RestAccessMode: u8 {
        /// In-band access to the REST service.
        InBand = 1,
        /// Out-of-band access to the REST service.
        OutOfBand = 2,
    }
}

device_path_node! {
    /// REST service device path node (UEFI 2.10 Tables 10.50 and 10.51).
    ///
    /// A Redfish, `OData`, or vendor-specific REST service and its in-band or out-of-band
    /// access mode. The vendor-specific form adds a GUID and vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#rest-service-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{RestAccessMode, RestService, RestServiceType};
    ///
    /// // `RestService(1,2)`: an out-of-band Redfish service, from
    /// // `PciRoot(0x2)/Pci(0x2,0x0)/Pci(0x0,0x0)/MAC(FD19FA100672,0x0)/IPv4(...)/RestService(1,2)`
    /// // (UEFI 2.10 Section 10.3.4.31).
    /// let node = RestService { service: RestServiceType::Redfish, access_mode: RestAccessMode::OutOfBand };
    /// assert_eq!(node.to_string(), "RestService(0x1,0x2)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "RestService(0x1,0x2)");
    /// assert_eq!(format!("{node:#}"), "RestService(0x1,0x2)");
    /// assert_eq!(format!("{node:-#}"), "RestService(0x1,0x2)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::RestService,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq)]
    pub struct RestService {
        /// Standard service or vendor payload.
        pub service: RestServiceType,
        /// Access mode of the service.
        pub access_mode: RestAccessMode,
    }
}
impl fmt::Display for RestService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        match &node.service {
            RestServiceType::Redfish => write!(f, "RestService(0x1,0x{:x})", u8::from(node.access_mode)),
            RestServiceType::OData => write!(f, "RestService(0x2,0x{:x})", u8::from(node.access_mode)),
            RestServiceType::Vendor { guid, data } => {
                write!(f, "RestService(0xff,0x{:x},{}", u8::from(node.access_mode), guid)?;
                for byte in data {
                    write!(f, ",0x{byte:x}")?;
                }
                f.write_str(")")
            }
        }
    }
}

impl RestService {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        let vendor = match &self.service {
            RestServiceType::Vendor { guid, data } => size_of_val(guid) + size_of_val(data.as_slice()),
            _ => 0,
        };
        size_of::<u8>() // service type, encoded as one byte
            + size_of_val(&self.access_mode)
            + vendor
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
                let vendor_data = data
                    .get(offset..)
                    .ok_or(scroll::Error::BadInput { size: 0, msg: "missing REST vendor payload" })?
                    .to_vec();
                offset = data.len();
                RestServiceType::Vendor { guid, data: vendor_data }
            }
            _ => return Err(scroll::Error::BadInput { size: 0, msg: "unknown REST service type" }),
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
            data.gwrite_with(guid.as_bytes().as_slice(), &mut offset, ())?;
            data.gwrite_with(payload.as_slice(), &mut offset, ())?;
        }
        Ok(offset)
    }
}

device_path_enum! {
    /// Type of the namespace identifier of an [`NvmeOf`] node
    /// ([UEFI 2.10 Table 10.52](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvme-over-fabric-nvme-of-namespace-device-path)).
    pub enum NvmeOfIdentifierType: u8 {
        /// IEEE Extended Unique Identifier (EUI-64), in the first eight identifier bytes.
        Eui64 = 1,
        /// Namespace Globally Unique Identifier (NGUID).
        Nguid = 2,
        /// Namespace UUID.
        Uuid = 3,
    }
}

device_path_node! {
    /// `NVMe` over Fabrics namespace device path node (UEFI 2.10 Table 10.52).
    ///
    /// A bootable NVMe-oF namespace identified by its namespace identifier type and
    /// value and the NVM subsystem NQN.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvme-over-fabric-nvme-of-namespace-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{NvmeOf, NvmeOfIdentifierType};
    ///
    /// // `NVMEoF(nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target,urn:uuid:4eff7f8e-d353-4e9b-a4ec-deea8eab84d7)`
    /// // (UEFI 2.10 Section 10.3.4.33): the namespace identifier is an NSUUID (type 3), in big-endian order.
    /// let node = NvmeOf {
    ///     identifier_type: NvmeOfIdentifierType::Uuid,
    ///     identifier: [
    ///         0x4e, 0xff, 0x7f, 0x8e, 0xd3, 0x53, 0x4e, 0x9b, 0xa4, 0xec, 0xde, 0xea, 0x8e, 0xab, 0x84, 0xd7,
    ///     ],
    ///     subsystem_nqn: "nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target".into(),
    /// };
    /// assert_eq!(
    ///     node.to_string(),
    ///     "NVMEoF(nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target,urn:uuid:4eff7f8e-d353-4e9b-a4ec-deea8eab84d7)",
    /// );
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(
    ///     format!("{node:-}"),
    ///     "NVMEoF(nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target,urn:uuid:4eff7f8e-d353-4e9b-a4ec-deea8eab84d7)",
    /// );
    /// assert_eq!(
    ///     format!("{node:#}"),
    ///     "NVMEoF(nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target,urn:uuid:4eff7f8e-d353-4e9b-a4ec-deea8eab84d7)",
    /// );
    /// assert_eq!(
    ///     format!("{node:-#}"),
    ///     "NVMEoF(nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target,urn:uuid:4eff7f8e-d353-4e9b-a4ec-deea8eab84d7)",
    /// );
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::NvmeOf,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct NvmeOf {
        /// Type of the namespace identifier.
        pub identifier_type: NvmeOfIdentifierType,
        /// Identifier bytes; EUI-64 uses the first eight bytes, UUID uses RFC byte order.
        pub identifier: [u8; 16],
        /// UTF-8 subsystem NQN without its wire NUL terminator, at most 223 bytes.
        #[scroll(ctx = Utf8Nul)]
        pub subsystem_nqn: String,
    }
}
impl fmt::Display for NvmeOf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "NVMEoF({},", TextString(&node.subsystem_nqn))?;
        match node.identifier_type {
            NvmeOfIdentifierType::Eui64 => {
                f.write_str("eui.")?;
                for byte in &node.identifier[..8] {
                    write!(f, "{byte:02x}")?;
                }
            }
            NvmeOfIdentifierType::Nguid => {
                f.write_str("nguid.")?;
                for byte in &node.identifier {
                    write!(f, "{byte:02x}")?;
                }
            }
            NvmeOfIdentifierType::Uuid => {
                f.write_str("urn:uuid:")?;
                // The namespace UUID is written in byte order, not in the mixed-endian GUID layout.
                for (index, byte) in node.identifier.iter().enumerate() {
                    if matches!(index, 4 | 6 | 8 | 10) {
                        f.write_str("-")?;
                    }
                    write!(f, "{byte:02x}")?;
                }
            }
        }
        f.write_str(")")
    }
}

impl NvmeOf {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.identifier_type) + size_of_val(&self.identifier) + self.subsystem_nqn.len() + size_of::<u8>() // NUL terminator
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.subsystem_nqn.is_empty() || self.subsystem_nqn.len() > 223 {
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Sata;
    ///
    /// let node = Sata::new(0, 0xffff, 0);
    /// assert_eq!(node.to_string(), "Sata(0x0,0xffff,0x0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Sata(0x0,0xffff,0x0)");
    /// assert_eq!(format!("{node:#}"), "Sata(0x0,0xffff,0x0)");
    /// assert_eq!(format!("{node:-#}"), "Sata(0x0,0xffff,0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Messaging, MessagingSubType::Sata, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct Sata {
        /// HBA Port Number.
        pub hba_port: u16,
        /// Port Multiplier Port Number.
        pub port_multiplier_port: u16,
        /// Logical Unit Number.
        pub lun: u16,
    }
}
impl fmt::Display for Sata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Sata(0x{:x},0x{:x},0x{:x})", node.hba_port, node.port_multiplier_port, node.lun)
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

impl Sata {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::NvmExpress;
    ///
    /// let node = NvmExpress::new(1, 0x0011223344556677);
    /// assert_eq!(node.to_string(), "NVMe(0x1,00-11-22-33-44-55-66-77)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "NVMe(0x1,00-11-22-33-44-55-66-77)");
    /// assert_eq!(format!("{node:#}"), "NVMe(0x1,00-11-22-33-44-55-66-77)");
    /// assert_eq!(format!("{node:-#}"), "NVMe(0x1,00-11-22-33-44-55-66-77)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Messaging, MessagingSubType::NvmExpress,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct NvmExpress {
        /// Namespace Identifier (NSID).
        pub namespace_id: u32,
        /// IEEE Extended Unique Identifier (EUI-64).
        pub eui64: u64,
    }
}
impl fmt::Display for NvmExpress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "NVMe(0x{:x},", node.namespace_id)?;
        for (index, byte) in node.eui64.to_be_bytes().iter().enumerate() {
            if index != 0 {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
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

impl NvmExpress {
    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if matches!(self.namespace_id, 0 | u32::MAX) {
            return Err("NVMe namespace ID is reserved");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::{
        DevicePathNode, GenericDevicePathNode,
        tests::{check_fixture, check_invalid, check_node, check_rejected, encode_node, fixture},
    };
    use alloc::{format, string::String, vec};
    use scroll::{Pread, Pwrite, ctx::TryFromCtx};

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
        let display = format!("{sata}");
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
        let display = format!("{nvme}");
        assert_eq!(display, "NVMe(0x1,00-00-00-00-00-00-00-00)");
    }

    fixture!(
        test_device_path_atapi,
        Atapi { primary_secondary: AtapiChannel::Secondary, slave_master: AtapiDrive::Master, lun: 0x0403 },
        [3, 1, 8, 0, 1, 0, 3, 4]
    );

    fixture!(test_device_path_scsi, Scsi { pun: 0x0201, lun: 0x0403 }, [3, 2, 8, 0, 1, 2, 3, 4]);

    fixture!(
        test_device_path_fiber_channel,
        FiberChannel { reserved: 0, wwn: 0x0c0b0a0908070605, lun: 0x14131211100f0e0d },
        [3, 3, 24, 0, 0, 0, 0, 0, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_fiber_channel_ex,
        FiberChannelEx { reserved: 0, wwn: [5, 6, 7, 8, 9, 10, 11, 12], lun: [13, 14, 15, 16, 17, 18, 19, 20] },
        [3, 21, 24, 0, 0, 0, 0, 0, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_ieee1394,
        Ieee1394 { reserved: 0, guid: 0x0c0b0a0908070605 },
        [3, 4, 16, 0, 0, 0, 0, 0, 5, 6, 7, 8, 9, 10, 11, 12]
    );

    fixture!(test_device_path_usb, Usb { parent_port_number: 1, interface_number: 2 }, [3, 5, 6, 0, 1, 2]);

    fixture!(test_device_path_i2o, I2oRandomBlockStorageClass { tid: 0x04030201 }, [3, 6, 8, 0, 1, 2, 3, 4]);

    fixture!(
        test_device_path_mac_address,
        MacAddress {
            mac_address: efi::MacAddress {
                addr: [
                    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
                    28, 29, 30, 31, 32,
                ],
            },
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
            local_ip_address: Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
            remote_ip_address: Ipv6Addr::from([17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]),
            local_port: 0x2221,
            remote_port: 0x2423,
            protocol: IpProtocol(0x2625),
            ip_address_origin: Ipv6AddressOrigin::StatefulAutoConfigure,
            prefix_length: 128,
            gateway_ip_address: Ipv6Addr::from([41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56]),
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
            resource_flags: 0x1f,
            port_gid: [5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20],
            service_id: 0x1c1b1a1918171615,
            target_port_id: 0x24232221201f1e1d,
            device_id: 0x2c2b2a2928272625,
        },
        [
            3, 9, 48, 0, 0x1f, 0, 0, 0, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
            26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44,
        ]
    );

    fixture!(
        test_device_path_uart,
        Uart {
            reserved: 0,
            baud_rate: 0x0c0b0a0908070605,
            data_bits: 8,
            parity: UartParity::Space,
            stop_bits: UartStopBits::Two,
        },
        [3, 14, 19, 0, 0, 0, 0, 0, 5, 6, 7, 8, 9, 10, 11, 12, 8, 5, 3]
    );

    fixture!(
        test_device_path_sas_ex,
        SasEx {
            sas_address: [1, 2, 3, 4, 5, 6, 7, 8],
            lun: [9, 10, 11, 12, 13, 14, 15, 16],
            device_topology: SasTopology(0x1211),
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
        BluetoothLE { address: [1, 2, 3, 4, 5, 6], address_type: BluetoothLeAddressType::Random },
        [3, 30, 11, 0, 1, 2, 3, 4, 5, 6, 1]
    );

    fixture!(
        test_device_path_nvdimm,
        Nvdimm { uuid: crate::BinaryGuid::from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]) },
        [3, 32, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(test_device_path_device_logical_unit, DeviceLogicalUnit { lun: 7 }, [3, 17, 5, 0, 7]);

    fixture!(
        test_device_path_usb_class,
        UsbClass { vendor_id: 0x0201, product_id: 0x0403, device_class: 5, device_subclass: 6, device_protocol: 7 },
        [3, 15, 11, 0, 1, 2, 3, 4, 5, 6, 7]
    );

    #[test]
    fn test_device_path_atapi_selectors() {
        for channel in 0..=1 {
            for drive in 0..=1 {
                let primary_secondary = AtapiChannel::try_from(channel).unwrap();
                let slave_master = AtapiDrive::try_from(drive).unwrap();
                check_node(
                    Atapi { primary_secondary, slave_master, lun: u16::MAX },
                    &[3, 1, 8, 0, channel, drive, 0xff, 0xff],
                );
            }
        }
        for reserved in [2, u8::MAX] {
            check_rejected::<Atapi>(&[3, 1, 8, 0, reserved, 0, 0, 0]);
            check_rejected::<Atapi>(&[3, 1, 8, 0, 0, reserved, 0, 0]);
        }
    }

    #[test]
    fn test_device_path_vlan_range() {
        encode_node(Vlan { vlan_id: 0 }).unwrap();
        encode_node(Vlan { vlan_id: 4094 }).unwrap();
        check_invalid(Vlan { vlan_id: 4095 }, &[0xff, 0x0f]);
        check_invalid(Vlan { vlan_id: u16::MAX }, &[0xff, 0xff]);
    }

    #[test]
    fn test_device_path_ipv6_ranges() {
        let node = IpV6 {
            local_ip_address: Ipv6Addr::from([0; 16]),
            remote_ip_address: Ipv6Addr::from([0; 16]),
            local_port: 0,
            remote_port: 0,
            protocol: IpProtocol(0),
            ip_address_origin: Ipv6AddressOrigin::Static,
            prefix_length: 0,
            gateway_ip_address: Ipv6Addr::from([0; 16]),
        };
        for origin in 0..=2 {
            let ip_address_origin = Ipv6AddressOrigin::try_from(origin).unwrap();
            for prefix_length in [0, 64, 128] {
                encode_node(IpV6 { ip_address_origin, prefix_length, ..node.clone() }).unwrap();
            }
        }
        let wire = encode_node(node.clone()).unwrap();
        for reserved in [3, u8::MAX] {
            let mut reserved_origin = wire.clone();
            reserved_origin[42] = reserved;
            check_rejected::<IpV6>(&reserved_origin);
        }
        // The specification does not bound the prefix length.
        encode_node(IpV6 { prefix_length: u8::MAX, ..node.clone() }).unwrap();
    }

    #[test]
    fn test_device_path_uart_ranges() {
        let node = Uart {
            reserved: 0,
            baud_rate: 0,
            data_bits: 0,
            parity: UartParity::Default,
            stop_bits: UartStopBits::Default,
        };
        for parity in 0..=5 {
            for stop_bits in 0..=3 {
                let parity = UartParity::try_from(parity).unwrap();
                let stop_bits = UartStopBits::try_from(stop_bits).unwrap();
                encode_node(Uart { parity, stop_bits, ..node.clone() }).unwrap();
            }
        }
        let wire = encode_node(node).unwrap();
        for (offset, reserved) in [(17, 6), (17, u8::MAX), (18, 4), (18, u8::MAX)] {
            let mut reserved_value = wire.clone();
            reserved_value[offset] = reserved;
            check_rejected::<Uart>(&reserved_value);
        }
    }

    #[test]
    fn test_device_path_bluetooth_le_address_type() {
        for value in 0..=1 {
            let address_type = BluetoothLeAddressType::try_from(value).unwrap();
            check_node(BluetoothLE { address: [0; 6], address_type }, &[3, 30, 11, 0, 0, 0, 0, 0, 0, 0, value]);
        }
        for reserved in [2, u8::MAX] {
            check_rejected::<BluetoothLE>(&[3, 30, 11, 0, 0, 0, 0, 0, 0, 0, reserved]);
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
    fn test_device_path_opaque_arrays_ignore_endian_context_messaging() {
        let node = FiberChannelEx {
            reserved: 0x04030201,
            wwn: [5, 6, 7, 8, 9, 10, 11, 12],
            lun: [13, 14, 15, 16, 17, 18, 19, 20],
        };
        let mut output = [0; 20];
        output.pwrite_with(node, 0, scroll::BE).unwrap();
        assert_eq!(output, [4, 3, 2, 1, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
    }

    #[test]
    fn test_device_path_messaging_vendor() {
        let mut expected = vec![3, 10, 22, 0];
        expected.extend_from_slice(&[0x11; 16]);
        expected.extend_from_slice(&[0xaa, 0xbb]);
        check_node(
            MessagingVendor { guid: crate::BinaryGuid::from_bytes(&[0x11; 16]), data: vec![0xaa, 0xbb] },
            &expected,
        );
    }

    #[test]
    fn test_device_path_spec_vendor_messaging_nodes() {
        let mut expected = vec![3, 10, 24, 0];
        expected.extend_from_slice(UartFlowControl::GUID.as_bytes());
        expected.extend_from_slice(&[3, 0, 0, 0]);
        check_node(
            UartFlowControl { flow_control_map: UartFlowControl::HARDWARE | UartFlowControl::XON_XOFF },
            &expected,
        );
        check_invalid(
            UartFlowControl { flow_control_map: 4 },
            &[&UartFlowControl::GUID.as_bytes()[..], &[4, 0, 0, 0]].concat(),
        );

        let mut expected = vec![3, 10, 44, 0];
        expected.extend_from_slice(Sas::GUID.as_bytes());
        expected.extend_from_slice(&[
            0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 0x11, 0x12, 0x13, 0x14,
        ]);
        let sas = Sas {
            reserved: 0,
            sas_address: 0x0807060504030201,
            lun: 0x100f0e0d0c0b0a09,
            device_topology: SasTopology(0x1211),
            relative_target_port: 0x1413,
        };
        check_node(sas.clone(), &expected);
        let mut payload = expected[4..].to_vec();
        payload[16] = 1;
        check_invalid(Sas { reserved: 1, ..sas }, &payload);

        // A vendor node with another GUID stays a generic vendor node, and the specific
        // nodes reject it.
        let mut other = vec![3, 10, 24, 0];
        other.extend_from_slice(&[0x11; 16]);
        other.extend_from_slice(&[0; 4]);
        let raw = GenericDevicePathNode::from_bytes(&other).unwrap().0;
        assert!(format!("{:?}", <Box<dyn DevicePathNode>>::try_from(raw).unwrap()).starts_with("MessagingVendor"));
        assert!(UartFlowControl::try_from(raw).is_err());
        assert!(Sas::try_from(raw).is_err());
    }

    #[test]
    fn test_device_path_reserved_values_are_rejected_messaging() {
        let mut payload = vec![1, 0, 0, 0];
        payload.extend_from_slice(&[0; 16]);
        check_invalid(FiberChannel { reserved: 1, wwn: 0, lun: 0 }, &payload);
        check_invalid(FiberChannelEx { reserved: 1, wwn: [0; 8], lun: [0; 8] }, &payload);
        check_invalid(Ieee1394 { reserved: 1, guid: 0 }, &payload[..12]);
        check_invalid(
            Uart {
                reserved: 1,
                baud_rate: 0,
                data_bits: 0,
                parity: UartParity::Default,
                stop_bits: UartStopBits::Default,
            },
            &payload[..15],
        );
        let mut payload = vec![0x20, 0, 0, 0];
        payload.extend_from_slice(&[0; 40]);
        check_invalid(
            InfiniBand { resource_flags: 0x20, port_gid: [0; 16], service_id: 0, target_port_id: 0, device_id: 0 },
            &payload,
        );
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
            local_ip: Ipv4Addr::from([1, 2, 3, 4]),
            remote_ip: Ipv4Addr::from([5, 6, 7, 8]),
            local_port: 0x1234,
            remote_port: 0x5678,
            protocol: IpProtocol::TCP,
            static_ip: true,
            network: None,
        };
        let mut expected = vec![3, 12, 19, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0x34, 0x12, 0x78, 0x56, 6, 0, 1];
        check_node(node.clone(), &expected);
        node.network = Some(IpV4Network {
            gateway: Ipv4Addr::from([9, 10, 11, 12]),
            subnet_mask: Ipv4Addr::from([255, 255, 255, 0]),
        });
        expected[2] = 27;
        expected.extend_from_slice(&[9, 10, 11, 12, 255, 255, 255, 0]);
        check_node(node, &expected);
        for len in 16..23 {
            assert!(IpV4::try_from_ctx(&expected[4..4 + len], scroll::LE).is_err());
        }
        let mut invalid_origin = expected[4..].to_vec();
        invalid_origin[14] = 2;
        assert!(IpV4::try_from_ctx(&invalid_origin, scroll::LE).is_err());
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
        use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        let mut expected = vec![3, 31, 21, 0, 1];
        expected.extend_from_slice(&[0x12; 16]);
        let node = Dns { addresses: vec![IpAddr::V6(Ipv6Addr::from([0x12; 16]))] };
        assert!(node.is_ipv6());
        check_node(node, &expected);

        let mut expected = vec![3, 31, 37, 0, 0, 10, 0, 0, 1];
        expected.extend_from_slice(&[0; 12]);
        expected.extend_from_slice(&[10, 0, 0, 2]);
        expected.extend_from_slice(&[0; 12]);
        let node = Dns { addresses: vec![Ipv4Addr::new(10, 0, 0, 1).into(), Ipv4Addr::new(10, 0, 0, 2).into()] };
        assert!(!node.is_ipv6());
        check_node(node, &expected);

        // IPv4 padding is ignored when read.
        let mut padded = [0x12; 16];
        padded[..4].copy_from_slice(&[10, 0, 0, 1]);
        let mut payload = vec![0];
        payload.extend_from_slice(&padded);
        let (node, _) = Dns::try_from_ctx(&payload, scroll::LE).unwrap();
        assert_eq!(node.addresses, [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))]);

        payload[0] = 2;
        assert!(Dns::try_from_ctx(&payload, scroll::LE).is_err());
        let mixed = Dns { addresses: vec![Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()] };
        assert!(mixed.validate().is_err());
        assert!(encode_node(mixed).is_err());
    }

    #[test]
    fn test_device_path_rest_variants() {
        check_node(
            RestService { service: RestServiceType::Redfish, access_mode: RestAccessMode::InBand },
            &[3, 33, 6, 0, 1, 1],
        );
        check_node(
            RestService { service: RestServiceType::OData, access_mode: RestAccessMode::OutOfBand },
            &[3, 33, 6, 0, 2, 2],
        );
        let mut expected = vec![3, 33, 24, 0, 255, 1];
        expected.extend_from_slice(&[0x33; 16]);
        expected.extend_from_slice(&[0x44, 0x55]);
        check_node(
            RestService {
                service: RestServiceType::Vendor {
                    guid: crate::BinaryGuid::from_bytes(&[0x33; 16]),
                    data: vec![0x44, 0x55],
                },
                access_mode: RestAccessMode::InBand,
            },
            &expected,
        );
    }

    #[test]
    fn test_device_path_nvme_of() {
        let mut expected = vec![3, 34, 25, 0, 3];
        expected.extend_from_slice(&[0x22; 16]);
        expected.extend_from_slice(b"nqn\0");
        check_node(
            NvmeOf { identifier_type: NvmeOfIdentifierType::Uuid, identifier: [0x22; 16], subsystem_nqn: "nqn".into() },
            &expected,
        );
    }

    #[test]
    fn test_device_path_existing_wire_sizes_messaging() {
        check_node(Sata::new(1, 0xffff, 2), &[3, 18, 10, 0, 1, 0, 255, 255, 2, 0]);
        check_node(NvmExpress::new(1, 0x1234), &[3, 23, 16, 0, 1, 0, 0, 0, 0x34, 0x12, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_device_path_invalid_variable_payloads_messaging() {
        assert!(encode_node(Dns { addresses: vec![] }).is_err());
        assert!(
            encode_node(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0; 65] }).is_err()
        );
        assert!(
            encode_node(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0xd800] }).is_err()
        );
        // Reserved REST access modes and NVMe-oF identifier types cannot be decoded.
        check_rejected::<RestService>(&[3, 33, 6, 0, 1, 3]);
        assert!(encode_node(Uri { uri: "a\0b".into() }).is_err());
        assert!(encode_node(Uri { uri: "\u{e9}".into() }).is_err());
        assert!(encode_node(Uri { uri: "a".repeat(65532) }).is_err());
        assert!(
            encode_node(Iscsi { protocol: 1, options: 0, lun: [0; 8], portal_group: 0, target_name: "iqn".into() })
                .is_err()
        );
        let mut reserved_type = vec![3, 34, 25, 0, 0];
        reserved_type.extend_from_slice(&[0; 16]);
        reserved_type.extend_from_slice(b"nqn\0");
        check_rejected::<NvmeOf>(&reserved_type);
        // The specification does not require the unused EUI-64 bytes to be zero.
        encode_node(NvmeOf {
            identifier_type: NvmeOfIdentifierType::Eui64,
            identifier: [1; 16],
            subsystem_nqn: "nqn".into(),
        })
        .unwrap();
        // An empty iSCSI target name fits the variable-length field.
        encode_node(Iscsi { protocol: 0, options: 0, lun: [0; 8], portal_group: 0, target_name: String::new() })
            .unwrap();
        assert!(Dns::try_from_ctx(&[0; 16], scroll::LE).is_err());
        assert!(RestService::try_from_ctx(&[0xff, 1], scroll::LE).is_err());
        assert!(Uri::try_from_ctx(&[0xff], scroll::LE).is_err());
    }

    #[test]
    fn test_device_path_existing_validation_messaging() {
        assert!(encode_node(Sata::new(u16::MAX, 0, 0)).is_err());
        for id in [0, u32::MAX] {
            assert!(encode_node(NvmExpress::new(id, 0)).is_err());
        }
    }

    /// Check the text of each messaging node in the three text forms, and that the parseable forms
    /// parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_messaging() {
        use crate::uefi::device_path::node::tests::check_text;
        use alloc::format;

        check_text!(r"Ata(Primary,Master,0)", r"Ata(Primary,Master,0x0)", r"Ata(0x0)", r"Ata(Primary,Master,0x0)");
        check_text!(r"Scsi(1,2)", r"Scsi(0x1,0x2)", r"Scsi(0x1,0x2)", r"Scsi(0x1,0x2)");
        check_text!(
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)"
        );
        check_text!(
            @node FiberChannelEx { reserved: 0, wwn: [1, 2, 3, 4, 5, 6, 7, 8], lun: [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18] },
            r"FibreEx(0x0102030405060708,0x1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)"
        );
        check_text!(
            r"I1394(0x0123456789ABCDEF)",
            r"I1394(0x0123456789abcdef)",
            r"I1394(0x0123456789abcdef)",
            r"I1394(0x0123456789abcdef)"
        );
        check_text!(r"USB(1,0)", r"USB(0x1,0x0)", r"USB(0x1,0x0)", r"USB(0x1,0x0)");
        check_text!(r"I2O(5)", r"I2O(0x5)", r"I2O(0x5)", r"I2O(0x5)");
        check_text!(
            r"Infiniband(1,00112233-4455-6677-8899-AABBCCDDEEFF,1,2,3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)"
        );
        check_text!(
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)"
        );
        check_text!(r"VenPcAnsi()", r"VenPcAnsi()", r"VenPcAnsi()", r"VenMsg(E0C14753-F9BE-11D2-9A0C-0090273FC14D)");
        check_text!(r"VenVt100()", r"VenVt100()", r"VenVt100()", r"VenMsg(DFA66065-B419-11D3-9A2D-0090273FC14D)");
        check_text!(
            r"VenVt100Plus()",
            r"VenVt100Plus()",
            r"VenVt100Plus()",
            r"VenMsg(7BAEC70B-57E0-4C76-8E87-2F9E28088343)"
        );
        check_text!(r"VenUtf8()", r"VenUtf8()", r"VenUtf8()", r"VenMsg(AD15A0D6-8BEC-4ACF-A073-D01DE77E2D88)");
        check_text!(
            r"UartFlowCtrl(XonXoff)",
            r"UartFlowCtrl(XonXoff)",
            r"UartFlowCtrl(XonXoff)",
            r"VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,02000000)"
        );
        check_text!(
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,2)",
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,0x2)",
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,0x2)",
            r"VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,000000003412000000000000050000000000000062010600)"
        );
        check_text!(r"DebugPort()", r"DebugPort()", r"DebugPort()", r"VenMsg(EBA4E8D2-3858-41EC-A281-2647BA9660D0)");
        check_text!(
            r"MAC(001122334455,0x1)",
            r"MAC(001122334455,0x1)",
            r"MAC(001122334455,0x1)",
            r"MAC(001122334455,0x1)"
        );
        check_text!(
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,UDP,DHCP,0.0.0.0,0.0.0.0,0.0.0.0)",
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,UDP,DHCP,0.0.0.0,0.0.0.0,0.0.0.0)"
        );
        check_text!(
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)",
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)",
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)"
        );
        check_text!(
            r"IPv6(2001:db8::1)",
            r"IPv6(2001:db8::1,UDP,Static,::,0x0,::)",
            r"IPv6(2001:db8::1)",
            r"IPv6(2001:db8::1,UDP,Static,::,0x0,::)"
        );
        check_text!(
            @node IpV4 {
                local_ip: Ipv4Addr::new(192, 168, 0, 2),
                remote_ip: Ipv4Addr::new(192, 168, 0, 1),
                local_port: 8080,
                remote_port: 80,
                protocol: IpProtocol::TCP,
                static_ip: true,
                network: Some(IpV4Network { gateway: Ipv4Addr::UNSPECIFIED, subnet_mask: Ipv4Addr::UNSPECIFIED }),
            },
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080,0.0.0.0,0.0.0.0)",
            r"IPv4(192.168.0.1:80)",
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080,0.0.0.0,0.0.0.0)"
        );
        check_text!(
            @node IpV6 {
                local_ip_address: "2001:db8::2".parse().unwrap(),
                remote_ip_address: "2001:db8::1".parse().unwrap(),
                local_port: 8443,
                remote_port: 443,
                protocol: IpProtocol::TCP,
                ip_address_origin: Ipv6AddressOrigin::StatefulAutoConfigure,
                prefix_length: 64,
                gateway_ip_address: "2001:db8::ffff".parse().unwrap(),
            },
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,0x40,2001:db8::ffff)",
            r"IPv6([2001:db8::1]:443)",
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,0x40,2001:db8::ffff)"
        );
        check_text!(r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)");
        check_text!(
            r"UsbClass(0x1234,0x5678,0x8,0x6,0x50)",
            r"UsbMassStorage(0x1234,0x5678,0x6,0x50)",
            r"UsbMassStorage(0x1234,0x5678,0x6,0x50)",
            r"UsbClass(0x1234,0x5678,0x8,0x6,0x50)"
        );
        check_text!(
            r"UsbMassStorage(0xFFFF,0xFFFF,0x6,0x50)",
            r"UsbMassStorage(0xffff,0xffff,0x6,0x50)",
            r"UsbMassStorage(0xffff,0xffff,0x6,0x50)",
            r"UsbClass(0xffff,0xffff,0x8,0x6,0x50)"
        );
        check_text!(
            r#"UsbWwid(0x1234,0x5678,0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#
        );
        check_text!(r"Unit(1)", r"Unit(0x1)", r"Unit(0x1)", r"Unit(0x1)");
        check_text!(r"Sata(0,0xFFFF,0)", r"Sata(0x0,0xffff,0x0)", r"Sata(0x0,0xffff,0x0)", r"Sata(0x0,0xffff,0x0)");
        check_text!(
            r"iSCSI(iqn.x,1,0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            no_parse_back
        );
        check_text!(r"Vlan(100)", r"Vlan(100)", r"Vlan(100)", r"Vlan(100)");
        check_text!(
            r"SasEx(0102030405060708,1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            no_parse_back
        );
        check_text!(
            r"SasEx(0102030405060708,1112131415161718,0x6)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            no_parse_back
        );
        check_text!(
            r"SasEx(0102030405060708,1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            no_parse_back
        );
        check_text!(
            r"SAS(0x1234,0,0,SATA,Internal,Direct,256)",
            r"SAS(0x1234,0x0,0x0,SATA,Internal,Direct,0x100)",
            r"SAS(0x1234,0x0,0x0,SATA,Internal,Direct,0x100)",
            r"VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,000000003412000000000000000000000000000012ff0000)"
        );
        check_text!(
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)"
        );
        check_text!(r"Uri(http://a/b)", r"Uri(http://a/b)", r"Uri(http://a/b)", r"Uri(http://a/b)");
        check_text!(r"UFS(0,0x3)", r"UFS(0x0,0x3)", r"UFS(0x0,0x3)", r"UFS(0x0,0x3)");
        check_text!(r"SD(1)", r"SD(0x1)", r"SD(0x1)", r"SD(0x1)");
        check_text!(
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)"
        );
        check_text!(r"Wi-Fi(ssid)", r"Wi-Fi(ssid)", r"Wi-Fi(ssid)", r"Wi-Fi(ssid)");
        check_text!(r"eMMC(0)", r"eMMC(0x0)", r"eMMC(0x0)", r"eMMC(0x0)");
        check_text!(
            r"BluetoothLE(001122334455,0x1)",
            r"BluetoothLE(001122334455,0x01)",
            r"BluetoothLE(001122334455,0x01)",
            r"BluetoothLE(001122334455,0x01)"
        );
        check_text!(
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)"
        );
        check_text!(r"RestService(1,1)", r"RestService(0x1,0x1)", r"RestService(0x1,0x1)", r"RestService(0x1,0x1)");
        check_text!(r"RestService(2,2)", r"RestService(0x2,0x2)", r"RestService(0x2,0x2)", r"RestService(0x2,0x2)");
        check_text!(
            r"RestService(0xff,1,00112233-4455-6677-8899-AABBCCDDEEFF,0xaa,0x55)",
            r"RestService(0xff,0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0xaa,0x55)",
            r"RestService(0xff,0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0xaa,0x55)",
            r"RestService(0xff,0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0xaa,0x55)"
        );
        check_text!(
            r"NVMEoF(nqn.x,eui.0011223344556677)",
            r"NVMEoF(nqn.x,eui.0011223344556677)",
            r"NVMEoF(nqn.x,eui.0011223344556677)",
            r"NVMEoF(nqn.x,eui.0011223344556677)"
        );
        check_text!(
            r"NVMEoF(nqn.x,nguid.00112233445566778899aabbccddeeff)",
            r"NVMEoF(nqn.x,nguid.00112233445566778899aabbccddeeff)",
            r"NVMEoF(nqn.x,nguid.00112233445566778899aabbccddeeff)",
            r"NVMEoF(nqn.x,nguid.00112233445566778899aabbccddeeff)"
        );
        check_text!(
            r"iSCSI(iqn.x,1,0001020304050607,None,None,CHAP_UNI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,None,None,CHAP_UNI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,None,None,CHAP_UNI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,None,None,CHAP_UNI,TCP)",
            no_parse_back
        );
        check_text!(
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)"
        );

        let uart = UartFlowControl { flow_control_map: UartFlowControl::HARDWARE };
        assert_eq!(format!("{uart}"), "UartFlowCtrl(Hardware)");
        assert_eq!(format!("{uart:#}"), "VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,01000000)");
    }

    /// UEFI text forms for values the shortcut keywords cannot hold.
    #[test]
    fn test_device_path_messaging_text_edge_values() {
        // Zero, the device default, is written as a number.
        let uart = Uart {
            reserved: 0,
            baud_rate: 0,
            data_bits: 0,
            parity: UartParity::Default,
            stop_bits: UartStopBits::Default,
        };
        assert_eq!(format!("{uart}"), "Uart(0,0,D,D)");
        // Both flow control bits set has no `UartFlowCtrl` keyword, so the vendor form is used.
        let flow = UartFlowControl { flow_control_map: 3 };
        assert_eq!(format!("{flow}"), "VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,03000000)");
        // A string with `=` is quoted, so it is not read as a named parameter.
        assert_eq!(format!("{}", Uri { uri: "http://a/b?c=d".into() }), r#"Uri("http://a/b?c=d")"#);
    }

    /// Each USB class with a shortcut name is written with it, and the classes that fix the
    /// subclass leave the subclass out (UEFI 2.10 Table 10.67).
    #[test]
    fn test_device_path_usb_class_shortcuts() {
        let shortcuts = [
            (1, 0x10, "UsbAudio", false),
            (2, 0x10, "UsbCDCControl", false),
            (3, 0x10, "UsbHID", false),
            (6, 0x10, "UsbImage", false),
            (7, 0x10, "UsbPrinter", false),
            (8, 0x10, "UsbMassStorage", false),
            (9, 0x10, "UsbHub", false),
            (10, 0x10, "UsbCDCData", false),
            (11, 0x10, "UsbSmartCard", false),
            (14, 0x10, "UsbVideo", false),
            (220, 0x10, "UsbDiagnostic", false),
            (224, 0x10, "UsbWireless", false),
            (254, 1, "UsbDeviceFirmwareUpdate", true),
            (254, 2, "UsbIrdaBridge", true),
            (254, 3, "UsbTestAndMeasurement", true),
        ];
        for (class, subclass, name, fixed_subclass) in shortcuts {
            let node = UsbClass {
                vendor_id: 0x1234,
                product_id: 0x5678,
                device_class: class,
                device_subclass: subclass,
                device_protocol: 0x20,
            };
            let expected = if fixed_subclass {
                format!("{name}(0x1234,0x5678,0x20)")
            } else {
                format!("{name}(0x1234,0x5678,0x{subclass:x},0x20)")
            };
            assert_eq!(format!("{node}"), expected);
            assert_eq!(format!("{node:-}"), expected);
            assert_eq!(format!("{node:#}"), format!("UsbClass(0x1234,0x5678,0x{class:x},0x{subclass:x},0x20)"));
        }

        // A class without a shortcut, or a fixed-subclass class with another subclass, has no name.
        for (class, subclass) in [(4, 0x10), (254, 4)] {
            let node = UsbClass {
                vendor_id: 0x1234,
                product_id: 0x5678,
                device_class: class,
                device_subclass: subclass,
                device_protocol: 0x20,
            };
            assert_eq!(format!("{node}"), format!("UsbClass(0x1234,0x5678,0x{class:x},0x{subclass:x},0x20)"));
        }
    }

    /// The subsystem NQN of an NVMe-oF node holds 1 to 223 characters.
    #[test]
    fn test_device_path_nvme_of_nqn_length() {
        for length in [0, 224] {
            let nqn = "a".repeat(length);
            let mut payload = vec![3];
            payload.extend_from_slice(&[0x22; 16]);
            payload.extend_from_slice(nqn.as_bytes());
            payload.push(0);
            check_invalid(
                NvmeOf { identifier_type: NvmeOfIdentifierType::Uuid, identifier: [0x22; 16], subsystem_nqn: nqn },
                &payload,
            );
        }
        let nqn = "a".repeat(223);
        let node = NvmeOf { identifier_type: NvmeOfIdentifierType::Uuid, identifier: [0x22; 16], subsystem_nqn: nqn };
        assert!(node.validate().is_ok());
    }

    /// A nonzero reserved SAS field is written as the eighth argument, after the empty topology
    /// arguments (UEFI 2.10 Table 10.67).
    #[test]
    fn test_device_path_sas_text_reserved() {
        for (topology, text) in
            [(0, "NoTopology,,,,0x5"), (0x0001, "SAS,Internal,Direct,,0x5"), (0x0062, "SAS,External,Expanded,0x1,0x5")]
        {
            let node = Sas {
                reserved: 5,
                sas_address: 0x1234,
                lun: 0,
                device_topology: SasTopology(topology),
                relative_target_port: 0,
            };
            assert_eq!(format!("{node}"), format!("SAS(0x1234,0x0,0x0,{text})"));
        }
    }
}
