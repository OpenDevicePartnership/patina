//! Media device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, MediaSubType, check_length, invalid};
use crate::{
    Char16Str, Char16String, device_path_node,
    uefi::device_path::parse_node::{self, DevicePathNode, Header},
};
use alloc::{string::String, vec::Vec};
use core::fmt::Display;
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

device_path_node! {
    /// A CD-ROM system partition.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::CdRom)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct CdRom {
        /// Boot catalog entry number; zero denotes the initial/default entry.
        pub boot_entry: u32,
        /// Starting relative logical block address.
        pub partition_start: u64,
        /// Partition size in blocks.
        pub partition_size: u64,
    }
}

device_path_node! {
    /// A protocol used at a location in a media device path.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::MediaProtocol)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct MediaProtocol {
        /// Protocol identifier as 16 EFI GUID wire bytes, without additional byte swapping.
        pub protocol: [u8; 16],
    }
}

device_path_node! {
    /// A firmware file in a PI firmware volume.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareFile)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareFile {
        /// File name as 16 EFI GUID wire bytes, without additional byte swapping.
        pub fv_file_name: [u8; 16],
    }
}

device_path_node! {
    /// A PI firmware volume.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareVolume)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareVolume {
        /// Volume name as 16 EFI GUID wire bytes, without additional byte swapping.
        pub fv_name: [u8; 16],
    }
}

device_path_node! {
    /// A range of offsets within a media device.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RelativeOffsetRange)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct RelativeOffsetRange {
        /// Reserved; producers should use zero, while decoding preserves the value.
        pub reserved: u32,
        /// Starting offset relative to the media device.
        pub starting_offset: u64,
        /// Ending offset relative to the media device.
        pub ending_offset: u64,
    }
}

device_path_node! {
    /// A memory-backed disk.
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RamDisk)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
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

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        let header = self.header();
        let mut header_bytes = [0u8; 4];
        header_bytes.pwrite_with(header, 0, scroll::LE)?;
        if buffer.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: buffer.len() });
        }
        let payload = buffer.get_mut(4..header.length).ok_or_else(|| invalid("invalid device path output range"))?;
        let written = payload.pwrite_with(self, 0, scroll::LE)?;
        if written != header.length - 4 {
            return Err(invalid("encoded node length differs from header"));
        }
        buffer.pwrite_with(header_bytes.as_slice(), 0, ())?;
        Ok(header.length)
    }
}

impl TryFromCtx<'_, scroll::Endian> for MediaVendor {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid = data.gread_with(&mut offset, scroll::LE)?;
        let payload = data.get(offset..).ok_or_else(|| invalid("missing vendor payload"))?;
        let (node, consumed) = (Self { guid, data: payload.to_vec() }, data.len());
        check_length(node.header().length - 4)?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for MediaVendor {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        check_length(self.header().length - 4)?;
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

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        let header = self.header();
        let mut header_bytes = [0u8; 4];
        header_bytes.pwrite_with(header, 0, scroll::LE)?;
        if buffer.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: buffer.len() });
        }
        let payload = buffer.get_mut(4..header.length).ok_or_else(|| invalid("invalid device path output range"))?;
        let written = payload.pwrite_with(self, 0, scroll::LE)?;
        if written != header.length - 4 {
            return Err(invalid("encoded node length differs from header"));
        }
        buffer.pwrite_with(header_bytes.as_slice(), 0, ())?;
        Ok(header.length)
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
        if !matches!(self.partition_format, 1 | 2) || self.signature_type > 2 {
            return Err(invalid("invalid hard drive partition format or signature type"));
        }
        if (self.signature_type == 0 && self.partition_signature != [0; 16])
            || (self.signature_type == 1 && self.partition_signature[4..].iter().any(|byte| *byte != 0))
        {
            return Err(invalid("invalid hard drive signature padding"));
        }
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
        if !matches!(node.partition_format, 1 | 2) || node.signature_type > 2 {
            return Err(invalid("invalid hard drive partition format or signature type"));
        }
        if (node.signature_type == 0 && node.partition_signature != [0; 16])
            || (node.signature_type == 1 && node.partition_signature[4..].iter().any(|byte| *byte != 0))
        {
            return Err(invalid("invalid hard drive signature padding"));
        }
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

    fn write_into(self, buffer: &mut [u8]) -> Result<usize, scroll::Error> {
        let header = self.header();
        let mut header_bytes = [0u8; 4];
        header_bytes.pwrite_with(header, 0, scroll::LE)?;
        if buffer.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: buffer.len() });
        }
        let payload = buffer.get_mut(4..header.length).ok_or_else(|| invalid("invalid device path output range"))?;
        let written = payload.pwrite_with(self, 0, scroll::LE)?;
        if written != header.length - 4 {
            return Err(invalid("encoded node length differs from header"));
        }
        buffer.pwrite_with(header_bytes.as_slice(), 0, ())?;
        Ok(header.length)
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
        check_length(self.utf16_size())?;
        Char16String::try_from_str(&self.path).map_err(|_| invalid("file path is not valid UCS-2"))?;
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

        check_length(offset)?;
        Ok((Self { path }, offset))
    }
}
