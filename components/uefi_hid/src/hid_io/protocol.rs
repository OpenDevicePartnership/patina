//! HidIo protocol FFI definitions.
//!
//! Defines the HidIo protocol interface struct and GUID.
//!
//! The underlying protocol is defined in
//! [HidIo.h](https://github.com/microsoft/mu_plus/blob/release/202502/HidPkg/Include/Protocol/HidIo.h).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use core::ffi::c_void;

use alloc::vec;
use r_efi::efi;

use super::HidIo;
use hidparser::ReportDescriptor;

/// HidIo interface GUID: 3EA93936-6BF4-49D6-AA50-D9F5B9AD8CFF
pub const HID_IO_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x3ea93936, 0x6bf4, 0x49d6, 0xaa, 0x50, &[0xd9, 0xf5, 0xb9, 0xad, 0x8c, 0xff]);

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
    extern "efiapi" fn(report_buffer_size: u16, report_buffer: *mut c_void, context: *mut c_void);

/// The HID_IO protocol FFI interface.
#[repr(C)]
pub struct HidIoProtocol {
    /// Retrieves the HID report descriptor from the device.
    pub get_report_descriptor: extern "efiapi" fn(
        this: *const HidIoProtocol,
        report_descriptor_size: *mut usize,
        report_descriptor_buffer: *mut c_void,
    ) -> efi::Status,
    /// Retrieves a HID report of the specified type from the device.
    pub get_report: extern "efiapi" fn(
        this: *const HidIoProtocol,
        report_id: u8,
        report_type: HidReportType,
        report_buffer_size: usize,
        report_buffer: *mut c_void,
    ) -> efi::Status,
    /// Sends a HID report of the specified type to the device.
    pub set_report: extern "efiapi" fn(
        this: *const HidIoProtocol,
        report_id: u8,
        report_type: HidReportType,
        report_buffer_size: usize,
        report_buffer: *mut c_void,
    ) -> efi::Status,
    /// Registers a callback for asynchronous input report notifications.
    pub register_report_callback: extern "efiapi" fn(
        this: *const HidIoProtocol,
        callback: HidIoReportCallback,
        context: *mut c_void,
    ) -> efi::Status,
    /// Unregisters a previously registered input report callback.
    pub unregister_report_callback:
        extern "efiapi" fn(this: *const HidIoProtocol, callback: HidIoReportCallback) -> efi::Status,
}

// SAFETY: HidIoProtocol is a C-compatible struct whose layout matches the HidIo GUID interface.
unsafe impl patina::uefi_protocol::ProtocolInterface for HidIoProtocol {
    const PROTOCOL_GUID: efi::Guid = HID_IO_PROTOCOL_GUID;
}

#[cfg(test)]
impl HidIoProtocol {
    /// Returns a stub protocol with no-op function pointers for testing.
    #[coverage(off)]
    pub fn stub() -> &'static mut Self {
        extern "efiapi" fn get_report_descriptor(
            _this: *const HidIoProtocol,
            report_descriptor_size: *mut usize,
            _report_descriptor_buffer: *mut c_void,
        ) -> efi::Status {
            // SAFETY: report_descriptor_size is a valid pointer provided by the caller in the test stub.
            unsafe { *report_descriptor_size = 0 };
            efi::Status::BUFFER_TOO_SMALL
        }
        extern "efiapi" fn get_report(
            _this: *const HidIoProtocol,
            _report_id: u8,
            _report_type: HidReportType,
            _report_buffer_size: usize,
            _report_buffer: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn set_report(
            _this: *const HidIoProtocol,
            _report_id: u8,
            _report_type: HidReportType,
            _report_buffer_size: usize,
            _report_buffer: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn register_report_callback(
            _this: *const HidIoProtocol,
            _callback: HidIoReportCallback,
            _context: *mut c_void,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn unregister_report_callback(
            _this: *const HidIoProtocol,
            _callback: HidIoReportCallback,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }

        let protocol = HidIoProtocol {
            get_report_descriptor,
            get_report,
            set_report,
            register_report_callback,
            unregister_report_callback,
        };
        // SAFETY: Leaked for 'static lifetime in tests.
        unsafe { Box::into_raw(Box::new(protocol)).as_mut().unwrap() }
    }
}

/// Initial buffer size for `get_report_descriptor`. Covers virtually all real
/// devices in a single transfer; larger descriptors fall back to a second call.
const INITIAL_REPORT_DESCRIPTOR_SIZE: usize = 4096;

/// Retrieves and parses the report descriptor via the HidIo protocol.
///
/// Tries an initial 4KB buffer first. If the device returns `BUFFER_TOO_SMALL`,
/// re-allocates to the exact required size and retries.
fn get_report_descriptor_impl(hid_io: &HidIoProtocol) -> Result<ReportDescriptor, efi::Status> {
    let mut report_descriptor_size = INITIAL_REPORT_DESCRIPTOR_SIZE;
    let mut buffer = vec![0u8; report_descriptor_size];

    match (hid_io.get_report_descriptor)(
        hid_io as *const HidIoProtocol,
        &mut report_descriptor_size,
        buffer.as_mut_ptr() as *mut c_void,
    ) {
        efi::Status::SUCCESS => {
            buffer.truncate(report_descriptor_size);
        }
        efi::Status::BUFFER_TOO_SMALL => {
            buffer.resize(report_descriptor_size, 0);
            match (hid_io.get_report_descriptor)(
                hid_io as *const HidIoProtocol,
                &mut report_descriptor_size,
                buffer.as_mut_ptr() as *mut c_void,
            ) {
                efi::Status::SUCCESS => {
                    buffer.truncate(report_descriptor_size);
                }
                err => return Err(err),
            }
        }
        err => return Err(err),
    }

    hidparser::parse_report_descriptor(&buffer).map_err(|_| efi::Status::DEVICE_ERROR)
}

/// Sends an output report through the HidIo protocol.
fn set_output_report_impl(hid_io: &HidIoProtocol, id: Option<u8>, report: &[u8]) -> Result<(), efi::Status> {
    match (hid_io.set_report)(
        hid_io as *const HidIoProtocol,
        id.unwrap_or(0),
        HidReportType::OutputReport,
        report.len(),
        report.as_ptr() as *mut c_void,
    ) {
        efi::Status::SUCCESS => Ok(()),
        err => Err(err),
    }
}

impl HidIo for HidIoProtocol {
    fn get_report_descriptor(&self) -> Result<ReportDescriptor, efi::Status> {
        get_report_descriptor_impl(self)
    }

    fn set_output_report(&self, id: Option<u8>, report: &[u8]) -> Result<(), efi::Status> {
        set_output_report_impl(self, id, report)
    }
}
