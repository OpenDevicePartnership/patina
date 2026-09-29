//! BIOS Boot Specification device path nodes.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{BiosSubType, DevicePathType};
use crate::{
    device_path_node,
    uefi::device_path::{encoding::AsciiNul, parse_node::MeasureSize},
};
use alloc::string::String;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// BIOS Boot Specification device path with a NUL-terminated ASCII description.
    @[DevicePathNode(DevicePathType::Bios, BiosSubType::BiosBootSpecification)]
    @[DevicePathNodeDerive(Debug, Display)]
    #[derive(Clone, Eq, PartialEq, Pread, Pwrite)]
    pub struct Bios {
        /// BIOS device type.
        pub device_type: u16,
        /// Status flags.
        pub status_flag: u16,
        /// Description string without the wire terminator.
        #[scroll(ctx = AsciiNul)]
        pub description_str: String,
    }
}

impl MeasureSize for Bios {
    fn measure_size(&self) -> usize {
        size_of_val(&self.device_type) + size_of_val(&self.status_flag) + self.description_str.len() + size_of::<u8>() // NUL terminator
    }
}
