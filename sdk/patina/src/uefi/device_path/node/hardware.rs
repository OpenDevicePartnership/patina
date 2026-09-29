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
    uefi::device_path::parse_node::{DevicePathNode, MeasureSize},
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

impl MeasureSize for HardwareVendor {
    fn measure_size(&self) -> usize {
        size_of_val(&self.guid) + size_of_val(self.data.as_slice())
    }
}

impl DevicePathNode for HardwareVendor {
    fn r#type() -> u8 {
        DevicePathType::Hardware as u8
    }

    fn sub_type() -> u8 {
        HardwareSubType::Vendor as u8
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

impl TryFromCtx<'_, scroll::Endian> for HardwareVendor {
    type Error = scroll::Error;
    fn try_from_ctx(data: &[u8], _: Endian) -> Result<(Self, usize), scroll::Error> {
        let mut offset = 0;
        let guid = data.gread_with(&mut offset, scroll::LE)?;
        let payload = data.get(offset..).ok_or_else(|| invalid("missing vendor payload"))?;
        let (node, consumed) = (Self { guid, data: payload.to_vec() }, data.len());
        check_length(node.header().length - 4)?;
        Ok((node, consumed))
    }
}

impl TryIntoCtx<scroll::Endian> for HardwareVendor {
    type Error = scroll::Error;
    fn try_into_ctx(self, data: &mut [u8], _: Endian) -> Result<usize, scroll::Error> {
        check_length(self.header().length - 4)?;
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
