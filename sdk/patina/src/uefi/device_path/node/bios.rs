//! BIOS Boot Specification device path nodes (UEFI 2.10 Section 10.3.6, type `0x05`).
//!
//! A BIOS Boot Specification device path points to a legacy, non-UEFI operating
//! system, based on the BIOS Boot Specification Version 1.01. It only lets platform
//! firmware select a legacy operating system as a boot option.
//!
//! This node is a complete device path: it is followed only by the End Entire Device
//! Path node (Section 10.4.6). When UEFI Secure Boot is enabled, booting a non-UEFI
//! operating system fails.
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bios-boot-specification-device-path>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{BiosSubType, DevicePathType};
use crate::{device_path_node, uefi::device_path::encoding::AsciiNul};
use alloc::string::String;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// BIOS Boot Specification device path node (UEFI 2.10 Table 10.63).
    ///
    /// A legacy boot device, described by its device type, status flags, and a
    /// NUL-terminated ASCII description.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bios-boot-specification-device-path>
    @[DevicePathNode(
        DevicePathType::Bios, BiosSubType::BiosBootSpecification,
        measure_size = bios_measure_size,
    )]
    @[DevicePathNodeDerive(Debug)]
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

fn bios_measure_size(node: &Bios) -> usize {
    size_of_val(&node.device_type) + size_of_val(&node.status_flag) + node.description_str.len() + size_of::<u8>() // NUL terminator
}
