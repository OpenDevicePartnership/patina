//! Hardware device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, HardwareSubType};
use crate::{
    device_path_node,
    uefi::device_path::{encoding::Remaining, parse_node::MeasureSize},
};
use alloc::vec::Vec;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// Vendor-defined hardware node.
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Vendor)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct HardwareVendor {
        /// Vendor GUID in EFI wire byte order.
        pub guid: [u8; 16],
        /// Opaque vendor-defined bytes following the GUID.
        #[scroll(ctx = Remaining)]
        pub data: Vec<u8>,
    }
}

impl MeasureSize for HardwareVendor {
    fn measure_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pci-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pci)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Pci {
        /// PCI Function Number.
        pub function: u8,
        /// PCI Device Number.
        pub device: u8,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pci-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pccard)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct PcCard {
        /// Function Number, 0 is the first one.
        pub function_number: u8,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#memory-mapped-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::MemoryMapped)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
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
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#controller-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Controller)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Controller {
        /// Controller Number.
        pub number: u32,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bmc-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc)]
    @[DevicePathNodeDerive(Debug, Display, MeasureSize)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Bmc {
        /// BMC interface type.
        pub interface_type: u8,
        /// BMC base address.
        pub base_address: u64,
    }
}
