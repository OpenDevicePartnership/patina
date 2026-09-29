//! Integration tests for the public `devpath!` macro.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::devpath;

const REQUESTED_PATH: [u8; 22] = devpath!("PciRoot(0)/Pci(0x11,0)");
const CUSTOM_VENDOR_PATH: [u8; 39] = devpath!(
    vendor-defined {
        AcmeController {
            type: hardware,
            guid: "00112233-4455-6677-8899-aabbccddeeff",
            fields: [port: u8, flags: u16le],
        },
    };
    "PciRoot(0)/AcmeController(flags=0x1234,port=3)"
);
const CUSTOM_VENDOR_MESSAGING_PATH: [u8; 27] = devpath!(
    vendor-defined {
        AcmeTransport {
            type: messaging,
            guid: "00112233-4455-6677-8899-aabbccddeeff",
            fields: [channel: u8, flags: u16le],
        },
    };
    "AcmeTransport(flags=0x1234,channel=3)"
);
const CUSTOM_VENDOR_MEDIA_PATH: [u8; 27] = devpath!(
    vendor-defined {
        AcmeMedia {
            type: media,
            guid: "00112233-4455-6677-8899-aabbccddeeff",
            fields: [instance: u8, flags: u16le],
        },
    };
    "AcmeMedia(flags=0x1234,instance=3)"
);

#[test]
fn test_devpath_public_macro_returns_owned_array() {
    let mut path = devpath!("PciRoot(0)/Pci(0x11,0)");
    path[0] = 0xff;

    assert_eq!(REQUESTED_PATH[0], 0x02);
    assert_eq!(path[0], 0xff);
}

#[test]
fn test_devpath_public_macro_encodes_multiple_instances() {
    let path: [u8; 20] = devpath!("Pci(1,0),USB(2,1)");

    assert_eq!(
        path,
        [
            0x01, 0x01, 0x06, 0x00, 0x00, 0x01, 0x7f, 0x01, 0x04, 0x00, 0x03, 0x05, 0x06, 0x00, 0x02, 0x01, 0x7f, 0xff,
            0x04, 0x00,
        ]
    );
}

#[test]
fn test_devpath_public_macro_encodes_vendor_hardware_schema() {
    assert_eq!(CUSTOM_VENDOR_PATH, devpath!("PciRoot(0)/VenHw(00112233-4455-6677-8899-aabbccddeeff,033412)"));
}

#[test]
fn test_devpath_public_macro_encodes_vendor_messaging_schema() {
    assert_eq!(CUSTOM_VENDOR_MESSAGING_PATH, devpath!("VenMsg(00112233-4455-6677-8899-aabbccddeeff,033412)"));
}

#[test]
fn test_devpath_public_macro_encodes_vendor_media_schema() {
    assert_eq!(CUSTOM_VENDOR_MEDIA_PATH, devpath!("VenMedia(00112233-4455-6677-8899-aabbccddeeff,033412)"));
}

#[cfg(feature = "unstable-device-path")]
mod runtime_cross_checks {
    use patina::{
        devpath,
        uefi::device_path::{
            node_defs::{Controller, FilePath, HardDrive, NvmExpress, PcCard, Pci, Sata},
            paths::DevicePathBuf,
        },
    };

    fn runtime_path<T>(node: T) -> DevicePathBuf
    where
        T: patina::uefi::device_path::parse_node::DevicePathNode,
    {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(node))
    }

    #[test]
    fn test_devpath_matches_runtime_fixed_hardware_nodes() {
        // Limit cross-checks to runtime nodes whose writers have packed wire layouts.
        let cases: &[(&[u8], DevicePathBuf)] = &[
            (&devpath!("Pci(0x11,0)"), runtime_path(Pci { function: 0, device: 0x11 })),
            (&devpath!("PcCard(2)"), runtime_path(PcCard { function_number: 2 })),
            (&devpath!("Ctrl(7)"), runtime_path(Controller { number: 7 })),
        ];

        for (macro_path, runtime_path) in cases {
            assert_eq!(*macro_path, runtime_path.as_bytes());
        }
    }

    #[test]
    fn test_devpath_matches_runtime_storage_and_variable_nodes() {
        let guid = [0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let cases: &[(&[u8], DevicePathBuf)] = &[
            (&devpath!("Sata(2,0,1)"), runtime_path(Sata::new(2, 0, 1))),
            (&devpath!("NVMe(1,12-34-56-78-9a-bc-de-f0)"), runtime_path(NvmExpress::new(1, 0x1234_5678_9abc_def0))),
            (
                &devpath!("HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)"),
                runtime_path(HardDrive::new_gpt(1, 0x800, 0x1000, guid)),
            ),
            (&devpath!("EFI"), runtime_path(FilePath::new("EFI"))),
            (&devpath!(r"\EFI\BOOT\BOOTX64.EFI"), runtime_path(FilePath::new(r"\EFI\BOOT\BOOTX64.EFI"))),
        ];

        for (macro_path, runtime_path) in cases {
            assert_eq!(*macro_path, runtime_path.as_bytes());
        }
    }

    #[test]
    fn test_devpath_matches_runtime_composed_path() {
        let mut runtime = runtime_path(Pci { function: 0, device: 0x11 });
        let controller = runtime_path(Controller { number: 7 });
        runtime.append_device_path(&controller);

        assert_eq!(devpath!("Pci(0x11,0)/Ctrl(7)"), runtime.as_bytes());
    }
}
