//! Device searches used to expand short-form device paths
//! ([3.1.2. Load Option Processing](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html#load-option-processing)).
//!
//! [`DevicePath::expand_shorthand`](super::DevicePath::expand_shorthand) uses these helpers to
//! find the devices that match the first node of a short-form device path.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
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
    let device_path = unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) }.ok()?;
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
    if interface_descriptor(usb_io).is_none_or(|interface| u16::from(interface.interface_number) != wwid.interface) {
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    extern crate std;

    use super::*;
    use crate::uefi::{
        boot_services::{MockBootServices, boxed::BootServicesBox, protocol_handler::HandleSearchType},
        device_path::{
            DevicePathBuf,
            node::{
                Acpi, DevicePathNode, EndEntire, FilePath, HardDrive, Pci, PiwgFirmwareVolume, Uri, Usb, UsbClass,
                UsbWwid,
            },
        },
        protocols::usb_io,
    };
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
    fn shorthand_path(guid: [u8; 16]) -> DevicePathBuf {
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
            !path([PiwgFirmwareVolume { fv_name: crate::BinaryGuid::from_bytes(&[0x11; 16]) }]).is_shorthand_notation()
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
