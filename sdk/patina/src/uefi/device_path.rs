//! Device Path Definitions
//!
//! Defines UEFI device path node structures and utilities for walking and parsing paths and nodes.
//! Uses the UEFI specification as a reference for struct definitions and parsing logic:
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html>
//!
//! ## Text representation
//!
//! [`DevicePath`], [`DevicePathBuf`] and every node implement [`Display`]
//! with the text representation of `EFI_DEVICE_PATH_TO_TEXT_PROTOCOL` (UEFI 2.10 Section 10.6).
//! Standalone end nodes display as their conventional separators (`EndInstance` as `,`,
//! `EndEntire` as an empty string); in a path, they separate instances and end the path.
//!
//! ### Format flags
//!
//! The `DisplayOnly` and `AllowShortcuts` parameters of
//! [`ConvertDevicePathToText()`](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-to-text-protocol-convertdevicepathtotext)
//! are selected with format flags:
//!
//! | Format    | `DisplayOnly` | `AllowShortcuts` | Example                       |
//! |-----------|---------------|------------------|-------------------------------|
//! | `{}`      | `FALSE`       | `TRUE`           | `PciRoot(0x0)/HD(1,MBR,0x12345678,0x800,0x1000)` |
//! | `{:-}`    | `TRUE`        | `TRUE`           | `PciRoot(0x0)/HD(1,MBR,0x12345678)` |
//! | `{:#}`    | `FALSE`       | `FALSE`          | `Acpi(PNP0A03,0x0)/HD(1,MBR,0x12345678,0x800,0x1000)` |
//! | `{:-#}`   | `TRUE`        | `FALSE`          | `Acpi(PNP0A03,0x0)/HD(1,MBR,0x12345678)` |
//!
//! The default form is lossless and parses back to the same device path. The display-only
//! form (`-`) is shorter and may omit fields. A shortcut is a node name chosen from
//! information other than the type and sub-type, such as `PciRoot` for an ACPI node with
//! the PCI root bridge HID; `#` uses only the names of the type and sub-type
//! (UEFI 2.10 Section 10.6.4 and Table 10.67).
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// String encoding contexts for device path fields.
pub mod encoding;
/// Module for FV-related Device Path struct implementations.
pub mod fv_types;
/// Device path node definitions.
pub mod node;
/// Module for walking UEFI device paths, providing utilities for traversing and analyzing device path structures.
pub mod walker;

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    borrow::Borrow,
    clone::Clone,
    convert::{AsRef, From},
    fmt::{self, Debug, Display},
    iter::Iterator,
    ops::{Deref, DerefMut},
};

use scroll::{Pread, Pwrite, ctx::TryIntoCtx};

use crate::{
    base::error::EfiError,
    standard::efi,
    uefi::{
        boot_services::BootServices,
        device_path::node::{
            DevicePathNode, DevicePathType, EndEntire, EndInstance, FilePath, GenericDevicePathNode, HardDrive, Header,
            Iter, Uri, UsbClass, UsbWwid,
        },
        protocols::usb_io,
    },
};

/// `DevicePathBuf` is an owned version of device path. This is used to create device paths or when performing mutable operations on them.
///
/// A `DevicePathBuf` always holds a complete device path: its last node is an
/// [`EndEntire`] node, and every operation keeps it that way.
#[derive(Debug, Clone)]
pub struct DevicePathBuf {
    buffer: Vec<u8>,
}

/// The bytes of an [`EndEntire`] node, which terminates every device path.
const END_ENTIRE_BYTES: [u8; 4] = [0x7f, 0xff, 4, 0];

impl Default for DevicePathBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl DevicePathBuf {
    /// Create a device path that contains only its [`EndEntire`] node.
    pub fn new() -> Self {
        Self { buffer: Vec::from(END_ENTIRE_BYTES) }
    }

    /// Append a node to the device path, before its [`EndEntire`] node, like the
    /// `AppendDeviceNode` function of the device path utilities protocol (UEFI 2.10
    /// Section 10.5.1).
    ///
    /// [UEFI Spec Documentation: 10.5.5. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.AppendDeviceNode()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicenode)
    ///
    /// Append an [`EndInstance`] node to start a new instance. [`EndEntire`] cannot be
    /// appended: the device path already ends with it.
    ///
    /// The node is checked with [`validate`](DevicePathNode::validate), then its header and
    /// payload are encoded directly into the device path buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the node is an [`EndEntire`] node or fails validation, the node
    /// is too large for a 16-bit length, the header length differs from
    /// [`measure_size`](DevicePathNode::measure_size), allocation fails, or the payload
    /// cannot be encoded. The device path is unchanged on failure.
    pub fn append<T>(&mut self, node: T) -> Result<(), scroll::Error>
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        let header = node.header();
        if EndEntire::is_type(header.r#type, header.sub_type) {
            return Err(scroll::Error::BadInput { size: 0, msg: "a device path already ends with EndEntire" });
        }
        let end_start = self.buffer.len() - END_ENTIRE_BYTES.len();
        self.buffer.truncate(end_start);
        let result = Self::encode_node(&mut self.buffer, node);
        // On failure `encode_node` restores the buffer, so the end node is restored either way.
        self.buffer.extend_from_slice(&END_ENTIRE_BYTES);
        result
    }

    /// Encode `node`, header included, at the end of `buffer`.
    ///
    /// On failure, `buffer` is unchanged.
    fn encode_node<T>(buffer: &mut Vec<u8>, node: T) -> Result<(), scroll::Error>
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        node.validate().map_err(|msg| scroll::Error::BadInput { size: 0, msg })?;
        let payload_length = node.measure_size();
        let node_length = Header::size_of_header()
            .checked_add(payload_length)
            .filter(|length| u16::try_from(*length).is_ok())
            .ok_or(scroll::Error::BadInput { size: payload_length, msg: "device path node is too large" })?;
        let header = node.header();
        if usize::from(header.length) != node_length {
            return Err(scroll::Error::BadInput {
                size: usize::from(header.length),
                msg: "device path node header length differs from its size",
            });
        }

        let start = buffer.len();
        // Also reserve room for the end node that `append` puts back after the node.
        buffer
            .try_reserve_exact(node_length + END_ENTIRE_BYTES.len())
            .map_err(|_| scroll::Error::BadInput { size: node_length, msg: "cannot allocate device path node" })?;
        // No allocation will be done here, handled by the `try_reserve`.
        buffer.resize(start + node_length, 0);

        let node_bytes = buffer.get_mut(start..).expect("buffer was just resized to fit");
        let result = node_bytes.pwrite_with(header, 0, scroll::LE).and_then(|_| {
            match node_bytes.pwrite_with(node, Header::size_of_header(), scroll::LE)? {
                written if written == payload_length => Ok(()),
                written => Err(scroll::Error::BadInput {
                    size: written,
                    msg: "encoded payload size differs from measure_size",
                }),
            }
        });
        if result.is_err() {
            buffer.truncate(start);
        }
        result
    }

    /// Append a device path to this device path, the `EndEntire` node of self will be removed when appending the other path.
    ///
    /// [UEFI Spec Documentation: 10.5.4. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.AppendDevicePath()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicepath)
    ///
    /// # Errors
    ///
    /// Returns an error if allocation fails.
    pub fn append_device_path(&mut self, device_path: &DevicePath) -> Result<(), scroll::Error> {
        self.append_path(device_path, false)
    }

    /// Append a device path to this device path, the `EndEntire` node of self will be replaced with an end instance.
    ///
    /// [UEFI Spec Documentation: 10.5.6. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.AppendDevicePathInstance()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicepathinstance)
    ///
    /// # Errors
    ///
    /// Returns an error if allocation fails.
    pub fn append_device_path_instance(&mut self, device_path: &DevicePath) -> Result<(), scroll::Error> {
        self.append_path(device_path, true)
    }

    fn append_path(&mut self, other: &DevicePath, instance: bool) -> Result<(), scroll::Error> {
        self.buffer
            .try_reserve_exact(other.size())
            .map_err(|_| scroll::Error::BadInput { size: other.size(), msg: "cannot allocate appended device path" })?;
        let end_start = self.buffer.len() - END_ENTIRE_BYTES.len();
        if instance {
            // Turn the end node of self into an end instance node.
            *self.buffer.get_mut(end_start + 1).expect("a device path ends with EndEntire") =
                node::EndSubType::Instance as u8;
        } else {
            self.buffer.truncate(end_start);
        }
        self.buffer.extend_from_slice(other.as_bytes());
        Ok(())
    }

    /// Create a device path from an iterator of device path nodes.
    ///
    /// [UEFI Spec Documentation: 10.5.5. EFI_DEVICE_PATH_UTILITIES_PROTOCOL.AppendDeviceNode()](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#efi-device-path-utilities-protocol-appenddevicenode)
    ///
    /// The [`EndEntire`] node is added after the nodes. The iterator may also yield it as
    /// its last node, as iterating a [`DevicePath`] does.
    ///
    /// # Errors
    ///
    /// Returns an error if a node cannot be encoded, allocation fails, or nodes
    /// occur after `EndEntire`.
    pub fn from_device_path_node_iter<I, T>(iter: I) -> Result<DevicePathBuf, scroll::Error>
    where
        I: Iterator<Item = T>,
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        let mut device_path = DevicePathBuf::new();
        let mut ended = false;
        for node in iter {
            if ended {
                return Err(scroll::Error::BadInput { size: 0, msg: "node follows EndEntire" });
            }
            let header = node.header();
            if EndEntire::is_type(header.r#type, header.sub_type) {
                ended = true;
                continue;
            }
            device_path.append(node)?;
        }
        Ok(device_path)
    }

    /// Create a device path from the bytes of whole nodes, followed by the bytes of a terminated device path.
    fn concat(nodes: &[u8], path: &[u8]) -> Result<DevicePathBuf, EfiError> {
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(nodes.len() + path.len()).map_err(|_| EfiError::OutOfResources)?;
        buffer.extend_from_slice(nodes);
        buffer.extend_from_slice(path);
        Ok(DevicePathBuf { buffer })
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

impl DerefMut for DevicePathBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: DevicePath has the same memory layout as `[u8]`.
        unsafe { &mut *(core::ptr::from_mut::<[u8]>(self.buffer.as_mut_slice()) as *mut DevicePath) }
    }
}

impl AsRef<DevicePath> for DevicePathBuf {
    fn as_ref(&self) -> &DevicePath {
        self
    }
}

impl AsMut<DevicePath> for DevicePathBuf {
    fn as_mut(&mut self) -> &mut DevicePath {
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
        unsafe { &*(core::ptr::from_ref::<[u8]>(value.buffer.as_slice()) as *const DevicePath) }
    }
}

impl Display for DevicePathBuf {
    /// Write the UEFI text representation of the device path; see [`DevicePath`].
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Display::fmt(self.as_ref(), f)
    }
}

impl PartialEq for DevicePathBuf {
    fn eq(&self, other: &DevicePathBuf) -> bool {
        self.buffer == other.buffer
    }
}

impl Eq for DevicePathBuf {}

/// `DevicePath` is the borrowed version of a [`DevicePathBuf`].
/// Only immutable operations are possible on this type.
#[derive(Debug)]
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
    /// The nodes are walked up to the [`EndEntire`] node, then the device path is checked like
    /// [`try_from_bytes`](Self::try_from_bytes) does, including each node's payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the pointer is null, a node is shorter than its header or
    /// malformed, or the device path is too large to address.
    ///
    /// # Safety
    ///
    /// The buffer pointer must point to valid device path data that remains valid
    /// for the lifetime 'a. The device path must be properly terminated with an
    /// `EndEntire` node.
    pub unsafe fn try_from_ptr<'a>(buffer: *const u8) -> Result<&'a DevicePath, &'static str> {
        if buffer.is_null() {
            return Err("Null pointer provided");
        }

        // Find the size of the device path by walking the node headers.
        let mut size = 0usize;
        loop {
            // SAFETY: The caller guarantees a valid device path up to its EndEntire node. The
            // previous nodes were not EndEntire, so a node header starts at `size`.
            let header_bytes = unsafe { core::slice::from_raw_parts(buffer.add(size), Header::size_of_header()) };
            let header =
                header_bytes.pread_with::<Header>(0, scroll::LE).map_err(|_| "Error while trying to read header.")?;
            let length = usize::from(header.length);
            if length < Header::size_of_header() {
                return Err("Device path node is shorter than its header.");
            }
            size = size
                .checked_add(length)
                .filter(|size| isize::try_from(*size).is_ok())
                .ok_or("Device path is too large.")?;
            if EndEntire::is_type(header.r#type, header.sub_type) {
                break;
            }
        }

        // SAFETY: The caller guarantees that the device path, which spans `size` bytes up to and
        // including its EndEntire node, is valid for 'a.
        let bytes = unsafe { core::slice::from_raw_parts(buffer, size) };
        DevicePath::try_from_bytes(bytes).map_err(|_| "Invalid device path.")
    }

    /// Borrow a complete device path from `bytes`.
    ///
    /// [UEFI Spec Documentation: 10.3.1. Generic Device Path Structures](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html#generic-device-path-structures)
    ///
    /// Every node must have a length of at least its header, end nodes must follow the
    /// specification, and the last node, and only the last node, must be [`EndEntire`]. Each
    /// node of a known type is decoded, and must have a well-formed payload that passes
    /// [`validate`](DevicePathNode::validate); nodes of an unknown type are accepted, since
    /// the specification lets them be skipped.
    ///
    /// # Errors
    ///
    /// Returns an error if `bytes` is not a complete device path, or a node is malformed.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<&DevicePath, scroll::Error> {
        let mut rest = bytes;
        loop {
            let (node, next) = GenericDevicePathNode::from_bytes(rest)?;
            // Decode the node as the type its header identifies, which checks its payload and the
            // specification rules. Node types that are not known are accepted as they are.
            <Box<dyn DevicePathNode>>::try_from(node)?;
            if EndEntire::is_type(node.header.r#type, node.header.sub_type) {
                if !next.is_empty() {
                    return Err(scroll::Error::BadInput { size: next.len(), msg: "data follows EndEntire" });
                }
                break;
            }
            rest = next;
        }
        // SAFETY: DevicePath is repr(transparent) over [u8], and the bytes were checked to be a
        // complete device path.
        Ok(unsafe { &*(core::ptr::from_ref::<[u8]>(bytes) as *const DevicePath) })
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
        self.iter().any(|n| EndInstance::is_type(n.header.r#type, n.header.sub_type))
    }

    /// Return true if the device path starts with the other device path.
    pub fn starts_with(&self, other: &DevicePath) -> bool {
        let self_iter = self.iter();
        let other_iter = other.iter();
        for (self_node, other_node) in self_iter.zip(other_iter) {
            if other_node.header.r#type == DevicePathType::End as u8 {
                return true;
            }
            if self_node != other_node {
                return false;
            }
        }
        false
    }

    /// Return the rest of this device path after `prefix`.
    ///
    /// Every node of `prefix` before its end node must be equal, byte for byte, to the node at
    /// the same position in this device path. The result borrows this device path, starts at
    /// the first node after the matched ones, and keeps this device path's end node. This is
    /// the match that `LocateDevicePath` makes between the device path of a handle and the
    /// device path it searches for.
    ///
    /// Returns `None` if `prefix` is not a prefix of this device path.
    pub fn strip_prefix(&self, prefix: &DevicePath) -> Option<&DevicePath> {
        let mut nodes = self.iter();
        let mut offset = 0;
        for prefix_node in prefix.iter() {
            if EndEntire::is_type(prefix_node.header.r#type, prefix_node.header.sub_type) {
                break;
            }
            let node = nodes.next()?;
            if node != prefix_node {
                return None;
            }
            offset += node.as_bytes().len();
        }
        let rest = self.buffer.get(offset..)?;
        // SAFETY: DevicePath is repr(transparent) over [u8]. `rest` starts at a node boundary of a
        // complete device path, so it is itself a complete device path.
        Some(unsafe { &*(core::ptr::from_ref::<[u8]>(rest) as *const DevicePath) })
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

    /// Return true if this is a shorthand device path, which the specification calls a
    /// short-form device path.
    ///
    /// [UEFI Spec Documentation: 3.1.2. Load Option Processing](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html#load-option-processing)
    ///
    /// A short-form device path starts with a [`HardDrive`], [`FilePath`], [`Uri`],
    /// [`UsbWwid`], or [`UsbClass`] node instead of the system root, and must be expanded with
    /// [`expand_shorthand`](Self::expand_shorthand) before the image it points to can be
    /// loaded.
    pub fn is_shorthand_notation(&self) -> bool {
        self.iter().next().is_some_and(|node| {
            let (r#type, sub_type) = (node.header.r#type, node.header.sub_type);
            HardDrive::is_type(r#type, sub_type)
                || FilePath::is_type(r#type, sub_type)
                || Uri::is_type(r#type, sub_type)
                || UsbWwid::is_type(r#type, sub_type)
                || UsbClass::is_type(r#type, sub_type)
        })
    }

    /// Expand a shorthand device path into the full device paths it can refer to, in the
    /// order a boot manager tries them.
    ///
    /// [UEFI Spec Documentation: 3.1.2. Load Option Processing](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html#load-option-processing)
    ///
    /// A device path that is not shorthand is returned unchanged, as the only candidate. For a
    /// shorthand device path, the first node selects how the devices are searched:
    ///
    /// | First node | Matching devices | Candidate |
    /// |---|---|---|
    /// | [`HardDrive`] | Device paths with a hard drive node of the same partition number, partition format, signature type, and signature | The device path up to the hard drive node, then the nodes after the first one |
    /// | [`UsbWwid`] | USB I/O handles with the same vendor ID, product ID, interface number, and serial number suffix | The handle's device path, then the nodes after the first one |
    /// | [`UsbClass`] | USB I/O handles with the same vendor ID, product ID, class, subclass, and protocol; a field of all ones matches any value | The handle's device path, then the nodes after the first one |
    /// | [`FilePath`] | Simple file system handles: removable media, then fixed media, then file systems without block I/O | The handle's device path, then the whole shorthand path |
    /// | [`Uri`] | Load file handles | The handle's device path, then the whole shorthand path, so that loading it passes the shorthand path to the handle's `LoadFile` |
    ///
    /// For USB, the class, subclass, and protocol come from the interface descriptor when the
    /// device descriptor class is zero. The USB WWID serial number is compared with the end of
    /// the device's serial number string in each language the device supports.
    ///
    /// When several devices match, the specification lets the boot manager pick any of them;
    /// trying the candidates in order does that. Controllers are not connected: connect them
    /// first so that the devices to search exist.
    ///
    /// # Errors
    ///
    /// - [`EfiError::InvalidParameter`] if the path has no node besides its end node.
    /// - [`EfiError::NotFound`] if no device matches.
    /// - [`EfiError::OutOfResources`] if a candidate cannot be allocated.
    /// - Any error other than `NOT_FOUND` returned by `LocateHandleBuffer`.
    pub fn expand_shorthand<B: BootServices>(&self, boot_services: &B) -> Result<Vec<DevicePathBuf>, EfiError> {
        let Some(first_node) = self.iter().next().filter(|node| node.header.r#type != DevicePathType::End as u8) else {
            log::error!("Cannot expand an empty device path");
            return Err(EfiError::InvalidParameter);
        };
        if !self.is_shorthand_notation() {
            return Ok(Vec::from([self.into()]));
        }
        let rest = self.buffer.get(first_node.as_bytes().len()..).unwrap_or_default();

        let mut candidates = Vec::new();
        if let Ok(target) = HardDrive::try_from(first_node) {
            for handle in shorthand::handles(boot_services, &efi::protocols::device_path::PROTOCOL_GUID)? {
                let Some(handle_path) = shorthand::device_path(boot_services, handle) else {
                    continue;
                };
                let mut prefix_length = 0;
                for node in handle_path.iter().take_while(|node| node.header.r#type != DevicePathType::End as u8) {
                    prefix_length += node.as_bytes().len();
                    if HardDrive::try_from(node).is_ok_and(|hard_drive| shorthand::same_partition(&hard_drive, &target))
                    {
                        let prefix = handle_path.as_bytes().get(..prefix_length).unwrap_or_default();
                        candidates.push(DevicePathBuf::concat(prefix, rest)?);
                        break;
                    }
                }
            }
        } else if let Ok(wwid) = UsbWwid::try_from(first_node) {
            for (handle_path, usb_io) in shorthand::usb_devices(boot_services)? {
                if shorthand::usb_wwid_matches(boot_services, usb_io, &wwid) {
                    candidates.push(DevicePathBuf::concat(handle_path.nodes(), rest)?);
                }
            }
        } else if let Ok(class) = UsbClass::try_from(first_node) {
            for (handle_path, usb_io) in shorthand::usb_devices(boot_services)? {
                if shorthand::usb_class_matches(usb_io, &class) {
                    candidates.push(DevicePathBuf::concat(handle_path.nodes(), rest)?);
                }
            }
        } else if FilePath::try_from(first_node).is_ok() {
            let file_systems = shorthand::handles(boot_services, &efi::protocols::simple_file_system::PROTOCOL_GUID)?;
            let (mut removable, mut fixed, mut without_block_io) = (Vec::new(), Vec::new(), Vec::new());
            for handle in file_systems {
                let Some(handle_path) = shorthand::device_path(boot_services, handle) else {
                    continue;
                };
                // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is block I/O.
                match unsafe { boot_services.handle_protocol::<efi::protocols::block_io::Protocol>(handle) } {
                    // SAFETY: The block I/O protocol of a handle points to its media description.
                    Ok(block_io) if unsafe { block_io.media.as_ref() }.is_some_and(|media| media.removable_media) => {
                        removable.push(handle_path);
                    }
                    Ok(_) => fixed.push(handle_path),
                    Err(_) => without_block_io.push(handle_path),
                }
            }
            for handle_path in removable.into_iter().chain(fixed).chain(without_block_io) {
                candidates.push(DevicePathBuf::concat(handle_path.nodes(), self.as_bytes())?);
            }
        } else if Uri::try_from(first_node).is_ok() {
            for handle in shorthand::handles(boot_services, &efi::protocols::load_file::PROTOCOL_GUID)? {
                if let Some(handle_path) = shorthand::device_path(boot_services, handle) {
                    candidates.push(DevicePathBuf::concat(handle_path.nodes(), self.as_bytes())?);
                }
            }
        } else {
            log::error!("Shorthand device path starts with an invalid node");
            return Err(EfiError::InvalidParameter);
        }

        if candidates.is_empty() {
            log::error!("No device matches the shorthand device path");
            return Err(EfiError::NotFound);
        }
        Ok(candidates)
    }

    /// Return the bytes of the nodes before the end node of a single-instance device path.
    fn nodes(&self) -> &[u8] {
        self.buffer.get(..self.buffer.len() - END_ENTIRE_BYTES.len()).unwrap_or_default()
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
        let mut need_separator = false;
        for generic in self.iter() {
            if EndEntire::is_type(generic.header.r#type, generic.header.sub_type) {
                break;
            }
            if EndInstance::is_type(generic.header.r#type, generic.header.sub_type) {
                f.write_str(",")?;
                need_separator = false;
                continue;
            }
            if need_separator {
                f.write_str("/")?;
            }
            node::fmt_generic_node(generic, f)?;
            need_separator = true;
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

impl PartialEq for &DevicePath {
    fn eq(&self, other: &&DevicePath) -> bool {
        self.buffer == other.buffer
    }
}

impl Eq for &DevicePath {}

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

/// Device searches used to expand shorthand device paths.
mod shorthand {
    use alloc::vec::Vec;

    use super::{DevicePath, HardDrive, UsbClass, UsbWwid, usb_io};
    use crate::{
        base::error::EfiError,
        standard::efi,
        uefi::boot_services::{BootServices, protocol_handler::HandleSearchType},
    };

    /// Maximum number of UTF-16 code units read from a USB string descriptor, which holds at
    /// most 126 characters.
    const MAX_USB_STRING_LENGTH: usize = 127;

    /// Return the handles that support `protocol`; none if no handle supports it.
    pub(super) fn handles<B: BootServices>(
        boot_services: &B,
        protocol: &'static efi::Guid,
    ) -> Result<Vec<efi::Handle>, EfiError> {
        match boot_services.locate_handle_buffer(HandleSearchType::ByProtocol(protocol)) {
            Ok(handles) => Ok(handles.iter().copied().collect()),
            Err(efi::Status::NOT_FOUND) => Ok(Vec::new()),
            Err(status) => Err(EfiError::from(status)),
        }
    }

    /// Return the device path installed on `handle`, if it has a valid one.
    pub(super) fn device_path<B: BootServices>(boot_services: &B, handle: efi::Handle) -> Option<&'static DevicePath> {
        // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is the device path protocol.
        let device_path =
            unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) }.ok()?;
        // SAFETY: The pointer comes from a device path protocol interface installed on the handle.
        unsafe { DevicePath::try_from_ptr(core::ptr::from_ref(device_path).cast::<u8>()) }.ok()
    }

    /// Return true if two hard drive nodes identify the same partition.
    pub(super) fn same_partition(hard_drive: &HardDrive, target: &HardDrive) -> bool {
        hard_drive.partition_number == target.partition_number
            && hard_drive.partition_format == target.partition_format
            && hard_drive.signature_type == target.signature_type
            && hard_drive.partition_signature == target.partition_signature
    }

    /// Return the device path and USB I/O protocol of every USB I/O handle.
    pub(super) fn usb_devices<B: BootServices>(
        boot_services: &B,
    ) -> Result<Vec<(&'static DevicePath, &'static mut usb_io::Protocol)>, EfiError> {
        let mut devices = Vec::new();
        for handle in handles(boot_services, &usb_io::PROTOCOL_GUID)? {
            let Some(handle_path) = device_path(boot_services, handle) else {
                continue;
            };
            // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is USB I/O. Each
            // handle is visited once, so the interface is borrowed once.
            if let Ok(usb_io) = unsafe { boot_services.handle_protocol::<usb_io::Protocol>(handle) } {
                devices.push((handle_path, usb_io));
            }
        }
        Ok(devices)
    }

    fn device_descriptor(usb_io: &mut usb_io::Protocol) -> Option<usb_io::DeviceDescriptor> {
        let mut descriptor = usb_io::DeviceDescriptor::default();
        ((usb_io.get_device_descriptor)(usb_io, &raw mut descriptor) == efi::Status::SUCCESS).then_some(descriptor)
    }

    fn interface_descriptor(usb_io: &mut usb_io::Protocol) -> Option<usb_io::InterfaceDescriptor> {
        let mut descriptor = usb_io::InterfaceDescriptor::default();
        ((usb_io.get_interface_descriptor)(usb_io, &raw mut descriptor) == efi::Status::SUCCESS).then_some(descriptor)
    }

    /// Return true if the USB device matches a USB class node; a field of all ones matches any value.
    pub(super) fn usb_class_matches(usb_io: &mut usb_io::Protocol, class: &UsbClass) -> bool {
        let Some(device) = device_descriptor(usb_io) else {
            return false;
        };
        let (vendor_id, product_id) = (device.id_vendor, device.id_product);
        let (mut device_class, mut device_subclass, mut device_protocol) =
            (device.device_class, device.device_sub_class, device.device_protocol);
        if device_class == 0 {
            // Each interface declares its own class.
            let Some(interface) = interface_descriptor(usb_io) else {
                return false;
            };
            (device_class, device_subclass, device_protocol) =
                (interface.interface_class, interface.interface_sub_class, interface.interface_protocol);
        }
        (class.vendor_id == u16::MAX || class.vendor_id == vendor_id)
            && (class.product_id == u16::MAX || class.product_id == product_id)
            && (class.device_class == u8::MAX || class.device_class == device_class)
            && (class.device_subclass == u8::MAX || class.device_subclass == device_subclass)
            && (class.device_protocol == u8::MAX || class.device_protocol == device_protocol)
    }

    /// Return true if the USB device matches a USB WWID node.
    pub(super) fn usb_wwid_matches<B: BootServices>(
        boot_services: &B,
        usb_io: &mut usb_io::Protocol,
        wwid: &UsbWwid,
    ) -> bool {
        let Some(device) = device_descriptor(usb_io) else {
            return false;
        };
        if device.id_vendor != wwid.vendor_id || device.id_product != wwid.product_id || device.str_serial_number == 0 {
            return false;
        }
        if interface_descriptor(usb_io).is_none_or(|interface| u16::from(interface.interface_number) != wwid.interface)
        {
            return false;
        }

        let mut serial_suffix = wwid.serial_number.as_slice();
        if let [rest @ .., 0] = serial_suffix {
            serial_suffix = rest;
        }
        let mut languages: *mut u16 = core::ptr::null_mut();
        let mut table_size = 0u16;
        if (usb_io.get_supported_languages)(usb_io, &raw mut languages, &raw mut table_size) != efi::Status::SUCCESS
            || languages.is_null()
        {
            return false;
        }
        // SAFETY: The protocol returned a language table of `table_size` bytes, which it owns.
        let languages = unsafe { core::slice::from_raw_parts(languages, usize::from(table_size) / 2) };
        for &language in languages {
            let mut serial: *mut u16 = core::ptr::null_mut();
            if (usb_io.get_string_descriptor)(usb_io, language, device.str_serial_number, &raw mut serial)
                != efi::Status::SUCCESS
                || serial.is_null()
            {
                continue;
            }
            // SAFETY: The protocol returned a NUL-terminated string, read up to the longest USB string.
            let length = (0..MAX_USB_STRING_LENGTH).find(|&index| unsafe { serial.add(index).read() } == 0);
            let matches = length.is_some_and(|length| {
                // SAFETY: The first `length` code units of the string were just read.
                let serial = unsafe { core::slice::from_raw_parts(serial, length) };
                serial.ends_with(serial_suffix)
            });
            // SAFETY: The string was allocated from pool memory by the protocol, and is not used after this.
            if unsafe { boot_services.free_pool(serial.cast::<u8>()) }.is_err() {
                log::warn!("Cannot free a USB serial number string");
            }
            if matches {
                return true;
            }
        }
        false
    }
}

/// Device path instance iterator.
pub struct IterInstance<'a> {
    node_iterator: Iter<'a>,
}

impl Iterator for IterInstance<'_> {
    type Item = Result<DevicePathBuf, scroll::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut item = None;
        for node in self.node_iterator.by_ref() {
            let header = node.header;
            if EndInstance::is_type(header.r#type, header.sub_type)
                || EndEntire::is_type(header.r#type, header.sub_type)
            {
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
    use super::*;
    use core::assert_eq;

    use crate::uefi::device_path::node::{Acpi, AcpiSubType, EndSubType, Pci};

    #[test]
    fn test_new_device_path_buf_is_terminated() {
        let device_path = DevicePathBuf::new();
        assert_eq!(device_path.buffer, END_ENTIRE_BYTES);
        assert_eq!(DevicePathBuf::default(), device_path);
    }

    #[test]
    fn test_append_inserts_before_end_entire() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();
        device_path.append(EndInstance).unwrap();
        device_path.append(Pci { function: 1, device: 2 }).unwrap();
        let nodes = device_path.iter().map(|node| (node.header.r#type, node.header.sub_type)).collect::<Vec<_>>();
        assert_eq!(nodes, [(2, 1), (0x7f, 1), (1, 1), (0x7f, 0xff)]);

        let before = device_path.clone();
        assert!(device_path.append(EndEntire).is_err());
        assert!(device_path.append(crate::uefi::device_path::node::Sata::new(u16::MAX, 0, 0)).is_err());
        assert_eq!(device_path, before);
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

        // A PCI node with a payload one byte too short, and a SATA node with the reserved HBA port.
        assert!(DevicePath::try_from_bytes(&[1, 1, 5, 0, 0, 0x7f, 0xff, 4, 0]).is_err());
        assert!(DevicePath::try_from_bytes(&[3, 18, 10, 0, 0xff, 0xff, 0, 0, 0, 0, 0x7f, 0xff, 4, 0]).is_err());
        // A node of a reserved type is not decoded.
        assert!(DevicePath::try_from_bytes(&[9, 2, 6, 0, 0xaa, 0xbb, 0x7f, 0xff, 4, 0]).is_ok());
    }

    #[test]
    fn test_device_path_from_ptr_rejects_malformed_nodes() {
        // A zero length would never advance; a length below the header is also invalid.
        for length in [0u8, 3] {
            let bytes = [1, 1, length, 0, 0x7f, 0xff, 4, 0];
            // SAFETY: The buffer is readable up to its end node.
            assert!(unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) }.is_err());
        }
        // An end node with a payload.
        let bytes = [0x7f, 0xff, 5, 0, 0];
        // SAFETY: The buffer holds the five bytes the end node claims.
        assert!(unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) }.is_err());
        // SAFETY: A null pointer is rejected before any read.
        assert!(unsafe { DevicePath::try_from_ptr(core::ptr::null()) }.is_err());
    }

    #[test]
    fn test_iter_instances_splits_on_end_instance() {
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
    fn test_append_node() {
        let mut device_path = DevicePathBuf::new();
        device_path.append(Acpi::new_pci_root(0)).unwrap();

        let expected_data = [
            DevicePathType::Acpi as u8,
            AcpiSubType::Acpi as u8,
            12, // length lower byte
            0,  // length upper byte
            (Acpi::PCI_ROOT_HID & 0xFF) as u8,
            ((Acpi::PCI_ROOT_HID >> 8) & 0xFF) as u8,
            ((Acpi::PCI_ROOT_HID >> 16) & 0xFF) as u8,
            ((Acpi::PCI_ROOT_HID >> 24) & 0xFF) as u8,
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
    fn test_append_device_path() {
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
    fn test_append_device_path_instance() {
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
    fn test_device_path_buff_from_node_iter() {
        let mut expected_device_path = DevicePathBuf::new();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();

        let device_path = DevicePathBuf::from_device_path_node_iter(expected_device_path.iter()).unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buff_to_boxed_device_path() {
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
    fn test_device_path_start_with() {
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
    }

    mod shorthand {
        extern crate std;

        use super::*;
        use crate::uefi::{
            boot_services::{MockBootServices, boxed::BootServicesBox, protocol_handler::HandleSearchType},
            device_path::node::{FilePath, HardDrive, PiwgFirmwareVolume, Uri, Usb, UsbClass, UsbWwid},
            protocols::usb_io,
        };

        fn path<T>(nodes: impl IntoIterator<Item = T>) -> DevicePathBuf
        where
            T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
        {
            DevicePathBuf::from_device_path_node_iter(nodes.into_iter()).unwrap()
        }

        fn join(paths: &[&DevicePathBuf]) -> DevicePathBuf {
            let mut joined = DevicePathBuf::new();
            for path in paths {
                joined.append_device_path(path).unwrap();
            }
            joined
        }

        fn hard_drive(guid: [u8; 16]) -> HardDrive {
            HardDrive::new_gpt(1, 2048, 1_000_000, guid)
        }

        fn boot_file() -> DevicePathBuf {
            path([FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI")])
        }

        /// `HD(1,GPT,<guid>)/\EFI\BOOT\BOOTX64.EFI`.
        fn shorthand_path(guid: [u8; 16]) -> DevicePathBuf {
            join(&[&path([hard_drive(guid)]), &boot_file()])
        }

        /// `PciRoot(0)/Pci(0x1D,0)/HD(1,GPT,<guid>)`.
        fn disk_path(guid: [u8; 16]) -> DevicePathBuf {
            join(&[
                &path([Acpi::new_pci_root(0)]),
                &path([Pci { function: 0, device: 0x1D }]),
                &path([hard_drive(guid)]),
            ])
        }

        /// `PciRoot(0)/Pci(0x14,0)/USB(<port>,0)`.
        fn usb_path(port: u8) -> DevicePathBuf {
            join(&[
                &path([Acpi::new_pci_root(0)]),
                &path([Pci { function: 0, device: 0x14 }]),
                &path([Usb { parent_port_number: port, interface_number: 0 }]),
            ])
        }

        /// A USB I/O protocol whose functions read the descriptors that follow it.
        #[repr(C)]
        struct FakeUsb {
            protocol: usb_io::Protocol,
            device: usb_io::DeviceDescriptor,
            interface: usb_io::InterfaceDescriptor,
            serial: Vec<u16>,
            languages: Vec<u16>,
        }

        fn fake(this: *mut usb_io::Protocol) -> &'static FakeUsb {
            // SAFETY: Test code - every protocol is the first field of a leaked FakeUsb.
            unsafe { &*this.cast::<FakeUsb>() }
        }

        extern "efiapi" fn get_device_descriptor(
            this: *mut usb_io::Protocol,
            descriptor: *mut usb_io::DeviceDescriptor,
        ) -> efi::Status {
            // SAFETY: Test code - the caller passes a valid descriptor.
            unsafe { descriptor.write(fake(this).device) };
            efi::Status::SUCCESS
        }

        extern "efiapi" fn get_interface_descriptor(
            this: *mut usb_io::Protocol,
            descriptor: *mut usb_io::InterfaceDescriptor,
        ) -> efi::Status {
            // SAFETY: Test code - the caller passes a valid descriptor.
            unsafe { descriptor.write(fake(this).interface) };
            efi::Status::SUCCESS
        }

        extern "efiapi" fn get_string_descriptor(
            this: *mut usb_io::Protocol,
            _lang_id: u16,
            _string_id: u8,
            string: *mut *mut u16,
        ) -> efi::Status {
            // SAFETY: Test code - the caller passes a valid output pointer; free_pool is a no-op.
            unsafe { string.write(fake(this).serial.as_ptr().cast_mut()) };
            efi::Status::SUCCESS
        }

        extern "efiapi" fn get_supported_languages(
            this: *mut usb_io::Protocol,
            table: *mut *mut u16,
            size: *mut u16,
        ) -> efi::Status {
            let languages = &fake(this).languages;
            // SAFETY: Test code - the caller passes valid output pointers.
            unsafe {
                table.write(languages.as_ptr().cast_mut());
                size.write(u16::try_from(languages.len() * 2).unwrap());
            }
            efi::Status::SUCCESS
        }

        fn usb_device(vendor_id: u16, product_id: u16, class: [u8; 3], interface: [u8; 3], serial: &str) -> FakeUsb {
            let mut serial = serial.encode_utf16().collect::<Vec<_>>();
            serial.push(0);
            FakeUsb {
                protocol: usb_io::Protocol {
                    control_transfer: core::ptr::null(),
                    bulk_transfer: core::ptr::null(),
                    async_interrupt_transfer: core::ptr::null(),
                    sync_interrupt_transfer: core::ptr::null(),
                    isochronous_transfer: core::ptr::null(),
                    async_isochronous_transfer: core::ptr::null(),
                    get_device_descriptor,
                    get_config_descriptor: core::ptr::null(),
                    get_interface_descriptor,
                    get_endpoint_descriptor: core::ptr::null(),
                    get_string_descriptor,
                    get_supported_languages,
                    port_reset: core::ptr::null(),
                },
                device: usb_io::DeviceDescriptor {
                    device_class: class[0],
                    device_sub_class: class[1],
                    device_protocol: class[2],
                    id_vendor: vendor_id,
                    id_product: product_id,
                    str_serial_number: 3,
                    ..Default::default()
                },
                interface: usb_io::InterfaceDescriptor {
                    interface_number: 0,
                    interface_class: interface[0],
                    interface_sub_class: interface[1],
                    interface_protocol: interface[2],
                    ..Default::default()
                },
                serial,
                languages: Vec::from([0x0409]),
            }
        }

        extern "efiapi" fn reset(_: *mut efi::protocols::block_io::Protocol, _: efi::Boolean) -> efi::Status {
            efi::Status::UNSUPPORTED
        }

        extern "efiapi" fn transfer(
            _: *mut efi::protocols::block_io::Protocol,
            _: u32,
            _: efi::Lba,
            _: usize,
            _: *mut core::ffi::c_void,
        ) -> efi::Status {
            efi::Status::UNSUPPORTED
        }

        extern "efiapi" fn flush(_: *mut efi::protocols::block_io::Protocol) -> efi::Status {
            efi::Status::UNSUPPORTED
        }

        fn block_io(removable: bool) -> &'static mut efi::protocols::block_io::Protocol {
            let media = std::boxed::Box::leak(std::boxed::Box::new(efi::protocols::block_io::Media {
                media_id: 0,
                removable_media: removable,
                media_present: true,
                logical_partition: false,
                read_only: false,
                write_caching: false,
                block_size: 512,
                io_align: 0,
                last_block: 0,
                lowest_aligned_lba: 0,
                logical_blocks_per_physical_block: 1,
                optimal_transfer_length_granularity: 0,
            }));
            std::boxed::Box::leak(std::boxed::Box::new(efi::protocols::block_io::Protocol {
                revision: 0,
                media,
                reset,
                read_blocks: transfer,
                write_blocks: transfer,
                flush_blocks: flush,
            }))
        }

        /// A handle of the mocked system and the protocols installed on it.
        #[derive(Default)]
        struct Device {
            path: Option<DevicePathBuf>,
            usb: Option<FakeUsb>,
            file_system: bool,
            removable: Option<bool>,
            load_file: bool,
        }

        impl Device {
            fn with_path(path: DevicePathBuf) -> Self {
                Self { path: Some(path), ..Default::default() }
            }
        }

        /// A mock whose `LocateHandleBuffer` returns the handles that support a protocol, and
        /// whose `HandleProtocol` returns the protocols of a handle. Handle `n` is `devices[n - 1]`.
        fn mock_system(devices: Vec<Device>) -> MockBootServices {
            let mut paths: Vec<Option<usize>> = Vec::new();
            let mut usb_addresses: Vec<Option<usize>> = Vec::new();
            let mut block_ios: Vec<Option<usize>> = Vec::new();
            // The protocols each handle supports: device path, USB I/O, simple file system, load file.
            let mut protocols: Vec<[bool; 4]> = Vec::new();
            for device in devices {
                protocols.push([device.path.is_some(), device.usb.is_some(), device.file_system, device.load_file]);
                paths.push(
                    device.path.map(|path| {
                        std::boxed::Box::leak(path.as_bytes().to_vec().into_boxed_slice()).as_ptr() as usize
                    }),
                );
                usb_addresses.push(
                    device
                        .usb
                        .map(|usb| core::ptr::from_mut(std::boxed::Box::leak(std::boxed::Box::new(usb))) as usize),
                );
                block_ios.push(device.removable.map(|removable| core::ptr::from_mut(block_io(removable)) as usize));
            }
            let supports = move |guid: &efi::Guid, index: usize| -> bool {
                let [path, usb, file_system, load_file] = protocols[index];
                (*guid == efi::protocols::device_path::PROTOCOL_GUID && path)
                    || (*guid == usb_io::PROTOCOL_GUID && usb)
                    || (*guid == efi::protocols::simple_file_system::PROTOCOL_GUID && file_system)
                    || (*guid == efi::protocols::load_file::PROTOCOL_GUID && load_file)
            };
            let count = paths.len();
            let free_pool: &'static MockBootServices = std::boxed::Box::leak(std::boxed::Box::new({
                let mut mock = MockBootServices::new();
                mock.expect_free_pool().returning(|_| Ok(()));
                mock
            }));

            let mut mock = MockBootServices::new();
            mock.expect_free_pool().returning(|_| Ok(()));
            mock.expect_locate_handle_buffer().returning(move |search| {
                let HandleSearchType::ByProtocol(guid) = search else {
                    return Err(efi::Status::INVALID_PARAMETER);
                };
                let handles: Vec<efi::Handle> =
                    (0..count).filter(|&index| supports(guid, index)).map(|index| (index + 1) as efi::Handle).collect();
                if handles.is_empty() {
                    return Err(efi::Status::NOT_FOUND);
                }
                let handles = handles.leak();
                // SAFETY: Test code - the leaked handle buffer outlives the returned box, and free_pool is a no-op.
                Ok(unsafe { BootServicesBox::from_raw_parts_mut(handles.as_mut_ptr(), handles.len(), free_pool) })
            });
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>().returning(move |handle| {
                match paths.get(handle as usize - 1).copied().flatten() {
                    // SAFETY: Test code - the address points to a leaked, valid device path.
                    Some(address) => Ok(unsafe { &mut *(address as *mut efi::protocols::device_path::Protocol) }),
                    None => Err(efi::Status::UNSUPPORTED),
                }
            });
            mock.expect_handle_protocol::<usb_io::Protocol>().returning(move |handle| {
                match usb_addresses.get(handle as usize - 1).copied().flatten() {
                    // SAFETY: Test code - the address points to a leaked FakeUsb, which starts with its protocol.
                    Some(address) => Ok(unsafe { &mut *(address as *mut usb_io::Protocol) }),
                    None => Err(efi::Status::UNSUPPORTED),
                }
            });
            mock.expect_handle_protocol::<efi::protocols::block_io::Protocol>().returning(move |handle| {
                match block_ios.get(handle as usize - 1).copied().flatten() {
                    // SAFETY: Test code - the address points to a leaked block I/O protocol.
                    Some(address) => Ok(unsafe { &mut *(address as *mut efi::protocols::block_io::Protocol) }),
                    None => Err(efi::Status::UNSUPPORTED),
                }
            });
            mock
        }

        #[test]
        fn test_device_path_is_shorthand_notation() {
            assert!(shorthand_path([0xaa; 16]).is_shorthand_notation());
            assert!(boot_file().is_shorthand_notation());
            assert!(path([Uri { uri: "http://a/b.efi".into() }]).is_shorthand_notation());
            assert!(
                path([UsbWwid { interface: 0, vendor_id: 1, product_id: 2, serial_number: Vec::from([0x41]) }])
                    .is_shorthand_notation()
            );
            assert!(
                path([UsbClass {
                    vendor_id: u16::MAX,
                    product_id: u16::MAX,
                    device_class: u8::MAX,
                    device_subclass: u8::MAX,
                    device_protocol: u8::MAX,
                }])
                .is_shorthand_notation()
            );
            assert!(!disk_path([0xaa; 16]).is_shorthand_notation());
            assert!(!path([Pci { function: 0, device: 0x1D }]).is_shorthand_notation());
            assert!(
                !path([PiwgFirmwareVolume { fv_name: crate::BinaryGuid::from_bytes(&[0x11; 16]) }])
                    .is_shorthand_notation()
            );
            assert!(!path([EndEntire]).is_shorthand_notation());
        }

        #[test]
        fn test_device_path_expand_shorthand_full_path_is_unchanged() {
            let full = disk_path([0xaa; 16]);
            assert_eq!(full.expand_shorthand(&MockBootServices::new()).unwrap(), [full]);
        }

        #[test]
        fn test_device_path_expand_shorthand_rejects_empty_path() {
            assert_eq!(path([EndEntire]).expand_shorthand(&MockBootServices::new()), Err(EfiError::InvalidParameter));
        }

        #[test]
        fn test_device_path_expand_shorthand_matches_hard_drive() {
            let guid = [0xaa; 16];
            let file_system = join(&[&disk_path(guid), &path([FilePath::new("\\ignored")])]);
            let mock = mock_system(Vec::from([
                Device::default(),
                Device::with_path(disk_path([0xbb; 16])),
                Device::with_path(file_system),
            ]));
            let expected = join(&[&disk_path(guid), &boot_file()]);
            assert_eq!(shorthand_path(guid).expand_shorthand(&mock).unwrap(), [expected]);
        }

        #[test]
        fn test_device_path_expand_shorthand_requires_full_identity() {
            let guid = [0xaa; 16];
            let other_partition =
                join(&[&path([Acpi::new_pci_root(0)]), &path([HardDrive::new_gpt(2, 2048, 1_000_000, guid)])]);
            let mock = mock_system(Vec::from([Device::with_path(other_partition)]));
            assert_eq!(shorthand_path(guid).expand_shorthand(&mock), Err(EfiError::NotFound));
        }

        #[test]
        fn test_device_path_expand_shorthand_returns_every_match() {
            let guid = [0xaa; 16];
            let second = join(&[&path([Acpi::new_pci_root(1)]), &path([hard_drive(guid)])]);
            let mock = mock_system(Vec::from([Device::with_path(disk_path(guid)), Device::with_path(second.clone())]));
            let expected = [join(&[&disk_path(guid), &boot_file()]), join(&[&second, &boot_file()])];
            assert_eq!(shorthand_path(guid).expand_shorthand(&mock).unwrap(), expected);
        }

        #[test]
        fn test_device_path_expand_shorthand_reports_locate_errors() {
            let mut mock = MockBootServices::new();
            mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));
            assert_eq!(shorthand_path([0xaa; 16]).expand_shorthand(&mock), Err(EfiError::NotFound));

            let mut mock = MockBootServices::new();
            mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::DEVICE_ERROR));
            assert_eq!(shorthand_path([0xaa; 16]).expand_shorthand(&mock), Err(EfiError::DeviceError));
        }

        #[test]
        fn test_device_path_expand_shorthand_matches_usb_wwid() {
            let wwid = UsbWwid {
                interface: 0,
                vendor_id: 0x1234,
                product_id: 0x5678,
                serial_number: "123456".encode_utf16().collect(),
            };
            let shorthand = join(&[&path([wwid]), &boot_file()]);
            let devices = Vec::from([
                // Wrong serial number.
                Device {
                    usb: Some(usb_device(0x1234, 0x5678, [0; 3], [8, 6, 0x50], "SN-999")),
                    ..Device::with_path(usb_path(1))
                },
                // The WWID holds the end of the serial number.
                Device {
                    usb: Some(usb_device(0x1234, 0x5678, [0; 3], [8, 6, 0x50], "SN-123456")),
                    ..Device::with_path(usb_path(2))
                },
                // Wrong product.
                Device {
                    usb: Some(usb_device(0x1234, 0x0001, [0; 3], [8, 6, 0x50], "123456")),
                    ..Device::with_path(usb_path(3))
                },
            ]);
            let expected = join(&[&usb_path(2), &boot_file()]);
            assert_eq!(shorthand.expand_shorthand(&mock_system(devices)).unwrap(), [expected]);
        }

        #[test]
        fn test_device_path_expand_shorthand_matches_usb_class() {
            let mass_storage = UsbClass {
                vendor_id: u16::MAX,
                product_id: u16::MAX,
                device_class: 8,
                device_subclass: u8::MAX,
                device_protocol: 0x50,
            };
            let devices = || {
                Vec::from([
                    // A HID device.
                    Device { usb: Some(usb_device(1, 1, [0; 3], [3, 1, 1], "a")), ..Device::with_path(usb_path(1)) },
                    // The device class is zero, so the interface descriptor declares the class.
                    Device { usb: Some(usb_device(2, 2, [0; 3], [8, 6, 0x50], "b")), ..Device::with_path(usb_path(2)) },
                    // The device descriptor declares the class.
                    Device { usb: Some(usb_device(3, 3, [8, 6, 0x50], [0; 3], "c")), ..Device::with_path(usb_path(3)) },
                ])
            };
            let expected = [usb_path(2), usb_path(3)];
            assert_eq!(path([mass_storage.clone()]).expand_shorthand(&mock_system(devices())).unwrap(), expected);

            let vendor_two = UsbClass { vendor_id: 2, ..mass_storage };
            assert_eq!(path([vendor_two]).expand_shorthand(&mock_system(devices())).unwrap(), [usb_path(2)]);
        }

        #[test]
        fn test_device_path_expand_shorthand_orders_file_systems() {
            let fixed = disk_path([1; 16]);
            let removable = usb_path(1);
            let no_block_io = path([Acpi::new_pci_root(7)]);
            let mock = mock_system(Vec::from([
                Device { file_system: true, removable: Some(false), ..Device::with_path(fixed.clone()) },
                Device { file_system: true, ..Device::with_path(no_block_io.clone()) },
                Device { file_system: true, removable: Some(true), ..Device::with_path(removable.clone()) },
                // Not a file system.
                Device { removable: Some(true), ..Device::with_path(path([Acpi::new_pci_root(8)])) },
            ]));
            let expected =
                [join(&[&removable, &boot_file()]), join(&[&fixed, &boot_file()]), join(&[&no_block_io, &boot_file()])];
            assert_eq!(boot_file().expand_shorthand(&mock).unwrap(), expected);
        }

        #[test]
        fn test_device_path_expand_shorthand_uses_load_file_handles() {
            let uri = path([Uri { uri: "http://192.168.0.1/boot.efi".into() }]);
            let network = path([Acpi::new_pci_root(3)]);
            let mock = mock_system(Vec::from([
                Device { load_file: true, ..Device::with_path(network.clone()) },
                Device::with_path(disk_path([1; 16])),
            ]));
            assert_eq!(uri.expand_shorthand(&mock).unwrap(), [join(&[&network, &uri])]);
        }
    }
}

#[cfg(test)]
mod text_tests {
    use crate::{
        devpath,
        uefi::device_path::{DevicePath, node::*},
    };
    use alloc::format;

    fn round_trip<const N: usize>(_text: &str, bytes: &[u8; N], expected: &str) {
        let path = DevicePath::try_from_bytes(bytes).expect("macro output is a valid path");
        assert_eq!(format!("{path}"), expected);
    }

    #[test]
    fn test_device_path_text_common_nodes() {
        round_trip("PciRoot(0)/Pci(0x11,0)", &devpath!("PciRoot(0)/Pci(0x11,0)"), "PciRoot(0x0)/Pci(0x11,0x0)");
        round_trip(
            "USB(2,1)/HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)",
            &devpath!("USB(2,1)/HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)"),
            "USB(0x2,0x1)/HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
        );
        round_trip(r"\EFI\BOOT\BOOTX64.EFI", &devpath!(r"\EFI\BOOT\BOOTX64.EFI"), r"\EFI\BOOT\BOOTX64.EFI");
    }

    #[test]
    fn test_device_path_text_instances_and_options() {
        let bytes = devpath!("Pci(1,0),USB(2,1)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(format!("{path}"), "Pci(0x1,0x0),USB(0x2,0x1)");

        let hd = DevicePath::try_from_bytes(&devpath!("HD(1,MBR,0x12345678,0x800,0x1000)")).expect("valid path");
        assert_eq!(format!("{hd:-}"), "HD(1,MBR,0x12345678)");

        let uart = UartFlowControl { flow_control_map: UartFlowControl::HARDWARE };
        assert_eq!(format!("{uart}"), "UartFlowCtrl(Hardware)");
        assert_eq!(format!("{uart:#}"), "VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,01000000)");
    }

    #[test]
    fn test_device_path_text_format_flags() {
        let bytes = devpath!("PciRoot(0)/HD(1,MBR,0x12345678,0x800,0x1000)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(format!("{path}"), "PciRoot(0x0)/HD(1,MBR,0x12345678,0x800,0x1000)");
        assert_eq!(format!("{path:-}"), "PciRoot(0x0)/HD(1,MBR,0x12345678)");
        assert_eq!(format!("{path:#}"), "Acpi(PNP0A03,0x0)/HD(1,MBR,0x12345678,0x800,0x1000)");
        assert_eq!(format!("{path:-#}"), "Acpi(PNP0A03,0x0)/HD(1,MBR,0x12345678)");
        let buffer = crate::uefi::device_path::DevicePathBuf::from(path);
        assert_eq!(format!("{buffer:-#}"), format!("{path:-#}"));

        // A shortcut is not used without `AllowShortcuts`, even in the display-only form.
        let acpi_ex = ExtendedAcpi {
            hid: Acpi::PCI_ROOT_HID,
            uid: 0,
            cid: 0,
            hid_string: String::new(),
            uid_string: String::new(),
            cid_string: String::new(),
        };
        assert_eq!(format!("{acpi_ex:-}"), "PciRoot(0x0)");
        assert_eq!(format!("{acpi_ex:-#}"), "AcpiEx(PNP0A03,0,0x0)");

        // The flags select the text form only; they do not change how field values are written.
        let pci = Pci { function: 2, device: 0x1f };
        for text in [format!("{pci}"), format!("{pci:-}"), format!("{pci:#}"), format!("{pci:+}")] {
            assert_eq!(text, "Pci(0x1f,0x2)");
        }
    }

    #[test]
    fn test_device_path_text_into_string() {
        let bytes = devpath!("PciRoot(0)/Pci(0x1D,0)");
        let path = DevicePath::try_from_bytes(&bytes).expect("macro output is a valid path");
        assert_eq!(String::from(path), "PciRoot(0x0)/Pci(0x1d,0x0)");
        let text: String = path.into();
        assert_eq!(text, path.to_string());
    }

    #[test]
    fn test_generic_device_path_text() {
        let bytes = [9u8, 2, 6, 0, 0xaa, 0xbb, 0x7f, 0xff, 4, 0];
        let path = DevicePath::try_from_bytes(&bytes).expect("generic path is valid");
        assert_eq!(format!("{path}"), "Path(9,2,aabb)");
    }

    /// Check the text of every kind of node in the three text forms, and that the parseable
    /// forms parse back to the same bytes (UEFI 2.10 Section 10.6.1.3).
    #[test]
    fn test_device_path_text_round_trip() {
        macro_rules! check {
            ($input:literal, $text:literal, $display_only:literal, $no_shortcuts:literal) => {{
                let bytes = devpath!($input);
                let path = DevicePath::try_from_bytes(&bytes).expect($input);
                assert_eq!(format!("{path}"), $text, "default text of {}", $input);
                assert_eq!(format!("{path:-}"), $display_only, "display-only text of {}", $input);
                assert_eq!(format!("{path:#}"), $no_shortcuts, "text without shortcuts of {}", $input);
                assert_eq!(devpath!($text), bytes, "default text of {} does not parse back", $input);
                assert_eq!(devpath!($no_shortcuts), bytes, "text without shortcuts of {} does not parse back", $input);
            }};
        }
        check!(r"Pci(0x1F,0x2)", r"Pci(0x1f,0x2)", r"Pci(0x1f,0x2)", r"Pci(0x1f,0x2)");
        check!(r"PcCard(1)", r"PcCard(0x1)", r"PcCard(0x1)", r"PcCard(0x1)");
        check!(
            r"MemoryMapped(11,0x1000,0x1FFF)",
            r"MemoryMapped(0xb,0x1000,0x1fff)",
            r"MemoryMapped(0xb,0x1000,0x1fff)",
            r"MemoryMapped(0xb,0x1000,0x1fff)"
        );
        check!(
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenHw(00112233-4455-6677-8899-AABBCCDDEEFF,0102)"
        );
        check!(r"Ctrl(3)", r"Ctrl(0x3)", r"Ctrl(0x3)", r"Ctrl(0x3)");
        check!(r"BMC(1,0x1000)", r"BMC(0x1,0x1000)", r"BMC(0x1,0x1000)", r"BMC(0x1,0x1000)");
        check!(r"PciRoot(1)", r"PciRoot(0x1)", r"PciRoot(0x1)", r"Acpi(PNP0A03,0x1)");
        check!(r"PcieRoot(2)", r"PcieRoot(0x2)", r"PcieRoot(0x2)", r"Acpi(PNP0A08,0x2)");
        check!(r"Floppy(0)", r"Floppy(0x0)", r"Floppy(0x0)", r"Acpi(PNP0604,0x0)");
        check!(r"Keyboard(0)", r"Keyboard(0x0)", r"Keyboard(0x0)", r"Acpi(PNP0301,0x0)");
        check!(r"Serial(1)", r"Serial(0x1)", r"Serial(0x1)", r"Acpi(PNP0501,0x1)");
        check!(r"ParallelPort(0)", r"ParallelPort(0x0)", r"ParallelPort(0x0)", r"Acpi(PNP0401,0x0)");
        check!(r"Acpi(HWP0002,3)", r"Acpi(HWP0002,0x3)", r"Acpi(HWP0002,0x3)", r"Acpi(HWP0002,0x3)");
        check!(
            r"AcpiEx(HWP0002,PNP0A03,0)",
            r"AcpiEx(HWP0002,PNP0A03,0x0,,,)",
            r"PciRoot(0x0)",
            r"AcpiEx(HWP0002,PNP0A03,0x0,,,)"
        );
        check!(
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)",
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)",
            r"PciRoot(UniqueId)",
            r"AcpiExp(HWP0002,PNP0A03,UniqueId)"
        );
        check!(
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)",
            r"AcpiAdr(0x80010100,0x80010200)"
        );
        check!(r"Ata(Primary,Master,0)", r"Ata(Primary,Master,0x0)", r"Ata(0x0)", r"Ata(Primary,Master,0x0)");
        check!(r"Scsi(1,2)", r"Scsi(0x1,0x2)", r"Scsi(0x1,0x2)", r"Scsi(0x1,0x2)");
        check!(
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)",
            r"Fibre(0x1122334455667788,0x10)"
        );
        check!(
            r"FibreEx(0102030405060708,1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)",
            r"FibreEx(0x0102030405060708,0x1112131415161718)"
        );
        check!(
            r"I1394(0x0123456789ABCDEF)",
            r"I1394(0x0123456789abcdef)",
            r"I1394(0x0123456789abcdef)",
            r"I1394(0x0123456789abcdef)"
        );
        check!(r"USB(1,0)", r"USB(0x1,0x0)", r"USB(0x1,0x0)", r"USB(0x1,0x0)");
        check!(r"I2O(5)", r"I2O(0x5)", r"I2O(0x5)", r"I2O(0x5)");
        check!(
            r"Infiniband(1,00112233-4455-6677-8899-AABBCCDDEEFF,1,2,3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)",
            r"Infiniband(0x1,00112233-4455-6677-8899-AABBCCDDEEFF,0x1,0x2,0x3)"
        );
        check!(
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)",
            r"VenMsg(00112233-4455-6677-8899-AABBCCDDEEFF,0102)"
        );
        check!(r"VenPcAnsi()", r"VenPcAnsi()", r"VenPcAnsi()", r"VenMsg(E0C14753-F9BE-11D2-9A0C-0090273FC14D)");
        check!(r"VenVt100()", r"VenVt100()", r"VenVt100()", r"VenMsg(DFA66065-B419-11D3-9A2D-0090273FC14D)");
        check!(
            r"VenVt100Plus()",
            r"VenVt100Plus()",
            r"VenVt100Plus()",
            r"VenMsg(7BAEC70B-57E0-4C76-8E87-2F9E28088343)"
        );
        check!(r"VenUtf8()", r"VenUtf8()", r"VenUtf8()", r"VenMsg(AD15A0D6-8BEC-4ACF-A073-D01DE77E2D88)");
        check!(
            r"UartFlowCtrl(XonXoff)",
            r"UartFlowCtrl(XonXoff)",
            r"UartFlowCtrl(XonXoff)",
            r"VenMsg(37499A9D-542F-4C89-A026-35DA142094E4,02000000)"
        );
        check!(
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,2)",
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,0x2)",
            r"SAS(0x1234,0x5,0x6,SAS,External,Expanded,0x2)",
            r"VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,000000003412000000000000050000000000000062010600)"
        );
        check!(r"DebugPort()", r"DebugPort()", r"DebugPort()", r"VenMsg(EBA4E8D2-3858-41EC-A281-2647BA9660D0)");
        check!(r"MAC(001122334455,0x1)", r"MAC(001122334455,0x1)", r"MAC(001122334455,0x1)", r"MAC(001122334455,0x1)");
        check!(
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,UDP,DHCP,0.0.0.0,0.0.0.0,0.0.0.0)",
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,UDP,DHCP,0.0.0.0,0.0.0.0,0.0.0.0)"
        );
        check!(
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)",
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)",
            r"IPv4(192.168.0.1)",
            r"IPv4(192.168.0.1,TCP,Static,192.168.0.2,192.168.0.254,255.255.255.0)"
        );
        check!(
            r"IPv6(2001:db8::1)",
            r"IPv6(2001:db8::1,UDP,Static,::,0x0,::)",
            r"IPv6(2001:db8::1)",
            r"IPv6(2001:db8::1,UDP,Static,::,0x0,::)"
        );
        check!(
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080)",
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080,0.0.0.0,0.0.0.0)",
            r"IPv4(192.168.0.1:80)",
            r"IPv4(192.168.0.1:80,TCP,Static,192.168.0.2:8080,0.0.0.0,0.0.0.0)"
        );
        check!(
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,64,2001:db8::ffff)",
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,0x40,2001:db8::ffff)",
            r"IPv6([2001:db8::1]:443)",
            r"IPv6([2001:db8::1]:443,TCP,StatefulAutoConfigure,[2001:db8::2]:8443,0x40,2001:db8::ffff)"
        );
        check!(r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)", r"Uart(115200,8,N,1)");
        check!(
            r"UsbClass(0x1234,0x5678,0x8,0x6,0x50)",
            r"UsbMassStorage(0x1234,0x5678,0x6,0x50)",
            r"UsbMassStorage(0x1234,0x5678,0x6,0x50)",
            r"UsbClass(0x1234,0x5678,0x8,0x6,0x50)"
        );
        check!(
            r"UsbMassStorage(0xFFFF,0xFFFF,0x6,0x50)",
            r"UsbMassStorage(0xffff,0xffff,0x6,0x50)",
            r"UsbMassStorage(0xffff,0xffff,0x6,0x50)",
            r"UsbClass(0xffff,0xffff,0x8,0x6,0x50)"
        );
        check!(
            r#"UsbWwid(0x1234,0x5678,0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#,
            r#"UsbWwid(0x1234,0x5678,0x0,"SN123")"#
        );
        check!(r"Unit(1)", r"Unit(0x1)", r"Unit(0x1)", r"Unit(0x1)");
        check!(r"Sata(0,0xFFFF,0)", r"Sata(0x0,0xffff,0x0)", r"Sata(0x0,0xffff,0x0)", r"Sata(0x0,0xffff,0x0)");
        check!(
            r"iSCSI(iqn.x,1,0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)",
            r"iSCSI(iqn.x,0x1,0x0001020304050607,CRC32C,None,CHAP_BI,TCP)"
        );
        check!(r"Vlan(100)", r"Vlan(100)", r"Vlan(100)", r"Vlan(100)");
        check!(
            r"SasEx(0102030405060708,1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,SATA,Internal,Direct)"
        );
        check!(
            r"SasEx(0102030405060708,1112131415161718,0x6)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,NoTopology)"
        );
        check!(
            r"SasEx(0102030405060708,1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)",
            r"SasEx(0x0102030405060708,0x1112131415161718,0x6,0x81)"
        );
        check!(
            r"SAS(0x1234,0,0,SATA,Internal,Direct,256)",
            r"SAS(0x1234,0x0,0x0,SATA,Internal,Direct,0x100)",
            r"SAS(0x1234,0x0,0x0,SATA,Internal,Direct,0x100)",
            r"VenMsg(D487DDB4-008B-11D9-AFDC-001083FFCA4D,000000003412000000000000000000000000000012ff0000)"
        );
        check!(
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)",
            r"NVMe(0x1,00-11-22-33-44-55-66-77)"
        );
        check!(r"Uri(http://a/b)", r"Uri(http://a/b)", r"Uri(http://a/b)", r"Uri(http://a/b)");
        check!(r"UFS(0,0x3)", r"UFS(0x0,0x3)", r"UFS(0x0,0x3)", r"UFS(0x0,0x3)");
        check!(r"SD(1)", r"SD(0x1)", r"SD(0x1)", r"SD(0x1)");
        check!(
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)",
            r"Bluetooth(001122334455)"
        );
        check!(r"Wi-Fi(ssid)", r"Wi-Fi(ssid)", r"Wi-Fi(ssid)", r"Wi-Fi(ssid)");
        check!(r"eMMC(0)", r"eMMC(0x0)", r"eMMC(0x0)", r"eMMC(0x0)");
        check!(
            r"BluetoothLE(001122334455,0x1)",
            r"BluetoothLE(001122334455,0x01)",
            r"BluetoothLE(001122334455,0x01)",
            r"BluetoothLE(001122334455,0x01)"
        );
        check!(
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)",
            r"Dns(192.168.0.1,192.168.0.2)"
        );
        check!(r"RestService(1,1)", r"RestService(0x1,0x1)", r"RestService(0x1,0x1)", r"RestService(0x1,0x1)");
        check!(
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            r"NVMEoF(nqn.x,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)"
        );
        check!(
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"HD(1,GPT,00112233-4455-6677-8899-AABBCCDDEEFF,0x800,0x1000)"
        );
        check!(
            r"HD(2,MBR,0x12345678,0x800,0x1000)",
            r"HD(2,MBR,0x12345678,0x800,0x1000)",
            r"HD(2,MBR,0x12345678)",
            r"HD(2,MBR,0x12345678,0x800,0x1000)"
        );
        check!(r"CDROM(1,0x10,0x20)", r"CDROM(0x1,0x10,0x20)", r"CDROM(0x1)", r"CDROM(0x1,0x10,0x20)");
        check!(
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)",
            r"VenMedia(00112233-4455-6677-8899-AABBCCDDEEFF,01)"
        );
        check!(r"\file", r"\file", r"\file", r"\file");
        check!(
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Media(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"Fv(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"FvFile(00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(r"Offset(0x10,0x20)", r"Offset(0x10,0x20)", r"Offset(0x10,0x20)", r"Offset(0x10,0x20)");
        check!(
            r"RamDisk(0x1000,0x1FFF,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)",
            r"RamDisk(0x1000,0x1fff,0,00112233-4455-6677-8899-AABBCCDDEEFF)"
        );
        check!(
            r"VirtualDisk(0x1000,0x1FFF,0)",
            r"VirtualDisk(0x1000,0x1fff,0)",
            r"VirtualDisk(0x1000,0x1fff,0)",
            r"RamDisk(0x1000,0x1fff,0,77AB535A-45FC-624B-5560-F7B281D1F96E)"
        );
        check!(r"BBS(HD,boot,0x0)", r"BBS(HD,boot,0x0)", r"BBS(HD,boot)", r"BBS(HD,boot,0x0)");
        check!(r"Path(9,2,aabb)", r"Path(9,2,aabb)", r"Path(9,2,aabb)", r"Path(9,2,aabb)");
        check!(r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)", r"HardwarePath(9,aabb)");
        check!(r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)", r"AcpiPath(9,aabb)");
        check!(r"Msg(99,aabb)", r"Msg(99,aabb)", r"Msg(99,aabb)", r"Msg(99,aabb)");
        check!(r"MediaPath(99,aabb)", r"MediaPath(99,aabb)", r"MediaPath(99,aabb)", r"MediaPath(99,aabb)");
        check!(r"BbsPath(9,aabb)", r"BbsPath(9,aabb)", r"BbsPath(9,aabb)", r"BbsPath(9,aabb)");
    }
}
