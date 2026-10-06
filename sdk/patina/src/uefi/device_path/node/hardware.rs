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

use super::write_vendor;
use super::{DevicePathType, HardwareSubType};
use crate::{BinaryGuid, standard::efi};
use crate::{device_path_node, uefi::device_path::encoding::Vector};
use alloc::vec::Vec;
use core::fmt;
use scroll::{Pread, Pwrite};

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
        measure_size = hardware_vendor_measure_size,
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
        write_vendor(f, "VenHw", &node.guid, &node.data)
    }
}

fn hardware_vendor_measure_size(node: &HardwareVendor) -> usize {
    size_of_val(&node.guid) + size_of_val(node.data.as_slice())
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
    /// let node = Pci { function: 0, device: 0x1f };
    /// assert_eq!(node.to_string(), "Pci(0x1f,0x0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "Pci(0x1f,0x0)");
    /// assert_eq!(format!("{node:#}"), "Pci(0x1f,0x0)");
    /// assert_eq!(format!("{node:-#}"), "Pci(0x1f,0x0)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pci)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
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
    #[derive(Pwrite, Pread, Clone)]
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
    /// let node = MemoryMapped { memory_type: 0x0b, start_address: 0x1000, end_address: 0x1fff };
    /// assert_eq!(node.to_string(), "MemoryMapped(0xb,0x1000,0x1fff)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "MemoryMapped(0xb,0x1000,0x1fff)");
    /// assert_eq!(format!("{node:#}"), "MemoryMapped(0xb,0x1000,0x1fff)");
    /// assert_eq!(format!("{node:-#}"), "MemoryMapped(0xb,0x1000,0x1fff)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::MemoryMapped)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
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
    #[derive(Pwrite, Pread, Clone)]
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
    /// use patina::uefi::device_path::node::Bmc;
    ///
    /// let node = Bmc { interface_type: 1, base_address: 0x1000 };
    /// assert_eq!(node.to_string(), "BMC(0x1,0x1000)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "BMC(0x1,0x1000)");
    /// assert_eq!(format!("{node:#}"), "BMC(0x1,0x1000)");
    /// assert_eq!(format!("{node:-#}"), "BMC(0x1,0x1000)");
    /// ```
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc, validate = bmc_validate)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Bmc {
        /// BMC interface type: 0 unknown, 1 KCS, 2 SMIC, or 3 BT; other values are reserved.
        pub interface_type: u8,
        /// BMC base address.
        pub base_address: u64,
    }
}
impl fmt::Display for Bmc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        write!(f, "BMC(0x{:x},0x{:x})", node.interface_type, node.base_address)
    }
}

fn bmc_validate(node: &Bmc) -> Result<(), &'static str> {
    if node.interface_type > 3 {
        return Err("BMC interface types above 0x03 are reserved");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::test_support::{check_invalid, check_node, encode_node};
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
        for interface_type in 0..=3 {
            encode_node(Bmc { interface_type, base_address: 0 }).unwrap();
        }
        check_invalid(Bmc { interface_type: 4, base_address: 0 }, &[4, 0, 0, 0, 0, 0, 0, 0, 0]);
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
        check_node(Bmc { interface_type: 1, base_address: 0x1234 }, &[1, 6, 13, 0, 1, 0x34, 0x12, 0, 0, 0, 0, 0, 0]);
    }
}
