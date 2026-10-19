//! Hardware device path nodes (UEFI 2.10 Section 10.3.2, type `0x01`).
//!
//! A hardware device path defines how a device is attached to the resource domain of
//! the system: its shared memory, memory-mapped I/O, and I/O space. A device path can
//! contain several levels of hardware nodes, such as a PC Card device attached to a
//! PC Card PCI controller.
//!
//! Hardware nodes are used for buses that have a standard enumeration algorithm and
//! produce resources in the coherency domain of the system (Section 10.4.4).
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#hardware-device-path>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, HardwareSubType, device_path_node};
use crate::uefi::device_path::encoding::Vector;
use crate::{BinaryGuid, standard::efi};
use alloc::vec::Vec;
use core::fmt;
use scroll::{
    Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

device_path_node! {
    /// Vendor-defined hardware device path node (UEFI 2.10 Table 10.6).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::{BinaryGuid, uefi::device_path::node::HardwareVendor};
    ///
    /// let node = HardwareVendor {
    ///     guid: BinaryGuid::from_string("00112233-4455-6677-8899-AABBCCDDEEFF"),
    ///     data: vec![0x01, 0x02],
    /// };
    /// assert_eq!(node.to_string(), "VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)");
    /// assert_eq!(format!("{node:#}"), "VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)");
    /// assert_eq!(format!("{node:-#}"), "VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)");
    /// ```
    @[DevicePathNode(
        DevicePathType::Hardware, HardwareSubType::Vendor,
        measure_size = Self::payload_size,
    )]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct HardwareVendor {
        /// Vendor GUID.
        pub guid: BinaryGuid,
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Vector)]
        pub data: Vec<u8>,
    }
}
impl fmt::Display for HardwareVendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "VenHw({}", node.guid)?;
        if !node.data.is_empty() {
            f.write_str(",")?;
            for byte in &node.data {
                write!(f, "{byte:02x}")?;
            }
        }
        f.write_str(")")
    }
}

impl HardwareVendor {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

device_path_node! {
    /// PCI device path node (UEFI 2.10 Table 10.3).
    ///
    /// The PCI device and function number of a device. The node must be preceded by an
    /// ACPI node that identifies the PCI root bus.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pci-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Pci;
    ///
    /// // The USB host controller of `PciRoot(0)/PCI(31,2)/USB(0,0)` (UEFI 2.10 Section 10.3.4.5.1).
    /// let node = Pci { device: 31, function: 2 };
    /// assert_eq!(node.to_string(), "Pci(0x1f,0x2)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Pci(0x1f,0x2)");
    /// assert_eq!(format!("{node:#}"), "Pci(0x1f,0x2)");
    /// assert_eq!(format!("{node:-#}"), "Pci(0x1f,0x2)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pci, validate = Self::validate_fields)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone, Eq, PartialEq)]
    pub struct Pci {
        /// PCI Function Number.
        pub function: u8,
        /// PCI Device Number.
        pub device: u8,
    }
}
impl fmt::Display for Pci {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Pci(0x{:x},0x{:x})", node.device, node.function)
    }
}

impl Pci {
    /// Check the field values, for `DevicePathNode::validate`: a PCI device number is 0-31 and a
    /// function number is 0-7 (UEFI 2.10 Table 10.67).
    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.device > 31 || self.function > 7 {
            return Err("PCI device must be 0-31 and function 0-7");
        }
        Ok(())
    }
}

device_path_node! {
    /// PC Card device path node (UEFI 2.10 Table 10.4).
    ///
    /// The PC Card function number, where zero is the first function.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pccard-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::PcCard;
    ///
    /// let node = PcCard { function_number: 1 };
    /// assert_eq!(node.to_string(), "PcCard(0x1)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "PcCard(0x1)");
    /// assert_eq!(format!("{node:#}"), "PcCard(0x1)");
    /// assert_eq!(format!("{node:-#}"), "PcCard(0x1)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pccard)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone, Eq, PartialEq)]
    pub struct PcCard {
        /// Function Number, 0 is the first one.
        pub function_number: u8,
    }
}
impl fmt::Display for PcCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "PcCard(0x{:x})", node.function_number)
    }
}

device_path_node! {
    /// Memory-mapped device path node (UEFI 2.10 Table 10.5).
    ///
    /// A memory range of an `EFI_MEMORY_TYPE`, from a starting to an ending address.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#memory-mapped-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::MemoryMapped;
    ///
    /// // Flash memory at the top of the 4 GiB space, of type `EfiMemoryMappedIO` (11). The ending
    /// // address is the last byte of the range.
    /// let node = MemoryMapped { memory_type: 11, start_address: 0xff00_0000, end_address: 0xffff_ffff };
    /// assert_eq!(node.to_string(), "MemoryMapped(0xb,0xff000000,0xffffffff)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "MemoryMapped(0xb,0xff000000,0xffffffff)");
    /// assert_eq!(format!("{node:#}"), "MemoryMapped(0xb,0xff000000,0xffffffff)");
    /// assert_eq!(format!("{node:-#}"), "MemoryMapped(0xb,0xff000000,0xffffffff)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::MemoryMapped)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone, Eq, PartialEq)]
    pub struct MemoryMapped {
        /// EFI memory type.
        pub memory_type: efi::MemoryType,
        /// Starting memory Address.
        pub start_address: u64,
        /// Ending Memory Address.
        pub end_address: u64,
    }
}
impl fmt::Display for MemoryMapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "MemoryMapped(0x{:x},0x{:x},0x{:x})", node.memory_type, node.start_address, node.end_address)
    }
}

device_path_node! {
    /// Controller device path node (UEFI 2.10 Table 10.7).
    ///
    /// A controller number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#controller-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Controller;
    ///
    /// let node = Controller { number: 3 };
    /// assert_eq!(node.to_string(), "Ctrl(0x3)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Ctrl(0x3)");
    /// assert_eq!(format!("{node:#}"), "Ctrl(0x3)");
    /// assert_eq!(format!("{node:-#}"), "Ctrl(0x3)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Controller)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone, Eq, PartialEq)]
    pub struct Controller {
        /// Controller Number.
        pub number: u32,
    }
}
impl fmt::Display for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "Ctrl(0x{:x})", node.number)
    }
}

/// Host interface type of a Baseboard Management Controller, in a [`Bmc`] node
/// ([UEFI 2.10 Table 10.8](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bmc-device-path)).
///
/// Table 10.8 lists [`UNKNOWN`](Self::UNKNOWN), [`KCS`](Self::KCS), [`SMIC`](Self::SMIC), and
/// [`BT`](Self::BT) without reserving the other values, and the device path text allows any value
/// from 0 to 255 (Table 10.67), so every value is accepted. The device path text writes it as a
/// number, such as `BMC(0x1,0xca2)`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct BmcInterfaceType(pub u8);

impl BmcInterfaceType {
    /// The interface type is not known.
    pub const UNKNOWN: Self = Self(0);
    /// KCS: Keyboard Controller Style.
    pub const KCS: Self = Self(1);
    /// SMIC: Server Management Interface Chip.
    pub const SMIC: Self = Self(2);
    /// BT: Block Transfer.
    pub const BT: Self = Self(3);
}

// Scroll encoding: an 8-bit value, like the `u8` it wraps.
impl<'a> TryFromCtx<'a, scroll::Endian> for BmcInterfaceType {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let (value, size) = u8::try_from_ctx(src, ctx)?;
        Ok((Self(value), size))
    }
}

impl TryIntoCtx<scroll::Endian> for &BmcInterfaceType {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

impl TryIntoCtx<scroll::Endian> for BmcInterfaceType {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        self.0.try_into_ctx(dst, ctx)
    }
}

device_path_node! {
    /// BMC device path node (UEFI 2.10 Table 10.8).
    ///
    /// The host interface of a Baseboard Management Controller: its interface type and
    /// memory-mapped or I/O base address.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bmc-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::{Bmc, BmcInterfaceType};
    ///
    /// // The BMC of `PciRoot(0x2)/Pci(0x2,0x0)/Pci(0x0,0x0)/BMC(0,0xf0000000)/RestService(...)`
    /// // (UEFI 2.10 Section 10.3.4.31).
    /// let node = Bmc { interface_type: BmcInterfaceType::UNKNOWN, base_address: 0xf000_0000 };
    /// assert_eq!(node.to_string(), "BMC(0x0,0xf0000000)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "BMC(0x0,0xf0000000)");
    /// assert_eq!(format!("{node:#}"), "BMC(0x0,0xf0000000)");
    /// assert_eq!(format!("{node:-#}"), "BMC(0x0,0xf0000000)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone, Eq, PartialEq)]
    pub struct Bmc {
        /// BMC host interface type.
        pub interface_type: BmcInterfaceType,
        /// BMC base address.
        pub base_address: u64,
    }
}
impl fmt::Display for Bmc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "BMC(0x{:x},0x{:x})", node.interface_type.0, node.base_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::{DevicePathNode, tests::check_node};
    use alloc::vec;

    #[test]
    fn test_device_path_hardware_vendor() {
        let mut expected = vec![1, 4, 22, 0];
        expected.extend_from_slice(&[0x11; 16]);
        expected.extend_from_slice(&[0xaa, 0xbb]);
        check_node(
            HardwareVendor { guid: crate::BinaryGuid::from_bytes(&[0x11; 16]), data: vec![0xaa, 0xbb] },
            &expected,
        );
    }

    #[test]
    fn test_device_path_reserved_values_are_rejected_hardware() {
        // Values past the listed ones, such as 4 for SSIF, are accepted.
        for value in [0, 1, 2, 3, 4, u8::MAX] {
            let interface_type = BmcInterfaceType(value);
            check_node(Bmc { interface_type, base_address: 0 }, &[1, 6, 13, 0, value, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
    }

    #[test]
    fn test_device_path_existing_wire_sizes_hardware() {
        check_node(Pci { function: 1, device: 2 }, &[1, 1, 6, 0, 1, 2]);
        check_node(PcCard { function_number: 3 }, &[1, 2, 5, 0, 3]);
        check_node(Controller { number: 0x12345678 }, &[1, 5, 8, 0, 0x78, 0x56, 0x34, 0x12]);
        check_node(
            MemoryMapped { memory_type: 3, start_address: 0x1234, end_address: 0x5678 },
            &[1, 3, 24, 0, 3, 0, 0, 0, 0x34, 0x12, 0, 0, 0, 0, 0, 0, 0x78, 0x56, 0, 0, 0, 0, 0, 0],
        );
        check_node(
            Bmc { interface_type: BmcInterfaceType::KCS, base_address: 0x1234 },
            &[1, 6, 13, 0, 1, 0x34, 0x12, 0, 0, 0, 0, 0, 0],
        );
    }

    /// Check the text of each hardware node in the three text forms, and that the parseable forms
    /// parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_hardware() {
        use crate::uefi::device_path::node::tests::check_text;
        use alloc::format;

        check_text!(r"Pci(0x1F,0x2)", r"Pci(0x1f,0x2)", r"Pci(0x1f,0x2)", r"Pci(0x1f,0x2)");
        check_text!(r"PcCard(1)", r"PcCard(0x1)", r"PcCard(0x1)", r"PcCard(0x1)");
        check_text!(
            r"MemoryMapped(11,0x1000,0x1FFF)",
            r"MemoryMapped(0xb,0x1000,0x1fff)",
            r"MemoryMapped(0xb,0x1000,0x1fff)",
            r"MemoryMapped(0xb,0x1000,0x1fff)"
        );
        check_text!(
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)"
        );
        check_text!(r"Ctrl(3)", r"Ctrl(0x3)", r"Ctrl(0x3)", r"Ctrl(0x3)");
        check_text!(r"BMC(1,0x1000)", r"BMC(0x1,0x1000)", r"BMC(0x1,0x1000)", r"BMC(0x1,0x1000)");

        // The flags select the text form only; they do not change how field values are written.
        let pci = Pci { function: 2, device: 0x1f };
        for text in [format!("{pci}"), format!("{pci:-}"), format!("{pci:#}"), format!("{pci:+}")] {
            assert_eq!(text, "Pci(0x1f,0x2)");
        }
    }

    /// A PCI device number is 0-31 and a function number 0-7 (UEFI 2.10 Table 10.67).
    #[test]
    fn test_device_path_pci_ranges() {
        check_node(Pci { function: 7, device: 31 }, &[1, 1, 6, 0, 7, 31]);
        assert!(Pci { function: 8, device: 0 }.validate().is_err());
        assert!(Pci { function: 0, device: 32 }.validate().is_err());
    }
}
