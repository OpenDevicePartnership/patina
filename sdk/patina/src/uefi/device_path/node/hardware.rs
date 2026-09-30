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
use crate::{
    device_path_node,
    uefi::device_path::{encoding::Vector, parse_node::MeasureSize},
};
use alloc::vec::Vec;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// Vendor-defined hardware device path node (UEFI 2.10 Table 10.6).
    ///
    /// A vendor-assigned GUID defines the contents of the vendor-defined data.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#vendor-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Vendor)]
    @[DevicePathNodeDerive(Debug, Display, Validate)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct HardwareVendor {
        /// Vendor GUID in EFI wire byte order.
        pub guid: [u8; 16],
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Vector)]
        pub data: Vec<u8>,
    }
}

impl MeasureSize for HardwareVendor {
    fn measure_size(&self) -> usize {
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
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pci)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
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
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
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
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct MemoryMapped {
        /// EFI memory type.
        pub memory_type: u32,
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
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
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
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize, Validate)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Bmc {
        /// BMC interface type.
        pub interface_type: u8,
        /// BMC base address.
        pub base_address: u64,
    }
}
