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

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
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
        boot_services::{BootServices, protocol_handler::HandleSearchType},
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
    pub fn append<T>(&mut self, node: T) -> Result<(), String>
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        node.validate()?;

        let header = node.header();

        // Addind an end entire do nothing. End Entire is managed automatically.
        if EndEntire::is_type(header.r#type, header.sub_type) {
            return Ok(());
        }

        let node_payload_size = node.measure_size();
        let node_size = Header::size_of_header()
            .checked_add(node_payload_size)
            .and_then(|s| u16::try_from(s).ok())
            .ok_or(format!("device path node is too large. Node size = {node_payload_size}"))?
            as usize;

        if header.length() as usize != node_size {
            return Err(format!(
                "device path node header length differs from its size. Header length = {}, Node size = {node_payload_size}",
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
    pub fn append_device_path(&mut self, device_path: &DevicePath) -> Result<(), scroll::Error> {
        self.reserve(device_path.size() - EndEntire::BYTES.len())?;
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
            let (r#type, sub_type) = (node.header.r#type, node.header.sub_type);
            HardDrive::is_type(r#type, sub_type)
                || FilePath::is_type(r#type, sub_type)
                || Uri::is_type(r#type, sub_type)
                || UsbWwid::is_type(r#type, sub_type)
                || UsbClass::is_type(r#type, sub_type)
        })
    }

    /// Expand a short-form device path into the full device paths it can refer to, in the
    /// order a boot manager tries them.
    ///
    /// [UEFI Spec Documentation: 3.1.2. Load Option Processing](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html#load-option-processing)
    ///
    /// A device path that is not short-form is returned unchanged, as the only candidate. For a
    /// short-form device path, the first node selects how the devices are searched:
    ///
    /// | First node | Matching devices | Candidate |
    /// |---|---|---|
    /// | [`HardDrive`] | Device paths with an equal hard drive node | The device path up to the hard drive node, then the nodes after the first one |
    /// | [`UsbWwid`] | USB I/O handles with the same vendor ID, product ID, interface number, and serial number suffix | The handle's device path, then the nodes after the first one |
    /// | [`UsbClass`] | USB I/O handles with the same vendor ID, product ID, class, subclass, and protocol; a field of all ones matches any value | The handle's device path, then the nodes after the first one |
    /// | [`FilePath`] | Simple file system handles: removable media, then fixed media, then file systems without block I/O | The handle's device path, then the whole short-form path |
    /// | [`Uri`] | Load file handles | The handle's device path, then the whole short-form path, so that loading it passes the short-form path to the handle's `LoadFile` |
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
    pub fn expand_short_form<B: BootServices>(&self, boot_services: &B) -> Result<Vec<DevicePathBuf>, EfiError> {
        let Some(first_node) = self.iter().next().filter(|node| node.header.r#type != DevicePathType::End as u8) else {
            log::error!("Cannot expand an empty device path");
            return Err(EfiError::InvalidParameter);
        };
        if !self.is_short_form() {
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

        // The handles that support `protocol`; none if no handle supports it.
        let handles = |protocol: &'static efi::Guid| -> Result<Vec<efi::Handle>, EfiError> {
            match boot_services.locate_handle_buffer(HandleSearchType::ByProtocol(protocol)) {
                Ok(handles) => Ok(handles.iter().copied().collect()),
                Err(efi::Status::NOT_FOUND) => Ok(Vec::new()),
                Err(status) => Err(EfiError::from(status)),
            }
        };
        // The device path installed on `handle`, if it has a valid one.
        let device_path = |handle: efi::Handle| -> Option<&'static DevicePath> {
            // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is the device path protocol.
            let protocol =
                unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) }.ok()?;
            // SAFETY: The pointer comes from a device path protocol interface installed on the handle.
            unsafe { DevicePath::try_from_ptr(core::ptr::from_ref(protocol).cast::<u8>()) }.ok()
        };

        let mut candidates = Vec::new();
        if let Ok(target) = HardDrive::try_from(first_node) {
            for handle in handles(&efi::protocols::device_path::PROTOCOL_GUID)? {
                let Some(handle_path) = device_path(handle) else {
                    continue;
                };
                // The device path of the handle, up to its partition node, then the rest of the short form.
                let mut candidate = DevicePathBuf::new();
                for node in handle_path.iter().take_while(|node| node.header.r#type != DevicePathType::End as u8) {
                    candidate.append(node).map_err(|_| EfiError::OutOfResources)?;
                    if HardDrive::try_from(node).is_ok_and(|hard_drive| hard_drive == target) {
                        candidate.append_device_path(rest).map_err(|_| EfiError::OutOfResources)?;
                        candidates.push(candidate);
                        break;
                    }
                }
            }
        } else if UsbWwid::is_type(first_node.header.r#type, first_node.header.sub_type)
            || UsbClass::is_type(first_node.header.r#type, first_node.header.sub_type)
        {
            let (wwid, class) = (UsbWwid::try_from(first_node).ok(), UsbClass::try_from(first_node).ok());
            for handle in handles(&usb_io::PROTOCOL_GUID)? {
                let Some(handle_path) = device_path(handle) else {
                    continue;
                };
                // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is USB I/O. Each
                // handle is visited once, so the interface is borrowed once.
                let Ok(usb_io) = (unsafe { boot_services.handle_protocol::<usb_io::Protocol>(handle) }) else {
                    continue;
                };
                if wwid.as_ref().is_some_and(|wwid| wwid.matches(boot_services, usb_io))
                    || class.as_ref().is_some_and(|class| class.matches(usb_io))
                {
                    candidates.push(extend(handle_path, rest)?);
                }
            }
        } else if FilePath::try_from(first_node).is_ok() {
            let file_systems = handles(&efi::protocols::simple_file_system::PROTOCOL_GUID)?;
            let (mut removable, mut fixed, mut without_block_io) = (Vec::new(), Vec::new(), Vec::new());
            for handle in file_systems {
                let Some(handle_path) = device_path(handle) else {
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
            for handle in handles(&efi::protocols::load_file::PROTOCOL_GUID)? {
                if let Some(handle_path) = device_path(handle) {
                    candidates.push(extend(handle_path, self)?);
                }
            }
        } else {
            log::error!("Short-form device path starts with an invalid node");
            return Err(EfiError::InvalidParameter);
        }

        if candidates.is_empty() {
            log::error!("No device matches the short-form device path");
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
    type Item = Result<DevicePathBuf, String>;

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
    extern crate std;

    use super::*;
    use crate::{
        devpath,
        uefi::{
            boot_services::{MockBootServices, boxed::BootServicesBox},
            device_path::node::{Acpi, AcpiSubType, EndSubType, Pci, PiwgFirmwareVolume, Usb},
        },
    };
    use alloc::format;
    use core::assert_eq;

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

    fn path<T>(nodes: impl IntoIterator<Item = T>) -> DevicePathBuf
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        nodes.into_iter().try_fold(DevicePathBuf::new(), |mut path, node| path.append(node).map(|()| path)).unwrap()
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
    fn short_form_path(guid: [u8; 16]) -> DevicePathBuf {
        join(&[&path([hard_drive(guid)]), &boot_file()])
    }

    /// `PciRoot(0)/Pci(0x1D,0)/HD(1,GPT,<guid>)`.
    fn disk_path(guid: [u8; 16]) -> DevicePathBuf {
        join(&[&path([Acpi::new_pci_root(0)]), &path([Pci { function: 0, device: 0x1D }]), &path([hard_drive(guid)])])
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
                device
                    .path
                    .map(|path| std::boxed::Box::leak(path.as_bytes().to_vec().into_boxed_slice()).as_ptr() as usize),
            );
            usb_addresses.push(
                device.usb.map(|usb| core::ptr::from_mut(std::boxed::Box::leak(std::boxed::Box::new(usb))) as usize),
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
    fn test_device_path_is_short_form() {
        assert!(short_form_path([0xaa; 16]).is_short_form());
        assert!(boot_file().is_short_form());
        assert!(path([Uri { uri: "http://a/b.efi".into() }]).is_short_form());
        assert!(
            path([UsbWwid { interface: 0, vendor_id: 1, product_id: 2, serial_number: Vec::from([0x41]) }])
                .is_short_form()
        );
        assert!(
            path([UsbClass {
                vendor_id: u16::MAX,
                product_id: u16::MAX,
                device_class: u8::MAX,
                device_subclass: u8::MAX,
                device_protocol: u8::MAX,
            }])
            .is_short_form()
        );
        assert!(!disk_path([0xaa; 16]).is_short_form());
        assert!(!path([Pci { function: 0, device: 0x1D }]).is_short_form());
        assert!(!path([PiwgFirmwareVolume { fv_name: crate::BinaryGuid::from_bytes(&[0x11; 16]) }]).is_short_form());
        assert!(!path([EndEntire]).is_short_form());
    }

    #[test]
    fn test_device_path_expand_short_form_full_path_is_unchanged() {
        let full = disk_path([0xaa; 16]);
        assert_eq!(full.expand_short_form(&MockBootServices::new()).unwrap(), [full]);
    }

    #[test]
    fn test_device_path_expand_short_form_rejects_empty_path() {
        assert_eq!(path([EndEntire]).expand_short_form(&MockBootServices::new()), Err(EfiError::InvalidParameter));
    }

    #[test]
    fn test_device_path_expand_short_form_matches_hard_drive() {
        let guid = [0xaa; 16];
        let file_system = join(&[&disk_path(guid), &path([FilePath::new("\\ignored")])]);
        let mock = mock_system(Vec::from([
            Device::default(),
            Device::with_path(disk_path([0xbb; 16])),
            Device::with_path(file_system),
        ]));
        let expected = join(&[&disk_path(guid), &boot_file()]);
        assert_eq!(short_form_path(guid).expand_short_form(&mock).unwrap(), [expected]);
    }

    #[test]
    fn test_device_path_expand_short_form_requires_full_identity() {
        let guid = [0xaa; 16];
        let other_partition =
            join(&[&path([Acpi::new_pci_root(0)]), &path([HardDrive::new_gpt(2, 2048, 1_000_000, guid)])]);
        let mock = mock_system(Vec::from([Device::with_path(other_partition)]));
        assert_eq!(short_form_path(guid).expand_short_form(&mock), Err(EfiError::NotFound));
    }

    #[test]
    fn test_device_path_expand_short_form_returns_every_match() {
        let guid = [0xaa; 16];
        let second = join(&[&path([Acpi::new_pci_root(1)]), &path([hard_drive(guid)])]);
        let mock = mock_system(Vec::from([Device::with_path(disk_path(guid)), Device::with_path(second.clone())]));
        let expected = [join(&[&disk_path(guid), &boot_file()]), join(&[&second, &boot_file()])];
        assert_eq!(short_form_path(guid).expand_short_form(&mock).unwrap(), expected);
    }

    #[test]
    fn test_device_path_expand_short_form_reports_locate_errors() {
        let mut mock = MockBootServices::new();
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));
        assert_eq!(short_form_path([0xaa; 16]).expand_short_form(&mock), Err(EfiError::NotFound));

        let mut mock = MockBootServices::new();
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::DEVICE_ERROR));
        assert_eq!(short_form_path([0xaa; 16]).expand_short_form(&mock), Err(EfiError::DeviceError));
    }

    #[test]
    fn test_device_path_expand_short_form_matches_usb_wwid() {
        let wwid = UsbWwid {
            interface: 0,
            vendor_id: 0x1234,
            product_id: 0x5678,
            serial_number: "123456".encode_utf16().collect(),
        };
        let short_form = join(&[&path([wwid]), &boot_file()]);
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
        assert_eq!(short_form.expand_short_form(&mock_system(devices)).unwrap(), [expected]);
    }

    #[test]
    fn test_device_path_expand_short_form_matches_usb_class() {
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
        assert_eq!(path([mass_storage.clone()]).expand_short_form(&mock_system(devices())).unwrap(), expected);

        let vendor_two = UsbClass { vendor_id: 2, ..mass_storage };
        assert_eq!(path([vendor_two]).expand_short_form(&mock_system(devices())).unwrap(), [usb_path(2)]);
    }

    #[test]
    fn test_device_path_expand_short_form_orders_file_systems() {
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
        assert_eq!(boot_file().expand_short_form(&mock).unwrap(), expected);
    }

    #[test]
    fn test_device_path_expand_short_form_uses_load_file_handles() {
        let uri = path([Uri { uri: "http://192.168.0.1/boot.efi".into() }]);
        let network = path([Acpi::new_pci_root(3)]);
        let mock = mock_system(Vec::from([
            Device { load_file: true, ..Device::with_path(network.clone()) },
            Device::with_path(disk_path([1; 16])),
        ]));
        assert_eq!(uri.expand_short_form(&mock).unwrap(), [join(&[&network, &uri])]);
    }
}
