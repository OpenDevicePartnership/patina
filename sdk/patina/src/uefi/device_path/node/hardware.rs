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

use super::{DevicePathType, HardwareSubType};
use crate::{BinaryGuid, standard::efi};
use crate::{device_path_node, uefi::device_path::encoding::Vector};
use alloc::vec::Vec;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// Vendor-defined hardware device path node (UEFI 2.10 Table 10.6).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-device-path>
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

device_path_node! {
    /// PC Card device path node (UEFI 2.10 Table 10.4).
    ///
    /// The PC Card function number, where zero is the first function.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pccard-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pccard)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct PcCard {
        /// Function Number, 0 is the first one.
        pub function_number: u8,
    }
}

device_path_node! {
    /// Memory-mapped device path node (UEFI 2.10 Table 10.5).
    ///
    /// A memory range of an `EFI_MEMORY_TYPE`, from a starting to an ending address.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#memory-mapped-device-path>
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

device_path_node! {
    /// Controller device path node (UEFI 2.10 Table 10.7).
    ///
    /// A controller number.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#controller-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Controller)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Controller {
        /// Controller Number.
        pub number: u32,
    }
}

device_path_node! {
    /// BMC device path node (UEFI 2.10 Table 10.8).
    ///
    /// The host interface of a Baseboard Management Controller: its interface type and
    /// memory-mapped or I/O base address.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bmc-device-path>
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

fn bmc_validate(node: &Bmc) -> Result<(), &'static str> {
    if node.interface_type > 3 {
        return Err("BMC interface types above 0x03 are reserved");
    }
    Ok(())
}
