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
//! use patina::uefi::device_path::node::{Acpi, ExtendedAcpi};
//!
//! // AcpiEx(HWP0002,PNP0A03,0)
//! let node = ExtendedAcpi {
//!     hid: Acpi::eisa_id("HWP0002"),
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
mod shorthand;

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
    pub fn append<T>(&mut self, node: T) -> Result<(), scroll::Error>
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        let header = node.header();
        if EndEntire::is_type(header.r#type, header.sub_type) {
            return Ok(());
        }
        let end_start = self.buffer.len() - EndEntire::BYTES.len();
        self.buffer.truncate(end_start);
        let result = Self::encode_node(&mut self.buffer, node);
        // On failure `encode_node` restores the buffer, so the end node is restored either way.
        self.buffer.extend_from_slice(&EndEntire::BYTES);
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
            .try_reserve_exact(node_length + EndEntire::BYTES.len())
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
        self.reserve(device_path.size())?;
        // The other device path continues this one, and ends it with its EndEntire node.
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
    pub fn append_device_path_instance(&mut self, device_path: &DevicePath) -> Result<(), scroll::Error> {
        self.reserve(device_path.size())?;
        // The EndEntire node of this device path becomes the EndInstance node that separates
        // the two instances: it differs only by its sub-type.
        let sub_type = self.buffer.len() - EndEntire::BYTES.len() + 1;
        *self.buffer.get_mut(sub_type).expect("a device path ends with EndEntire") = node::EndSubType::Instance as u8;
        self.buffer.extend_from_slice(device_path.as_bytes());
        Ok(())
    }

    /// Reserve space for `additional` more bytes, so that appending cannot fail after the
    /// device path is changed.
    fn reserve(&mut self, additional: usize) -> Result<(), scroll::Error> {
        self.buffer
            .try_reserve_exact(additional)
            .map_err(|_| scroll::Error::BadInput { size: additional, msg: "cannot allocate appended device path" })
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
        // The short-form device path after its first node.
        let rest = DevicePath::try_from_bytes(self.buffer.get(first_node.as_bytes().len()..).unwrap_or_default())
            .map_err(|_| EfiError::InvalidParameter)?;
        // A device path that continues `prefix` with `tail`.
        let extend = |prefix: &DevicePath, tail: &DevicePath| -> Result<DevicePathBuf, EfiError> {
            let mut candidate = DevicePathBuf::from(prefix);
            candidate.append_device_path(tail).map_err(|_| EfiError::OutOfResources)?;
            Ok(candidate)
        };

        let mut candidates = Vec::new();
        if let Ok(target) = HardDrive::try_from(first_node) {
            for handle in shorthand::handles(boot_services, &efi::protocols::device_path::PROTOCOL_GUID)? {
                let Some(handle_path) = shorthand::device_path(boot_services, handle) else {
                    continue;
                };
                // The device path of the handle, up to its partition node, then the rest of the short form.
                let mut candidate = DevicePathBuf::new();
                for node in handle_path.iter().take_while(|node| node.header.r#type != DevicePathType::End as u8) {
                    candidate.append(node).map_err(|_| EfiError::OutOfResources)?;
                    if HardDrive::try_from(node).is_ok_and(|hard_drive| shorthand::same_partition(&hard_drive, &target))
                    {
                        candidate.append_device_path(rest).map_err(|_| EfiError::OutOfResources)?;
                        candidates.push(candidate);
                        break;
                    }
                }
            }
        } else if let Ok(wwid) = UsbWwid::try_from(first_node) {
            for (handle_path, usb_io) in shorthand::usb_devices(boot_services)? {
                if shorthand::usb_wwid_matches(boot_services, usb_io, &wwid) {
                    candidates.push(extend(handle_path, rest)?);
                }
            }
        } else if let Ok(class) = UsbClass::try_from(first_node) {
            for (handle_path, usb_io) in shorthand::usb_devices(boot_services)? {
                if shorthand::usb_class_matches(usb_io, &class) {
                    candidates.push(extend(handle_path, rest)?);
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
                candidates.push(extend(handle_path, self)?);
            }
        } else if Uri::try_from(first_node).is_ok() {
            for handle in shorthand::handles(boot_services, &efi::protocols::load_file::PROTOCOL_GUID)? {
                if let Some(handle_path) = shorthand::device_path(boot_services, handle) {
                    candidates.push(extend(handle_path, self)?);
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
            node::fmt_generic_node(generic, f)?;
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
    use crate::devpath;
    use alloc::format;
    use core::assert_eq;

    use crate::uefi::device_path::node::{Acpi, AcpiSubType, EndSubType, Pci};

    #[test]
    fn test_new_device_path_buf_is_terminated() {
        let device_path = DevicePathBuf::new();
        assert_eq!(device_path.buffer, EndEntire::BYTES);
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
        // The device path already ends with EndEntire, so appending it changes nothing.
        device_path.append(EndEntire).unwrap();
        assert_eq!(device_path, before);
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
        // The iSCSI boot device of UEFI 2.10 Section 10.3.4.20.2, with the LUN written as the
        // 8-byte hex dump that Table 10.67 requires.
        let bytes = devpath!(
            "PciRoot(0)/Pci(0x19,0)/MAC(001320F5FA77,0x1)/IPv4(192.168.0.100,TCP,Static,192.168.0.1)/iSCSI(iqn.1991-05.com.microsoft:iscsitarget-iscsidisk-target,0x1,0x0000000000000000,None,None,None,TCP)/HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)"
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
