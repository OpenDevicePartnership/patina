//! USB IO protocol FFI types and descriptor structures.
//!
//! Defines the `EFI_USB_IO_PROTOCOL` interface and associated USB descriptor
//! types needed to interact with USB devices.
//!
//! The underlying protocol is defined in the UEFI Specification 2.11 section
//! 17.2.4 (USB I/O Protocol).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
pub mod types;

use core::ffi::c_void;

use r_efi::efi;

use self::types::{EfiUsbConfigDescriptor, EfiUsbEndpointDescriptor, EfiUsbInterfaceDescriptor};

/// USB IO protocol GUID: 2B2F68D6-0CD2-44CF-8E8B-BBA20B1B5B75
pub const USB_IO_PROTOCOL_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("2B2F68D6-0CD2-44CF-8E8B-BBA20B1B5B75");

/// Callback for async USB interrupt transfers.
pub type EfiAsyncUsbTransferCallback =
    unsafe extern "efiapi" fn(data: *mut c_void, data_length: usize, context: *mut c_void, status: u32) -> efi::Status;

/// The EFI_USB_IO_PROTOCOL FFI interface.
///
/// Only the functions required by the USB HID driver are included.
#[repr(C)]
pub struct EfiUsbIoProtocol {
    // USB I/O transfer functions
    pub usb_control_transfer: unsafe extern "efiapi" fn(
        this: *const EfiUsbIoProtocol,
        request: *const EfiUsbDeviceRequest,
        direction: EfiUsbDataDirection,
        timeout: u32,
        data: *mut c_void,
        data_length: usize,
        status: *mut u32,
    ) -> efi::Status,
    _usb_bulk_transfer: usize,
    pub usb_async_interrupt_transfer: unsafe extern "efiapi" fn(
        this: *const EfiUsbIoProtocol,
        endpoint: u8,
        is_new_transfer: bool,
        polling_interval: usize,
        data_length: usize,
        callback: Option<EfiAsyncUsbTransferCallback>,
        context: *mut c_void,
    ) -> efi::Status,
    _usb_sync_interrupt_transfer: usize,
    _usb_isochronous_transfer: usize,
    _usb_async_isochronous_transfer: usize,

    // USB descriptor access functions
    pub usb_get_device_descriptor: usize,
    pub usb_get_config_descriptor: unsafe extern "efiapi" fn(
        this: *const EfiUsbIoProtocol,
        config_descriptor: *mut EfiUsbConfigDescriptor,
    ) -> efi::Status,
    pub usb_get_interface_descriptor: unsafe extern "efiapi" fn(
        this: *const EfiUsbIoProtocol,
        interface_descriptor: *mut EfiUsbInterfaceDescriptor,
    ) -> efi::Status,
    pub usb_get_endpoint_descriptor: unsafe extern "efiapi" fn(
        this: *const EfiUsbIoProtocol,
        endpoint_index: u8,
        endpoint_descriptor: *mut EfiUsbEndpointDescriptor,
    ) -> efi::Status,

    // USB string access functions
    _usb_get_string_descriptor: usize,
    _usb_get_supported_languages: usize,

    // Miscellaneous functions
    pub usb_port_reset: unsafe extern "efiapi" fn(this: *const EfiUsbIoProtocol) -> efi::Status,
}

// SAFETY: EfiUsbIoProtocol is a C-compatible struct whose layout matches the USB IO GUID interface.
unsafe impl crate::uefi_protocol::ProtocolInterface for EfiUsbIoProtocol {
    const PROTOCOL_GUID: crate::BinaryGuid = USB_IO_PROTOCOL_GUID;
}

#[cfg(any(test, feature = "test-stubs"))]
impl EfiUsbIoProtocol {
    /// Returns a stub protocol with panicking function pointers for testing.
    ///
    /// Callers should replace the function pointers they need before use.
    #[coverage(off)]
    pub fn stub() -> Self {
        unsafe extern "efiapi" fn stub_control_transfer(
            _this: *const EfiUsbIoProtocol,
            _request: *const EfiUsbDeviceRequest,
            _direction: EfiUsbDataDirection,
            _timeout: u32,
            _data: *mut c_void,
            _data_length: usize,
            _status: *mut u32,
        ) -> efi::Status {
            panic!("unexpected call to usb_control_transfer")
        }
        unsafe extern "efiapi" fn stub_async_interrupt_transfer(
            _this: *const EfiUsbIoProtocol,
            _endpoint: u8,
            _is_new_transfer: bool,
            _polling_interval: usize,
            _data_length: usize,
            _callback: Option<EfiAsyncUsbTransferCallback>,
            _context: *mut c_void,
        ) -> efi::Status {
            panic!("unexpected call to usb_async_interrupt_transfer")
        }
        unsafe extern "efiapi" fn stub_get_config_descriptor(
            _this: *const EfiUsbIoProtocol,
            _config_descriptor: *mut EfiUsbConfigDescriptor,
        ) -> efi::Status {
            panic!("unexpected call to usb_get_config_descriptor")
        }
        unsafe extern "efiapi" fn stub_get_interface_descriptor(
            _this: *const EfiUsbIoProtocol,
            _interface_descriptor: *mut EfiUsbInterfaceDescriptor,
        ) -> efi::Status {
            panic!("unexpected call to usb_get_interface_descriptor")
        }
        unsafe extern "efiapi" fn stub_get_endpoint_descriptor(
            _this: *const EfiUsbIoProtocol,
            _endpoint_index: u8,
            _endpoint_descriptor: *mut EfiUsbEndpointDescriptor,
        ) -> efi::Status {
            panic!("unexpected call to usb_get_endpoint_descriptor")
        }
        unsafe extern "efiapi" fn stub_port_reset(_this: *const EfiUsbIoProtocol) -> efi::Status {
            panic!("unexpected call to usb_port_reset")
        }
        Self {
            usb_control_transfer: stub_control_transfer,
            _usb_bulk_transfer: 0,
            usb_async_interrupt_transfer: stub_async_interrupt_transfer,
            _usb_sync_interrupt_transfer: 0,
            _usb_isochronous_transfer: 0,
            _usb_async_isochronous_transfer: 0,
            usb_get_device_descriptor: 0,
            usb_get_config_descriptor: stub_get_config_descriptor,
            usb_get_interface_descriptor: stub_get_interface_descriptor,
            usb_get_endpoint_descriptor: stub_get_endpoint_descriptor,
            _usb_get_string_descriptor: 0,
            _usb_get_supported_languages: 0,
            usb_port_reset: stub_port_reset,
        }
    }
}

/// Direction of USB data transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum EfiUsbDataDirection {
    DataIn = 0,
    DataOut = 1,
    NoData = 2,
}

/// USB device request structure (SETUP packet).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EfiUsbDeviceRequest {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}
