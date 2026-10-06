//! End of hardware device path nodes (UEFI 2.10 Section 10.3.1, type `0x7F`).
//!
//! Every device path ends with an End Entire Device Path node (sub-type `0xFF`).
//! When a device path describes several devices, such as a console output variable
//! that sends output to both a VGA and a serial console, an End This Instance node
//! (sub-type `0x01`) ends each instance and starts the next one.
//!
//! End nodes have no payload; their length is 4 bytes.
//!
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{DevicePathType, EndSubType};
use crate::device_path_node;
use core::fmt;
use scroll::ctx::{TryFromCtx, TryIntoCtx};

device_path_node! {
    /// End Entire Device Path node (UEFI 2.10 Table 10.2).
    ///
    /// Terminates an entire device path. Every device path ends with this node.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::EndEntire;
    ///
    /// let node = EndEntire;
    /// assert_eq!(node.to_string(), "");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), "");
    /// assert_eq!(format!("{node:#}"), "");
    /// assert_eq!(format!("{node:-#}"), "");
    /// ```
    @[DevicePathNode(DevicePathType::End, EndSubType::Entire)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Copy)]
    pub struct EndEntire;
}

impl TryIntoCtx<scroll::Endian> for EndEntire {
    type Error = scroll::Error;

    fn try_into_ctx(self, _: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl TryFromCtx<'_, scroll::Endian> for EndEntire {
    type Error = scroll::Error;

    fn try_from_ctx(_: &[u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        Ok((Self, 0))
    }
}
impl fmt::Display for EndEntire {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

device_path_node! {
    /// End This Instance of a Device Path node (UEFI 2.10 Table 10.2).
    ///
    /// Terminates one device path instance and starts another, when a device path
    /// describes several devices.
    ///
    /// <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures>
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::device_path::node::EndInstance;
    ///
    /// let node = EndInstance;
    /// assert_eq!(node.to_string(), ",");
    /// // Display-only (`-`), without shortcuts (`#`), and both.
    /// assert_eq!(format!("{node:-}"), ",");
    /// assert_eq!(format!("{node:#}"), ",");
    /// assert_eq!(format!("{node:-#}"), ",");
    /// ```
    @[DevicePathNode(DevicePathType::End, EndSubType::Instance)]
    @[DevicePathNodeDerive(Debug)]
    #[derive(Clone, Copy)]
    pub struct EndInstance;
}

impl TryIntoCtx<scroll::Endian> for EndInstance {
    type Error = scroll::Error;

    fn try_into_ctx(self, _: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl TryFromCtx<'_, scroll::Endian> for EndInstance {
    type Error = scroll::Error;

    fn try_from_ctx(_: &[u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        Ok((Self, 0))
    }
}
impl fmt::Display for EndInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::device_path::node::{
        DevicePathNode, GenericDevicePathNode,
        test_support::{check_node, encode_node, raw_generic_node},
    };
    use scroll::Pread;

    #[test]
    fn test_device_path_existing_wire_sizes_end() {
        check_node(EndEntire, &[0x7f, 0xff, 4, 0]);
        check_node(EndInstance, &[0x7f, 1, 4, 0]);
    }

    #[test]
    fn test_device_path_invalid_end_nodes_are_rejected_at_io_boundaries() {
        for (sub_type, length) in [(1, 5), (0xff, 5), (2, 4)] {
            let wire = [0x7f, sub_type, length, 0, 0];
            assert!(wire.pread_with::<&GenericDevicePathNode>(0, scroll::LE).is_err());
            let node = raw_generic_node(&wire[..usize::from(length)]);
            assert!(<Box<dyn DevicePathNode>>::try_from(node).is_err());
            assert!(encode_node(node).is_err());
        }
    }
}
