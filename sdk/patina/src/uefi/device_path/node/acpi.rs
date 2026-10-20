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

use super::TextString;
use super::{AcpiSubType, DevicePathType, device_path_node};
use crate::uefi::device_path::encoding::{AsciiNul, Vector};
use alloc::{string::String, vec::Vec};
use core::fmt;
use scroll::{
    Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

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
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "NvdimmAcpiAdr(0x2a)");
    /// assert_eq!(format!("{node:#}"), "NvdimmAcpiAdr(0x2a)");
    /// assert_eq!(format!("{node:-#}"), "NvdimmAcpiAdr(0x2a)");
    /// ```
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Nvdimm)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pread, Pwrite, Clone, Eq, PartialEq)]
    pub struct NvdimmAcpi {
        /// NFIT device handle corresponding to the NVDIMM's ACPI `_ADR`.
        pub nfit_device_handle: u32,
    }
}
impl fmt::Display for NvdimmAcpi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NvdimmAcpiAdr(0x{:x})", self.nfit_device_handle)
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
    /// use patina::uefi::device_path::node::{Acpi, EisaId, ExtendedAcpi};
    ///
    /// // `AcpiEx(HWP0002,PNP0A03,0)`: a device with the HID HWP0002 that is compatible with a PCI
    /// // root bridge (UEFI 2.10 Section 10.6.1.5).
    /// let node = ExtendedAcpi {
    ///     hid: EisaId::new("HWP0002"),
    ///     uid: 0,
    ///     cid: Acpi::PCI_ROOT_HID,
    ///     hid_string: String::new(),
    ///     uid_string: String::new(),
    ///     cid_string: String::new(),
    /// };
    /// assert_eq!(node.to_string(), "AcpiEx(HWP0002,PNP0A03,0x0,,,)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "PciRoot(0x0)");
    /// assert_eq!(format!("{node:#}"), "AcpiEx(HWP0002,PNP0A03,0x0,,,)");
    /// assert_eq!(format!("{node:-#}"), "AcpiEx(HWP0002,PNP0A03,0x0)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Acpi, AcpiSubType::ExtendedAcpi,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct ExtendedAcpi {
        /// Numeric hardware identifier.
        pub hid: EisaId,
        /// Numeric unique identifier.
        pub uid: u32,
        /// Numeric compatible identifier.
        pub cid: EisaId,
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

impl ExtendedAcpi {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.hid)
            + size_of_val(&self.uid)
            + size_of_val(&self.cid)
            + [&self.hid_string, &self.uid_string, &self.cid_string]
                .iter()
                .map(|value| value.len() + size_of::<u8>()) // NUL terminator
                .sum::<usize>()
    }
}
impl fmt::Display for ExtendedAcpi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `PciRoot` and `PcieRoot` are shortcuts, and only the shorter display-only form uses them
        // for this self: their parseable form is the smaller `Acpi` self.
        if f.sign_minus() && !f.alternate() {
            let root = if self.hid == Acpi::PCI_ROOT_HID
                || (self.cid == Acpi::PCI_ROOT_HID && self.hid != Acpi::PCIE_ROOT_HID)
            {
                Some("PciRoot")
            } else if self.hid == Acpi::PCIE_ROOT_HID || self.cid == Acpi::PCIE_ROOT_HID {
                Some("PcieRoot")
            } else {
                None
            };
            if let Some(name) = root {
                return if self.uid_string.is_empty() {
                    write!(f, "{name}(0x{:x})", self.uid)
                } else {
                    write!(f, "{name}({})", TextString(&self.uid_string))
                };
            }
        }

        let (hid, cid) = (self.hid, self.cid);
        // `AcpiExp` has no UID number, so it is used only when the UID number is zero.
        if self.hid_string.is_empty() && self.cid_string.is_empty() && !self.uid_string.is_empty() && self.uid == 0 {
            write!(f, "AcpiExp({hid},{cid},{})", TextString(&self.uid_string))
        } else if f.sign_minus() {
            // The display-only form shows each identifier once: its string when present, else its
            // numeric value.
            let hid: &dyn fmt::Display = if self.hid_string.is_empty() { &hid } else { &TextString(&self.hid_string) };
            let cid: &dyn fmt::Display = if self.cid_string.is_empty() { &cid } else { &TextString(&self.cid_string) };
            if self.uid_string.is_empty() {
                write!(f, "AcpiEx({hid},{cid},0x{:x})", self.uid)
            } else {
                write!(f, "AcpiEx({hid},{cid},{})", TextString(&self.uid_string))
            }
        } else {
            write!(
                f,
                "AcpiEx({hid},{cid},0x{:x},{},{},{})",
                self.uid,
                TextString(&self.hid_string),
                TextString(&self.cid_string),
                TextString(&self.uid_string)
            )
        }
    }
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
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "AcpiAdr(0x80010100,0x80010200)");
    /// assert_eq!(format!("{node:#}"), "AcpiAdr(0x80010100,0x80010200)");
    /// assert_eq!(format!("{node:-#}"), "AcpiAdr(0x80010100,0x80010200)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Acpi, AcpiSubType::Adr,
        measure_size = Self::payload_size,
        validate = Self::validate_fields,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct AcpiAdr {
        /// ACPI `_ADR` values, in their original order. At least one is required.
        #[scroll(ctx = Vector)]
        pub addresses: Vec<u32>,
    }
}

impl AcpiAdr {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(self.addresses.as_slice())
    }

    /// Check the field values, for `DevicePathNode::validate`.
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.addresses.is_empty() {
            return Err("ACPI address node requires at least one address");
        }
        Ok(())
    }
}

impl fmt::Display for AcpiAdr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AcpiAdr(")?;
        for (index, address) in self.addresses.iter().enumerate() {
            if index != 0 {
                f.write_str(",")?;
            }
            write!(f, "0x{address:x}")?;
        }
        f.write_str(")")
    }
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
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "PciRoot(0x0)");
    /// assert_eq!(format!("{node:#}"), "Acpi(PNP0A03,0x0)");
    /// assert_eq!(format!("{node:-#}"), "Acpi(PNP0A03,0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Acpi)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone, Eq, PartialEq)]
    pub struct Acpi {
        /// Numeric hardware identifier, the ACPI `_HID`.
        pub hid: EisaId,
        /// Numeric unique identifier, the ACPI `_UID`.
        pub uid: u32,
    }
}

impl Acpi {
    /// PCI Root Bridge HID (PNP0A03).
    pub const PCI_ROOT_HID: EisaId = EisaId::new("PNP0A03");
    /// `PCIe` Root Bridge HID (PNP0A08).
    pub const PCIE_ROOT_HID: EisaId = EisaId::new("PNP0A08");
    /// Floppy disk controller `_HID` (`PNP0604`), `Floppy` in text ([UEFI 2.10 Table 10.67](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-node-table)).
    pub const FLOPPY_HID: EisaId = EisaId::new("PNP0604");
    /// Keyboard `_HID` (`PNP0301`), `Keyboard` in text ([UEFI 2.10 Table 10.67](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-node-table)).
    pub const KEYBOARD_HID: EisaId = EisaId::new("PNP0301");
    /// Serial port `_HID` (`PNP0501`), `Serial` in text ([UEFI 2.10 Table 10.67](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-node-table)).
    pub const SERIAL_HID: EisaId = EisaId::new("PNP0501");
    /// Parallel port `_HID` (`PNP0401`), `ParallelPort` in text ([UEFI 2.10 Table 10.67](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#device-node-table)).
    pub const PARALLEL_PORT_HID: EisaId = EisaId::new("PNP0401");

    /// Create a new PCI root bridge ACPI device path node.
    pub fn new_pci_root(uid: u32) -> Self {
        Self { hid: Acpi::PCI_ROOT_HID, uid }
    }
}

/// A compressed EISA identifier: the numeric form of an ACPI `_HID` or `_CID`, such as
/// `PNP0A03` for a PCI root bridge
/// ([UEFI 2.10 Section 10.3.3](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#acpi-device-path)).
///
/// The three letters of the vendor are stored as 5-bit values in the low 16 bits, and the
/// four hex digits of the product in the high 16 bits. Zero means no identifier. The text
/// form, from `Display`, is the seven-character identifier, or `0` when the value is zero.
///
/// # Examples
///
/// ```
/// use patina::uefi::device_path::node::EisaId;
///
/// let id = EisaId::new("PNP0A03");
/// assert_eq!(id.0, 0x0a03_41d0);
/// assert_eq!(id.to_string(), "PNP0A03");
/// assert!(EisaId::try_new("PNPZZZZ").is_err());
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, PartialEq, Hash)]
pub struct EisaId(pub u32);

impl EisaId {
    /// Compress a seven-character EISA identifier, such as `PNP0A03`.
    ///
    /// # Panics
    ///
    /// Panics for an invalid identifier. Use [`Self::try_new`] for external input.
    pub const fn new(id: &str) -> Self {
        match Self::try_new(id) {
            Ok(id) => id,
            Err(_) => panic!("invalid EISA device path identifier"),
        }
    }

    /// Compress a seven-character EISA identifier, such as `PNP0A03`.
    ///
    /// # Errors
    ///
    /// Rejects identifiers without three ASCII letters followed by four hex digits.
    #[allow(clippy::indexing_slicing)]
    pub const fn try_new(id: &str) -> Result<Self, &'static str> {
        let bytes = id.as_bytes();
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
        Ok(Self((product << 16) | vendor))
    }
}

impl fmt::Display for EisaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        if value == 0 {
            return f.write_str("0");
        }
        let vendor = value & 0x7fff;
        let product = (value >> 16) & 0xffff;
        let a = char::from_u32(((vendor >> 10) & 0x1f) + u32::from(b'A') - 1).unwrap_or('?');
        let b = char::from_u32(((vendor >> 5) & 0x1f) + u32::from(b'A') - 1).unwrap_or('?');
        let c = char::from_u32((vendor & 0x1f) + u32::from(b'A') - 1).unwrap_or('?');
        write!(f, "{a}{b}{c}{product:04X}")
    }
}

impl fmt::Debug for EisaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EisaId").field(&format_args!("{self}")).finish()
    }
}

// Scroll encoding: a 32-bit value, like the `u32` it wraps.
impl<'a> TryFromCtx<'a, scroll::Endian> for EisaId {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let (value, size) = u32::try_from_ctx(src, ctx)?;
        Ok((Self(value), size))
    }
}

impl TryIntoCtx<scroll::Endian> for &EisaId {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

impl TryIntoCtx<scroll::Endian> for EisaId {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

impl fmt::Display for Acpi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !f.alternate() {
            // The shortcut text names of well-known `_HID` values (UEFI 2.10 Table 10.67).
            let shortcut = match self.hid {
                Acpi::PCI_ROOT_HID => Some("PciRoot"),
                Acpi::PCIE_ROOT_HID => Some("PcieRoot"),
                Acpi::FLOPPY_HID => Some("Floppy"),
                Acpi::KEYBOARD_HID => Some("Keyboard"),
                Acpi::SERIAL_HID => Some("Serial"),
                Acpi::PARALLEL_PORT_HID => Some("ParallelPort"),
                _ => None,
            };
            if let Some(name) = shortcut {
                return write!(f, "{name}(0x{:x})", self.uid);
            }
        }
        write!(f, "Acpi({},0x{:x})", self.hid, self.uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::tests::{check_node, encode_node, fixture};
    use alloc::{format, string::String, vec};

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
                hid: EisaId(1),
                uid: 2,
                cid: EisaId(3),
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
                hid: EisaId(0),
                uid: 0,
                cid: EisaId(0),
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
            assert!(EisaId::try_new(hid).is_err());
        }
        assert_eq!(EisaId::try_new("PNP0A03"), Ok(EisaId(0x0a0341d0)));
        assert_eq!(EisaId::try_new("pnp0a08"), Ok(EisaId(0x0a0841d0)));
    }

    /// Check the text of each acpi node in the three text forms, and that the parseable forms
    /// parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_acpi() {
        use crate::uefi::device_path::node::tests::check_text;
        use alloc::format;

        check_text!(r"PciRoot(1)", r"PciRoot(0x1)", r"PciRoot(0x1)", r"Acpi(PNP0A03,0x1)");
        check_text!(r"PcieRoot(2)", r"PcieRoot(0x2)", r"PcieRoot(0x2)", r"Acpi(PNP0A08,0x2)");
        check_text!(r"Floppy(0)", r"Floppy(0x0)", r"Floppy(0x0)", r"Acpi(PNP0604,0x0)");
        check_text!(r"Keyboard(0)", r"Keyboard(0x0)", r"Keyboard(0x0)", r"Acpi(PNP0301,0x0)");
        check_text!(r"Serial(1)", r"Serial(0x1)", r"Serial(0x1)", r"Acpi(PNP0501,0x1)");
        check_text!(r"ParallelPort(0)", r"ParallelPort(0x0)", r"ParallelPort(0x0)", r"Acpi(PNP0401,0x0)");
        check_text!(r"Acpi(HWP0002,3)", r"Acpi(HWP0002,0x3)", r"Acpi(HWP0002,0x3)", r"Acpi(HWP0002,0x3)");
        check_text!(
            r"AcpiEx(HWP0002,PNP0A03,0)",
            r"AcpiEx(HWP0002,PNP0A03,0x0,,,)",
            r"PciRoot(0x0)",
            r"AcpiEx(HWP0002,PNP0A03,0x0,,,)"
        );
        check_text!(
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)",
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)",
            r"PciRoot(UniqueId)",
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)"
        );
        check_text!(
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)"
        );

        // A shortcut is not used without `AllowShortcuts`, even in the display-only form.
        let acpi_ex = ExtendedAcpi {
            hid: Acpi::PCI_ROOT_HID,
            uid: 0,
            cid: EisaId(0),
            hid_string: String::new(),
            uid_string: String::new(),
            cid_string: String::new(),
        };
        assert_eq!(format!("{acpi_ex:-}"), "PciRoot(0x0)");
        assert_eq!(format!("{acpi_ex:-#}"), "AcpiEx(PNP0A03,0,0x0)");
    }

    /// `AcpiExp` has no UID number, and strings that the text cannot hold bare are quoted.
    #[test]
    fn test_device_path_extended_acpi_text_uid_and_quoting() {
        let node = ExtendedAcpi {
            hid: EisaId::new("HWP0002"),
            uid: 1,
            cid: EisaId(0),
            hid_string: String::new(),
            uid_string: "Unique Id".into(),
            cid_string: String::new(),
        };
        // A nonzero UID number cannot use `AcpiExp`, so the full form keeps it.
        assert_eq!(format!("{node}"), r#"AcpiEx(HWP0002,0,0x1,,,"Unique Id")"#);
        let node = ExtendedAcpi { uid: 0, ..node };
        assert_eq!(format!("{node}"), r#"AcpiExp(HWP0002,0,"Unique Id")"#);
    }
}
