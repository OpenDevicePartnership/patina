//! ACPI device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{AcpiSubType, DevicePathType, check_length, invalid};
use crate::{
    device_path_node,
    uefi::device_path::{
        encoding::AsciiNul,
        parse_node::{DevicePathNode, Header, MeasureSize},
    },
};
use alloc::{string::String, vec::Vec};
use core::fmt::Display;
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

device_path_node! {
    /// An ACPI physical NVDIMM device, distinct from a messaging namespace node.
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Nvdimm)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
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
    @[DevicePathNodeDerive(Debug, Display)]
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

/// ACPI address node containing one or more display device addresses.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AcpiAdr {
    /// ACPI `_ADR` values, in their original order.
    pub addresses: Vec<u32>,
}

impl MeasureSize for AcpiAdr {
    fn measure_size(&self) -> usize {
        size_of_val(self.addresses.as_slice())
    }
}

impl DevicePathNode for AcpiAdr {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Acpi as u8, AcpiSubType::Adr as u8, Header::size_of_header() + self.measure_size())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Acpi as u8 && subtype == AcpiSubType::Adr as u8
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

impl TryFromCtx<'_, scroll::Endian> for AcpiAdr {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], endian: Endian) -> Result<(Self, usize), scroll::Error> {
        if !data.len().is_multiple_of(4) {
            return Err(invalid("ACPI address payload is not a sequence of u32 values"));
        }
        let mut addresses = Vec::new();
        let mut offset = 0;
        while offset < data.len() {
            addresses.push(data.gread_with(&mut offset, endian)?);
        }
        let (node, consumed) = (Self { addresses }, offset);
        check_length(node.header().length - 4)?;
        if node.addresses.is_empty() {
            return Err(invalid("ACPI address node requires at least one address"));
        }
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for AcpiAdr {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], endian: Endian) -> Result<usize, scroll::Error> {
        check_length(self.header().length - 4)?;
        if self.addresses.is_empty() {
            return Err(invalid("ACPI address node requires at least one address"));
        }
        let mut offset = 0;
        for address in &self.addresses {
            data.gwrite_with(*address, &mut offset, endian)?;
        }
        Ok(offset)
    }
}

impl core::fmt::Display for AcpiAdr {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    @[DevicePathNode(DevicePathType::Acpi, AcpiSubType::Acpi)]
    @[DevicePathNodeDerive(Debug, MeasureSize)]
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
