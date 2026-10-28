//! `HidIo` protocol FFI definitions.
//!
//! Defines the `HidIo` protocol interface struct, types, and GUID shared between
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

/// `HidIo` interface GUID: 3EA93936-6BF4-49D6-AA50-D9F5B9AD8CFF
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

/// The `HID_IO` protocol FFI interface.
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
    /// Providers must reject `None` with `EFI_INVALID_PARAMETER`; the callback is required.
    pub register_report_callback: unsafe extern "efiapi" fn(
        this: *const HidIoProtocol,
        callback: Option<HidIoReportCallback>,
        context: *mut c_void,
    ) -> efi::Status,
    /// Unregisters a previously registered input report callback.
    /// Providers must reject `None` with `EFI_INVALID_PARAMETER`; the callback is required.
    pub unregister_report_callback:
        unsafe extern "efiapi" fn(this: *const HidIoProtocol, callback: Option<HidIoReportCallback>) -> efi::Status,
}

// SAFETY: HidIoProtocol is a C-compatible struct whose layout matches the HidIo GUID interface.
unsafe impl crate::base::protocol::ProtocolInterface for HidIoProtocol {
    const PROTOCOL_GUID: crate::BinaryGuid = HID_IO_PROTOCOL_GUID;
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    extern "efiapi" fn get_report_descriptor(
        _this: *const HidIoProtocol,
        _report_descriptor_size: *mut usize,
        _report_descriptor_buffer: *mut c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn get_or_set_report(
        _this: *const HidIoProtocol,
        _report_id: u8,
        _report_type: HidReportType,
        _report_buffer_size: usize,
        _report_buffer: *mut c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn register_report_callback(
        _this: *const HidIoProtocol,
        callback: Option<HidIoReportCallback>,
        _context: *mut c_void,
    ) -> efi::Status {
        if callback.is_some() { efi::Status::SUCCESS } else { efi::Status::INVALID_PARAMETER }
    }

    extern "efiapi" fn unregister_report_callback(
        _this: *const HidIoProtocol,
        callback: Option<HidIoReportCallback>,
    ) -> efi::Status {
        if callback.is_some() { efi::Status::SUCCESS } else { efi::Status::INVALID_PARAMETER }
    }

    extern "efiapi" fn report_callback(_report_buffer_size: u16, _report_buffer: *mut c_void, _context: *mut c_void) {}

    #[test]
    fn test_hid_io_accepts_nullable_report_callbacks() {
        let protocol = HidIoProtocol {
            get_report_descriptor,
            get_report: get_or_set_report,
            set_report: get_or_set_report,
            register_report_callback,
            unregister_report_callback,
        };

        // SAFETY: The test stubs do not dereference pointers or invoke the callbacks.
        unsafe {
            assert_eq!(
                (protocol.register_report_callback)(&raw const protocol, None, core::ptr::null_mut()),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                (protocol.register_report_callback)(&raw const protocol, Some(report_callback), core::ptr::null_mut()),
                efi::Status::SUCCESS
            );
            assert_eq!(
                (protocol.unregister_report_callback)(&raw const protocol, None),
                efi::Status::INVALID_PARAMETER
            );
            assert_eq!(
                (protocol.unregister_report_callback)(&raw const protocol, Some(report_callback)),
                efi::Status::SUCCESS
            );
        }
    }
}
