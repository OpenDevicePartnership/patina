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
use crate::BinaryGuid;
use crate::{
    device_path_node,
    uefi::device_path::encoding::{Char16Nul, Vector},
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

device_path_node! {
    /// Media protocol device path node (UEFI 2.10 Table 10.58).
    ///
    /// The GUID of the protocol used at this location in the device path.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#media-protocol-device-path>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::MediaProtocol)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct MediaProtocol {
        /// Protocol GUID.
        pub protocol: BinaryGuid,
    }
}

device_path_node! {
    /// PIWG firmware file device path node (UEFI 2.10 Table 10.59).
    ///
    /// A firmware file, whose contents are defined by the UEFI PI Specification.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#piwg-firmware-file>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareFile)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareFile {
        /// Firmware file name GUID.
        pub fv_file_name: BinaryGuid,
    }
}

device_path_node! {
    /// PIWG firmware volume device path node (UEFI 2.10 Table 10.60).
    ///
    /// A firmware volume, whose contents are defined by the UEFI PI Specification.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#piwg-firmware-volume>
    @[DevicePathNode(DevicePathType::Media, MediaSubType::PiwgFirmwareVolume)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct PiwgFirmwareVolume {
        /// Firmware volume name GUID.
        pub fv_name: BinaryGuid,
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

device_path_node! {
    /// RAM disk device path node (UEFI 2.10 Table 10.62).
    ///
    /// The memory range, disk type GUID, and instance number of a RAM disk.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#ram-disk>
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

device_path_node! {
    /// Vendor-defined media device path node (UEFI 2.10 Table 10.56).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-defined-media-device-path>
    @[DevicePathNode(
        DevicePathType::Media, MediaSubType::Vendor,
        measure_size = media_vendor_measure_size,
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

fn media_vendor_measure_size(node: &MediaVendor) -> usize {
    size_of_val(&node.guid) + size_of_val(node.data.as_slice())
}

device_path_node! {
    /// Hard drive media device path node (UEFI 2.10 Table 10.54).
    ///
    /// A partition from an MBR or GPT partition table, with its disk signature.
    /// Partition number zero represents the entire device.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hard-drive>
    @[DevicePathNode(
        DevicePathType::Media, MediaSubType::HardDrive,
        validate = hard_drive_validate,
    )]
    @[DevicePathNodeDerive(Debug)]
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
        crate::uefi::device_path::text::DevicePathText::fmt_text(
            self,
            f,
            crate::uefi::device_path::text::TextOptions::default(),
        )
    }
}

fn hard_drive_validate(node: &HardDrive) -> Result<(), &'static str> {
    if !matches!(node.partition_format, HardDrive::FORMAT_MBR | HardDrive::FORMAT_GPT) || node.signature_type > 2 {
        return Err("invalid hard drive partition format or signature type");
    }
    if node.partition_format == HardDrive::FORMAT_MBR && node.partition_number > 4 {
        return Err("MBR partition number must be at most 4");
    }
    if (node.signature_type == 0 && node.partition_signature != [0; 16])
        || (node.signature_type == HardDrive::SIGNATURE_TYPE_MBR
            && node.partition_signature[4..].iter().any(|byte| *byte != 0))
    {
        return Err("invalid hard drive signature padding");
    }
    Ok(())
}

device_path_node! {
    /// File path media device path node (UEFI 2.10 Table 10.57).
    ///
    /// A NUL-terminated path name. A device path can contain several of these nodes,
    /// which are concatenated to form the complete path.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#file-path-media-device-path>
    @[DevicePathNode(
        DevicePathType::Media, MediaSubType::FilePath,
        measure_size = file_path_measure_size,
    )]
    @[DevicePathNodeDerive(Debug)]
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

fn file_path_measure_size(node: &FilePath) -> usize {
    node.utf16_size()
}

impl Display for FilePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::uefi::device_path::text::DevicePathText::fmt_text(
            self,
            f,
            crate::uefi::device_path::text::TextOptions::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::{
        DevicePathNode, DevicePathType, GenericDevicePathNode, Header, MediaSubType,
        test_support::{check_node, encode_node, fixture, generic_node_bytes},
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
        assert_eq!(parsed.partition_format, HardDrive::FORMAT_GPT);
        assert_eq!(parsed.signature_type, HardDrive::SIGNATURE_TYPE_GUID);
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
}
