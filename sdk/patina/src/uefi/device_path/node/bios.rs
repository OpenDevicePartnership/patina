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

use super::display_only;
use super::{BiosSubType, DevicePathType};
use crate::{device_path_node, uefi::device_path::encoding::AsciiNul};
use alloc::{
    format,
    string::{String, ToString},
};
use core::fmt;
use scroll::{Pread, Pwrite};

device_path_node! {
    /// BIOS Boot Specification device path node (UEFI 2.10 Table 10.63).
    ///
    /// A legacy boot device, described by its device type, status flags, and a
    /// NUL-terminated ASCII description.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#bios-boot-specification-device-path>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::Bios;
    ///
    /// let node = Bios { device_type: 2, status_flag: 0, description_str: "boot".into() };
    /// assert_eq!(node.to_string(), "BBS(HD,boot,0x0)");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "BBS(HD,boot)");
    /// assert_eq!(format!("{node:#}"), "BBS(HD,boot,0x0)");
    /// assert_eq!(format!("{node:-#}"), "BBS(HD,boot)");
    /// ```
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
impl fmt::Display for Bios {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self;
        let ty = match node.device_type {
            1 => "Floppy".to_string(),
            2 => "HD".to_string(),
            3 => "CDROM".to_string(),
            4 => "PCMCIA".to_string(),
            5 => "USB".to_string(),
            6 => "Network".to_string(),
            other => format!("0x{other:x}"),
        };
        write!(f, "BBS({ty},{}", node.description_str)?;
        if !display_only(f) {
            write!(f, ",0x{:x}", node.status_flag)?;
        }
        f.write_str(")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::test_support::{check_node, encode_node};
    use scroll::ctx::TryFromCtx;

    #[test]
    fn test_device_path_existing_wire_sizes_bios() {
        check_node(
            Bios { device_type: 2, status_flag: 1, description_str: "foo".into() },
            &[5, 1, 12, 0, 2, 0, 1, 0, b'f', b'o', b'o', 0],
        );
    }

    #[test]
    fn test_device_path_invalid_variable_payloads_bios() {
        assert!(encode_node(Bios { device_type: 0, status_flag: 0, description_str: "a\0b".into() }).is_err());
        assert!(Bios::try_from_ctx(&[0, 0, 0, 0, b'A'], scroll::LE).is_err());
    }
}
