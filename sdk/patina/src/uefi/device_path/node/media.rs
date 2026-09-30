//! Media device path nodes (UEFI 2.10 Section 10.3.5, type `0x04`).
//!
//! A media device path describes the portion of a medium that a boot service
//! abstracts, such as the partition of a hard drive or an El Torito boot partition
//! of a CD-ROM. It also describes the location of a file in a file system.
//!
//! The device path of a partition is the device path of its raw device followed by
//! a media node that identifies the partition (Section 10.4.5).
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#media-device-path>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, MediaSubType};
use crate::{
    device_path_node,
    uefi::device_path::{
        encoding::{Char16Nul, Vector},
        parse_node::{self, MeasureSize, Validate},
    },
};
use alloc::{string::String, vec::Vec};
use core::fmt::Display;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// CD-ROM media device path node (UEFI 2.10 Table 10.55).
    ///
    /// A system partition on an El Torito CD-ROM, identified by its boot catalog entry.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#cd-rom-media-device-path>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::CdRom)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
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
    /// Media protocol device path node (UEFI 2.10 Table 10.58).
    ///
    /// The GUID of the protocol used at this location in the device path.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#media-protocol-device-path>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::MediaProtocol)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct MediaProtocol {
        /// Protocol identifier as 16 EFI GUID wire bytes, without additional byte swapping.
        pub protocol: [u8; 16],
    }
}

device_path_node! {
    /// PIWG firmware file device path node (UEFI 2.10 Table 10.59).
    ///
    /// A firmware file, whose contents are defined by the UEFI PI Specification.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#piwg-firmware-file>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareFile)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareFile {
        /// File name as 16 EFI GUID wire bytes, without additional byte swapping.
        pub fv_file_name: [u8; 16],
    }
}

device_path_node! {
    /// PIWG firmware volume device path node (UEFI 2.10 Table 10.60).
    ///
    /// A firmware volume, whose contents are defined by the UEFI PI Specification.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#piwg-firmware-volume>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareVolume)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareVolume {
        /// Volume name as 16 EFI GUID wire bytes, without additional byte swapping.
        pub fv_name: [u8; 16],
    }
}

device_path_node! {
    /// Relative offset range device path node (UEFI 2.10 Table 10.61).
    ///
    /// A range of byte offsets relative to the parent device node. The ending offset is
    /// the last byte of the range.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#relative-offset-range>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RelativeOffsetRange)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
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
    /// RAM disk device path node (UEFI 2.10 Table 10.62).
    ///
    /// The memory range, disk type GUID, and instance number of a RAM disk.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RamDisk)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
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

device_path_node! {
    /// Vendor-defined media device path node (UEFI 2.10 Table 10.56).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-media-device-path>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::Vendor)]
    @[DevicePathNodeDerive(Debug, Display, Validate)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct MediaVendor {
        /// Vendor GUID in EFI wire byte order.
        pub guid: [u8; 16],
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Vector)]
        pub data: Vec<u8>,
    }
}

impl MeasureSize for MediaVendor {
    fn measure_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

device_path_node! {
    /// Hard drive media device path node (UEFI 2.10 Table 10.54).
    ///
    /// A partition from an MBR or GPT partition table, with its disk signature.
    /// Partition number zero represents the entire device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hard-drive>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::HardDrive)]
    @[DevicePathNodeDerive(Debug, MeasureSize)]
    #[derive(Pread, Pwrite, Clone)]
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
}

impl HardDrive {
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
    /// `HardDrive` node, if the node data is not exactly one `HardDrive`
    /// payload, or if the node fails [`parse_node::Validate::validate`].
    ///
    /// Useful when walking a device path with
    /// [`crate::uefi::device_path::paths::DevicePath`] to recover the typed
    /// `HardDrive` payload from each node without doing the
    /// type/subtype check at the call site.
    pub fn try_from_node(node: &super::GenericDevicePathNode) -> Option<Self> {
        if !<Self as parse_node::DevicePathNode>::is_type(node.header.r#type, node.header.sub_type) {
            return None;
        }
        parse_node::decode_payload(&node.data).ok()
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

impl Display for HardDrive {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "HD({}, GPT, ...)", self.partition_number)
    }
}

impl Validate for HardDrive {
    fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.partition_format, Self::FORMAT_MBR | Self::FORMAT_GPT) || self.signature_type > 2 {
            return Err("invalid hard drive partition format or signature type");
        }
        if self.partition_format == Self::FORMAT_MBR && self.partition_number > 4 {
            return Err("MBR partition number must be at most 4");
        }
        if (self.signature_type == 0 && self.partition_signature != [0; 16])
            || (self.signature_type == Self::SIGNATURE_TYPE_MBR
                && self.partition_signature[4..].iter().any(|byte| *byte != 0))
        {
            return Err("invalid hard drive signature padding");
        }
        Ok(())
    }
}

device_path_node! {
    /// File path media device path node (UEFI 2.10 Table 10.57).
    ///
    /// A NUL-terminated path name. A device path can contain several of these nodes,
    /// which are concatenated to form the complete path.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#file-path-media-device-path>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::FilePath)]
    @[DevicePathNodeDerive(Debug, Validate)]
    #[derive(Clone, Pread, Pwrite)]
    pub struct FilePath {
        /// The file path string (stored as UTF-8, serialized as UTF-16).
        #[scroll(ctx = Char16Nul)]
        pub path: String,
    }
}

impl FilePath {
    /// Create a new `FilePath` device path node from a path string.
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    /// Calculate the CHAR16 encoded size of the path including its NUL terminator.
    fn utf16_size(&self) -> usize {
        // Each UCS-2 character is one CHAR16 code unit, followed by a CHAR16 terminator.
        (self.path.chars().count() + 1) * size_of::<u16>()
    }
}

impl MeasureSize for FilePath {
    fn measure_size(&self) -> usize {
        self.utf16_size()
    }
}

impl Display for FilePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.path)
    }
}
