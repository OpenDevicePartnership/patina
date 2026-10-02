//! Device Path Definitions
//!
//! Defines UEFI device path node structures and utilities for walking and parsing paths and nodes.
//! Uses the UEFI specification as a reference for struct definitions and parsing logic:
//! <https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html>
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

use alloc::{borrow::ToOwned, boxed::Box, vec::Vec};
use core::{
    borrow::Borrow,
    clone::Clone,
    convert::{AsRef, From},
    debug_assert,
    fmt::{Debug, Display, Write},
    format_args,
    iter::Iterator,
    mem,
    ops::{Deref, DerefMut},
};

use scroll::{Pread, ctx::TryIntoCtx};

use crate::{
    base::error::EfiError,
    standard::efi,
    uefi::{
        boot_services::{BootServices, protocol_handler::HandleSearchType},
        device_path::node::{
            DevicePathNode, DevicePathType, EndEntire, EndInstance, GenericDevicePathNode, HardDrive, Header, Iter,
            MediaSubType,
        },
    },
};

/// `DevicePathBuf` is an owned version of device path. This is used to create device paths or when performing mutable operations on them.
#[derive(Debug, Clone)]
pub struct DevicePathBuf {
    buffer: Vec<u8>,
}

impl DevicePathBuf {
    /// Create a `DevicePathBuf` with an empty buffer.
    fn new_empty() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Append a node to the device path.
    /// This function does not ensure that the device path is valid, the `EndEntire` node must be manually added to the device path.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid nodes, allocation failure, or serialization
    /// failure. The existing bytes remain unchanged on failure.
    pub fn append<T>(&mut self, node: T) -> Result<(), scroll::Error>
    where
        T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
    {
        let writing_start_offset = self.buffer.len();
        let expected_length = Header::size_of_header()
            .checked_add(node.measure_size())
            .ok_or(scroll::Error::BadInput { size: usize::MAX, msg: "device path node length overflow" })?;
        if expected_length > usize::from(u16::MAX) {
            return Err(scroll::Error::BadInput { size: expected_length, msg: "device path node is too large" });
        }
        let header = node.header();
        let header_length = usize::from(header.length);
        if header_length != expected_length || header_length < Header::size_of_header() {
            return Err(scroll::Error::BadInput { size: header_length, msg: "invalid device path node length" });
        }
        let new_length = writing_start_offset
            .checked_add(header_length)
            .ok_or(scroll::Error::BadInput { size: header_length, msg: "device path length overflow" })?;
        self.buffer
            .try_reserve_exact(header_length)
            .map_err(|_| scroll::Error::BadInput { size: header_length, msg: "cannot allocate device path node" })?;

        // No allocation will be done here, handled by the `try_reserve`.
        self.buffer.resize(new_length, 0);

        let writing_buffer =
            self.buffer.as_mut_slice().get_mut(writing_start_offset..).expect("buffer was just resized to fit");
        let result = match node.write_into(writing_buffer) {
            Ok(written) if written == header_length => Ok(()),
            Ok(written) => {
                Err(scroll::Error::BadInput { size: written, msg: "encoded node length differs from header" })
            }
            Err(error) => Err(error),
        };
        if result.is_err() {
            self.buffer.truncate(writing_start_offset);
        }
        result
    }

    /// Append a device path to this device path, the `EndEntire` node of self will be removed when appending the other path.
    ///
    /// # Errors
    ///
    /// Returns an error for unterminated paths or allocation failure.
    pub fn append_device_path(&mut self, device_path: &DevicePath) -> Result<(), scroll::Error> {
        self.append_path(device_path, false)
    }

    /// Append a device path to this device path, the `EndEntire` node of self will be replaced with an end instance.
    ///
    /// # Errors
    ///
    /// Returns an error for unterminated paths or allocation failure.
    pub fn append_device_path_instances(&mut self, device_path: &DevicePath) -> Result<(), scroll::Error> {
        self.append_path(device_path, true)
    }

    fn append_path(&mut self, other: &DevicePath, instance: bool) -> Result<(), scroll::Error> {
        fn check(path: &[u8]) -> Result<(), scroll::Error> {
            let mut offset = 0;
            while offset < path.len() {
                let node = path.gread_with::<&GenericDevicePathNode>(&mut offset, scroll::LE)?;
                if EndEntire::is_type(node.header.r#type, node.header.sub_type) {
                    if offset == path.len() {
                        return Ok(());
                    }
                    break;
                }
            }
            Err(scroll::Error::BadInput { size: path.len(), msg: "path is not terminated by EndEntire" })
        }
        check(&self.buffer)?;
        check(other.as_bytes())?;
        self.buffer
            .try_reserve_exact(other.size())
            .map_err(|_| scroll::Error::BadInput { size: other.size(), msg: "cannot allocate appended device path" })?;
        if instance {
            let subtype_offset = self.buffer.len() - 3;
            *self.buffer.get_mut(subtype_offset).expect("validated path has an end node") =
                node::EndSubType::Instance as u8;
        } else {
            self.buffer.truncate(self.buffer.len() - 4);
        }
        self.buffer.extend_from_slice(other.as_bytes());
        Ok(())
    }

    /// Create a device path from an iterator of device path nodes.
    /// If the iterator does not end with an `EndEntire` node, it will be added to the device path.
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
        let mut device_path = DevicePathBuf::new_empty();
        let mut last_node_header = None;
        for n in iter {
            if last_node_header
                .is_some_and(|h: crate::uefi::device_path::node::Header| EndEntire::is_type(h.r#type, h.sub_type))
            {
                return Err(scroll::Error::BadInput { size: 0, msg: "node follows EndEntire" });
            }
            last_node_header.replace(n.header());
            device_path.append(n)?;
        }
        match last_node_header {
            Some(h) if EndEntire::is_type(h.r#type, h.sub_type) => (),
            _ => device_path.append(EndEntire)?,
        }
        Ok(device_path)
    }

    /// Convert the device path to a boxed [`DevicePath`].
    pub fn into_box_device_path(self) -> Box<DevicePath> {
        // SAFETY: DevicePath has the same memory layout as [u8].
        unsafe { mem::transmute(self.buffer.into_boxed_slice()) }
    }
}

impl Deref for DevicePathBuf {
    type Target = DevicePath;

    fn deref(&self) -> &Self::Target {
        DevicePath::from(self)
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

impl From<&DevicePath> for DevicePathBuf {
    fn from(value: &DevicePath) -> Self {
        DevicePathBuf { buffer: Vec::from(&value.buffer) }
    }
}

impl Display for DevicePathBuf {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("{}", self.as_ref()))
    }
}

impl PartialEq for DevicePathBuf {
    fn eq(&self, other: &DevicePathBuf) -> bool {
        DevicePath::from(self) == DevicePath::from(other)
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
    /// Create a &`DevicePath` for a `DevicePathBuf`.
    pub fn from(device_path_buff: &DevicePathBuf) -> &Self {
        // SAFETY: This is safe because DevicePath have the same memory layout as `[u8]`.
        unsafe { &*(core::ptr::from_ref::<[u8]>(device_path_buff.buffer.as_slice()) as *const Self) }
    }

    /// Create a &`DevicePath` from a pointer to a byte buffer.
    /// This is used to interface with device paths from C code.
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

        // SAFETY: The caller contract requires that the buffer pointer is valid and points to at least
        // Header::size_of_header() bytes.
        let mut buffer =
            unsafe { core::slice::from_raw_parts(buffer, crate::uefi::device_path::node::Header::size_of_header()) };
        let mut offset = 0;
        loop {
            let header = buffer
                .pread_with::<crate::uefi::device_path::node::Header>(offset, scroll::LE)
                .map_err(|_| "Error while trying to read header.")?;
            if EndEntire::is_type(header.r#type, header.sub_type) {
                break;
            }

            let header_length = usize::from(header.length);
            let new_length = buffer.len() + header_length;
            offset += header_length;
            // SAFETY: The buffer slice is extended based on the length field from the device path header.
            // The caller guarantees that the buffer points to valid device path data for the entire lifetime.
            buffer = unsafe { core::slice::from_raw_parts(buffer.as_ptr(), new_length) };
        }

        // SAFETY: The device path structure was validated by iterating through all nodes and finding an end node.
        // The buffer is properly sized and contains a valid device path. The cast is considered safe due to
        // DevicePath's repr(transparent).
        let device_path = unsafe { &*(core::ptr::from_ref::<[u8]>(buffer) as *const DevicePath) };
        Ok(device_path)
    }

    /// Return the size in bytes of the device path.
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
    pub fn is_multi_instance(&self) -> bool {
        self.iter().any(|n| EndInstance::is_type(n.header.r#type, n.header.sub_type))
    }

    /// Return a &`DevicePath` for the n last nodes of the device path.
    /// This operation does not copy memory since the trailing end of a device path is a valid device path.
    pub fn slice_end(&self, n: usize) -> &DevicePath {
        let count = self.node_count();

        debug_assert!(n >= 1, "Device path needs to have at least the end node.");
        debug_assert!(n <= count, "Cannot return a device path bigger than self.");

        let nb_skip = count - n;
        let mut idx = 0;
        for _ in 0..nb_skip {
            let header = self.buffer.pread_with::<crate::uefi::device_path::node::Header>(idx, scroll::LE).unwrap();
            idx += usize::from(header.length);
        }
        let end_buffer = self.buffer.get(idx..).expect("idx is within device path bounds");
        // SAFETY: The slice `end_buffer` is a valid trailing portion of the device path that contains
        // the last n nodes including the end node. DevicePath is repr(transparent) over [u8], so this cast is
        // considered valid.
        unsafe { &*(core::ptr::from_ref::<[u8]>(end_buffer) as *const DevicePath) }
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

    /// Iterate through every node of a device path.
    pub fn iter(&self) -> Iter<'_> {
        Iter::new(self)
    }

    /// Iterate through every instance of a device path.
    pub fn iter_instances(&self) -> IterInstance<'_> {
        IterInstance { node_iterator: self.iter() }
    }

    /// Return true if this is a shorthand device path, which the specification calls a
    /// short-form device path (UEFI 2.10 Section 3.1.2).
    ///
    /// A full device path starts at the system root, with a hardware or ACPI node. A
    /// shorthand device path starts with another node, such as a hard drive or file path
    /// media node, and must be expanded with [`expand_shorthand`](Self::expand_shorthand)
    /// before the image it points to can be loaded. Paths that start with a PIWG firmware volume or
    /// firmware file node are complete, because `LoadImage` resolves them directly. An
    /// empty path is not a shorthand device path.
    pub fn is_shorthand_notation(&self) -> bool {
        let Some(first_node) = self.iter().next() else {
            return false;
        };
        let (node_type, node_sub_type) = (first_node.header.r#type, first_node.header.sub_type);
        if node_type == DevicePathType::Media as u8
            && (node_sub_type == MediaSubType::PiwgFirmwareFile as u8
                || node_sub_type == MediaSubType::PiwgFirmwareVolume as u8)
        {
            return false;
        }
        node_type != DevicePathType::Hardware as u8
            && node_type != DevicePathType::Acpi as u8
            && node_type != DevicePathType::End as u8
    }

    /// Expand a shorthand device path into the full device path of the matching device
    /// (UEFI 2.10 Section 3.1.2).
    ///
    /// A full device path is returned unchanged. A shorthand device path must contain a
    /// [`HardDrive`] node. Every handle with a device path is searched for a hard drive
    /// node with the same partition number, partition format, signature type, and
    /// signature. The result is the matching handle's device path up to and including
    /// that node, followed by the nodes after the hard drive node in the shorthand path.
    ///
    /// The specification lets the boot manager pick any device when several match.
    /// This function instead rejects an ambiguous match, so the device it returns does
    /// not depend on handle enumeration order.
    ///
    /// # Errors
    ///
    /// - [`EfiError::InvalidParameter`] if the path is empty, or is a shorthand path with
    ///   no hard drive node.
    /// - [`EfiError::NotFound`] if no device matches the hard drive node.
    /// - [`EfiError::SecurityViolation`] if several devices match the hard drive node.
    /// - [`EfiError::DeviceError`] if the expanded device path cannot be built.
    /// - Any error returned by `LocateHandleBuffer`.
    pub fn expand_shorthand<B: BootServices>(&self, boot_services: &B) -> Result<DevicePathBuf, EfiError> {
        if self.iter().next().is_none_or(|node| node.header.r#type == DevicePathType::End as u8) {
            log::error!("Cannot expand an empty device path");
            return Err(EfiError::InvalidParameter);
        }
        if !self.is_shorthand_notation() {
            return Ok(self.into());
        }

        let mut shorthand_nodes = self.iter().filter(|node| node.header.r#type != DevicePathType::End as u8);
        let Some(target) = shorthand_nodes.by_ref().find_map(HardDrive::try_from_node) else {
            log::error!("Shorthand device path has no hard drive node to expand");
            return Err(EfiError::InvalidParameter);
        };
        let remaining_path = DevicePathBuf::from_device_path_node_iter(shorthand_nodes).map_err(|error| {
            log::error!("Cannot build the rest of the shorthand device path: {error}");
            EfiError::DeviceError
        })?;

        let handles = boot_services
            .locate_handle_buffer(HandleSearchType::ByProtocol(&efi::protocols::device_path::PROTOCOL_GUID))
            .map_err(EfiError::from)?;

        let mut matched_path: Option<DevicePathBuf> = None;
        for &handle in handles.iter() {
            // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is the device path protocol.
            let Ok(device_path) =
                (unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) })
            else {
                continue;
            };
            // SAFETY: The pointer comes from a device path protocol interface installed on the handle.
            let Ok(handle_path) = (unsafe { DevicePath::try_from_ptr(core::ptr::from_ref(device_path) as *const u8) })
            else {
                continue;
            };

            let mut prefix_nodes = Vec::new();
            let mut found = false;
            for node in handle_path.iter().take_while(|node| node.header.r#type != DevicePathType::End as u8) {
                prefix_nodes.push(node);
                if HardDrive::try_from_node(node).is_some_and(|hard_drive| {
                    hard_drive.partition_number == target.partition_number
                        && hard_drive.partition_format == target.partition_format
                        && hard_drive.signature_type == target.signature_type
                        && hard_drive.partition_signature == target.partition_signature
                }) {
                    found = true;
                    break;
                }
            }
            if !found {
                continue;
            }

            let expanded = DevicePathBuf::from_device_path_node_iter(prefix_nodes.into_iter())
                .and_then(|mut path| path.append_device_path(&remaining_path).map(|()| path))
                .map_err(|error| {
                    log::error!("Cannot build the expanded device path: {error}");
                    EfiError::DeviceError
                })?;
            if matched_path.replace(expanded).is_some() {
                log::error!("Shorthand device path matches several devices");
                return Err(EfiError::SecurityViolation);
            }
        }

        matched_path.ok_or_else(|| {
            log::error!("No device matches the shorthand device path");
            EfiError::NotFound
        })
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
        DevicePathBuf::from(self.as_ref()).into_box_device_path()
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
            if node.header().r#type == DevicePathType::End as u8 {
                if let Err(error) = item.get_or_insert(DevicePathBuf::new_empty()).append(EndEntire) {
                    return Some(Err(error));
                }
                break;
            }
            if let Err(error) = item.get_or_insert(DevicePathBuf::new_empty()).append(node) {
                return Some(Err(error));
            }
        }
        item.map(Ok)
    }
}

impl Display for DevicePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut nodes = self.iter().peekable();

        while let Some(node) = nodes.next() {
            let is_end = node.header.r#type == DevicePathType::End as u8;
            f.write_fmt(format_args!("{}", node::cast_to_dyn_device_path_node(node)))?;

            if let Some(next) = nodes.peek()
                && !is_end
                && next.header.r#type != DevicePathType::End as u8
            {
                f.write_char('/')?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::assert_eq;

    use crate::uefi::device_path::node::{Acpi, AcpiSubType, EndSubType, Pci};

    #[test]
    fn test_new_empty_device_path_buf() {
        let device_path = DevicePathBuf::new_empty();
        assert!(device_path.buffer.is_empty());
    }

    #[test]
    fn test_device_path_as_bytes() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(EndEntire).unwrap();

        let device_path: &DevicePath = device_path_buf.as_ref();
        assert_eq!(device_path.as_bytes(), device_path_buf.buffer.as_slice());
        assert_eq!(device_path.as_bytes().len(), device_path.size());
    }

    #[test]
    fn test_append_node() {
        let mut device_path = DevicePathBuf::new_empty();
        device_path.append(Acpi::new_pci_root(0)).unwrap();
        device_path.append(EndEntire).unwrap();

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
        let mut device_path = DevicePathBuf::new_empty();
        device_path.append(Acpi::new_pci_root(0)).unwrap();
        device_path.append(EndEntire).unwrap();

        let mut device_path_to_add = DevicePathBuf::new_empty();
        device_path_to_add.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_to_add.append(EndEntire).unwrap();

        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();
        expected_device_path.append(EndEntire).unwrap();

        device_path.append_device_path(&device_path_to_add).unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_append_device_path_instance() {
        let mut device_path = DevicePathBuf::new_empty();
        device_path.append(Acpi::new_pci_root(0)).unwrap();
        device_path.append(EndEntire).unwrap();

        let mut device_path_to_add = DevicePathBuf::new_empty();
        device_path_to_add.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_to_add.append(EndEntire).unwrap();

        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(EndInstance).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();
        expected_device_path.append(EndEntire).unwrap();

        device_path.append_device_path_instances(&device_path_to_add).unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buff_from_node_iter() {
        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Acpi::new_pci_root(0)).unwrap();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();
        expected_device_path.append(EndEntire).unwrap();

        let device_path = DevicePathBuf::from_device_path_node_iter(expected_device_path.iter()).unwrap();

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buff_to_boxed_device_path() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_buf.append(EndEntire).unwrap();

        let device_path = DevicePathBuf::clone(&device_path_buf).into_box_device_path();

        assert_eq!(device_path_buf.as_ref(), device_path.as_ref());
    }

    #[test]
    fn test_device_path_from_ptr() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_buf.append(EndEntire).unwrap();
        let buffer_ptr = device_path_buf.buffer.as_slice().as_ptr();

        // SAFETY: Test code - creating DevicePath from a valid buffer pointer for testing.
        let device_path = unsafe { DevicePath::try_from_ptr(buffer_ptr) }.unwrap();

        assert_eq!(device_path_buf.as_ref(), device_path);
    }

    #[test]
    fn test_device_path_size() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        assert_eq!(0, device_path_buf.size());
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        assert_eq!(12, device_path_buf.size());
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(18, device_path_buf.size());
        device_path_buf.append(EndEntire).unwrap();
        assert_eq!(22, device_path_buf.size());
    }

    #[test]
    fn test_device_path_node_count() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        assert_eq!(0, device_path_buf.node_count());
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        assert_eq!(1, device_path_buf.node_count());
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        assert_eq!(2, device_path_buf.node_count());
        device_path_buf.append(EndEntire).unwrap();
        assert_eq!(3, device_path_buf.node_count());
    }

    #[test]
    fn test_device_path_is_multi_instances() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_buf.append(EndEntire).unwrap();

        let mut device_path_buf_2 = DevicePathBuf::new_empty();
        device_path_buf_2.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf_2.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_buf_2.append(EndEntire).unwrap();

        assert!(!device_path_buf.is_multi_instance());

        device_path_buf.append_device_path_instances(&device_path_buf_2).unwrap();

        assert!(device_path_buf.is_multi_instance());
    }

    #[test]
    fn test_device_path_slice_end() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_buf.append(EndEntire).unwrap();

        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Pci { function: 1, device: 2 }).unwrap();
        expected_device_path.append(EndEntire).unwrap();

        assert_eq!(expected_device_path.as_ref(), device_path_buf.slice_end(2));
    }

    #[test]
    fn test_device_path_start_with() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0)).unwrap();
        device_path_buf.append(Pci { function: 1, device: 2 }).unwrap();
        device_path_buf.append(EndEntire).unwrap();

        let mut start = DevicePathBuf::new_empty();
        start.append(Acpi::new_pci_root(0)).unwrap();
        start.append(EndEntire).unwrap();

        assert!(device_path_buf.starts_with(&start));

        let mut start = DevicePathBuf::new_empty();
        start.append(Acpi::new_pci_root(1)).unwrap();
        start.append(EndEntire).unwrap();

        assert!(!device_path_buf.starts_with(&start));
    }

    mod shorthand {
        extern crate std;

        use super::*;
        use crate::uefi::{
            boot_services::{MockBootServices, boxed::BootServicesBox},
            device_path::node::{FilePath, HardDrive, PiwgFirmwareVolume},
        };

        fn path<T>(nodes: impl IntoIterator<Item = T>) -> DevicePathBuf
        where
            T: DevicePathNode + TryIntoCtx<scroll::Endian, Error = scroll::Error>,
        {
            DevicePathBuf::from_device_path_node_iter(nodes.into_iter()).unwrap()
        }

        fn hard_drive(guid: [u8; 16]) -> HardDrive {
            HardDrive::new_gpt(1, 2048, 1_000_000, guid)
        }

        /// `HD(1,GPT,<guid>)/File(\EFI\BOOT\BOOTX64.EFI)`.
        fn shorthand_path(guid: [u8; 16]) -> DevicePathBuf {
            let mut shorthand = path([hard_drive(guid)]);
            shorthand.append_device_path(&path([FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI")])).unwrap();
            shorthand
        }

        /// `PciRoot(0)/Pci(0x1D,0)/HD(1,GPT,<guid>)`.
        fn disk_path(guid: [u8; 16]) -> DevicePathBuf {
            let mut disk = path([Acpi::new_pci_root(0)]);
            disk.append_device_path(&path([Pci { function: 0, device: 0x1D }])).unwrap();
            disk.append_device_path(&path([hard_drive(guid)])).unwrap();
            disk
        }

        /// A mock whose `LocateHandleBuffer` returns one handle per device path, and whose
        /// `HandleProtocol` returns the device path of that handle. `None` entries model
        /// handles without a device path.
        fn mock_devices(paths: &[Option<&DevicePathBuf>]) -> MockBootServices {
            let addresses: Vec<Option<usize>> = paths
                .iter()
                .map(|path| {
                    path.map(|path| {
                        std::boxed::Box::leak(path.as_bytes().to_vec().into_boxed_slice()).as_ptr() as usize
                    })
                })
                .collect();
            let count = addresses.len();
            let free_pool: &'static MockBootServices = std::boxed::Box::leak(std::boxed::Box::new({
                let mut mock = MockBootServices::new();
                mock.expect_free_pool().returning(|_| Ok(()));
                mock
            }));

            let mut mock = MockBootServices::new();
            mock.expect_locate_handle_buffer().returning(move |_| {
                let handles: Vec<efi::Handle> = (1..=count).map(|index| index as efi::Handle).collect();
                let handles = handles.leak();
                // SAFETY: Test code - the leaked handle buffer outlives the returned box, and free_pool is a no-op.
                Ok(unsafe { BootServicesBox::from_raw_parts_mut(handles.as_mut_ptr(), handles.len(), free_pool) })
            });
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>().returning(move |handle| {
                match addresses.get(handle as usize - 1).copied().flatten() {
                    // SAFETY: Test code - the address points to a leaked, valid device path.
                    Some(address) => Ok(unsafe { &mut *(address as *mut efi::protocols::device_path::Protocol) }),
                    None => Err(efi::Status::UNSUPPORTED),
                }
            });
            mock
        }

        #[test]
        fn test_device_path_is_shorthand_notation() {
            assert!(shorthand_path([0xaa; 16]).is_shorthand_notation());
            assert!(path([FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI")]).is_shorthand_notation());
            assert!(!disk_path([0xaa; 16]).is_shorthand_notation());
            assert!(!path([Pci { function: 0, device: 0x1D }]).is_shorthand_notation());
            assert!(!path([PiwgFirmwareVolume { fv_name: [0x11; 16] }]).is_shorthand_notation());
            assert!(!path([EndEntire]).is_shorthand_notation());
        }

        #[test]
        fn test_device_path_expand_shorthand_full_path_is_unchanged() {
            let full = disk_path([0xaa; 16]);
            assert_eq!(full.expand_shorthand(&MockBootServices::new()).unwrap(), full);
        }

        #[test]
        fn test_device_path_expand_shorthand_rejects_empty_and_missing_hard_drive() {
            let mock = MockBootServices::new();
            assert_eq!(path([EndEntire]).expand_shorthand(&mock), Err(EfiError::InvalidParameter));
            let file_only = path([FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI")]);
            assert_eq!(file_only.expand_shorthand(&mock), Err(EfiError::InvalidParameter));
        }

        #[test]
        fn test_device_path_expand_shorthand_matches_hard_drive() {
            let guid = [0xaa; 16];
            let other_disk = disk_path([0xbb; 16]);
            let mut file_system = disk_path(guid);
            file_system.append_device_path(&path([FilePath::new("\\ignored")])).unwrap();
            let mock = mock_devices(&[None, Some(&other_disk), Some(&file_system)]);

            let mut expected = disk_path(guid);
            expected.append_device_path(&path([FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI")])).unwrap();
            assert_eq!(shorthand_path(guid).expand_shorthand(&mock).unwrap(), expected);
        }

        #[test]
        fn test_device_path_expand_shorthand_requires_full_identity() {
            let guid = [0xaa; 16];
            let mut other_partition = path([Acpi::new_pci_root(0)]);
            other_partition.append_device_path(&path([HardDrive::new_gpt(2, 2048, 1_000_000, guid)])).unwrap();
            let mock = mock_devices(&[Some(&other_partition)]);
            assert_eq!(shorthand_path(guid).expand_shorthand(&mock), Err(EfiError::NotFound));
        }

        #[test]
        fn test_device_path_expand_shorthand_rejects_ambiguous_match() {
            let guid = [0xaa; 16];
            let first = disk_path(guid);
            let mut second = path([Acpi::new_pci_root(1)]);
            second.append_device_path(&path([hard_drive(guid)])).unwrap();
            let mock = mock_devices(&[Some(&first), Some(&second)]);
            assert_eq!(shorthand_path(guid).expand_shorthand(&mock), Err(EfiError::SecurityViolation));
        }

        #[test]
        fn test_device_path_expand_shorthand_reports_locate_errors() {
            let mut mock = MockBootServices::new();
            mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));
            assert_eq!(shorthand_path([0xaa; 16]).expand_shorthand(&mock), Err(EfiError::NotFound));
        }
    }
}
