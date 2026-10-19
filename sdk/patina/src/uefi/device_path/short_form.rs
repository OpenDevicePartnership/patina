//! Expansion of short-form device paths.
//!
//! [UEFI Spec Documentation: 3.1.2. Load Option Processing](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html#load-option-processing)
//!
//! [`DevicePath::expand_short_form`] decodes the first node of a short-form device path and lets
//! that node find the devices it refers to: each node type that can start a short-form device
//! path has an `expand_short_form` method that returns the full device path candidates.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;
use core::ptr;

use crate::{
    base::error::EfiError,
    standard::efi,
    uefi::{
        boot_services::{BootServices, protocol_handler::HandleSearchType},
        device_path::{
            DevicePath, DevicePathBuf,
            node::{EndEntire, FilePath, HardDrive, Uri, UsbClass, UsbWwid},
        },
        protocols::usb_io,
    },
};

impl DevicePath {
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
        let Some(first_node) = self.iter().next().filter(|node| !node.header.is_type::<EndEntire>()) else {
            log::error!("Cannot expand an empty device path");
            return Err(EfiError::InvalidParameter);
        };
        if !self.is_short_form() {
            return Ok(Vec::from([self.into()]));
        }
        // The short-form device path after its first node.
        let rest = DevicePath::try_from_bytes(self.buffer.get(first_node.as_bytes().len()..).unwrap_or_default())
            .map_err(|_| EfiError::InvalidParameter)?;

        let header = first_node.header;
        let candidates = if header.is_type::<HardDrive>() {
            HardDrive::try_from(first_node)
                .map_err(|_| EfiError::InvalidParameter)?
                .expand_short_form(boot_services, rest)?
        } else if header.is_type::<UsbWwid>() {
            UsbWwid::try_from(first_node)
                .map_err(|_| EfiError::InvalidParameter)?
                .expand_short_form(boot_services, rest)?
        } else if header.is_type::<UsbClass>() {
            UsbClass::try_from(first_node)
                .map_err(|_| EfiError::InvalidParameter)?
                .expand_short_form(boot_services, rest)?
        } else if header.is_type::<FilePath>() {
            FilePath::expand_short_form(boot_services, self)?
        } else if header.is_type::<Uri>() {
            Uri::expand_short_form(boot_services, self)?
        } else {
            log::error!("Short-form device path starts with an invalid node");
            return Err(EfiError::InvalidParameter);
        };

        if candidates.is_empty() {
            log::error!("No device matches the short-form device path");
            return Err(EfiError::NotFound);
        }
        Ok(candidates)
    }

    /// Return the handles that support `protocol`; none if no handle supports it.
    fn handles<B: BootServices>(boot_services: &B, protocol: &'static efi::Guid) -> Result<Vec<efi::Handle>, EfiError> {
        match boot_services.locate_handle_buffer(HandleSearchType::ByProtocol(protocol)) {
            Ok(handles) => Ok(handles.iter().copied().collect()),
            Err(efi::Status::NOT_FOUND) => Ok(Vec::new()),
            Err(status) => Err(EfiError::from(status)),
        }
    }

    /// Return the device path installed on `handle`, if it has a valid one.
    fn handle_device_path<B: BootServices>(boot_services: &B, handle: efi::Handle) -> Option<&'static DevicePath> {
        // SAFETY: The handle comes from LocateHandleBuffer and the requested protocol is the device path protocol.
        let protocol =
            unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) }.ok()?;
        // SAFETY: The pointer comes from a device path protocol interface installed on the handle.
        unsafe { DevicePath::try_from_ptr(ptr::from_ref(protocol).cast::<u8>()) }.ok()
    }

    /// Return the USB devices that have a device path, with their USB I/O protocol.
    fn usb_devices<B: BootServices>(
        boot_services: &B,
    ) -> Result<Vec<(&'static DevicePath, &'static mut usb_io::Protocol)>, EfiError> {
        let mut devices = Vec::new();
        for handle in Self::handles(boot_services, &usb_io::PROTOCOL_GUID)? {
            let Some(handle_path) = Self::handle_device_path(boot_services, handle) else {
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

    /// Return a device path that continues `prefix` with `tail`.
    fn extend(prefix: &DevicePath, tail: &DevicePath) -> Result<DevicePathBuf, EfiError> {
        let mut candidate = DevicePathBuf::from(prefix);
        candidate.append_device_path(tail).map_err(|_| EfiError::OutOfResources)?;
        Ok(candidate)
    }
}

impl HardDrive {
    /// Expand a short-form device path that starts with this node: for each device path with an
    /// equal hard drive node, that path up to its hard drive node, then `rest`.
    fn expand_short_form<B: BootServices>(
        &self,
        boot_services: &B,
        rest: &DevicePath,
    ) -> Result<Vec<DevicePathBuf>, EfiError> {
        let mut candidates = Vec::new();
        for handle in DevicePath::handles(boot_services, &efi::protocols::device_path::PROTOCOL_GUID)? {
            let Some(handle_path) = DevicePath::handle_device_path(boot_services, handle) else {
                continue;
            };
            let mut candidate = DevicePathBuf::new();
            for node in handle_path.iter().take_while(|node| !node.header.is_type::<EndEntire>()) {
                candidate.append(node).map_err(|_| EfiError::OutOfResources)?;
                if HardDrive::try_from(node).is_ok_and(|hard_drive| hard_drive == *self) {
                    candidate.append_device_path(rest).map_err(|_| EfiError::OutOfResources)?;
                    candidates.push(candidate);
                    break;
                }
            }
        }
        Ok(candidates)
    }
}

impl UsbWwid {
    /// Expand a short-form device path that starts with this node: for each USB device that
    /// matches this node, its device path, then `rest`.
    fn expand_short_form<B: BootServices>(
        &self,
        boot_services: &B,
        rest: &DevicePath,
    ) -> Result<Vec<DevicePathBuf>, EfiError> {
        DevicePath::usb_devices(boot_services)?
            .into_iter()
            .filter_map(|(handle_path, usb_io)| self.matches(boot_services, usb_io).then_some(handle_path))
            .map(|handle_path| DevicePath::extend(handle_path, rest))
            .collect()
    }
}

impl UsbClass {
    /// Expand a short-form device path that starts with this node: for each USB device that
    /// matches this node, its device path, then `rest`.
    fn expand_short_form<B: BootServices>(
        &self,
        boot_services: &B,
        rest: &DevicePath,
    ) -> Result<Vec<DevicePathBuf>, EfiError> {
        DevicePath::usb_devices(boot_services)?
            .into_iter()
            .filter_map(|(handle_path, usb_io)| self.matches(usb_io).then_some(handle_path))
            .map(|handle_path| DevicePath::extend(handle_path, rest))
            .collect()
    }
}

impl FilePath {
    /// Expand `path`, a short-form device path that starts with a file path node: for each file
    /// system, its device path, then `path`. File systems on removable media come first, then
    /// those on fixed media, then those without block I/O.
    fn expand_short_form<B: BootServices>(
        boot_services: &B,
        path: &DevicePath,
    ) -> Result<Vec<DevicePathBuf>, EfiError> {
        let (mut removable, mut fixed, mut without_block_io) = (Vec::new(), Vec::new(), Vec::new());
        for handle in DevicePath::handles(boot_services, &efi::protocols::simple_file_system::PROTOCOL_GUID)? {
            let Some(handle_path) = DevicePath::handle_device_path(boot_services, handle) else {
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
        removable
            .into_iter()
            .chain(fixed)
            .chain(without_block_io)
            .map(|handle_path| DevicePath::extend(handle_path, path))
            .collect()
    }
}

impl Uri {
    /// Expand `path`, a short-form device path that starts with a URI node: for each load file
    /// handle, its device path, then `path`, so that loading it passes the short-form path to the
    /// handle's `LoadFile`.
    fn expand_short_form<B: BootServices>(
        boot_services: &B,
        path: &DevicePath,
    ) -> Result<Vec<DevicePathBuf>, EfiError> {
        DevicePath::handles(boot_services, &efi::protocols::load_file::PROTOCOL_GUID)?
            .into_iter()
            .filter_map(|handle| DevicePath::handle_device_path(boot_services, handle))
            .map(|handle_path| DevicePath::extend(handle_path, path))
            .collect()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;

    use super::*;
    use crate::uefi::{
        boot_services::{MockBootServices, boxed::BootServicesBox},
        device_path::node::{Acpi, DevicePathNode, Pci, PiwgFirmwareVolume, Usb},
    };
    use alloc::boxed::Box;
    use core::ffi::c_void;
    use scroll::ctx::TryIntoCtx;

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
    struct MockUsb {
        protocol: usb_io::Protocol,
        device: usb_io::DeviceDescriptor,
        interface: usb_io::InterfaceDescriptor,
        serial: Vec<u16>,
        languages: Vec<u16>,
    }

    fn mock_usb(this: *mut usb_io::Protocol) -> &'static MockUsb {
        // SAFETY: Test code - every protocol is the first field of a leaked MockUsb.
        unsafe { &*this.cast::<MockUsb>() }
    }

    extern "efiapi" fn get_device_descriptor(
        this: *mut usb_io::Protocol,
        descriptor: *mut usb_io::DeviceDescriptor,
    ) -> efi::Status {
        // SAFETY: Test code - the caller passes a valid descriptor.
        unsafe { descriptor.write(mock_usb(this).device) };
        efi::Status::SUCCESS
    }

    extern "efiapi" fn get_interface_descriptor(
        this: *mut usb_io::Protocol,
        descriptor: *mut usb_io::InterfaceDescriptor,
    ) -> efi::Status {
        // SAFETY: Test code - the caller passes a valid descriptor.
        unsafe { descriptor.write(mock_usb(this).interface) };
        efi::Status::SUCCESS
    }

    extern "efiapi" fn get_string_descriptor(
        this: *mut usb_io::Protocol,
        _lang_id: u16,
        _string_id: u8,
        string: *mut *mut u16,
    ) -> efi::Status {
        // SAFETY: Test code - the caller passes a valid output pointer; free_pool is a no-op.
        unsafe { string.write(mock_usb(this).serial.as_ptr().cast_mut()) };
        efi::Status::SUCCESS
    }

    extern "efiapi" fn get_supported_languages(
        this: *mut usb_io::Protocol,
        table: *mut *mut u16,
        size: *mut u16,
    ) -> efi::Status {
        let languages = &mock_usb(this).languages;
        // SAFETY: Test code - the caller passes valid output pointers.
        unsafe {
            table.write(languages.as_ptr().cast_mut());
            size.write(u16::try_from(languages.len() * 2).unwrap());
        }
        efi::Status::SUCCESS
    }

    fn usb_device(vendor_id: u16, product_id: u16, class: [u8; 3], interface: [u8; 3], serial: &str) -> MockUsb {
        let mut serial = serial.encode_utf16().collect::<Vec<_>>();
        serial.push(0);
        MockUsb {
            protocol: usb_io::Protocol {
                control_transfer: ptr::null(),
                bulk_transfer: ptr::null(),
                async_interrupt_transfer: ptr::null(),
                sync_interrupt_transfer: ptr::null(),
                isochronous_transfer: ptr::null(),
                async_isochronous_transfer: ptr::null(),
                get_device_descriptor,
                get_config_descriptor: ptr::null(),
                get_interface_descriptor,
                get_endpoint_descriptor: ptr::null(),
                get_string_descriptor,
                get_supported_languages,
                port_reset: ptr::null(),
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
        _: *mut c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn flush(_: *mut efi::protocols::block_io::Protocol) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    fn block_io(removable: bool) -> &'static mut efi::protocols::block_io::Protocol {
        let media = Box::leak(Box::new(efi::protocols::block_io::Media {
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
        Box::leak(Box::new(efi::protocols::block_io::Protocol {
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
        usb: Option<MockUsb>,
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
            paths
                .push(device.path.map(|path| Box::leak(path.as_bytes().to_vec().into_boxed_slice()).as_ptr() as usize));
            usb_addresses.push(device.usb.map(|usb| ptr::from_mut(Box::leak(Box::new(usb))) as usize));
            block_ios.push(device.removable.map(|removable| ptr::from_mut(block_io(removable)) as usize));
        }
        let supports = move |guid: &efi::Guid, index: usize| -> bool {
            let [path, usb, file_system, load_file] = protocols[index];
            (*guid == efi::protocols::device_path::PROTOCOL_GUID && path)
                || (*guid == usb_io::PROTOCOL_GUID && usb)
                || (*guid == efi::protocols::simple_file_system::PROTOCOL_GUID && file_system)
                || (*guid == efi::protocols::load_file::PROTOCOL_GUID && load_file)
        };
        let count = paths.len();
        let free_pool: &'static MockBootServices = Box::leak(Box::new({
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
                // SAFETY: Test code - the address points to a leaked MockUsb, which starts with its protocol.
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

    #[test]
    fn test_device_path_short_form_handles_treats_not_found_as_empty() {
        let mut mock = MockBootServices::new();
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));
        assert_eq!(DevicePath::handles(&mock, &usb_io::PROTOCOL_GUID), Ok(Vec::new()));

        let mut mock = MockBootServices::new();
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::DEVICE_ERROR));
        assert_eq!(DevicePath::handles(&mock, &usb_io::PROTOCOL_GUID), Err(EfiError::DeviceError));
    }

    #[test]
    fn test_device_path_short_form_extend() {
        let prefix = disk_path([1; 16]);
        assert_eq!(DevicePath::extend(&prefix, &boot_file()).unwrap(), join(&[&prefix, &boot_file()]));
    }

    #[test]
    fn test_device_path_short_form_usb_devices_skip_handles_without_device_path() {
        let mock = mock_system(Vec::from([
            Device { usb: Some(usb_device(1, 1, [0; 3], [0; 3], "a")), ..Device::default() },
            Device { usb: Some(usb_device(2, 2, [0; 3], [0; 3], "b")), ..Device::with_path(usb_path(2)) },
        ]));
        let devices = DevicePath::usb_devices(&mock).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].0, &*usb_path(2));
    }

    /// The hard drive candidate keeps the device path up to its hard drive node only.
    #[test]
    fn test_device_path_short_form_hard_drive_keeps_path_up_to_its_node() {
        let guid = [0xaa; 16];
        let installed = join(&[&disk_path(guid), &path([FilePath::new("\\ignored")])]);
        let mock = mock_system(Vec::from([Device::with_path(installed)]));
        let rest = path([FilePath::new("\\a.efi")]);
        assert_eq!(hard_drive(guid).expand_short_form(&mock, &rest).unwrap(), [join(&[&disk_path(guid), &rest])]);
    }

    /// Without a matching device, each node returns no candidates; only the dispatcher reports
    /// `NotFound`.
    #[test]
    fn test_device_path_short_form_nodes_return_no_candidates_without_devices() {
        let mock = mock_system(Vec::from([Device::with_path(disk_path([1; 16]))]));
        let rest = boot_file();
        let wwid = UsbWwid { interface: 0, vendor_id: 1, product_id: 2, serial_number: Vec::from([0x41]) };
        let class = UsbClass {
            vendor_id: u16::MAX,
            product_id: u16::MAX,
            device_class: u8::MAX,
            device_subclass: u8::MAX,
            device_protocol: u8::MAX,
        };
        let uri = path([Uri { uri: "http://a/b.efi".into() }]);

        assert_eq!(hard_drive([0xaa; 16]).expand_short_form(&mock, &rest), Ok(Vec::new()));
        assert_eq!(wwid.expand_short_form(&mock, &rest), Ok(Vec::new()));
        assert_eq!(class.expand_short_form(&mock, &rest), Ok(Vec::new()));
        assert_eq!(FilePath::expand_short_form(&mock, &boot_file()), Ok(Vec::new()));
        assert_eq!(Uri::expand_short_form(&mock, &uri), Ok(Vec::new()));
    }
}
