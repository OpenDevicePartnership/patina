//! USB I/O protocol (UEFI 2.10 Section 17.2.4).
//!
//! The protocol layout and descriptor structures match `EFI_USB_IO_PROTOCOL` and the USB 2.0
//! standard descriptors. Only the descriptor functions are typed; the transfer and reset
//! functions are kept as opaque pointers so the layout stays correct.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use crate::{BinaryGuid, base::protocol::ProtocolInterface, standard::efi};

/// `EFI_USB_IO_PROTOCOL_GUID`.
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x2b2f68d6, 0x0cd2, 0x44cf, 0x8e, 0x8b, &[0xbb, 0xa2, 0x0b, 0x1b, 0x5b, 0x75]);

/// Standard USB device descriptor (USB 2.0 Section 9.6.1).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceDescriptor {
    /// Size of the descriptor in bytes.
    pub length: u8,
    /// Descriptor type.
    pub descriptor_type: u8,
    /// USB specification release number in binary-coded decimal.
    pub bcd_usb: u16,
    /// Class code; zero means that each interface specifies its own class.
    pub device_class: u8,
    /// Subclass code.
    pub device_sub_class: u8,
    /// Protocol code.
    pub device_protocol: u8,
    /// Maximum packet size for endpoint zero.
    pub max_packet_size0: u8,
    /// Vendor ID.
    pub id_vendor: u16,
    /// Product ID.
    pub id_product: u16,
    /// Device release number in binary-coded decimal.
    pub bcd_device: u16,
    /// Index of the manufacturer string descriptor.
    pub str_manufacturer: u8,
    /// Index of the product string descriptor.
    pub str_product: u8,
    /// Index of the serial number string descriptor, or zero if there is none.
    pub str_serial_number: u8,
    /// Number of possible configurations.
    pub num_configurations: u8,
}

/// Standard USB interface descriptor (USB 2.0 Section 9.6.5).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InterfaceDescriptor {
    /// Size of the descriptor in bytes.
    pub length: u8,
    /// Descriptor type.
    pub descriptor_type: u8,
    /// Number of the interface.
    pub interface_number: u8,
    /// Alternate setting of the interface.
    pub alternate_setting: u8,
    /// Number of endpoints, excluding endpoint zero.
    pub num_endpoints: u8,
    /// Class code.
    pub interface_class: u8,
    /// Subclass code.
    pub interface_sub_class: u8,
    /// Protocol code.
    pub interface_protocol: u8,
    /// Index of the interface string descriptor.
    pub interface: u8,
}

/// `EFI_USB_IO_GET_DEVICE_DESCRIPTOR`.
pub type GetDeviceDescriptor =
    extern "efiapi" fn(this: *mut Protocol, descriptor: *mut DeviceDescriptor) -> efi::Status;

/// `EFI_USB_IO_GET_INTERFACE_DESCRIPTOR`.
pub type GetInterfaceDescriptor =
    extern "efiapi" fn(this: *mut Protocol, descriptor: *mut InterfaceDescriptor) -> efi::Status;

/// `EFI_USB_IO_GET_STRING_DESCRIPTOR`.
///
/// On success, `string` receives a NUL-terminated UTF-16 string allocated from pool memory,
/// which the caller frees.
pub type GetStringDescriptor =
    extern "efiapi" fn(this: *mut Protocol, lang_id: u16, string_id: u8, string: *mut *mut u16) -> efi::Status;

/// `EFI_USB_IO_GET_SUPPORTED_LANGUAGE`.
///
/// On success, `lang_id_table` receives the language ID table owned by the protocol, and
/// `table_size` its size in bytes.
pub type GetSupportedLanguages =
    extern "efiapi" fn(this: *mut Protocol, lang_id_table: *mut *mut u16, table_size: *mut u16) -> efi::Status;

/// `EFI_USB_IO_PROTOCOL`.
#[repr(C)]
pub struct Protocol {
    /// `UsbControlTransfer`.
    pub control_transfer: *const c_void,
    /// `UsbBulkTransfer`.
    pub bulk_transfer: *const c_void,
    /// `UsbAsyncInterruptTransfer`.
    pub async_interrupt_transfer: *const c_void,
    /// `UsbSyncInterruptTransfer`.
    pub sync_interrupt_transfer: *const c_void,
    /// `UsbIsochronousTransfer`.
    pub isochronous_transfer: *const c_void,
    /// `UsbAsyncIsochronousTransfer`.
    pub async_isochronous_transfer: *const c_void,
    /// `UsbGetDeviceDescriptor`.
    pub get_device_descriptor: GetDeviceDescriptor,
    /// `UsbGetConfigDescriptor`.
    pub get_config_descriptor: *const c_void,
    /// `UsbGetInterfaceDescriptor`.
    pub get_interface_descriptor: GetInterfaceDescriptor,
    /// `UsbGetEndpointDescriptor`.
    pub get_endpoint_descriptor: *const c_void,
    /// `UsbGetStringDescriptor`.
    pub get_string_descriptor: GetStringDescriptor,
    /// `UsbGetSupportedLanguages`.
    pub get_supported_languages: GetSupportedLanguages,
    /// `UsbPortReset`.
    pub port_reset: *const c_void,
}

// SAFETY: `Protocol` has the layout of `EFI_USB_IO_PROTOCOL`, which `PROTOCOL_GUID` identifies.
unsafe impl ProtocolInterface for Protocol {
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid(PROTOCOL_GUID);
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_usb_io_layout() {
        assert_eq!(core::mem::size_of::<DeviceDescriptor>(), 18);
        assert_eq!(core::mem::size_of::<InterfaceDescriptor>(), 9);
        assert_eq!(core::mem::size_of::<Protocol>(), 13 * core::mem::size_of::<usize>());
        assert_eq!(core::mem::offset_of!(Protocol, get_device_descriptor), 6 * core::mem::size_of::<usize>());
        assert_eq!(core::mem::offset_of!(Protocol, get_string_descriptor), 10 * core::mem::size_of::<usize>());
    }
}
