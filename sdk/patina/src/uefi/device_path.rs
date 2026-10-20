//! Device Path Definitions
//!
//! Defines UEFI device path node structures and utilities.
//! Uses the UEFI specification as a reference: <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html>
//!
//! ## Text representation
//!
//! [`DevicePath`], [`DevicePathBuf`] and every node implement [`Display`] with the text
//! representation of `EFI_DEVICE_PATH_TO_TEXT_PROTOCOL`
//! ([10.6. EFI Device Path Display Format Overview](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-display-format-overview)).
//! Standalone end nodes display as their conventional separators.
//!
//! ### Format flags
//!
//! Parameters from [`EFI_DEVICE_PATH_TO_TEXT_PROTOCOL.ConvertDevicePathToText()`](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-to-text-protocol-convertdevicepathtotext)
//! are used to format device path:
//!
//! - `DisplayOnly`: If `DisplayOnly` is TRUE, then the shorter text representation of the display node is used, where applicable.
//!   If `DisplayOnly` is FALSE, then the longer text representation of the display node is used.
//!
//! - `AllowShortcuts`: The `AllowShortcuts` is FALSE, then the shortcut forms of text representation for a device node cannot be used.
//!
//! | Format  | `DisplayOnly` | `AllowShortcuts` |
//! |---------|---------------|------------------|
//! | `{}`    | `FALSE`       | `TRUE`           |
//! | `{:-}`  | `TRUE`        | `TRUE`           |
//! | `{:#}`  | `FALSE`       | `FALSE`          |
//! | `{:-#}` | `TRUE`        | `FALSE`          |
//!
//! ### Example
//!
//! The example is the `AcpiEx(HWP0002,PNP0A03,0)` node of
//! [10.6.1.5. Text Representation Basics](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#text-representation-basics),
//! a device compatible with a PCI root bridge, which the display-only form shows with the
//! `PciRoot` shortcut.
//!
//! ```
//! use patina::uefi::device_path::node::{Acpi, EisaId, ExtendedAcpi};
//!
//! // AcpiEx(HWP0002,PNP0A03,0)
//! let node = ExtendedAcpi {
//!     hid: EisaId::new("HWP0002"),
//!     uid: 0,
//!     cid: Acpi::PCI_ROOT_HID,
//!     hid_string: String::new(),
//!     uid_string: String::new(),
//!     cid_string: String::new(),
//! };
//! assert_eq!(format!("{node}"), "AcpiEx(HWP0002,PNP0A03,0x0,,,)");
//! assert_eq!(format!("{node:-}"), "PciRoot(0x0)");
//! assert_eq!(format!("{node:#}"), "AcpiEx(HWP0002,PNP0A03,0x0,,,)");
//! assert_eq!(format!("{node:-#}"), "AcpiEx(HWP0002,PNP0A03,0x0)");
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod encoding;
pub mod node;
mod short_form;

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    borrow::Borrow,
    fmt::{self, Debug, Display},
    ops::Deref,
    ptr, slice,
};

use scroll::{Pread, Pwrite, ctx::TryIntoCtx};

use crate::uefi::device_path::node::{
    DevicePathNode, EndEntire, EndInstance, FilePath, GenericDevicePathNode, HardDrive, Header, Iter, Uri, UsbClass,
    UsbWwid,
};

/// `DevicePathBuf` is an owned version of device path. This is used to create device paths or when performing mutable operations on them.
///
/// A `DevicePathBuf` always holds a complete device path: its last node is an
/// [`EndEntire`] node, and every operation keeps it that way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePathBuf {
    buffer: Vec<u8>,
}

impl DevicePathBuf {
    /// Create a device path that contains only its [`EndEntire`] node.
    pub fn new() -> Self {
        Self { buffer: Vec::from(EndEntire::BYTES) }
    }

    /// Append a node to the device path, before its [`EndEntire`] node, like the
    /// `AppendDeviceNode` function of the device path utilities protocol ([UEFI 2.10
    /// Section 10.5.5](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicenode)).
    ///
    /// Append an [`EndInstance`] node to start a new instance. Appending an [`EndEntire`]
    /// node does nothing: the device path already ends with it.
    ///
    /// The node is checked with [`validate`](DevicePathNode::validate), then its header and
    /// payload are encoded directly into the device path buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the node fails validation, the node
    /// is too large for a 16-bit length, the header length differs from
    /// [`measure_size`](DevicePathNode::measure_size), allocation fails, or the payload
    /// cannot be encoded. The device path is unchanged on failure.
    pub fn append<T>(&mut self, node: T) -> Result<(), String>
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        node.validate()?;

        let header = node.header();

        // Appending an EndEntire node does nothing: the device path always ends with one.
        if header.is_type::<EndEntire>() {
            return Ok(());
        }

        let node_payload_size = node.measure_size();
        let node_size = size_of::<Header>()
            .checked_add(node_payload_size)
            .and_then(|s| u16::try_from(s).ok())
            .ok_or(format!("device path node is too large. Payload size = {node_payload_size}"))?
            as usize;

        if header.length() != node_size {
            return Err(format!(
                "device path node header length differs from its size. Header length = {}, Node size = {node_size}",
                header.length()
            ));
        }

        self.buffer
            .try_reserve_exact(node_size)
            .map_err(|_| format!("Cannot allocate device path node. Node size = {node_size}"))?;

        // Write the node over the end node, then put the end node back after it. The buffer
        // already has room for both, so this does not allocate.
        let start = self.buffer.len() - EndEntire::BYTES.len();
        self.buffer.truncate(start);
        self.buffer.resize(start + node_size, 0);

        let node_bytes_buffer = self.buffer.get_mut(start..).expect("Buffer was just resized to fit.");

        let result = node_bytes_buffer
            .pwrite_with(header, 0, scroll::LE)
            .and_then(|offset| node_bytes_buffer.pwrite_with(node, offset, scroll::LE))
            .map(|_| ())
            .map_err(|e| format!("Cannot encode device path node. Error: {e}"));

        if result.is_err() {
            self.buffer.truncate(start);
        }

        self.buffer.extend_from_slice(&EndEntire::BYTES);

        result
    }

    /// Append a device path to this device path, the `EndEntire` node of self will be removed when appending the other path.
    ///
    /// [UEFI Spec Documentation: 10.5.4. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.AppendDevicePath()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicepath)
    ///
    /// # Errors
    ///
    /// Returns an error if allocation fails.
    pub fn append_device_path(&mut self, device_path: &DevicePath) -> Result<(), String> {
        // Reserve first, so that appending cannot fail after the device path is changed.
        self.buffer.try_reserve_exact(device_path.size() - EndEntire::BYTES.len()).map_err(|_| {
            format!("Cannot allocate {} bytes to append a device path.", device_path.size() - EndEntire::BYTES.len())
        })?;
        self.buffer.truncate(self.buffer.len() - EndEntire::BYTES.len());
        self.buffer.extend_from_slice(device_path.as_bytes());
        Ok(())
    }

    /// Append a device path to this device path, the `EndEntire` node of self will be replaced with an end instance.
    ///
    /// [UEFI Spec Documentation: 10.5.6. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.AppendDevicePathInstance()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicepathinstance)
    ///
    /// # Errors
    ///
    /// Returns an error if allocation fails.
    pub fn append_device_path_instance(&mut self, device_path: &DevicePath) -> Result<(), String> {
        // Reserve first, so that appending cannot fail after the device path is changed.
        self.buffer
            .try_reserve_exact(device_path.size())
            .map_err(|_| format!("Cannot allocate {} bytes to append a device path.", device_path.size()))?;
        // The EndEntire node of this device path becomes the EndInstance node that separates
        // the two instances: it differs only by its sub-type.
        let sub_type = self.buffer.len() - EndEntire::BYTES.len() + 1;
        *self.buffer.get_mut(sub_type).expect("A device path ends with EndEntire") = node::EndSubType::Instance as u8;
        self.buffer.extend_from_slice(device_path.as_bytes());
        Ok(())
    }
}

impl Default for DevicePathBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl From<DevicePathBuf> for Box<DevicePath> {
    fn from(value: DevicePathBuf) -> Self {
        let bytes = Box::into_raw(value.buffer.into_boxed_slice());
        // SAFETY: `bytes` comes from `Box::into_raw` of a `Box<[u8]>`, and `DevicePath` is
        // repr(transparent) over `[u8]`, so the pointer and its allocation are valid for it.
        unsafe { Box::from_raw(bytes as *mut DevicePath) }
    }
}

impl Deref for DevicePathBuf {
    type Target = DevicePath;

    fn deref(&self) -> &Self::Target {
        self.into()
    }
}

impl AsRef<DevicePath> for DevicePathBuf {
    fn as_ref(&self) -> &DevicePath {
        self
    }
}

/// Copy a device path.
///
/// [UEFI Spec Documentation: 10.5.3. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.DuplicateDevicePath()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-duplicatedevicepath)
impl From<&DevicePath> for DevicePathBuf {
    fn from(value: &DevicePath) -> Self {
        DevicePathBuf { buffer: Vec::from(&value.buffer) }
    }
}

/// Borrow a device path buffer as a device path.
impl<'a> From<&'a DevicePathBuf> for &'a DevicePath {
    fn from(value: &'a DevicePathBuf) -> Self {
        // SAFETY: DevicePath is repr(transparent) over `[u8]`, so it has the same memory layout.
        unsafe { &*(ptr::from_ref::<[u8]>(value.buffer.as_slice()) as *const DevicePath) }
    }
}

impl Display for DevicePathBuf {
    /// Write the UEFI text representation of the device path; see [`DevicePath`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_ref(), f)
    }
}

/// `DevicePath` is the borrowed version of a [`DevicePathBuf`].
/// Only immutable operations are possible on this type.
#[derive(Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct DevicePath {
    buffer: [u8],
}

impl DevicePath {
    /// Create a &`DevicePath` from a pointer to a byte buffer.
    /// This is used to interface with device paths from C code.
    ///
    /// [UEFI Spec Documentation: 10.3.1. Generic Device Path Structures](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures)
    ///
    /// The nodes are walked up to the [`EndEntire`] node, checking the structure like
    /// [`try_from_bytes`](Self::try_from_bytes) does. Node payloads are not decoded; use
    /// [`validate`](Self::validate) for that.
    ///
    /// # Errors
    ///
    /// Returns an error if the pointer is null, a node is shorter than its header, an end node
    /// is malformed, or the device path is too large to address.
    ///
    /// # Safety
    ///
    /// The buffer pointer must point to valid device path data that remains valid
    /// for the lifetime 'a. The device path must be properly terminated with an
    /// `EndEntire` node.
    pub unsafe fn try_from_ptr<'a>(buffer: *const u8) -> Result<&'a DevicePath, String> {
        if buffer.is_null() {
            return Err("Null device path pointer.".into());
        }

        // Find the size of the device path by walking the node headers.
        let mut size = 0usize;
        loop {
            // SAFETY: The caller guarantees a valid device path up to its EndEntire node. The
            // previous nodes were not EndEntire, so a node header starts at `size`.
            let header_bytes = unsafe { slice::from_raw_parts(buffer.add(size), size_of::<Header>()) };
            let header = header_bytes
                .pread_with::<Header>(0, scroll::LE)
                .map_err(|error| format!("Cannot read a device path node header: {error}"))?;
            let length = header.length();
            if length < size_of::<Header>() {
                return Err(format!("Device path node at offset {size} is shorter than its header."));
            }
            // Check end nodes before using their length, so that a malformed end node cannot make
            // the device path span memory past its end.
            if !header.is_valid_end() {
                return Err(format!("Invalid end device path node at offset {size}."));
            }
            size = size
                .checked_add(length)
                .filter(|size| isize::try_from(*size).is_ok())
                .ok_or("Device path is too large.")?;

            if header.is_type::<EndEntire>() {
                break;
            }
        }

        // SAFETY: The caller guarantees that the device path, which spans `size` bytes up to and
        // including its EndEntire node, is valid for 'a.
        let bytes = unsafe { slice::from_raw_parts(buffer, size) };
        DevicePath::try_from_bytes(bytes)
    }

    /// Borrow a complete device path from `bytes`.
    ///
    /// [UEFI Spec Documentation: 10.3.1. Generic Device Path Structures](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures)
    ///
    /// Only the structure is checked: every node must have a length of at least its header,
    /// end nodes must follow the specification, and the last node, and only the last node, must
    /// be [`EndEntire`]. Node payloads are not decoded, so that a device path from firmware can
    /// be walked and compared even when it holds a node this crate would reject; use
    /// [`validate`](Self::validate) to also check every node.
    ///
    /// # Errors
    ///
    /// Returns an error if `bytes` is not a complete device path.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<&DevicePath, String> {
        let mut rest = bytes;
        loop {
            let (node, next) = GenericDevicePathNode::from_bytes(rest)
                .map_err(|error| format!("Invalid device path node: {error}"))?;
            if node.header.is_type::<EndEntire>() {
                if !next.is_empty() {
                    return Err(format!("Device path has {} bytes after its EndEntire node.", next.len()));
                }
                break;
            }
            rest = next;
        }
        // SAFETY: DevicePath is repr(transparent) over [u8], and the bytes were checked to be a
        // complete device path.
        Ok(unsafe { &*(ptr::from_ref::<[u8]>(bytes) as *const DevicePath) })
    }

    /// Check that every node of a known type has a well-formed payload that passes
    /// [`validate`](DevicePathNode::validate).
    ///
    /// Nodes of an unknown type are accepted, since the specification lets them be skipped.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first node that does not decode.
    pub fn validate(&self) -> Result<(), String> {
        for node in self.iter() {
            <Box<dyn DevicePathNode>>::try_from(node).map_err(|error| format!("Invalid device path node: {error}"))?;
        }
        Ok(())
    }

    /// Return the size in bytes of the device path.
    ///
    /// [UEFI Spec Documentation: 10.5.2. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.GetDevicePathSize()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-getdevicepathsize)
    pub fn size(&self) -> usize {
        self.buffer.len()
    }

    /// Return the raw byte representation of the device path.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer
    }

    /// Return the number of nodes in the device path.
    pub fn node_count(&self) -> usize {
        self.iter().count()
    }

    /// Return true if the device path contains more than one instance, otherwise false.
    ///
    /// [UEFI Spec Documentation: 10.5.9. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.IsDevicePathMultiInstance()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-isdevicepathmultiinstance)
    pub fn is_multi_instance(&self) -> bool {
        self.iter().any(|n| n.header.is_type::<EndInstance>())
    }

    /// Return true if the device path starts with the other device path.
    pub fn starts_with(&self, other: &DevicePath) -> bool {
        for (node, other_node) in self.iter().zip(other.iter()) {
            if other_node.header.is_type::<EndEntire>() || other_node.header.is_type::<EndInstance>() {
                return true;
            }
            if node != other_node {
                return false;
            }
        }
        false
    }

    /// Return the rest of this device path after `prefix`.
    ///
    /// Every node of `prefix` before its first end node, [`EndInstance`] or [`EndEntire`], must
    /// be equal, byte for byte, to the node at the same position in this device path, as
    /// [`starts_with`](Self::starts_with) checks. The result borrows this device path, starts at
    /// the first node after the matched ones, and keeps this device path's end node. This is
    /// the match that `LocateDevicePath` makes between the device path of a handle and the
    /// device path it searches for.
    ///
    /// Returns `None` if `prefix` is not a prefix of this device path.
    pub fn strip_prefix(&self, prefix: &DevicePath) -> Option<&DevicePath> {
        let mut offset = 0;
        for (node, prefix_node) in self.iter().zip(prefix.iter()) {
            if prefix_node.header.is_type::<EndEntire>() || prefix_node.header.is_type::<EndInstance>() {
                break;
            }
            if node != prefix_node {
                return None;
            }
            offset += node.header.length();
        }

        let rest = self.buffer.get(offset..)?;
        // SAFETY: DevicePath is repr(transparent) over [u8]. `rest` starts at a node boundary of a
        // complete device path, so it is itself a complete device path.
        Some(unsafe { &*(ptr::from_ref::<[u8]>(rest) as *const DevicePath) })
    }

    /// Iterate through every node of a device path.
    pub fn iter(&self) -> Iter<'_> {
        Iter::new(self)
    }

    /// Iterate through every instance of a device path.
    ///
    /// [UEFI Spec Documentation: 10.5.7. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.GetNextDevicePathInstance()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-getnextdevicepathinstance)
    pub fn iter_instances(&self) -> IterInstance<'_> {
        IterInstance { node_iterator: self.iter() }
    }

    /// Return true if this is a short-form device path.
    ///
    /// [UEFI Spec Documentation: 3.1.2. Load Option Processing](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html#load-option-processing)
    ///
    /// A short-form device path starts with a [`HardDrive`], [`FilePath`], [`Uri`],
    /// [`UsbWwid`], or [`UsbClass`] node instead of the system root, and must be expanded with
    /// [`expand_short_form`](Self::expand_short_form) before the image it points to can be
    /// loaded.
    pub fn is_short_form(&self) -> bool {
        self.iter().next().is_some_and(|node| {
            let header = node.header;
            header.is_type::<HardDrive>()
                || header.is_type::<FilePath>()
                || header.is_type::<Uri>()
                || header.is_type::<UsbWwid>()
                || header.is_type::<UsbClass>()
        })
    }
}

impl fmt::Display for DevicePath {
    /// Write the UEFI text representation of the device path.
    ///
    /// [UEFI Spec Documentation: 10.6.4. EFI_DEVICE_PATH_TO_TEXT_PROTOCOL.ConvertDevicePathToText()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-to-text-protocol-convertdevicepathtotext)
    ///
    /// The format flags select the `DisplayOnly` and `AllowShortcuts` parameters: `{}` is the
    /// default parseable form with shortcuts, `{:-}` the shorter display-only form, and `{:#}`
    /// the form without shortcuts. See the [format flags](crate::uefi::device_path#format-flags).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // End nodes print their own text (`,` or nothing); they only end the `/` separators.
        let mut need_separator = false;
        for generic in self.iter() {
            let is_end = generic.header.r#type == EndEntire::r#type();
            if need_separator && !is_end {
                f.write_str("/")?;
            }
            // A node that is unknown or cannot be decoded is printed in the generic form.
            match <Box<dyn DevicePathNode>>::try_from(generic) {
                Ok(node) => fmt::Display::fmt(&node, f)?,
                Err(_) => fmt::Display::fmt(generic, f)?,
            }
            need_separator = !is_end;
        }
        Ok(())
    }
}

/// Convert a device path to UEFI text in the default form, like `path.to_string()`.
///
/// [UEFI Spec Documentation: 10.6.4. EFI_DEVICE_PATH_TO_TEXT_PROTOCOL.ConvertDevicePathToText()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-to-text-protocol-convertdevicepathtotext)
impl From<&DevicePath> for String {
    fn from(path: &DevicePath) -> Self {
        path.to_string()
    }
}

impl ToOwned for DevicePath {
    type Owned = DevicePathBuf;

    fn to_owned(&self) -> Self::Owned {
        self.into()
    }
}

impl Borrow<DevicePath> for DevicePathBuf {
    fn borrow(&self) -> &DevicePath {
        self.as_ref()
    }
}

impl Clone for Box<DevicePath> {
    fn clone(&self) -> Self {
        DevicePathBuf::from(self.as_ref()).into()
    }
}

/// Device path instance iterator.
pub struct IterInstance<'a> {
    node_iterator: Iter<'a>,
}

impl Iterator for IterInstance<'_> {
    type Item = Result<DevicePathBuf, String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut item = None;
        for node in self.node_iterator.by_ref() {
            let header = node.header;
            if header.is_type::<EndInstance>() || header.is_type::<EndEntire>() {
                item.get_or_insert_with(DevicePathBuf::new);
                break;
            }
            if let Err(error) = item.get_or_insert_with(DevicePathBuf::new).append(node) {
                return Some(Err(error));
            }
        }
        item.map(Ok)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;

    use super::*;
    use crate::{
        devpath,
        uefi::device_path::node::{Acpi, AcpiSubType, DevicePathType, EndSubType, Pci, Sata},
    };
    use alloc::format;
    use core::assert_eq;

    #[test]
    fn test_device_path_buf_new_is_terminated() {
        let device_path = DevicePathBuf::new();
        assert_eq!(device_path.buffer, EndEntire::BYTES);
        assert_eq!(DevicePathBuf::default(), device_path);
    }

    #[test]
    fn test_device_path_append_inserts_before_end_entire() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();
        device_path.append(EndInstance).unwrap();
        device_path.append(Pci { function: 1, device: 2 }).unwrap();
        let nodes = device_path.iter().map(|node| (node.header.r#type, node.header.sub_type)).collect::<Vec<_>>();
        assert_eq!(nodes, [(2, 1), (0x7f, 1), (1, 1), (0x7f, 0xff)]);

        let before = device_path.clone();
        // The device path already ends with EndEntire, so appending it changes nothing.
        device_path.append(EndEntire).unwrap();
        assert_eq!(device_path, before);
        assert!(device_path.append(Sata::new(u16::MAX, 0, 0)).is_err());
        assert_eq!(device_path, before);
    }

    /// A node whose header length differs from its size is refused (UEFI 2.10 Table 10.1), and
    /// the device path is left unchanged.
    #[test]
    fn test_device_path_append_rejects_inconsistent_header_length() {
        use crate::uefi::device_path::node::{
            Header,
            tests::{generic_node_bytes, raw_generic_node},
        };

        let bytes = generic_node_bytes(Header::new(1, 1, 7), &[0, 2]);
        let mut device_path = DevicePathBuf::new();
        assert!(device_path.append(raw_generic_node(&bytes)).is_err());
        assert_eq!(device_path, DevicePathBuf::new());
    }

    #[test]
    fn test_device_path_try_from_bytes() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(DevicePath::try_from_bytes(device_path.as_bytes()).unwrap(), device_path.as_ref());

        let pci = &device_path.as_bytes()[..6];
        let invalid: [&[u8]; 6] = [
            &[],
            pci,
            &[0x7f, 0xff, 4],
            &[0x7f, 0xff, 4, 0, 0x7f, 0xff, 4, 0],
            &[0x7f, 0xff, 5, 0, 0],
            &[1, 1, 0, 0, 0x7f, 0xff, 4, 0],
        ];
        for bytes in invalid {
            assert!(DevicePath::try_from_bytes(bytes).is_err(), "accepted {bytes:?}");
        }

        // A PCI node with a payload one byte too short, and a SATA node with the reserved HBA port,
        // have a valid structure but do not decode.
        for bytes in [&[1, 1, 5, 0, 0, 0x7f, 0xff, 4, 0][..], &[3, 18, 10, 0, 0xff, 0xff, 0, 0, 0, 0, 0x7f, 0xff, 4, 0]]
        {
            let path = DevicePath::try_from_bytes(bytes).unwrap();
            assert!(path.validate().is_err(), "validated {bytes:?}");
        }
        // A node of a reserved type is not decoded.
        let path = DevicePath::try_from_bytes(&[9, 2, 6, 0, 0xaa, 0xbb, 0x7f, 0xff, 4, 0]).unwrap();
        assert!(path.validate().is_ok());
        assert!(device_path.validate().is_ok());
    }

    #[test]
    fn test_device_path_from_ptr_rejects_malformed_nodes() {
        // A zero length would never advance; a length below the header is also invalid.
        for length in [0u8, 3] {
            let bytes = [1, 1, length, 0, 0x7f, 0xff, 4, 0];
            // SAFETY: The buffer is readable up to its end node.
            assert!(unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) }.is_err());
        }
        // An end node with a payload is rejected before its length is used, so only its
        // header needs to be readable.
        let bytes = [0x7f, 0xff, 0xff, 0xff];
        // SAFETY: The end node header is readable; its claimed length is not trusted.
        assert!(unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) }.is_err());
        // SAFETY: A null pointer is rejected before any read.
        assert!(unsafe { DevicePath::try_from_ptr(ptr::null()) }.is_err());
    }

    #[test]
    fn test_device_path_iter_instances_splits_on_end_instance() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();
        device_path.append(EndInstance).unwrap();
        device_path.append(Pci { function: 1, device: 2 }).unwrap();
        let instances = device_path.iter_instances().collect::<Result<Vec<_>, _>>().unwrap();

        let mut first = DevicePathBuf::new();
        first.append(Acpi::new_pci_root(0)).unwrap();
        let mut second = DevicePathBuf::new();
        second.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(instances, [first, second]);
    }

    #[test]
    fn test_device_path_as_bytes() {
        let mut device_path_buf = DevicePathBuf::new();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();

        let device_path: &DevicePath = device_path_buf.as_ref();
        assert_eq!(device_path.as_bytes(), device_path_buf.buffer.as_slice());
        assert_eq!(device_path.as_bytes().len(), device_path.size());
    }

    #[test]
    fn test_device_path_append_node() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();

        let expected_data = [
            DevicePathType::Acpi as u8,
            AcpiSubType::Acpi as u8,
            12, // length lower byte
            0,  // length upper byte
            (Acpi::PCI_ROOT_HID.0 & 0xFF) as u8,
            ((Acpi::PCI_ROOT_HID.0 >> 8) & 0xFF) as u8,
            ((Acpi::PCI_ROOT_HID.0 >> 16) & 0xFF) as u8,
            ((Acpi::PCI_ROOT_HID.0 >> 24) & 0xFF) as u8,
            0, // uid byte 0
            0, // uid byte 1
            0, // uid byte 2
            0, // uid byte 3
            DevicePathType::End as u8,
            EndSubType::Entire as u8,
            4, // length lower byte
            0, // length upper byte
        ];

        assert_eq!(expected_data, device_path.buffer.as_slice());
    }

    #[test]
    fn test_device_path_append_device_path() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();

        let mut device_path_to_add = DevicePathBuf::new();
        device_path_to_add.append(Pci { function: 1, device: 2 }).unwrap();

        let mut expected_device_path = DevicePathBuf::new();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();

        device_path.append_device_path(&device_path_to_add).unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_append_device_path_instance() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();

        let mut device_path_to_add = DevicePathBuf::new();
        device_path_to_add.append(Pci { function: 1, device: 2 }).unwrap();

        let mut expected_device_path = DevicePathBuf::new();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(EndInstance).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();

        device_path.append_device_path_instance(&device_path_to_add).unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buf_from_nodes() {
        let mut expected_device_path = DevicePathBuf::new();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();

        // Iterating a device path yields its EndEntire node last, which `append` ignores.
        let device_path = expected_device_path
            .iter()
            .try_fold(DevicePathBuf::new(), |mut path, node| path.append(node).map(|()| path))
            .unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buf_to_boxed_device_path() {
        let mut device_path_buf = DevicePathBuf::new();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();

        let device_path = Box::<DevicePath>::from(DevicePathBuf::clone(&device_path_buf));

        assert_eq!(device_path_buf.as_ref(), device_path.as_ref());
    }

    #[test]
    fn test_device_path_from_ptr() {
        let mut device_path_buf = DevicePathBuf::new();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        let buffer_ptr = device_path_buf.buffer.as_slice().as_ptr();

        // SAFETY: Test code - creating DevicePath from a valid buffer pointer for testing.
        let device_path = unsafe { DevicePath::try_from_ptr(buffer_ptr) }.unwrap();

        assert_eq!(device_path_buf.as_ref(), device_path);
    }

    #[test]
    fn test_device_path_size() {
        let mut device_path_buf = DevicePathBuf::new();
        assert_eq!(4, device_path_buf.size());
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        assert_eq!(16, device_path_buf.size());
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(22, device_path_buf.size());
    }

    #[test]
    fn test_device_path_node_count() {
        let mut device_path_buf = DevicePathBuf::new();
        assert_eq!(1, device_path_buf.node_count());
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        assert_eq!(2, device_path_buf.node_count());
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(3, device_path_buf.node_count());
    }

    #[test]
    fn test_device_path_is_multi_instances() {
        let mut device_path_buf = DevicePathBuf::new();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();

        let mut device_path_buf_2 = DevicePathBuf::new();
        device_path_buf_2.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf_2.append(Pci { function: 1, device: 2 }).unwrap();

        assert!(!device_path_buf.is_multi_instance());

        device_path_buf.append_device_path_instance(&device_path_buf_2).unwrap();

        assert!(device_path_buf.is_multi_instance());
    }

    #[test]
    fn test_device_path_starts_with() {
        let mut device_path_buf = DevicePathBuf::new();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();

        let mut start = DevicePathBuf::new();
        start.append(Acpi::new_pci_root(0)).unwrap();

        assert!(device_path_buf.starts_with(&start));

        let mut start = DevicePathBuf::new();
        start.append(Acpi::new_pci_root(1)).unwrap();

        assert!(!device_path_buf.starts_with(&start));
    }

    #[test]
    fn test_device_path_strip_prefix() {
        let mut path = DevicePathBuf::new();
        path.append(Acpi::new_pci_root(0)).unwrap();
        path.append(Pci { function: 1, device: 2 }).unwrap();

        let mut prefix = DevicePathBuf::new();
        prefix.append(Acpi::new_pci_root(0)).unwrap();
        let mut rest = DevicePathBuf::new();
        rest.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(path.strip_prefix(&prefix), Some(rest.as_ref()));
        assert_eq!(path.strip_prefix(&path), Some(DevicePathBuf::new().as_ref()));
        assert_eq!(path.strip_prefix(&DevicePathBuf::new()), Some(path.as_ref()));

        let mut other = DevicePathBuf::new();
        other.append(Acpi::new_pci_root(1)).unwrap();
        assert_eq!(path.strip_prefix(&other), None);
        // A prefix longer than the path does not match.
        assert_eq!(prefix.strip_prefix(&path), None);
        // The remaining path borrows the original bytes.
        let remaining = path.strip_prefix(&prefix).unwrap();
        assert_eq!(remaining.as_bytes().as_ptr(), path.as_bytes()[12..].as_ptr());
        // Only the first instance of a multi-instance prefix is matched.
        let mut multi_instance = prefix.clone();
        multi_instance.append_device_path_instance(&other).unwrap();
        assert!(path.starts_with(&multi_instance));
        assert_eq!(path.strip_prefix(&multi_instance), Some(rest.as_ref()));
    }

    #[test]
    fn test_device_path_text_common_nodes() {
        let text = |bytes: &[u8]| DevicePath::try_from_bytes(bytes).expect("macro output is a valid path").to_string();
        assert_eq!(text(&devpath!("PciRoot(0)/Pci(0x11,0)")), "PciRoot(0x0)/Pci(0x11,0x0)");
        assert_eq!(
            text(&devpath!("USB(2,1)/HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)")),
            "USB(0x2,0x1)/HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)"
        );
        assert_eq!(text(&devpath!(r"\EFI\BOOT\BOOTX64.EFI")), r"\EFI\BOOT\BOOTX64.EFI");
    }

    #[test]
    fn test_device_path_text_instances() {
        let bytes = devpath!("Pci(1,0),USB(2,1)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(format!("{path}"), "Pci(0x1,0x0),USB(0x2,0x1)");
    }

    // cspell:ignore iscsitarget iscsidisk
    #[test]
    fn test_device_path_text_format_flags() {
        // The iSCSI boot device of UEFI 2.10 Section 10.3.4.20.2.
        let bytes = devpath!(
            "PciRoot(0)/Pci(0x19,0)/MAC(001320F5FA77,0x1)/IPv4(192.168.0.100,TCP,Static,192.168.0.1)/iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0000000000000000,None,None,None,TCP)/HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)"
        );
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        let nodes = "/Pci(0x19,0x0)/MAC(001320f5fa77,0x1)/IPv4(192.168.0.100,TCP,Static,192.168.0.1,0.0.0.0,0.0.0.0)/iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)/HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)";
        let display_only_nodes = "/Pci(0x19,0x0)/MAC(001320f5fa77,0x1)/IPv4(192.168.0.100)/iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)/HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC)";
        assert_eq!(format!("{path}"), format!("PciRoot(0x0){nodes}"));
        assert_eq!(format!("{path:-}"), format!("PciRoot(0x0){display_only_nodes}"));
        assert_eq!(format!("{path:#}"), format!("Acpi(PNP0A03,0x0){nodes}"));
        assert_eq!(format!("{path:-#}"), format!("Acpi(PNP0A03,0x0){display_only_nodes}"));
        let buffer = DevicePathBuf::from(path);
        assert_eq!(format!("{buffer:-#}"), format!("{path:-#}"));
    }

    #[test]
    fn test_device_path_text_into_string() {
        let bytes = devpath!("PciRoot(0)/Pci(0x1D,0)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(String::from(path), "PciRoot(0x0)/Pci(0x1d,0x0)");
        let text: String = path.into();
        assert_eq!(text, path.to_string());
    }
}
