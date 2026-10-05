//! ACPI device path nodes (UEFI 2.10 Section 10.3.3, type `0x02`).
//!
//! An ACPI device path describes a device whose enumeration is not defined in an
//! industry-standard fashion. The device is described with ACPI AML in the ACPI
//! namespace, and the device path links to it, so the stored identifiers must match
//! the ACPI namespace exactly.
//!
//! The ACPI node holds numeric `_HID` and `_UID` values and should be used when
//! possible because it is smaller. The Expanded ACPI node is required when `_HID` or
//! `_UID` is a string, or when a `_CID` is needed. A PCI root bridge uses the
//! `PNP0A03` ID, in `_HID` or in the expanded node's `_CID` (Section 10.4.2).
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-device-path>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{AcpiSubType, DevicePathType};
use crate::{
    device_path_node,
    uefi::device_path::encoding::{AsciiNul, Vector},
};
use alloc::{string::String, vec::Vec};
use core::fmt::Display;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// ACPI NVDIMM device path node (UEFI 2.10 Table 10.12).
    ///
    /// An NVDIMM device identified by its ACPI NFIT device handle. This is distinct from
    /// the messaging [`Nvdimm`](super::Nvdimm) namespace node.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#nvdimm-device-path>
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Nvdimm)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct NvdimmAcpi {
        /// NFIT device handle corresponding to the NVDIMM's ACPI `_ADR`.
        pub nfit_device_handle: u32,
    }
}

device_path_node! {
    /// Expanded ACPI device path node (UEFI 2.10 Table 10.10).
    ///
    /// Numeric `_HID`, `_UID`, and `_CID` values with NUL-terminated ASCII string
    /// forms. A nonempty string supersedes its corresponding numeric identifier.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-device-path>
    @[DevicePathNode(
        DevicePathType::Acpi, AcpiSubType::ExtendedAcpi,
        measure_size = extended_acpi_measure_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct ExtendedAcpi {
        /// Numeric hardware identifier.
        pub hid: u32,
        /// Numeric unique identifier.
        pub uid: u32,
        /// Numeric compatible identifier.
        pub cid: u32,
        /// Hardware identifier override, without its terminator.
        #[scroll(ctx = AsciiNul)]
        pub hid_string: String,
        /// Unique identifier override, without its terminator.
        #[scroll(ctx = AsciiNul)]
        pub uid_string: String,
        /// Compatible identifier override, without its terminator.
        #[scroll(ctx = AsciiNul)]
        pub cid_string: String,
    }
}

fn extended_acpi_measure_size(node: &ExtendedAcpi) -> usize {
    size_of_val(&node.hid)
        + size_of_val(&node.uid)
        + size_of_val(&node.cid)
        + [&node.hid_string, &node.uid_string, &node.cid_string]
            .iter()
            .map(|value| value.len() + size_of::<u8>()) // NUL terminator
            .sum::<usize>()
}

device_path_node! {
    /// ACPI `_ADR` device path node (UEFI 2.10 Table 10.11).
    ///
    /// One or more ACPI `_ADR` values for video output devices used by the Graphics
    /// Output Protocol. At least one `_ADR` value is required.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-adr-device-path>
    @[DevicePathNode(
        DevicePathType::Acpi, AcpiSubType::Adr,
        measure_size = acpi_adr_measure_size,
        validate = acpi_adr_validate,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct AcpiAdr {
        /// ACPI `_ADR` values, in their original order. At least one is required.
        #[scroll(ctx = Vector)]
        pub addresses: Vec<u32>,
    }
}

fn acpi_adr_measure_size(node: &AcpiAdr) -> usize {
    size_of_val(node.addresses.as_slice())
}

fn acpi_adr_validate(node: &AcpiAdr) -> Result<(), &'static str> {
    if node.addresses.is_empty() {
        return Err("ACPI address node requires at least one address");
    }
    Ok(())
}

device_path_node! {
    /// ACPI device path node (UEFI 2.10 Table 10.9).
    ///
    /// Numeric 32-bit `_HID` and `_UID` values that must match the ACPI namespace. This
    /// node is smaller than [`ExtendedAcpi`] and should be used when possible.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-device-path>
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Acpi)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Acpi {
        /// _HID
        pub hid: u32,
        /// _UID
        pub uid: u32,
    }
}

impl Acpi {
    /// PCI Root Bridge HID (PNP0A03).
    pub const PCI_ROOT_HID: u32 = Acpi::eisa_id("PNP0A03");
    /// `PCIe` Root Bridge HID (PNP0A08).
    pub const PCIE_ROOT_HID: u32 = Acpi::eisa_id("PNP0A08");

    /// Create a new PCI root bridge ACPI device path node.
    pub fn new_pci_root(uid: u32) -> Self {
        Self { hid: Acpi::PCI_ROOT_HID, uid }
    }

    /// Compress a seven-character identifier using UEFI device path byte order.
    ///
    /// # Panics
    ///
    /// Panics for an invalid identifier. Use [`Self::try_eisa_id`] for external input.
    pub const fn eisa_id(hid: &str) -> u32 {
        match Self::try_eisa_id(hid) {
            Ok(id) => id,
            Err(_) => panic!("invalid EISA device path identifier"),
        }
    }

    /// Compress a seven-character EISA identifier in UEFI device path byte order.
    ///
    /// # Errors
    ///
    /// Rejects identifiers without three ASCII letters followed by four hex digits.
    #[allow(clippy::indexing_slicing)]
    pub const fn try_eisa_id(hid: &str) -> Result<u32, &'static str> {
        let bytes = hid.as_bytes();
        if bytes.len() != 7 {
            return Err("EISA identifier must contain seven ASCII characters");
        }
        let mut vendor = 0u32;
        let mut index = 0;
        while index < 3 {
            if !bytes[index].is_ascii_alphabetic() {
                return Err("EISA vendor must contain ASCII letters");
            }
            vendor = (vendor << 5) | (bytes[index].to_ascii_uppercase() - b'A' + 1) as u32;
            index += 1;
        }
        let mut product = 0u32;
        while index < 7 {
            let digit = match (bytes[index] as char).to_digit(16) {
                Some(digit) => digit,
                None => return Err("EISA product must contain hex digits"),
            };
            product = (product << 4) | digit;
            index += 1;
        }
        Ok((product << 16) | vendor)
    }
}

impl Display for Acpi {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::uefi::device_path::text::DevicePathText::fmt_text(
            self,
            f,
            crate::uefi::device_path::text::TextOptions::default(),
        )
    }
}
