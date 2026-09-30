//! ACPI device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{AcpiSubType, DevicePathType};
use crate::{
    device_path_node,
    uefi::device_path::{
        encoding::{AsciiNul, Vector},
        parse_node::{MeasureSize, Validate},
    },
};
use alloc::{string::String, vec::Vec};
use core::fmt::Display;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// An ACPI physical NVDIMM device, distinct from a messaging namespace node.
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Nvdimm)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct NvdimmAcpi {
        /// NFIT device handle corresponding to the NVDIMM's ACPI `_ADR`.
        pub nfit_device_handle: u32,
    }
}

device_path_node! {
    /// Expanded ACPI identifiers, with optional ASCII string overrides.
    ///
    /// A nonempty string supersedes its corresponding numeric identifier. Each string
    /// is stored as NUL-terminated ASCII, as specified by UEFI 2.11 Table 10.10.
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::ExtendedAcpi)]
    @[DevicePathNodeDerive(Debug, Display, Validate)]
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

impl MeasureSize for ExtendedAcpi {
    fn measure_size(&self) -> usize {
        size_of_val(&self.hid)
            + size_of_val(&self.uid)
            + size_of_val(&self.cid)
            + [&self.hid_string, &self.uid_string, &self.cid_string]
                .iter()
                .map(|value| value.len() + size_of::<u8>()) // NUL terminator
                .sum::<usize>()
    }
}

device_path_node! {
    /// ACPI address node containing one or more display device addresses.
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Adr)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct AcpiAdr {
        /// ACPI `_ADR` values, in their original order. At least one is required.
        #[scroll(ctx = Vector)]
        pub addresses: Vec<u32>,
    }
}

impl Validate for AcpiAdr {
    fn validate(&self) -> Result<(), &'static str> {
        if self.addresses.is_empty() {
            return Err("ACPI address node requires at least one address");
        }
        Ok(())
    }
}

impl MeasureSize for AcpiAdr {
    fn measure_size(&self) -> usize {
        size_of_val(self.addresses.as_slice())
    }
}

device_path_node! {
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Acpi)]
    @[DevicePathNodeDerive(Debug, MeasureSize, Validate)]
    #[derive(Pwrite, Pread, Clone)]
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-device-path>
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
        match self.hid {
            Acpi::PCI_ROOT_HID => f.debug_tuple("PciRoot").field(&self.uid).finish(),
            _ => f.debug_tuple("Acpi").field(&self.hid).field(&self.uid).finish(),
        }
    }
}
