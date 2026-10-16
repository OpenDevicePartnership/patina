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

use super::TextString;
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
    /// // A legacy hard drive entry of the BIOS Boot Specification table.
    /// let node = Bios { device_type: 2, status_flag: 0, description_str: "SATA HDD".into() };
    /// // The description has a space, so it is written in double quotes (UEFI 2.10 Section 10.6.1.5).
    /// assert_eq!(node.to_string(), r#"BBS(HD,"SATA HDD",0x0)"#);
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), r#"BBS(HD,"SATA HDD")"#);
    /// assert_eq!(format!("{node:#}"), r#"BBS(HD,"SATA HDD",0x0)"#);
    /// assert_eq!(format!("{node:-#}"), r#"BBS(HD,"SATA HDD")"#);
    /// ```
    @[DevicePathNode(
        DevicePathType::Bios, BiosSubType::BiosBootSpecification,
        measure_size = Self::payload_size,
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

impl Bios {
    /// Size of the payload in bytes, for `DevicePathNode::measure_size`.
    fn payload_size(&self) -> usize {
        size_of_val(&self.device_type) + size_of_val(&self.status_flag) + self.description_str.len() + size_of::<u8>() // NUL terminator
    }
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
        write!(f, "BBS({ty},{}", TextString(&node.description_str))?;
        if !f.sign_minus() {
            write!(f, ",0x{:x}", node.status_flag)?;
        }
        f.write_str(")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::tests::{check_node, encode_node};
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

    /// Check the text of each bios node in the three text forms, and that the parseable forms
    /// parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_bios() {
        use crate::uefi::device_path::node::tests::check_text;

        check_text!(r"BBS(HD,boot,0x0)", r"BBS(HD,boot,0x0)", r"BBS(HD,boot)", r"BBS(HD,boot,0x0)");
        // The other device type keywords, and a type without a keyword (UEFI 2.10 Table 10.67).
        check_text!(r"BBS(Floppy,a,0)", r"BBS(Floppy,a,0x0)", r"BBS(Floppy,a)", r"BBS(Floppy,a,0x0)");
        check_text!(r"BBS(CDROM,a,0)", r"BBS(CDROM,a,0x0)", r"BBS(CDROM,a)", r"BBS(CDROM,a,0x0)");
        check_text!(r"BBS(PCMCIA,a,0)", r"BBS(PCMCIA,a,0x0)", r"BBS(PCMCIA,a)", r"BBS(PCMCIA,a,0x0)");
        check_text!(r"BBS(USB,a,0)", r"BBS(USB,a,0x0)", r"BBS(USB,a)", r"BBS(USB,a,0x0)");
        check_text!(r"BBS(Network,a,0)", r"BBS(Network,a,0x0)", r"BBS(Network,a)", r"BBS(Network,a,0x0)");
        check_text!(r"BBS(0x80,a,0)", r"BBS(0x80,a,0x0)", r"BBS(0x80,a)", r"BBS(0x80,a,0x0)");
    }
}
