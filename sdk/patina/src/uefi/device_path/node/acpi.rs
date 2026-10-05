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

// cspell:ignore BCDEFGHIJ PNPZZZZ

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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::NvdimmAcpi;
    ///
    /// let node = NvdimmAcpi { nfit_device_handle: 0x2a };
    /// assert_eq!(node.to_string(), "NvdimmAcpiAdr(0x2a)");
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{Acpi, ExtendedAcpi};
    ///
    /// let node = ExtendedAcpi {
    ///     hid: Acpi::PCI_ROOT_HID,
    ///     uid: 0,
    ///     cid: 0,
    ///     hid_string: String::new(),
    ///     uid_string: String::new(),
    ///     cid_string: String::new(),
    /// };
    /// assert_eq!(node.to_string(), "AcpiEx(PNP0A03,0,0x0,,,)");
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::AcpiAdr;
    ///
    /// let node = AcpiAdr { addresses: vec![0x80010100, 0x80010200] };
    /// assert_eq!(node.to_string(), "AcpiAdr(0x80010100,0x80010200)");
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Acpi;
    ///
    /// let node = Acpi::new_pci_root(0);
    /// assert_eq!(node.to_string(), "PciRoot(0x0)");
    /// ```
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::test_support::{check_node, encode_node, fixture};
    use alloc::{format, string::String, vec};
    use scroll::ctx::TryFromCtx;

    fixture!(test_device_path_nvdimm_acpi, NvdimmAcpi { nfit_device_handle: 0x04030201 }, [2, 4, 8, 0, 1, 2, 3, 4]);

    #[test]
    fn test_device_path_derived_nvdimm_formatting() {
        let node = NvdimmAcpi { nfit_device_handle: 42 };
        assert_eq!(format!("{node}"), "NvdimmAcpiAdr(0x2a)");
        assert_eq!(format!("{node:?}"), "NvdimmAcpi { type: 2, sub_type: 4, length: 8, nfit_device_handle: 42 }");
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
    fn test_device_path_existing_wire_sizes_acpi() {
        check_node(Acpi::new_pci_root(0), &[2, 1, 12, 0, 0xd0, 0x41, 3, 0x0a, 0, 0, 0, 0]);
    }

    #[test]
    fn test_device_path_invalid_variable_payloads_acpi() {
        assert!(encode_node(AcpiAdr { addresses: vec![] }).is_err());
        assert!(
            encode_node(ExtendedAcpi {
                hid: 0,
                uid: 0,
                cid: 0,
                hid_string: "\u{e9}".into(),
                uid_string: String::new(),
                cid_string: String::new(),
            })
            .is_err()
        );
        assert!(ExtendedAcpi::try_from_ctx(&[0; 14], scroll::LE).is_err());
        let mut non_ascii = [0u8; 15];
        non_ascii[12] = 0xe9;
        assert!(ExtendedAcpi::try_from_ctx(&non_ascii, scroll::LE).is_err());
    }

    #[test]
    fn test_device_path_existing_validation_acpi() {
        for hid in ["", "PNP", "1NP0A03", "PNPZZZZ", "\u{e9}NP0A03"] {
            assert!(Acpi::try_eisa_id(hid).is_err());
        }
        assert_eq!(Acpi::try_eisa_id("PNP0A03"), Ok(0x0a0341d0));
        assert_eq!(Acpi::try_eisa_id("pnp0a08"), Ok(0x0a0841d0));
    }
}
