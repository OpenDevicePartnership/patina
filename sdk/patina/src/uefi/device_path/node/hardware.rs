//! Hardware device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, HardwareSubType, check_length, invalid};
use crate::{
    device_path_node,
    uefi::device_path::parse_node::{DevicePathNode, Header},
};
use alloc::vec::Vec;
use scroll::{
    Endian, Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

/// Vendor-defined hardware node.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HardwareVendor {
    /// Vendor GUID in EFI wire byte order.
    pub guid: [u8; 16],
    /// Opaque vendor-defined bytes following the GUID.
    pub data: Vec<u8>,
}

impl DevicePathNode for HardwareVendor {
    fn header(&self) -> Header {
        Header::new(DevicePathType::Hardware as u8, HardwareSubType::Vendor as u8, 20 + self.data.len())
    }

    fn is_type(kind: u8, subtype: u8) -> bool {
        kind == DevicePathType::Hardware as u8 && subtype == HardwareSubType::Vendor as u8
    }

    fn validate(&self) -> Result<(), scroll::Error> {
        check_length(self.header().length - 4)?;
        Ok(())
    }

    fn write_into(self, data: &mut [u8]) -> Result<usize, scroll::Error> {
        self.validate()?;
        let header = self.header();
        if data.len() < header.length {
            return Err(scroll::Error::TooBig { size: header.length, len: data.len() });
        }
        let mut offset = 0;
        data.gwrite_with(header, &mut offset, scroll::LE)?;
        data.gwrite_with(self, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

impl TryFromCtx<'_, scroll::Endian> for HardwareVendor {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid = data.gread_with(&mut offset, scroll::LE)?;
        let payload = data.get(offset..).ok_or_else(|| invalid("missing vendor payload"))?;
        let (node, consumed) = (Self { guid, data: payload.to_vec() }, data.len());
        node.validate()?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for HardwareVendor {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        self.validate()?;
        let mut offset = 0;
        data.gwrite_with(self.guid.as_slice(), &mut offset, ())?;
        data.gwrite_with(self.data.as_slice(), &mut offset, ())?;
        Ok(offset)
    }
}

impl core::fmt::Display for HardwareVendor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, formatter)
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#pci-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Pci)]
    @[DevicePathNodeDerive(Debug, Display)]
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
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct PcCard {
        /// Function Number, 0 is the first one.
        pub function_number: u8,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#memory-mapped-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::MemoryMapped)]
    @[DevicePathNodeDerive(Debug, Display)]
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
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Controller {
        /// Controller Number.
        pub number: u32,
    }
}

device_path_node! {
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bmc-device-path>
    @[DevicePathNode(DevicePathType::Hardware, HardwareSubType::Bmc)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Pwrite, Pread, Clone)]
    pub struct Bmc {
        /// BMC interface type.
        pub interface_type: u8,
        /// BMC base address.
        pub base_address: u64,
    }
}
