//! HidIo protocol FFI definitions.
//!
//! Defines the HidIo protocol interface struct, types, and GUID shared between
//! producers (e.g. `usb_hid`) and consumers (e.g. `uefi_hid`) of the protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use r_efi::efi;

/// HidIo interface GUID: 3EA93936-6BF4-49D6-AA50-D9F5B9AD8CFF
pub const HID_IO_PROTOCOL_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("3EA93936-6BF4-49D6-AA50-D9F5B9AD8CFF");

/// HID report types per the HID specification.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(C)]
pub enum HidReportType {
    /// Input report (device to host).
    InputReport = 1,
    /// Output report (host to device).
    OutputReport = 2,
    /// Feature report (bidirectional).
    Feature = 3,
}

/// Callback type for receiving asynchronous input reports.
pub type HidIoReportCallback =
    unsafe extern "efiapi" fn(report_buffer_size: u16, report_buffer: *mut c_void, context: *mut c_void);

/// The HID_IO protocol FFI interface.
#[repr(C)]
pub struct HidIoProtocol {
    /// Retrieves the HID report descriptor from the device.
    pub get_report_descriptor: unsafe extern "efiapi" fn(
        this: *const HidIoProtocol,
        report_descriptor_size: *mut usize,
        report_descriptor_buffer: *mut c_void,
    ) -> efi::Status,
    /// Retrieves a HID report of the specified type from the device.
    pub get_report: unsafe extern "efiapi" fn(
        this: *const HidIoProtocol,
        report_id: u8,
        report_type: HidReportType,
        report_buffer_size: usize,
        report_buffer: *mut c_void,
    ) -> efi::Status,
    /// Sends a HID report of the specified type to the device.
    pub set_report: unsafe extern "efiapi" fn(
        this: *const HidIoProtocol,
        report_id: u8,
        report_type: HidReportType,
        report_buffer_size: usize,
        report_buffer: *mut c_void,
    ) -> efi::Status,
    /// Registers a callback for asynchronous input report notifications.
    pub register_report_callback: unsafe extern "efiapi" fn(
        this: *const HidIoProtocol,
        callback: HidIoReportCallback,
        context: *mut c_void,
    ) -> efi::Status,
    /// Unregisters a previously registered input report callback.
    pub unregister_report_callback:
        unsafe extern "efiapi" fn(this: *const HidIoProtocol, callback: HidIoReportCallback) -> efi::Status,
}

// SAFETY: HidIoProtocol is a C-compatible struct whose layout matches the HidIo GUID interface.
unsafe impl crate::uefi_protocol::ProtocolInterface for HidIoProtocol {
    const PROTOCOL_GUID: crate::BinaryGuid = HID_IO_PROTOCOL_GUID;
}
