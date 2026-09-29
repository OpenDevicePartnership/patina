//! UEFI device path node definitions, grouped by category.
//!
//! Binary conversions use `DevicePathNode` and the `scroll` conversion traits.
//! Diagnostic display is not a complete canonical UEFI text conversion API.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

/// ACPI device path nodes.
pub mod acpi;
/// BIOS Boot Specification device path nodes.
pub mod bios;
/// Device path end markers.
pub mod end;
/// Hardware device path nodes.
pub mod hardware;
/// Media device path nodes.
pub mod media;
/// Messaging device path nodes.
pub mod messaging;
/// Device path type and subtype identifiers.
pub mod node_types;

pub use acpi::*;
pub use bios::*;
pub use end::*;
pub use hardware::*;
pub use media::*;
pub use messaging::*;
pub use node_types::*;

use crate::uefi::device_path::parse_node::{DevicePathNode, UnknownDevicePathNode};
use alloc::{boxed::Box, string::String};
use scroll::ctx::TryFromCtx;

#[cfg(test)]
use alloc::vec::Vec;
#[cfg(test)]
use scroll::{Pread, Pwrite, ctx::TryIntoCtx};

fn invalid(msg: &'static str) -> scroll::Error {
    scroll::Error::BadInput { size: 0, msg }
}

fn check_length(size: usize) -> Result<(), scroll::Error> {
    if size > usize::from(u16::MAX) - 4 {
        return Err(invalid("device path payload exceeds the 16-bit node length"));
    }
    Ok(())
}

fn ascii(value: &str) -> Result<(), scroll::Error> {
    if !value.is_ascii() || value.as_bytes().contains(&0) {
        return Err(invalid("device path string must be ASCII without embedded NUL"));
    }
    Ok(())
}

fn read_ascii(data: &[u8], offset: &mut usize) -> Result<String, scroll::Error> {
    let remaining = data.get(*offset..).ok_or_else(|| invalid("missing ASCII string"))?;
    let len = remaining.iter().position(|byte| *byte == 0).ok_or_else(|| invalid("missing ASCII terminator"))?;
    let bytes = remaining.get(..len).ok_or_else(|| invalid("invalid ASCII string bounds"))?;
    let value = core::str::from_utf8(bytes).map_err(|_| invalid("invalid ASCII string"))?;
    ascii(value)?;
    *offset += len + 1;
    Ok(String::from(value))
}

/// Decode a node using its type and subtype, preserving unsupported nodes.
///
/// Malformed recognized nodes are logged and preserved for diagnostic display.
/// Use [`try_cast_to_dyn_device_path_node`] when decoding errors must be propagated.
pub fn cast_to_dyn_device_path_node(unknown: UnknownDevicePathNode<'_>) -> Box<dyn DevicePathNode + '_> {
    match try_cast_to_dyn_device_path_node(unknown.clone()) {
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
    unknown: UnknownDevicePathNode<'_>,
) -> Result<Box<dyn DevicePathNode + '_>, scroll::Error> {
    if unknown.header.length != 4 + unknown.data.len() {
        return Err(invalid("device path header and payload lengths differ"));
    }
    unknown.validate()?;
    macro_rules! cast {
        ($unknown:expr, $($ty:ty),*) => {
            match unknown.header {
                $(
                    h if <$ty>::is_type(h.r#type, h.sub_type) => {
                        let (node, consumed) = <$ty>::try_from_ctx(unknown.data, scroll::LE)?;
                        if consumed != unknown.data.len() {
                            return Err(invalid("unexpected trailing device path payload"));
                        }
                        node.validate()?;
                        Some(Box::new(node) as Box<dyn DevicePathNode>)
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
        HardwareVendor,
        // ACPI nodes.
        Acpi,
        ExtendedAcpi,
        AcpiAdr,
        NvdimmAcpi,
        // Messaging nodes.
        NvmExpress,
        Sata,
        Atapi,
        Scsi,
        FiberChannel,
        FiberChannelEx,
        Ieee1394,
        Usb,
        UsbWwid,
        UsbClass,
        DeviceLogicalUnit,
        I2oRandomBlockStorageClass,
        MacAddress,
        IpV4,
        IpV6,
        Vlan,
        InfiniBand,
        Uart,
        MessagingVendor,
        SasEx,
        Iscsi,
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
        // Media nodes.
        FilePath,
        HardDrive,
        CdRom,
        MediaVendor,
        MediaProtocol,
        PiwgFirmwareFile,
        PiwgFirmwareVolume,
        RelativeOffsetRange,
        RamDisk,
        // BIOS nodes.
        Bios,
        // End nodes
        EndEntire,
        EndInstance
    ) {
        Some(n) => Ok(n),
        None => Ok(Box::new(unknown)),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::uefi::device_path::parse_node;
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
        // DATA_SIZE = 4 + 8 + 8 + 16 + 1 + 1 = 38 bytes (no header in TryIntoCtx)
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
        let node = parse_node::UnknownDevicePathNode {
            header: parse_node::Header {
                r#type: DevicePathType::Media as u8,
                sub_type: MediaSubType::HardDrive as u8,
                length: parse_node::Header::size_of_header() + 38,
            },
            data: &data,
        };
        let parsed = HardDrive::try_from_node(&node).expect("valid HD node should parse");
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
        let node = parse_node::UnknownDevicePathNode {
            header: parse_node::Header {
                r#type: DevicePathType::Messaging as u8, // not Media
                sub_type: MediaSubType::HardDrive as u8,
                length: parse_node::Header::size_of_header() + 38,
            },
            data: &data,
        };
        assert!(HardDrive::try_from_node(&node).is_none());
    }

    #[test]
    fn test_hard_drive_try_from_node_short_data() {
        let data = [0u8; 16]; // shorter than HardDrive::DATA_SIZE (38)
        let node = parse_node::UnknownDevicePathNode {
            header: parse_node::Header {
                r#type: DevicePathType::Media as u8,
                sub_type: MediaSubType::HardDrive as u8,
                length: parse_node::Header::size_of_header() + 16,
            },
            data: &data,
        };
        assert!(HardDrive::try_from_node(&node).is_none());
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
    use crate::uefi::device_path::{
        node::try_cast_to_dyn_device_path_node,
        parse_node::{DevicePathNode, UnknownDevicePathNode},
    };
    use alloc::{format, vec};
    use scroll::ctx::{TryFromCtx, TryIntoCtx};

    fn check_fixture<T>(node: T, wire: &[u8])
    where
        T: DevicePathNode + Clone + Eq,
        T: for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
        T: TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        node.validate().unwrap();
        assert_eq!(node.header().length, wire.len());
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

        let unknown: UnknownDevicePathNode<'_> = wire.pread_with(0, scroll::LE).unwrap();
        assert_eq!(unknown.header, node.header());
        let dynamic = try_cast_to_dyn_device_path_node(unknown.clone()).unwrap();
        assert_eq!(dynamic.header(), node.header());
        let type_name = core::any::type_name::<T>().rsplit("::").next().unwrap();
        let debug = format!("{dynamic:?}");
        assert!(debug.starts_with(&format!("{type_name} {{")), "unexpected decoded type: {debug}");
        assert_eq!(debug, format!("{node:?}"));
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
                wire[..length].pread_with::<UnknownDevicePathNode<'_>>(0, scroll::LE).is_err(),
                "accepted truncated node at {length}"
            );
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
        let mut output = vec![0xa5; node.header().length];
        assert!(node.clone().write_into(&mut output).is_err());
        assert!(output.iter().all(|byte| *byte == 0xa5));
        assert!(output.pwrite_with(node.clone(), 0, scroll::LE).is_err());
        assert!(output.iter().all(|byte| *byte == 0xa5));

        assert_eq!(payload.len() + 4, node.header().length);
        assert!(payload.pread_with::<T>(0, scroll::LE).is_err());
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
    fn test_device_path_atapi_selectors() {
        for primary_secondary in 0..=1 {
            for slave_master in 0..=1 {
                Atapi { primary_secondary, slave_master, lun: u16::MAX }.validate().unwrap();
            }
        }
        for invalid in [2, u8::MAX] {
            check_invalid(Atapi { primary_secondary: invalid, slave_master: 0, lun: 0 }, &[invalid, 0, 0, 0]);
            check_invalid(Atapi { primary_secondary: 0, slave_master: invalid, lun: 0 }, &[0, invalid, 0, 0]);
        }
    }

    #[test]
    fn test_device_path_vlan_range() {
        Vlan { vlan_id: 0 }.validate().unwrap();
        Vlan { vlan_id: 4094 }.validate().unwrap();
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
                IpV6 { ip_address_origin, prefix_length, ..node.clone() }.validate().unwrap();
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
                Uart { parity, stop_bits, ..node.clone() }.validate().unwrap();
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
            BluetoothLE { address: [0; 6], address_type }.validate().unwrap();
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
    use crate::uefi::device_path::{parse_node::Header, paths::DevicePathBuf};
    use alloc::{format, vec};

    fn check_node<T>(node: T, expected: &[u8])
    where
        T: DevicePathNode + Clone + for<'a> TryFromCtx<'a, scroll::Endian, Error = scroll::Error>,
    {
        assert_eq!(node.header().length, expected.len());
        let mut encoded = vec![0xa5; expected.len() + 3];
        assert_eq!(node.clone().write_into(&mut encoded).unwrap(), expected.len());
        assert_eq!(&encoded[..expected.len()], expected);
        assert_eq!(&encoded[expected.len()..], &[0xa5; 3]);
        for len in 0..expected.len() {
            assert!(node.clone().write_into(&mut vec![0; len]).is_err(), "short output {len}");
            assert!(expected[..len].pread_with::<UnknownDevicePathNode>(0, scroll::LE).is_err());
        }
        let (parsed, consumed) = T::try_from_ctx(&expected[4..], scroll::LE).unwrap();
        assert_eq!(consumed + 4, expected.len());
        let mut roundtrip = vec![0; expected.len()];
        parsed.write_into(&mut roundtrip).unwrap();
        assert_eq!(roundtrip, expected);
        let raw = expected.pread_with::<UnknownDevicePathNode>(0, scroll::LE).unwrap();
        let decoded = try_cast_to_dyn_device_path_node(raw).unwrap();
        assert_eq!(decoded.header().length, expected.len());
        assert!(!format!("{decoded:?}").starts_with("UnknownDevicePathNode"));
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
    fn test_device_path_checked_lengths_and_dispatch_errors() {
        let mut output = [0u8; 4];
        assert!(Header::new(1, 1, 65536).try_into_ctx(&mut output, scroll::LE).is_err());
        assert!(Header::new(1, 1, 3).try_into_ctx(&mut output, scroll::LE).is_err());
        for length in 0..4 {
            assert!([1, 1, length, 0].pread_with::<UnknownDevicePathNode>(0, scroll::LE).is_err());
        }
        for data in [&[1u8][..], &[1, 2, 3][..]] {
            let raw = UnknownDevicePathNode { header: Header::new(1, 1, data.len() + 4), data };
            assert!(try_cast_to_dyn_device_path_node(raw).is_err());
        }
        let unknown = UnknownDevicePathNode { header: Header::new(6, 1, 6), data: &[1, 2] };
        assert!(
            format!("{:?}", try_cast_to_dyn_device_path_node(unknown.clone()).unwrap())
                .starts_with("UnknownDevicePathNode")
        );
        let mut encoded = [0; 6];
        unknown.write_into(&mut encoded).unwrap();
        assert_eq!(encoded, [6, 1, 6, 0, 1, 2]);
    }

    #[test]
    fn test_device_path_invalid_variable_payloads() {
        assert!(AcpiAdr { addresses: vec![] }.validate().is_err());
        assert!(Dns { is_ipv6: false, addresses: vec![] }.validate().is_err());
        assert!(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0; 65] }.validate().is_err());
        assert!(UsbWwid { interface: 0, vendor_id: 0, product_id: 0, serial_number: vec![0xd800] }.validate().is_err());
        assert!(RestService { service: RestServiceType::Redfish, access_mode: 3 }.validate().is_err());
        assert!(Uri { uri: "a\0b".into() }.validate().is_err());
        assert!(Uri { uri: "\u{e9}".into() }.validate().is_err());
        assert!(Uri { uri: "a".repeat(65532) }.validate().is_err());
        assert!(Bios { device_type: 0, status_flag: 0, description_str: "a\0b".into() }.validate().is_err());
        assert!(
            ExtendedAcpi {
                hid: 0,
                uid: 0,
                cid: 0,
                hid_string: "\u{e9}".into(),
                uid_string: "".into(),
                cid_string: "".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Iscsi { protocol: 1, options: 0, lun: [0; 8], portal_group: 0, target_name: "iqn".into() }
                .validate()
                .is_err()
        );
        assert!(NvmeOf { identifier_type: 0, identifier: [0; 16], subsystem_nqn: "nqn".into() }.validate().is_err());
        assert!(NvmeOf { identifier_type: 1, identifier: [1; 16], subsystem_nqn: "nqn".into() }.validate().is_err());
        assert!(FilePath::try_from_ctx(&[b'A', 0], scroll::LE).is_err());
        assert!(FilePath::try_from_ctx(&[b'A'], scroll::LE).is_err());
        assert!(Bios::try_from_ctx(&[0, 0, 0, 0, b'A'], scroll::LE).is_err());
        assert!(ExtendedAcpi::try_from_ctx(&[0; 14], scroll::LE).is_err());
        assert!(Dns::try_from_ctx(&[0; 16], scroll::LE).is_err());
        assert!(RestService::try_from_ctx(&[0xff, 1], scroll::LE).is_err());
        assert!(Uri::try_from_ctx(&[0xff], scroll::LE).is_err());
    }

    #[test]
    fn test_device_path_existing_validation() {
        assert!(Sata::new(u16::MAX, 0, 0).validate().is_err());
        for id in [0, u32::MAX] {
            assert!(NvmExpress::new(id, 0).validate().is_err());
        }
        assert!(FilePath::new("a\0b").validate().is_err());
        assert!(FilePath::new("\u{1f600}").validate().is_err());
        for hid in ["", "PNP", "1NP0A03", "PNPZZZZ", "\u{e9}NP0A03"] {
            assert!(Acpi::try_eisa_id(hid).is_err());
        }
        assert_eq!(Acpi::try_eisa_id("PNP0A03"), Ok(0x0a0341d0));
        assert_eq!(Acpi::try_eisa_id("pnp0a08"), Ok(0x0a0841d0));
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
