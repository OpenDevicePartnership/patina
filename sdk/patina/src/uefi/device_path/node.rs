//! UEFI device path node definitions, grouped by category (UEFI 2.10 Section 10.3).
//!
//! A device path is a series of variable-length nodes. Each node starts at the end
//! of the previous one, so nodes are byte-packed and their fields are unaligned.
//! Every node has a length field in a known place, which makes it possible to skip
//! nodes of an unknown type. The six node types are hardware, ACPI, messaging,
//! media, BIOS Boot Specification, and end of hardware device path.
//!
//! Every node starts with the same 4-byte [`Header`]: a type, a sub-type whose
//! meaning depends on the type, and the length of the node in bytes, including the
//! header. The specific device path data follows. [`GenericDevicePathNode`] holds a
//! node with its data kept as raw bytes (Section 10.3.1, Table 10.1).
//!
//! [`Header`]: crate::uefi::device_path::node::Header
//! [`GenericDevicePathNode`]: crate::uefi::device_path::node::GenericDevicePathNode
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-path-nodes>
//!
//! Binary conversions use `DevicePathNode` and the `scroll` conversion traits.
//! Diagnostic display is not a complete canonical UEFI text conversion API.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub mod acpi;
pub mod bios;
pub mod end;
pub mod hardware;
pub mod media;
pub mod messaging;
pub mod node_types;

pub use acpi::*;
pub use bios::*;
pub use end::*;
pub use hardware::*;
pub use media::*;
pub use messaging::*;
pub use node_types::*;

use super::encoding::invalid;
use crate::uefi::device_path::parse_node::{DevicePathNode, MeasureSize, Validate, decode_payload};
use alloc::boxed::Box;
use core::fmt::{Display, Write};
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

#[cfg(test)]
use alloc::{string::String, vec::Vec};

#[cfg(target_endian = "big")]
compile_error!("UEFI device path node headers use native u16 fields and require a little-endian target");

/// Header common to every device path node (UEFI 2.10 Table 10.1).
///
/// The type, the sub-type whose meaning depends on the type, and the node length.
/// The UEFI wire format stores the length as little-endian; this in-memory type
/// uses a native `u16` and therefore assumes a little-endian target.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
pub struct Header {
    /// Type of the device path node.
    pub r#type: u8,
    /// Subtype of the device path node.
    pub sub_type: u8,
    /// Total length in bytes of the device path node, including the header.
    pub length: u16,
}

impl Header {
    /// Creates a new [`Header`].
    pub const fn new(r#type: u8, sub_type: u8, length: u16) -> Self {
        Self { r#type, sub_type, length }
    }

    /// Return the size of the header in bytes
    pub const fn size_of_header() -> usize {
        core::mem::size_of::<Self>()
    }
}

impl TryIntoCtx<scroll::Endian> for Header {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        if usize::from(self.length) < Self::size_of_header() {
            return Err(scroll::Error::BadInput {
                size: usize::from(self.length),
                msg: "device path node is shorter than its header",
            });
        }
        let mut offset = 0;
        dest.gwrite_with(self.r#type, &mut offset, ctx)?;
        dest.gwrite_with(self.sub_type, &mut offset, ctx)?;
        dest.gwrite_with(self.length, &mut offset, ctx)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for Header {
    type Error = scroll::Error;

    fn try_from_ctx(from: &[u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        Ok((
            Header {
                r#type: from.gread_with(&mut offset, ctx)?,
                sub_type: from.gread_with(&mut offset, ctx)?,
                length: from.gread_with::<u16>(&mut offset, ctx)?,
            },
            offset,
        ))
    }
}

/// Generic device path node (UEFI 2.10 Table 10.1).
///
/// A node whose payload is kept as raw bytes: the specific device path data that its
/// type and sub-type define. It represents any node, including nodes that are not
/// decoded into a specific type or whose type is not defined by the specification.
#[repr(C)]
#[derive(Debug, Eq, PartialEq, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
pub struct GenericDevicePathNode {
    /// Header of the device path node.
    pub header: Header,
    /// Data of the device path node, could be anything depending on the header.
    pub data: [u8],
}

impl GenericDevicePathNode {
    /// Borrows the first device path node from `bytes` and returns the remaining bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer is shorter than a header, the header length is
    /// shorter than the header or longer than the buffer, or an end node has an
    /// invalid subtype or length.
    pub fn from_bytes(bytes: &[u8]) -> Result<(&Self, &[u8]), scroll::Error> {
        let header = bytes.pread_with::<Header>(0, scroll::LE)?;
        let length = usize::from(header.length);
        if length < Header::size_of_header() {
            return Err(scroll::Error::BadInput { size: length, msg: "device path node is shorter than its header" });
        }
        if length > bytes.len() {
            return Err(scroll::Error::TooBig { size: length, len: bytes.len() });
        }
        if header.r#type == 0x7f && (length != Header::size_of_header() || !matches!(header.sub_type, 1 | 0xff)) {
            return Err(scroll::Error::BadInput { size: length, msg: "invalid end device path node" });
        }
        <Self as zerocopy::FromBytes>::ref_from_prefix_with_elems(bytes, length - Header::size_of_header())
            .map_err(|_| scroll::Error::BadInput { size: length, msg: "invalid device path node layout" })
    }

    /// Returns this node's exact in-memory bytes.
    pub fn as_bytes(&self) -> &[u8] {
        <Self as zerocopy::IntoBytes>::as_bytes(self)
    }

    /// Decode the node into a dyn `DevicePathNode` of the type its header identifies.
    pub fn cast_to_dyn_device_path_node<'a>(&'a self) -> Box<dyn DevicePathNode + 'a> {
        crate::uefi::device_path::node::cast_to_dyn_device_path_node(self)
    }
}

impl MeasureSize for &GenericDevicePathNode {
    fn measure_size(&self) -> usize {
        self.data.len()
    }
}

impl Validate for &GenericDevicePathNode {
    /// Generic payloads are not interpreted, so there are no field rules to check.
    fn validate(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

impl DevicePathNode for &GenericDevicePathNode {
    /// Generic nodes have no fixed type. This placeholder is unused because
    /// `header()` and `is_type()` are overridden.
    fn r#type() -> u8 {
        0
    }

    /// Generic nodes have no fixed sub-type; see `r#type()`.
    fn sub_type() -> u8 {
        0
    }

    fn header(&self) -> Header {
        self.header
    }

    fn is_type(_type: u8, _sub_type: u8) -> bool {
        // A generic node can represent every type, so it always returns true.
        true
    }

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error>
    where
        Self: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        let mut offset = 0;
        buffer.gwrite_with(self, &mut offset, Endian::Little)?;
        Ok(offset)
    }

    fn read_from<'b>(buffer: &'b [u8]) -> Result<(Self, usize), scroll::Error>
    where
        Self: TryFromCtx<'b, scroll::Endian, Error = scroll::Error>,
    {
        Self::try_from_ctx(buffer, scroll::LE)
    }
}

impl TryIntoCtx<scroll::Endian> for &GenericDevicePathNode {
    type Error = scroll::Error;

    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let length = usize::from(self.header.length);
        if length != Header::size_of_header() + self.data.len() {
            return Err(scroll::Error::BadInput { size: length, msg: "inconsistent device path node length" });
        }
        if self.header.r#type == 0x7f
            && (length != Header::size_of_header() || !matches!(self.header.sub_type, 1 | 0xff))
        {
            return Err(scroll::Error::BadInput { size: length, msg: "invalid end device path node" });
        }
        let mut offset = 0;
        dest.gwrite_with(self.header, &mut offset, scroll::Endian::Little)?;
        dest.gwrite_with(&self.data, &mut offset, ())?;
        Ok(offset)
    }
}

impl<'a> TryFromCtx<'a, scroll::Endian> for &'a GenericDevicePathNode {
    type Error = scroll::Error;

    fn try_from_ctx(from: &'a [u8], _ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let (node, _) = GenericDevicePathNode::from_bytes(from)?;
        Ok((node, usize::from(node.header.length)))
    }
}

impl Display for GenericDevicePathNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("Path({}, {},", &self.header.r#type, &self.header.sub_type))?;
        for b in &self.data {
            f.write_fmt(format_args!(" {b:02X}"))?;
        }
        f.write_char(')')
    }
}

fn check_length(size: usize) -> Result<(), scroll::Error> {
    if size > usize::from(u16::MAX) - 4 {
        return Err(invalid("device path payload exceeds the 16-bit node length"));
    }
    Ok(())
}

/// Decode a node using its type and subtype, preserving unsupported nodes.
///
/// Malformed recognized nodes are logged and preserved for diagnostic display.
/// Use [`try_cast_to_dyn_device_path_node`] when decoding errors must be propagated.
pub fn cast_to_dyn_device_path_node(unknown: &GenericDevicePathNode) -> Box<dyn DevicePathNode + '_> {
    match try_cast_to_dyn_device_path_node(unknown) {
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
    unknown: &GenericDevicePathNode,
) -> Result<Box<dyn DevicePathNode + '_>, scroll::Error> {
    fn decode_node<'a, T>(data: &'a [u8]) -> Result<Box<dyn DevicePathNode + 'a>, scroll::Error>
    where
        T: DevicePathNode + TryFromCtx<'a, scroll::Endian, Error = scroll::Error> + 'a,
    {
        Ok(Box::new(decode_payload::<T>(data)?))
    }

    if usize::from(unknown.header.length) != Header::size_of_header() + unknown.data.len() {
        return Err(invalid("device path header and payload lengths differ"));
    }
    check_length(unknown.data.len())?;
    if unknown.header.r#type == 0x7f
        && (usize::from(unknown.header.length) != Header::size_of_header()
            || !matches!(unknown.header.sub_type, 1 | 0xff))
    {
        return Err(invalid("invalid end device path node"));
    }

    match unknown.header {
        // Hardware nodes.
        h if Pci::is_type(h.r#type, h.sub_type) => decode_node::<Pci>(&unknown.data),
        h if PcCard::is_type(h.r#type, h.sub_type) => decode_node::<PcCard>(&unknown.data),
        h if MemoryMapped::is_type(h.r#type, h.sub_type) => decode_node::<MemoryMapped>(&unknown.data),
        h if Controller::is_type(h.r#type, h.sub_type) => decode_node::<Controller>(&unknown.data),
        h if Bmc::is_type(h.r#type, h.sub_type) => decode_node::<Bmc>(&unknown.data),
        h if HardwareVendor::is_type(h.r#type, h.sub_type) => decode_node::<HardwareVendor>(&unknown.data),
        // ACPI nodes.
        h if Acpi::is_type(h.r#type, h.sub_type) => decode_node::<Acpi>(&unknown.data),
        h if ExtendedAcpi::is_type(h.r#type, h.sub_type) => decode_node::<ExtendedAcpi>(&unknown.data),
        h if AcpiAdr::is_type(h.r#type, h.sub_type) => decode_node::<AcpiAdr>(&unknown.data),
        h if NvdimmAcpi::is_type(h.r#type, h.sub_type) => decode_node::<NvdimmAcpi>(&unknown.data),
        // Messaging nodes.
        h if NvmExpress::is_type(h.r#type, h.sub_type) => decode_node::<NvmExpress>(&unknown.data),
        h if Sata::is_type(h.r#type, h.sub_type) => decode_node::<Sata>(&unknown.data),
        h if Atapi::is_type(h.r#type, h.sub_type) => decode_node::<Atapi>(&unknown.data),
        h if Scsi::is_type(h.r#type, h.sub_type) => decode_node::<Scsi>(&unknown.data),
        h if FiberChannel::is_type(h.r#type, h.sub_type) => decode_node::<FiberChannel>(&unknown.data),
        h if FiberChannelEx::is_type(h.r#type, h.sub_type) => decode_node::<FiberChannelEx>(&unknown.data),
        h if Ieee1394::is_type(h.r#type, h.sub_type) => decode_node::<Ieee1394>(&unknown.data),
        h if Usb::is_type(h.r#type, h.sub_type) => decode_node::<Usb>(&unknown.data),
        h if UsbWwid::is_type(h.r#type, h.sub_type) => decode_node::<UsbWwid>(&unknown.data),
        h if UsbClass::is_type(h.r#type, h.sub_type) => decode_node::<UsbClass>(&unknown.data),
        h if DeviceLogicalUnit::is_type(h.r#type, h.sub_type) => decode_node::<DeviceLogicalUnit>(&unknown.data),
        h if I2oRandomBlockStorageClass::is_type(h.r#type, h.sub_type) => {
            decode_node::<I2oRandomBlockStorageClass>(&unknown.data)
        }
        h if MacAddress::is_type(h.r#type, h.sub_type) => decode_node::<MacAddress>(&unknown.data),
        h if IpV4::is_type(h.r#type, h.sub_type) => decode_node::<IpV4>(&unknown.data),
        h if IpV6::is_type(h.r#type, h.sub_type) => decode_node::<IpV6>(&unknown.data),
        h if Vlan::is_type(h.r#type, h.sub_type) => decode_node::<Vlan>(&unknown.data),
        h if InfiniBand::is_type(h.r#type, h.sub_type) => decode_node::<InfiniBand>(&unknown.data),
        h if Uart::is_type(h.r#type, h.sub_type) => decode_node::<Uart>(&unknown.data),
        h if MessagingVendor::is_type(h.r#type, h.sub_type) => decode_node::<MessagingVendor>(&unknown.data),
        h if SasEx::is_type(h.r#type, h.sub_type) => decode_node::<SasEx>(&unknown.data),
        h if Iscsi::is_type(h.r#type, h.sub_type) => decode_node::<Iscsi>(&unknown.data),
        h if Uri::is_type(h.r#type, h.sub_type) => decode_node::<Uri>(&unknown.data),
        h if Ufs::is_type(h.r#type, h.sub_type) => decode_node::<Ufs>(&unknown.data),
        h if Sd::is_type(h.r#type, h.sub_type) => decode_node::<Sd>(&unknown.data),
        h if Bluetooth::is_type(h.r#type, h.sub_type) => decode_node::<Bluetooth>(&unknown.data),
        h if WiFi::is_type(h.r#type, h.sub_type) => decode_node::<WiFi>(&unknown.data),
        h if Emmc::is_type(h.r#type, h.sub_type) => decode_node::<Emmc>(&unknown.data),
        h if BluetoothLE::is_type(h.r#type, h.sub_type) => decode_node::<BluetoothLE>(&unknown.data),
        h if Dns::is_type(h.r#type, h.sub_type) => decode_node::<Dns>(&unknown.data),
        h if Nvdimm::is_type(h.r#type, h.sub_type) => decode_node::<Nvdimm>(&unknown.data),
        h if RestService::is_type(h.r#type, h.sub_type) => decode_node::<RestService>(&unknown.data),
        h if NvmeOf::is_type(h.r#type, h.sub_type) => decode_node::<NvmeOf>(&unknown.data),
        // Media nodes.
        h if FilePath::is_type(h.r#type, h.sub_type) => decode_node::<FilePath>(&unknown.data),
        h if HardDrive::is_type(h.r#type, h.sub_type) => decode_node::<HardDrive>(&unknown.data),
        h if CdRom::is_type(h.r#type, h.sub_type) => decode_node::<CdRom>(&unknown.data),
        h if MediaVendor::is_type(h.r#type, h.sub_type) => decode_node::<MediaVendor>(&unknown.data),
        h if MediaProtocol::is_type(h.r#type, h.sub_type) => decode_node::<MediaProtocol>(&unknown.data),
        h if PiwgFirmwareFile::is_type(h.r#type, h.sub_type) => decode_node::<PiwgFirmwareFile>(&unknown.data),
        h if PiwgFirmwareVolume::is_type(h.r#type, h.sub_type) => decode_node::<PiwgFirmwareVolume>(&unknown.data),
        h if RelativeOffsetRange::is_type(h.r#type, h.sub_type) => decode_node::<RelativeOffsetRange>(&unknown.data),
        h if RamDisk::is_type(h.r#type, h.sub_type) => decode_node::<RamDisk>(&unknown.data),
        // BIOS nodes.
        h if Bios::is_type(h.r#type, h.sub_type) => decode_node::<Bios>(&unknown.data),
        // End nodes.
        h if EndEntire::is_type(h.r#type, h.sub_type) => decode_node::<EndEntire>(&unknown.data),
        h if EndInstance::is_type(h.r#type, h.sub_type) => decode_node::<EndInstance>(&unknown.data),
        _ => Ok(Box::new(unknown)),
    }
}

#[cfg(test)]
fn encode_node<T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>>(
    node: T,
) -> Result<Vec<u8>, scroll::Error> {
    let mut bytes = alloc::vec![0; usize::from(node.header().length)];
    node.write_into(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
fn generic_node_bytes(header: Header, data: &[u8]) -> Vec<u8> {
    let mut bytes = alloc::vec![header.r#type, header.sub_type];
    bytes.extend_from_slice(&header.length.to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}

#[cfg(test)]
fn raw_generic_node(bytes: &[u8]) -> &GenericDevicePathNode {
    <GenericDevicePathNode as zerocopy::FromBytes>::ref_from_bytes_with_elems(
        bytes,
        bytes.len() - Header::size_of_header(),
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::uefi::device_path::parse_node::Validate;
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
        // Payload is 4 + 8 + 8 + 16 + 1 + 1 = 38 bytes (no header in TryIntoCtx).
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
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Media as u8, MediaSubType::HardDrive as u8, 42), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        let parsed = HardDrive::try_from_node(node).expect("valid HD node should parse");
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
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Messaging as u8, MediaSubType::HardDrive as u8, 42), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(HardDrive::try_from_node(node).is_none());
    }

    #[test]
    fn test_hard_drive_validate() {
        let mbr = |partition_number, signature: [u8; 16]| HardDrive {
            partition_number,
            partition_start: 0,
            partition_size: 0,
            partition_signature: signature,
            partition_format: HardDrive::FORMAT_MBR,
            signature_type: HardDrive::SIGNATURE_TYPE_MBR,
        };
        let mut signature = [0; 16];
        signature[..4].copy_from_slice(&[1, 2, 3, 4]);
        for partition_number in 0..=4 {
            assert!(mbr(partition_number, signature).validate().is_ok());
        }
        assert!(mbr(5, signature).validate().is_err());
        assert!(mbr(1, [1; 16]).validate().is_err());
        assert!(HardDrive { partition_format: 3, ..mbr(1, signature) }.validate().is_err());
        assert!(HardDrive { signature_type: 3, ..HardDrive::new_gpt(1, 0, 0, [1; 16]) }.validate().is_err());
        assert!(HardDrive { signature_type: 0, ..HardDrive::new_gpt(1, 0, 0, [1; 16]) }.validate().is_err());
        assert!(HardDrive::new_gpt(128, 0, 0, [1; 16]).validate().is_ok());

        let mut data = [0u8; 38];
        data.pwrite_with(mbr(5, signature), 0, scroll::LE).unwrap();
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Media as u8, MediaSubType::HardDrive as u8, 42), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(HardDrive::try_from_node(node).is_none());
        assert!(try_cast_to_dyn_device_path_node(node).is_err());
    }

    #[test]
    fn test_hard_drive_try_from_node_short_data() {
        let data = [0u8; 16]; // shorter than the 38-byte HardDrive payload
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Media as u8, MediaSubType::HardDrive as u8, 20), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(HardDrive::try_from_node(node).is_none());
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
    use crate::uefi::device_path::{node::try_cast_to_dyn_device_path_node, parse_node::DevicePathNode};
    use alloc::{format, vec};
    use scroll::ctx::{TryFromCtx, TryIntoCtx};

    fn check_fixture<T>(node: T, wire: &[u8])
    where
        T: DevicePathNode + Clone + Eq,
        T: for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        encode_node(node.clone()).unwrap();
        assert_eq!(usize::from(node.header().length), wire.len());
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

        let unknown: &GenericDevicePathNode = wire.pread_with(0, scroll::LE).unwrap();
        assert_eq!(unknown.header, node.header());
        let mut trailing = wire.to_vec();
        trailing.extend_from_slice(&[0x7f, 0xff, 4, 0]);
        assert_eq!(T::read_from(&trailing).unwrap(), (node.clone(), wire.len()));
        assert_eq!(<&GenericDevicePathNode>::read_from(wire).unwrap(), (unknown, wire.len()));
        let mut mismatched = wire.to_vec();
        mismatched[1] ^= 0x80;
        assert!(T::read_from(&mismatched).is_err());
        let dynamic = try_cast_to_dyn_device_path_node(unknown).unwrap();
        let type_name = core::any::type_name::<T>().rsplit("::").next().unwrap();
        let debug = format!("{dynamic:?}");
        assert!(debug.starts_with(&format!("{type_name} {{")), "unexpected decoded type: {debug}");
        assert_eq!(debug, format!("{node:?}"));
        assert!(format!("{node}").starts_with(&format!("{type_name}(")));
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
                wire[..length].pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err(),
                "accepted truncated node at {length}"
            );
            assert!(T::read_from(&wire[..length]).is_err(), "read truncated node at {length}");
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
        assert!(encode_node(node.clone()).is_err());
        let mut output = vec![0xa5; usize::from(node.header().length)];
        assert!(node.clone().write_into(&mut output).is_err());
        assert!(output.iter().all(|byte| *byte == 0xa5));

        assert_eq!(payload.len() + 4, usize::from(node.header().length));
        let mut wire = vec![0; 4];
        wire.pwrite_with(node.header(), 0, scroll::LE).unwrap();
        wire.extend_from_slice(payload);
        assert!(T::read_from(&wire).is_err());
        let bytes = generic_node_bytes(node.header(), payload);
        let unknown = raw_generic_node(&bytes);
        assert!(try_cast_to_dyn_device_path_node(unknown).is_err());
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
    fn test_device_path_derived_nvdimm_formatting() {
        let node = NvdimmAcpi { nfit_device_handle: 42 };
        assert_eq!(format!("{node}"), "NvdimmAcpi(42)");
        assert_eq!(format!("{node:?}"), "NvdimmAcpi { type: 2, sub_type: 4, length: 8, nfit_device_handle: 42 }",);
    }

    #[test]
    fn test_device_path_atapi_selectors() {
        for primary_secondary in 0..=1 {
            for slave_master in 0..=1 {
                encode_node(Atapi { primary_secondary, slave_master, lun: u16::MAX }).unwrap();
            }
        }
        for invalid in [2, u8::MAX] {
            check_invalid(Atapi { primary_secondary: invalid, slave_master: 0, lun: 0 }, &[invalid, 0, 0, 0]);
            check_invalid(Atapi { primary_secondary: 0, slave_master: invalid, lun: 0 }, &[0, invalid, 0, 0]);
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
                encode_node(IpV6 { ip_address_origin, prefix_length, ..node.clone() }).unwrap();
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
                encode_node(Uart { parity, stop_bits, ..node.clone() }).unwrap();
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
            encode_node(BluetoothLE { address: [0; 6], address_type }).unwrap();
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
    use crate::uefi::device_path::{parse_node::MeasureSize, paths::DevicePathBuf};
    use alloc::{format, vec};
    use core::{fmt, mem};

    fn check_node<T>(node: T, expected: &[u8])
    where
        T: DevicePathNode + Clone + for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        assert_eq!(node.measure_size() + 4, expected.len());
        assert_eq!(usize::from(node.header().length), expected.len());
        let mut encoded = vec![0xa5; expected.len() + 3];
        assert_eq!(node.clone().write_into(&mut encoded).unwrap(), expected.len());
        assert_eq!(&encoded[..expected.len()], expected);
        assert_eq!(&encoded[expected.len()..], &[0xa5; 3]);
        for len in 0..expected.len() {
            assert!(node.clone().write_into(&mut vec![0; len]).is_err(), "short output {len}");
            assert!(expected[..len].pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err());
        }
        let (parsed, consumed) = T::try_from_ctx(&expected[4..], scroll::LE).unwrap();
        assert_eq!(consumed + 4, expected.len());
        let mut roundtrip = vec![0; expected.len()];
        parsed.write_into(&mut roundtrip).unwrap();
        assert_eq!(roundtrip, expected);
        let raw = expected.pread_with::<&GenericDevicePathNode>(0, scroll::LE).unwrap();
        let decoded = try_cast_to_dyn_device_path_node(raw).unwrap();
        assert!(!format!("{decoded:?}").starts_with("GenericDevicePathNode"));
    }

    #[test]
    fn test_device_path_header_layout_is_c_packed() {
        assert_eq!(mem::align_of::<Header>(), 1);
        assert_eq!(mem::size_of::<Header>(), 4);
    }

    #[test]
    fn test_device_path_generic_node_borrows_odd_offset_bytes() {
        let bytes = [0xa5, 6, 1, 7, 0, 1, 2, 3, 0x5a];
        let (node, remaining) = GenericDevicePathNode::from_bytes(&bytes[1..]).unwrap();

        assert_eq!(usize::from(node.header.length), 7);
        assert_eq!(mem::size_of_val(node), 7);
        assert_eq!(node.as_bytes().as_ptr(), bytes[1..].as_ptr());
        assert_eq!(remaining, &[0x5a]);
    }

    #[derive(Debug)]
    struct HugeNode;

    impl fmt::Display for HugeNode {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("HugeNode")
        }
    }

    impl MeasureSize for HugeNode {
        fn measure_size(&self) -> usize {
            usize::from(u16::MAX)
        }
    }

    impl Validate for HugeNode {
        fn validate(&self) -> Result<(), &'static str> {
            Ok(())
        }
    }

    impl DevicePathNode for HugeNode {
        fn r#type() -> u8 {
            6
        }

        fn sub_type() -> u8 {
            1
        }
    }

    impl TryIntoCtx<scroll::Endian> for HugeNode {
        type Error = scroll::Error;

        fn try_into_ctx(self, _dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
            Ok(0)
        }
    }

    #[test]
    fn test_device_path_oversized_default_header_saturates_and_write_fails() {
        let length = HugeNode.header().length;
        assert_eq!(length, u16::MAX);
        assert!(HugeNode.write_into(&mut []).is_err());

        assert!(DevicePathBuf::from_device_path_node_iter([HugeNode].into_iter()).is_err());
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
    fn test_device_path_measure_size() {
        // Derived sizes sum encoded fields rather than including Rust layout padding.
        let mapped = MemoryMapped { memory_type: 3, start_address: 0x1000, end_address: 0x1fff };
        assert_eq!(mapped.measure_size(), 20);
        assert_eq!(core::mem::size_of::<MemoryMapped>(), 24);
        assert_eq!(Bmc { interface_type: 1, base_address: 0 }.measure_size(), 9);
        assert_eq!(EndEntire.measure_size(), 0);
        assert_eq!(EndInstance.measure_size(), 0);

        for (hid, uid, cid) in [("", "", ""), ("PNP0A03", "", "PNP0A08"), ("A", "BCDEFGHIJ", "K")] {
            let node = ExtendedAcpi {
                hid: 0,
                uid: 0,
                cid: 0,
                hid_string: hid.into(),
                uid_string: uid.into(),
                cid_string: cid.into(),
            };
            let size = node.measure_size();
            let mut payload = vec![0; size];
            assert_eq!(payload.pwrite_with(node.clone(), 0, scroll::LE).unwrap(), size);
            assert_eq!(usize::from(node.header().length), size + 4);
        }

        let bytes = generic_node_bytes(Header::new(6, 1, 7), &[1, 2, 3]);
        let unknown = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert_eq!(unknown.measure_size(), 3);
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
    fn test_device_path_decoder_errors_and_unknown_nodes() {
        for bytes in [
            generic_node_bytes(Header::new(1, 1, 4), &[1]),
            generic_node_bytes(Header::new(1, 1, 5), &[1]),
            generic_node_bytes(Header::new(1, 1, 7), &[1, 2, 3]),
            generic_node_bytes(Header::new(3, 18, 10), &[255, 255, 255, 255, 0, 0]),
            generic_node_bytes(Header::new(3, 23, 16), &[0; 12]),
            generic_node_bytes(Header::new(4, 4, 6), &[b'A', 0]),
            generic_node_bytes(Header::new(0x7f, 2, 4), &[]),
            generic_node_bytes(Header::new(0x7f, 1, 5), &[0]),
            generic_node_bytes(Header::new(0x7f, 0xff, 5), &[0]),
        ] {
            let unknown = raw_generic_node(&bytes);
            assert!(try_cast_to_dyn_device_path_node(unknown).is_err());
        }
        for bytes in
            [generic_node_bytes(Header::new(6, 1, 6), &[1, 2]), generic_node_bytes(Header::new(1, 0xff, 6), &[1, 2])]
        {
            let unknown = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
            let expected = format!("{unknown:?}");
            let decoded = try_cast_to_dyn_device_path_node(unknown).unwrap();
            assert_eq!(format!("{decoded:?}"), expected);
        }
        let oversized = vec![0; usize::from(u16::MAX) - 3];
        let oversized_length = u16::try_from(4 + oversized.len()).unwrap_or(u16::MAX);
        let bytes = generic_node_bytes(Header::new(6, 1, oversized_length), &oversized);
        let unknown = raw_generic_node(&bytes);
        assert!(try_cast_to_dyn_device_path_node(unknown).is_err());
    }

    #[test]
    fn test_device_path_checked_lengths_and_dispatch_errors() {
        let mut output = [0u8; 4];
        assert!(Header::new(1, 1, 3).try_into_ctx(&mut output, scroll::LE).is_err());
        for length in 0..4 {
            assert!([1, 1, length, 0].pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err());
        }
        for data in [&[1u8][..], &[1, 2, 3][..]] {
            let bytes = generic_node_bytes(Header::new(1, 1, u16::try_from(data.len() + 4).unwrap()), data);
            let raw = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
            assert!(try_cast_to_dyn_device_path_node(raw).is_err());
        }
        let bytes = generic_node_bytes(Header::new(6, 1, 6), &[1, 2]);
        let unknown = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(
            format!("{:?}", try_cast_to_dyn_device_path_node(unknown).unwrap()).starts_with("GenericDevicePathNode")
        );
        let mut encoded = [0; 6];
        unknown.write_into(&mut encoded).unwrap();
        assert_eq!(encoded, [6, 1, 6, 0, 1, 2]);
    }

    #[test]
    fn test_device_path_invalid_variable_payloads() {
        assert!(encode_node(AcpiAdr { addresses: vec![] }).is_err());
        assert!(encode_node(Dns { is_ipv6: false, addresses: vec![] }).is_err());
        assert!(
            encode_node(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0; 65] }).is_err()
        );
        assert!(
            encode_node(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0xd800] }).is_err()
        );
        assert!(encode_node(RestService { service: RestServiceType::Redfish, access_mode: 3 }).is_err());
        assert!(encode_node(Uri { uri: "a\0b".into() }).is_err());
        assert!(encode_node(Uri { uri: "\u{e9}".into() }).is_err());
        assert!(encode_node(Uri { uri: "a".repeat(65532) }).is_err());
        assert!(encode_node(Bios { device_type: 0, status_flag: 0, description_str: "a\0b".into() }).is_err());
        assert!(
            encode_node(ExtendedAcpi {
                hid: 0,
                uid: 0,
                cid: 0,
                hid_string: "\u{e9}".into(),
                uid_string: "".into(),
                cid_string: "".into()
            })
            .is_err()
        );
        assert!(
            encode_node(Iscsi { protocol: 1, options: 0, lun: [0; 8], portal_group: 0, target_name: "iqn".into() })
                .is_err()
        );
        assert!(encode_node(NvmeOf { identifier_type: 0, identifier: [0; 16], subsystem_nqn: "nqn".into() }).is_err());
        assert!(encode_node(NvmeOf { identifier_type: 1, identifier: [1; 16], subsystem_nqn: "nqn".into() }).is_err());
        assert!(FilePath::try_from_ctx(&[b'A', 0], scroll::LE).is_err());
        assert!(FilePath::try_from_ctx(&[b'A'], scroll::LE).is_err());
        assert!(Bios::try_from_ctx(&[0, 0, 0, 0, b'A'], scroll::LE).is_err());
        assert!(ExtendedAcpi::try_from_ctx(&[0; 14], scroll::LE).is_err());
        let mut non_ascii = [0u8; 15];
        non_ascii[12] = 0xe9;
        assert!(ExtendedAcpi::try_from_ctx(&non_ascii, scroll::LE).is_err());
        assert!(Dns::try_from_ctx(&[0; 16], scroll::LE).is_err());
        assert!(RestService::try_from_ctx(&[0xff, 1], scroll::LE).is_err());
        assert!(Uri::try_from_ctx(&[0xff], scroll::LE).is_err());
    }

    #[test]
    fn test_device_path_existing_validation() {
        assert!(encode_node(Sata::new(u16::MAX, 0, 0)).is_err());
        for id in [0, u32::MAX] {
            assert!(encode_node(NvmExpress::new(id, 0)).is_err());
        }
        assert!(encode_node(FilePath::new("a\0b")).is_err());
        assert!(encode_node(FilePath::new("\u{1f600}")).is_err());
        for hid in ["", "PNP", "1NP0A03", "PNPZZZZ", "\u{e9}NP0A03"] {
            assert!(Acpi::try_eisa_id(hid).is_err());
        }
        assert_eq!(Acpi::try_eisa_id("PNP0A03"), Ok(0x0a0341d0));
        assert_eq!(Acpi::try_eisa_id("pnp0a08"), Ok(0x0a0841d0));
    }

    #[test]
    fn test_device_path_invalid_end_nodes_are_rejected_at_io_boundaries() {
        for (sub_type, length) in [(1, 5), (0xff, 5), (2, 4)] {
            let wire = [0x7f, sub_type, length, 0, 0];
            assert!(wire.pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err());
            let node = raw_generic_node(&wire[..usize::from(length)]);
            assert!(try_cast_to_dyn_device_path_node(node).is_err());
            let mut output = [0xa5; 5];
            assert!(node.write_into(&mut output).is_err());
            assert_eq!(output, [0xa5; 5]);
        }
    }

    #[test]
    fn test_device_path_invalid_string_writer_preserves_output() {
        for path in ["a\0b", "\u{1f600}"] {
            let node = FilePath::new(path);
            let mut output = vec![0xa5; usize::from(node.header().length)];
            assert!(node.write_into(&mut output).is_err());
            assert!(output.iter().all(|byte| *byte == 0xa5));
        }
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
