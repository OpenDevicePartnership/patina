//! A Device Path is used to define the programmatic path to a device.
//!
//! The primary purpose of a Device Path is to allow an application, such as an OS loader, to determine the physical device that the interfaces are abstracting.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

pub mod device_path_node;
pub mod nodes;

use alloc::{borrow::ToOwned, boxed::Box, vec::Vec};
use core::{
    borrow::Borrow,
    clone::Clone,
    convert::{AsRef, From},
    debug_assert, debug_assert_eq,
    fmt::{Debug, Display, Write},
    format_args,
    iter::Iterator,
    mem,
    ops::Deref,
};

use scroll::Pread;

use device_path_node::{DevicePathNode, Header, UnknownDevicePathNode};
use nodes::{DevicePathType, EndEntire, EndInstance};

/// DevicePathBuf is an owned version of device path. This is used to create device paths or when performing mutable operations on them.
#[derive(Debug, Clone)]
pub struct DevicePathBuf {
    buffer: Vec<u8>,
}

impl DevicePathBuf {
    /// Create a DevicePathBuf with an empty buffer.
    fn new_empty() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Append a node to the device path.
    /// This function does not ensure that the device path is valid, the EndEntire node must be manually added to the device path.
    pub fn append<T>(&mut self, node: T)
    where
        T: DevicePathNode + Sized,
    {
        let writing_start_offset = self.buffer.len();
        let header = node.header();

        let nb_additional_byte = (writing_start_offset + header.length).saturating_sub(self.buffer.capacity());

        if let Err(e) = self.buffer.try_reserve_exact(nb_additional_byte) {
            debug_assert!(false, "Device Path: Cannot allocate enough memory to append a device path node. {e:?}");
            return;
        }

        // No allocation will be done here, handled by the `try_reserve`.
        self.buffer.resize(writing_start_offset + header.length, 0);

        let writing_buffer = &mut self.buffer.as_mut_slice()[writing_start_offset..];
        match node.write_into(writing_buffer) {
            Ok(nb_byte_written) => debug_assert_eq!(header.length, nb_byte_written),
            Err(_) => debug_assert!(false, "Unexpected error, buffer should be large enough at that point."),
        }
    }

    /// Append a device path to this device path, the EndEntire node of self will be removed when appending the other path.
    pub fn append_device_path(&mut self, device_path: &DevicePath) {
        self.buffer.truncate(self.buffer.len() - EndEntire.header().length);
        self.buffer.extend_from_slice(&device_path.buffer[..]);
    }

    /// Append a device path to this device path, the EndEntire node of self will be replaced with an end instance.
    pub fn append_device_path_instances(&mut self, device_path: &DevicePath) {
        self.buffer.truncate(self.buffer.len() - EndEntire.header().length);
        self.append(EndInstance);
        self.buffer.extend_from_slice(&device_path.buffer[..]);
    }

    /// Create a device path from an iterator of device path nodes.
    /// If the iterator does not end with an `EndEntire` node, it will be added to the device path.
    pub fn from_device_path_node_iter<I, T>(iter: I) -> DevicePathBuf
    where
        I: Iterator<Item = T>,
        T: DevicePathNode,
    {
        let mut device_path = DevicePathBuf::new_empty();
        let mut last_node_header = None;
        for n in iter {
            last_node_header.replace(n.header());
            device_path.append(n);
        }
        match last_node_header {
            Some(h) if EndEntire::is_type(h.r#type, h.sub_type) => (),
            _ => device_path.append(EndEntire),
        }
        device_path
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

impl AsRef<DevicePath> for DevicePathBuf {
    fn as_ref(&self) -> &DevicePath {
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

/// DevicePath is the borrowed version of a [`DevicePathBuf`].
/// Only immutable operations are possible on this type.
#[derive(Debug)]
#[repr(transparent)]
pub struct DevicePath {
    buffer: [u8],
}

impl DevicePath {
    /// Create a &DevicePath for a DevicePathBuf.
    pub fn from(device_path_buff: &DevicePathBuf) -> &Self {
        // SAFETY: This is safe because DevicePath have the same memory layout as `[u8]`.
        unsafe { &*(device_path_buff.buffer.as_slice() as *const [u8] as *const Self) }
    }

    /// Create a &DevicePath from a pointer to a byte buffer.
    /// This is used to interface with device paths from C code.
    ///
    /// # Safety
    ///
    /// The buffer pointer must point to valid device path data that remains valid
    /// for the lifetime 'a. The device path must be properly terminated with an
    /// EndEntire node.
    pub unsafe fn try_from_ptr<'a>(buffer: *const u8) -> Result<&'a DevicePath, &'static str> {
        if buffer.is_null() {
            return Err("Null pointer provided");
        }

        let mut buffer = unsafe { core::slice::from_raw_parts(buffer, Header::size_of_header()) };
        let mut offset = 0;
        loop {
            let header =
                buffer.pread_with::<Header>(offset, scroll::LE).map_err(|_| "Error while trying to read header.")?;

            if EndEntire::is_type(header.r#type, header.sub_type) {
                break;
            }

            let new_length = buffer.len() + header.length;
            offset += header.length;
            buffer = unsafe { core::slice::from_raw_parts(buffer.as_ptr(), new_length) };
        }

        let device_path = unsafe { &*(buffer as *const [u8] as *const DevicePath) };
        Ok(device_path)
    }

    /// Return the size in bytes of the device path.
    pub fn size(&self) -> usize {
        self.buffer.len()
    }

    /// Return the number of nodes in the device path.
    pub fn node_count(&self) -> usize {
        self.iter().count()
    }

    /// Return true if the device path contains more than one instance, otherwise false.
    pub fn is_multi_instance(&self) -> bool {
        self.iter().any(|n| EndInstance::is_type(n.header.r#type, n.header.sub_type))
    }

    /// Return a &DevicePath for the n last nodes of the device path.
    /// This operation does not copy memory since the trailing end of a device path is a valid device path.
    pub fn slice_end(&self, n: usize) -> &DevicePath {
        let count = self.node_count();

        debug_assert!(n >= 1, "Device path needs to have at least the end node.");
        debug_assert!(n <= count, "Cannot return a device path bigger than self.");

        let nb_skip = count - n;
        let mut idx = 0;
        for _ in 0..nb_skip {
            let header = self.buffer.pread_with::<Header>(idx, scroll::LE).unwrap();
            idx += header.length;
        }
        let end_buffer = &self.buffer[idx..];
        unsafe { &*(end_buffer as *const [u8] as *const DevicePath) }
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
    type Item = DevicePathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        let mut item = None;
        for node in self.node_iterator.by_ref() {
            if node.header().r#type == DevicePathType::End as u8 {
                item.get_or_insert(DevicePathBuf::new_empty()).append(EndEntire);
                break;
            } else {
                item.get_or_insert(DevicePathBuf::new_empty()).append(node);
            }
        }
        item
    }
}

/// Device path node iterator.
pub struct Iter<'a> {
    device_path: &'a DevicePath,
    offset: usize,
}

impl<'a> Iter<'a> {
    fn new(device_path: &'a DevicePath) -> Iter<'a> {
        Self { device_path, offset: 0 }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = UnknownDevicePathNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // No more byte to read in the device path.
        if self.offset == self.device_path.buffer.len() {
            return None;
        }

        let Ok(unknown_device_path) =
            self.device_path.buffer.gread_with::<UnknownDevicePathNode>(&mut self.offset, scroll::LE)
        else {
            debug_assert!(false, "The buffer is corrupted, could not read the device path node.");
            return None;
        };

        Some(unknown_device_path)
    }
}

impl Display for DevicePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut nodes = self.iter().map(nodes::cast_to_dyn_device_path_node).peekable();

        while let Some(node) = nodes.next() {
            f.write_fmt(format_args!("{}", &node))?;

            if let Some(next) = nodes.peek() {
                if node.header().r#type != DevicePathType::End as u8
                    && next.header().r#type != DevicePathType::End as u8
                {
                    f.write_char('/')?;
                }
            };
        }

        Ok(())
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::assert_eq;

    use super::{
        nodes::{Acpi, AcpiSubType, EndSubType, Pci},
        *,
    };

    #[test]
    fn test_new_empty_device_path_buf() {
        let device_path = DevicePathBuf::new_empty();
        assert!(device_path.buffer.is_empty());
    }

    #[test]
    fn test_append_node() {
        let mut device_path = DevicePathBuf::new_empty();
        device_path.append(Acpi::new_pci_root(0));
        device_path.append(EndEntire);

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
        device_path.append(Acpi::new_pci_root(0));
        device_path.append(EndEntire);

        let mut device_path_to_add = DevicePathBuf::new_empty();
        device_path_to_add.append(Pci { function: 1, device: 2 });
        device_path_to_add.append(EndEntire);

        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Acpi::new_pci_root(0));
        expected_device_path.append(Pci { function: 1, device: 2 });
        expected_device_path.append(EndEntire);

        device_path.append_device_path(&device_path_to_add);

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_append_device_path_instance() {
        let mut device_path = DevicePathBuf::new_empty();
        device_path.append(Acpi::new_pci_root(0));
        device_path.append(EndEntire);

        let mut device_path_to_add = DevicePathBuf::new_empty();
        device_path_to_add.append(Pci { function: 1, device: 2 });
        device_path_to_add.append(EndEntire);

        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Acpi::new_pci_root(0));
        expected_device_path.append(EndInstance);
        expected_device_path.append(Pci { function: 1, device: 2 });
        expected_device_path.append(EndEntire);

        device_path.append_device_path_instances(&device_path_to_add);

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buff_from_node_iter() {
        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Acpi::new_pci_root(0));
        expected_device_path.append(Pci { function: 1, device: 2 });
        expected_device_path.append(EndEntire);

        let device_path = DevicePathBuf::from_device_path_node_iter(expected_device_path.iter());

        assert_eq!(expected_device_path, device_path);
    }

    #[test]
    fn test_device_path_buff_to_boxed_device_path() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0));
        device_path_buf.append(Pci { function: 1, device: 2 });
        device_path_buf.append(EndEntire);

        let device_path = DevicePathBuf::clone(&device_path_buf).into_box_device_path();

        assert_eq!(device_path_buf.as_ref(), device_path.as_ref());
    }

    #[test]
    fn test_device_path_from_ptr() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0));
        device_path_buf.append(Pci { function: 1, device: 2 });
        device_path_buf.append(EndEntire);
        let buffer_ptr = device_path_buf.buffer.as_slice().as_ptr();

        let device_path = unsafe { DevicePath::try_from_ptr(buffer_ptr) }.unwrap();

        assert_eq!(device_path_buf.as_ref(), device_path);
    }

    #[test]
    fn test_device_path_size() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        assert_eq!(0, device_path_buf.size());
        device_path_buf.append(Acpi::new_pci_root(0));
        assert_eq!(12, device_path_buf.size());
        device_path_buf.append(Pci { function: 1, device: 2 });
        assert_eq!(18, device_path_buf.size());
        device_path_buf.append(EndEntire);
        assert_eq!(22, device_path_buf.size());
    }

    #[test]
    fn test_device_path_node_count() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        assert_eq!(0, device_path_buf.node_count());
        device_path_buf.append(Acpi::new_pci_root(0));
        assert_eq!(1, device_path_buf.node_count());
        device_path_buf.append(Pci { function: 1, device: 2 });
        assert_eq!(2, device_path_buf.node_count());
        device_path_buf.append(EndEntire);
        assert_eq!(3, device_path_buf.node_count());
    }

    #[test]
    fn test_device_path_is_multi_instances() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0));
        device_path_buf.append(Pci { function: 1, device: 2 });
        device_path_buf.append(EndEntire);

        let mut device_path_buf_2 = DevicePathBuf::new_empty();
        device_path_buf_2.append(Acpi::new_pci_root(0));
        device_path_buf_2.append(Pci { function: 1, device: 2 });
        device_path_buf_2.append(EndEntire);

        assert!(!device_path_buf.is_multi_instance());

        device_path_buf.append_device_path_instances(&device_path_buf_2);

        assert!(device_path_buf.is_multi_instance());
    }

    #[test]
    fn test_device_path_slice_end() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0));
        device_path_buf.append(Pci { function: 1, device: 2 });
        device_path_buf.append(EndEntire);

        let mut expected_device_path = DevicePathBuf::new_empty();
        expected_device_path.append(Pci { function: 1, device: 2 });
        expected_device_path.append(EndEntire);

        assert_eq!(expected_device_path.as_ref(), device_path_buf.slice_end(2));
    }

    #[test]
    fn test_device_path_start_with() {
        let mut device_path_buf = DevicePathBuf::new_empty();
        device_path_buf.append(Acpi::new_pci_root(0));
        device_path_buf.append(Pci { function: 1, device: 2 });
        device_path_buf.append(EndEntire);

        let mut start = DevicePathBuf::new_empty();
        start.append(Acpi::new_pci_root(0));
        start.append(EndEntire);

        assert!(device_path_buf.starts_with(&start));

        let mut start = DevicePathBuf::new_empty();
        start.append(Acpi::new_pci_root(1));
        start.append(EndEntire);

        assert!(!device_path_buf.starts_with(&start));
    }
}
