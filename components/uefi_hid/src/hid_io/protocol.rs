//! HidIo protocol re-exports and consumer-side helpers.
//!
//! The FFI types are defined in the [`patina::vendor_protocols::hid_io`] module. This module
//! re-exports them for internal use and provides helper functions that depend
//! on `hidparser` for parsing report descriptors.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
pub use patina::vendor_protocols::hid_io::{HID_IO_PROTOCOL_GUID, HidIoProtocol, HidIoReportCallback, HidReportType};

use alloc::vec;
use core::ffi::c_void;

use r_efi::efi;

use super::HidIo;
use hidparser::ReportDescriptor;

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

    // SAFETY: hid_io points to a valid HidIoProtocol; buffer and size are valid.
    match unsafe {
        (hid_io.get_report_descriptor)(
            hid_io as *const HidIoProtocol,
            &mut report_descriptor_size,
            buffer.as_mut_ptr() as *mut c_void,
        )
    } {
        efi::Status::SUCCESS => {
            buffer.truncate(report_descriptor_size);
        }
        efi::Status::BUFFER_TOO_SMALL => {
            buffer.resize(report_descriptor_size, 0);
            // SAFETY: hid_io points to a valid HidIoProtocol; buffer and size are valid.
            match unsafe {
                (hid_io.get_report_descriptor)(
                    hid_io as *const HidIoProtocol,
                    &mut report_descriptor_size,
                    buffer.as_mut_ptr() as *mut c_void,
                )
            } {
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
    // SAFETY: hid_io points to a valid HidIoProtocol; report buffer and size are valid.
    match unsafe {
        (hid_io.set_report)(
            hid_io as *const HidIoProtocol,
            id.unwrap_or(0),
            HidReportType::OutputReport,
            report.len(),
            report.as_ptr() as *mut c_void,
        )
    } {
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
