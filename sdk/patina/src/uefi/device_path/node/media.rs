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

use super::TextString;
use super::{DevicePathType, MediaSubType, device_path_enum};
use crate::BinaryGuid;
use crate::{
    device_path_node,
    uefi::device_path::encoding::{Char16Nul, Vector},
};
use alloc::{string::String, vec::Vec};
use core::fmt;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// CD-ROM media device path node (UEFI 2.10 Table 10.55).
    ///
    /// A system partition on an El Torito CD-ROM, identified by its boot catalog entry.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#cd-rom-media-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::CdRom;
    ///
    /// // The first El Torito boot entry, at sector 0x23 of the CD, for 0x2000 sectors.
    /// let node = CdRom { boot_entry: 0, partition_start: 0x23, partition_size: 0x2000 };
    /// assert_eq!(node.to_string(), "CDROM(0x0,0x23,0x2000)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "CDROM(0x0)");
    /// assert_eq!(format!("{node:#}"), "CDROM(0x0,0x23,0x2000)");
    /// assert_eq!(format!("{node:-#}"), "CDROM(0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Media, MediaSubType::CdRom)]
    @[DevicePathNodeDerive(Debug)]
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
impl fmt::Display for CdRom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        if f.sign_minus() {
            write!(f, "CDROM(0x{:x})", node.boot_entry)
        } else {
            write!(f, "CDROM(0x{:x},0x{:x},0x{:x})", node.boot_entry, node.partition_start, node.partition_size)
        }
    }
}

device_path_node! {
    /// Media protocol device path node (UEFI 2.10 Table 10.58).
    ///
    /// The GUID of the protocol used at this location in the device path.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#media-protocol-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::MediaProtocol};
    ///
    /// let node = MediaProtocol { protocol: BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF") };
    /// assert_eq!(node.to_string(), "Media(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Media(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:#}"), "Media(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:-#}"), "Media(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// ```
    @[DevicePathNode(DevicePathType::Media, MediaSubType::MediaProtocol)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct MediaProtocol {
        /// Protocol GUID.
        pub protocol: BinaryGuid,
    }
}
impl fmt::Display for MediaProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Media({})", node.protocol)
    }
}

device_path_node! {
    /// PIWG firmware file device path node (UEFI 2.10 Table 10.59).
    ///
    /// A firmware file, whose contents are defined by the UEFI PI Specification.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#piwg-firmware-file>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::PiwgFirmwareFile};
    ///
    /// let node = PiwgFirmwareFile { fv_file_name: BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF") };
    /// assert_eq!(node.to_string(), "FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:#}"), "FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:-#}"), "FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// ```
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareFile)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareFile {
        /// Firmware file name GUID.
        pub fv_file_name: BinaryGuid,
    }
}
impl fmt::Display for PiwgFirmwareFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "FvFile({})", node.fv_file_name)
    }
}

device_path_node! {
    /// PIWG firmware volume device path node (UEFI 2.10 Table 10.60).
    ///
    /// A firmware volume, whose contents are defined by the UEFI PI Specification.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#piwg-firmware-volume>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::PiwgFirmwareVolume};
    ///
    /// let node = PiwgFirmwareVolume { fv_name: BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF") };
    /// assert_eq!(node.to_string(), "Fv(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Fv(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:#}"), "Fv(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// assert_eq!(format!("{node:-#}"), "Fv(00112233-4455-6677-8899-AABBCCDDEEFF)");
    /// ```
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareVolume)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareVolume {
        /// Firmware volume name GUID.
        pub fv_name: BinaryGuid,
    }
}
impl fmt::Display for PiwgFirmwareVolume {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Fv({})", node.fv_name)
    }
}

device_path_node! {
    /// Relative offset range device path node (UEFI 2.10 Table 10.61).
    ///
    /// A range of byte offsets relative to the parent device node. The ending offset is
    /// the last byte of the range.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#relative-offset-range>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::RelativeOffsetRange;
    ///
    /// // The first 64 KiB of the device.
    /// let node = RelativeOffsetRange { reserved: 0, starting_offset: 0, ending_offset: 0xffff };
    /// assert_eq!(node.to_string(), "Offset(0x0,0xffff)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Offset(0x0,0xffff)");
    /// assert_eq!(format!("{node:#}"), "Offset(0x0,0xffff)");
    /// assert_eq!(format!("{node:-#}"), "Offset(0x0,0xffff)");
    /// ```
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RelativeOffsetRange)]
    @[DevicePathNodeDerive(Debug)]
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
impl fmt::Display for RelativeOffsetRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Offset(0x{:x},0x{:x})", node.starting_offset, node.ending_offset)
    }
}

device_path_node! {
    /// RAM disk device path node (UEFI 2.10 Table 10.62).
    ///
    /// The memory range, disk type GUID, and instance number of a RAM disk.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::RamDisk;
    ///
    /// // A 64 MiB RAM disk of the virtual disk type `EFI_VIRTUAL_DISK_GUID` (UEFI 2.10 Section 10.3.5.9),
    /// // which displays as `VirtualDisk`.
    /// let node = RamDisk {
    ///     starting_address: 0x7000_0000,
    ///     ending_address: 0x73ff_ffff,
    ///     type_guid: RamDisk::VIRTUAL_DISK_GUID,
    ///     instance: 0,
    /// };
    /// assert_eq!(node.to_string(), "VirtualDisk(0x70000000,0x73ffffff,0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "VirtualDisk(0x70000000,0x73ffffff,0)");
    /// assert_eq!(format!("{node:#}"), "RamDisk(0x70000000,0x73ffffff,0,77AB535A-45FC-624B-5560-F7B281D1F96E)");
    /// assert_eq!(format!("{node:-#}"), "RamDisk(0x70000000,0x73ffffff,0,77AB535A-45FC-624B-5560-F7B281D1F96E)");
    /// ```
    @[DevicePathNode(DevicePathType::Media, MediaSubType::RamDisk)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct RamDisk {
        /// Starting physical memory address.
        pub starting_address: u64,
        /// Ending physical memory address.
        pub ending_address: u64,
        /// Disk type GUID.
        pub type_guid: BinaryGuid,
        /// Disk instance number, or zero if unsupported.
        pub instance: u16,
    }
}

impl RamDisk {
    /// `EFI_VIRTUAL_DISK_GUID`: a raw disk format in volatile memory, `VirtualDisk` in text
    /// ([UEFI 2.10 Section 10.3.5.9](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk)).
    pub const VIRTUAL_DISK_GUID: BinaryGuid = BinaryGuid::from_string("77AB535A-45FC-624B-5560-F7B281D1F96E");
    /// `EFI_VIRTUAL_CD_GUID`: an ISO image in volatile memory, `VirtualCD` in text
    /// ([UEFI 2.10 Section 10.3.5.9](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk)).
    pub const VIRTUAL_CD_GUID: BinaryGuid = BinaryGuid::from_string("3D5ABD30-4175-87CE-6D64-D2ADE523C4BB");
    /// `EFI_PERSISTENT_VIRTUAL_DISK_GUID`: a raw disk format in persistent memory,
    /// `PersistentVirtualDisk` in text ([UEFI 2.10 Section 10.3.5.9](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk)).
    pub const PERSISTENT_VIRTUAL_DISK_GUID: BinaryGuid =
        BinaryGuid::from_string("5CEA02C9-4D07-69D3-269F-4496FBE096F9");
    /// `EFI_PERSISTENT_VIRTUAL_CD_GUID`: an ISO image in persistent memory, `PersistentVirtualCD`
    /// in text ([UEFI 2.10 Section 10.3.5.9](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk)).
    pub const PERSISTENT_VIRTUAL_CD_GUID: BinaryGuid = BinaryGuid::from_string("08018188-42CD-BB48-100F-5387D53DED3D");
}

impl fmt::Display for RamDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        let name = match node.type_guid {
            _ if f.alternate() => None,
            Self::VIRTUAL_DISK_GUID => Some("VirtualDisk"),
            Self::VIRTUAL_CD_GUID => Some("VirtualCD"),
            Self::PERSISTENT_VIRTUAL_DISK_GUID => Some("PersistentVirtualDisk"),
            Self::PERSISTENT_VIRTUAL_CD_GUID => Some("PersistentVirtualCD"),
            _ => None,
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
}

device_path_node! {
    /// Vendor-defined media device path node (UEFI 2.10 Table 10.56).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-media-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::MediaVendor};
    ///
    /// let node = MediaVendor {
    ///     guid: BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF"),
    ///     data: vec![0x01],
    /// };
    /// assert_eq!(node.to_string(), "VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)");
    /// assert_eq!(format!("{node:#}"), "VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)");
    /// assert_eq!(format!("{node:-#}"), "VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Media, MediaSubType::Vendor,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct MediaVendor {
        /// Vendor GUID.
        pub guid: BinaryGuid,
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Vector)]
        pub data: Vec<u8>,
    }
}
impl fmt::Display for MediaVendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "VenMedia({}", node.guid)?;
        if !node.data.is_empty() {
            f.write_str(",")?;
            for byte in &node.data {
                write!(f, "{byte:02x}")?;
            }
        }
        f.write_str(")")
    }
}

impl MediaVendor {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

device_path_enum! {
    /// Partition table format of a [`HardDrive`] node. Table 10.54 reserves unused values, so
    /// any other value is rejected
    /// ([UEFI 2.10 Table 10.54](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hard-drive)).
    pub enum PartitionFormat: u8 {
        /// PC-AT compatible legacy MBR.
        Mbr = 1,
        /// GUID Partition Table.
        Gpt = 2,
    }
}

device_path_enum! {
    /// Type of the disk signature of a [`HardDrive`] node. Table 10.54 reserves unused values, so
    /// any other value is rejected
    /// ([UEFI 2.10 Table 10.54](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hard-drive)).
    pub enum PartitionSignatureType: u8 {
        /// No disk signature.
        None = 0,
        /// The 32-bit signature from address `0x1b8` of the MBR.
        Mbr = 1,
        /// A GUID signature.
        Guid = 2,
    }
}

device_path_node! {
    /// Hard drive media device path node (UEFI 2.10 Table 10.54).
    ///
    /// A partition from an MBR or GPT partition table, with its disk signature.
    /// Partition number zero represents the entire device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hard-drive>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::HardDrive};
    ///
    /// // The partition of the iSCSI example
    /// // `HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)` (UEFI 2.10 Section 10.3.4.20.2).
    /// let partition_guid = *BinaryGuid::from_string("15E39A00-1DD2-1000-8D7F-00A0C92408FC").as_bytes();
    /// let node = HardDrive::new_gpt(1, 0x22, 0x2710000, partition_guid);
    /// assert_eq!(node.to_string(), "HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC)");
    /// assert_eq!(format!("{node:#}"), "HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)");
    /// assert_eq!(format!("{node:-#}"), "HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Media, MediaSubType::HardDrive,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct HardDrive {
        /// Partition number (1-based).
        pub partition_number: u32,
        /// Starting LBA of the partition.
        pub partition_start: u64,
        /// Size of the partition in blocks.
        pub partition_size: u64,
        /// Partition signature (GUID for GPT, 4-byte signature for MBR).
        pub partition_signature: [u8; 16],
        /// Partition table format.
        pub partition_format: PartitionFormat,
        /// Type of the partition signature.
        pub signature_type: PartitionSignatureType,
    }
}
impl fmt::Display for HardDrive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "HD({},", node.partition_number)?;
        match node.signature_type {
            PartitionSignatureType::Mbr => write!(
                f,
                "MBR,0x{:08x}",
                u32::from_le_bytes(node.partition_signature[..4].try_into().map_err(|_| fmt::Error)?)
            )?,
            PartitionSignatureType::Guid => {
                f.write_str("GPT,")?;
                write!(f, "{}", BinaryGuid::from_bytes(&node.partition_signature))?;
            }
            PartitionSignatureType::None => f.write_str("0,0")?,
        }
        // Start and Size are prohibited for partition 0, the whole device (UEFI 2.10 Table 10.67).
        if !f.sign_minus() && node.partition_number != 0 {
            write!(f, ",0x{:x},0x{:x}", node.partition_start, node.partition_size)?;
        }
        f.write_str(")")
    }
}

impl HardDrive {
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
            partition_format: PartitionFormat::Gpt,
            signature_type: PartitionSignatureType::Guid,
        }
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.partition_format == PartitionFormat::Mbr && self.partition_number > 4 {
            return Err("MBR partition number must be at most 4");
        }
        if (self.signature_type == PartitionSignatureType::None && self.partition_signature != [0; 16])
            || (self.signature_type == PartitionSignatureType::Mbr
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::FilePath;
    ///
    /// // The default boot file of removable media on x64 (UEFI 2.10 Section 3.5.1.1).
    /// let node = FilePath::new(r"\EFI\BOOT\BOOTX64.EFI");
    /// assert_eq!(node.to_string(), r#"\EFI\BOOT\BOOTX64.EFI"#);
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), r#"\EFI\BOOT\BOOTX64.EFI"#);
    /// assert_eq!(format!("{node:#}"), r#"\EFI\BOOT\BOOTX64.EFI"#);
    /// assert_eq!(format!("{node:-#}"), r#"\EFI\BOOT\BOOTX64.EFI"#);
    /// ```
    @[DevicePathNode(
        DevicePathType::Media, MediaSubType::FilePath,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Pread, Pwrite, Eq, PartialEq)]
    pub struct FilePath {
        /// The file path string (stored as UTF-8, serialized as UTF-16).
        #[scroll(ctx = Char16Nul)]
        pub path: String,
    }
}
impl fmt::Display for FilePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "{}", TextString(&node.path))
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

impl FilePath {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        self.utf16_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::{
        DevicePathNode, DevicePathType, GenericDevicePathNode, Header, MediaSubType,
        tests::{check_node, check_rejected, encode_node, fixture, generic_node_bytes},
    };
    use alloc::{format, vec};
    use scroll::{Pread, Pwrite, ctx::TryFromCtx};

    #[test]
    fn test_hard_drive_new_gpt() {
        let guid = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10];
        let hd = HardDrive::new_gpt(1, 2048, 1000000, guid);
        assert_eq!(hd.partition_number, 1);
        assert_eq!(hd.partition_start, 2048);
        assert_eq!(hd.partition_size, 1000000);
        assert_eq!(hd.partition_signature, guid);
        assert_eq!(hd.partition_format, PartitionFormat::Gpt);
        assert_eq!(hd.signature_type, PartitionSignatureType::Guid);
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
        assert_eq!(parsed.partition_format, PartitionFormat::Gpt);
        assert_eq!(parsed.signature_type, PartitionSignatureType::Guid);
    }

    #[test]
    fn test_hard_drive_display() {
        let hd = HardDrive::new_gpt(1, 0, 0, [0; 16]);
        let display = format!("{hd}");
        assert!(display.contains("HD"));
    }

    #[test]
    fn test_hard_drive_try_from_success() {
        let guid = [0x11u8; 16];
        let hd = HardDrive::new_gpt(3, 0x1000, 0x4000, guid);
        let mut data = [0u8; 38];
        data.pwrite_with(hd, 0, scroll::LE).unwrap();
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Media as u8, MediaSubType::HardDrive as u8, 42), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        let parsed = HardDrive::try_from(node).expect("valid HD node should parse");
        assert_eq!(parsed.partition_number, 3);
        assert_eq!(parsed.partition_start, 0x1000);
        assert_eq!(parsed.partition_size, 0x4000);
        assert_eq!(parsed.partition_signature, guid);
        assert_eq!(parsed.partition_format, PartitionFormat::Gpt);
        assert_eq!(parsed.signature_type, PartitionSignatureType::Guid);
    }

    #[test]
    fn test_hard_drive_try_from_wrong_type() {
        let data = [0u8; 38];
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Messaging as u8, MediaSubType::HardDrive as u8, 42), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(HardDrive::try_from(node).is_err());
    }

    #[test]
    fn test_hard_drive_validate() {
        let mbr = |partition_number, signature: [u8; 16]| HardDrive {
            partition_number,
            partition_start: 0,
            partition_size: 0,
            partition_signature: signature,
            partition_format: PartitionFormat::Mbr,
            signature_type: PartitionSignatureType::Mbr,
        };
        let mut signature = [0; 16];
        signature[..4].copy_from_slice(&[1, 2, 3, 4]);
        for partition_number in 0..=4 {
            assert!(mbr(partition_number, signature).validate().is_ok());
        }
        assert!(mbr(5, signature).validate().is_err());
        assert!(mbr(1, [1; 16]).validate().is_err());
        assert!(
            HardDrive { signature_type: PartitionSignatureType::None, ..HardDrive::new_gpt(1, 0, 0, [1; 16]) }
                .validate()
                .is_err()
        );
        // Reserved partition formats and signature types cannot be decoded.
        let wire = encode_node(HardDrive::new_gpt(1, 0, 0, [1; 16])).unwrap();
        for (offset, value) in [(40, 0), (40, 3), (41, 3)] {
            let mut reserved = wire.clone();
            reserved[offset] = value;
            check_rejected::<HardDrive>(&reserved);
        }
        assert!(PartitionFormat::try_from(3).is_err());
        assert!(PartitionSignatureType::try_from(3).is_err());
        assert!(HardDrive::new_gpt(128, 0, 0, [1; 16]).validate().is_ok());

        let mut data = [0u8; 38];
        data.pwrite_with(mbr(5, signature), 0, scroll::LE).unwrap();
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Media as u8, MediaSubType::HardDrive as u8, 42), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(HardDrive::try_from(node).is_err());
        assert!(<Box<dyn DevicePathNode>>::try_from(node).is_err());
    }

    #[test]
    fn test_hard_drive_try_from_short_data() {
        let data = [0u8; 16]; // shorter than the 38-byte HardDrive payload
        let bytes =
            generic_node_bytes(Header::new(DevicePathType::Media as u8, MediaSubType::HardDrive as u8, 20), &data);
        let node = GenericDevicePathNode::from_bytes(&bytes).unwrap().0;
        assert!(HardDrive::try_from(node).is_err());
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
        let display = format!("{fp}");
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

    fixture!(
        test_device_path_cdrom,
        CdRom { boot_entry: 0x04030201, partition_start: 0x0c0b0a0908070605, partition_size: 0x14131211100f0e0d },
        [4, 2, 24, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
    );

    fixture!(
        test_device_path_media_protocol,
        MediaProtocol {
            protocol: crate::BinaryGuid::from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
        },
        [4, 5, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(
        test_device_path_piwg_firmware_file,
        PiwgFirmwareFile {
            fv_file_name: crate::BinaryGuid::from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
        },
        [4, 6, 20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );

    fixture!(
        test_device_path_piwg_firmware_volume,
        PiwgFirmwareVolume {
            fv_name: crate::BinaryGuid::from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
        },
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
            type_guid: crate::BinaryGuid::from_bytes(&[17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]),
            instance: 0x2221,
        },
        [
            4, 9, 38, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
            27, 28, 29, 30, 31, 32, 33, 34,
        ]
    );

    #[test]
    fn test_device_path_media_vendor() {
        let mut expected = vec![4, 3, 22, 0];
        expected.extend_from_slice(&[0x11; 16]);
        expected.extend_from_slice(&[0xaa, 0xbb]);
        check_node(MediaVendor { guid: crate::BinaryGuid::from_bytes(&[0x11; 16]), data: vec![0xaa, 0xbb] }, &expected);
    }

    #[test]
    fn test_device_path_opaque_arrays_ignore_endian_context_media() {
        let guid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let mut output = [0; 16];
        output.pwrite_with(MediaProtocol { protocol: crate::BinaryGuid::from_bytes(&guid) }, 0, scroll::BE).unwrap();
        assert_eq!(output, guid);
    }

    #[test]
    fn test_device_path_existing_wire_sizes_media() {
        check_node(FilePath::new("A"), &[4, 4, 8, 0, b'A', 0, 0, 0]);
        let mut expected = vec![4, 1, 42, 0, 1, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0];
        expected.extend_from_slice(&[0x11; 16]);
        expected.extend_from_slice(&[2, 2]);
        check_node(HardDrive::new_gpt(1, 2048, 4096, [0x11; 16]), &expected);
    }

    #[test]
    fn test_device_path_invalid_variable_payloads_media() {
        assert!(FilePath::try_from_ctx(&[b'A', 0], scroll::LE).is_err());
        assert!(FilePath::try_from_ctx(b"A", scroll::LE).is_err());
    }

    #[test]
    fn test_device_path_existing_validation_media() {
        assert!(encode_node(FilePath::new("a\0b")).is_err());
        assert!(encode_node(FilePath::new("\u{1f600}")).is_err());
    }

    /// Check the text of each media node in the three text forms, and that the parseable forms
    /// parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_media() {
        use crate::uefi::device_path::node::tests::check_text;

        check_text!(
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)"
        );
        check_text!(
            r"HD(2,MBR,0x12345678,0x800,0x1000)",
            r"HD(2,MBR,0x12345678,0x800,0x1000)",
            r"HD(2,MBR,0x12345678)",
            r"HD(2,MBR,0x12345678,0x800,0x1000)"
        );
        check_text!(r"CDROM(1,0x10,0x20)", r"CDROM(0x1,0x10,0x20)", r"CDROM(0x1)", r"CDROM(0x1,0x10,0x20)");
        check_text!(
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)"
        );
        check_text!(r"\file", r"\file", r"\file", r"\file");
        check_text!(
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check_text!(
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check_text!(
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check_text!(r"Offset(0x10,0x20)", r"Offset(0x10,0x20)", r"Offset(0x10,0x20)", r"Offset(0x10,0x20)");
        check_text!(
            r"RamDisk(0x1000,0x1FFF,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check_text!(
            r"VirtualDisk(0x1000,0x1FFF,0)",
            r"VirtualDisk(0x1000,0x1fff,0)",
            r"VirtualDisk(0x1000,0x1fff,0)",
            r"RamDisk(0x1000,0x1fff,0,77AB535A-45FC-624B-5560-F7B281D1F96E)"
        );
        check_text!(
            r"VirtualCD(0x1000,0x1FFF,0)",
            r"VirtualCD(0x1000,0x1fff,0)",
            r"VirtualCD(0x1000,0x1fff,0)",
            r"RamDisk(0x1000,0x1fff,0,3D5ABD30-4175-87CE-6D64-D2ADE523C4BB)"
        );
        check_text!(
            r"PersistentVirtualDisk(0x1000,0x1FFF,0)",
            r"PersistentVirtualDisk(0x1000,0x1fff,0)",
            r"PersistentVirtualDisk(0x1000,0x1fff,0)",
            r"RamDisk(0x1000,0x1fff,0,5CEA02C9-4D07-69D3-269F-4496FBE096F9)"
        );
        check_text!(
            r"PersistentVirtualCD(0x1000,0x1FFF,0)",
            r"PersistentVirtualCD(0x1000,0x1fff,0)",
            r"PersistentVirtualCD(0x1000,0x1fff,0)",
            r"RamDisk(0x1000,0x1fff,0,08018188-42CD-BB48-100F-5387D53DED3D)"
        );
    }

    /// Partition 0 is the whole device: its text has no Start and Size (UEFI 2.10 Table 10.67).
    #[test]
    fn test_device_path_hard_drive_text_whole_device() {
        let node = HardDrive { partition_number: 0, ..HardDrive::new_gpt(1, 0x22, 0x100, [0x11; 16]) };
        assert_eq!(format!("{node}"), "HD(0,GPT,11111111-1111-1111-1111-111111111111)");
    }

    /// File paths with characters the text cannot hold bare are written in double quotes.
    #[test]
    fn test_device_path_file_path_text_quoting() {
        assert_eq!(format!("{}", FilePath::new(r"\EFI\BOOT\BOOTX64.EFI")), r"\EFI\BOOT\BOOTX64.EFI");
        assert_eq!(format!("{}", FilePath::new(r"\EFI\My App.efi")), r#""\EFI\My App.efi""#);
    }
}
