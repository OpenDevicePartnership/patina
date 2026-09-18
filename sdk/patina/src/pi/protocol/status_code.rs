//! Status Code Protocol
//!
//! Provides the service required to report a status code to the platform firmware.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Runtime_Protocols.html#efi-status-code-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(feature = "alloc")]
use core::mem;
use core::ptr;
#[cfg(feature = "alloc")]
use core::slice;

use crate::standard::efi;

/// Status Code Runtime Protocol GUID.
pub const PROTOCOL_GUID: crate::BinaryGuid = crate::BinaryGuid::from_string("D2B2B828-0826-48A7-B3DF-983C006024F0");

/// Status Code Type Definition.
///
pub type EfiStatusCodeType = u32;

/// Status Code Value Definition.
///
pub type EfiStatusCodeValue = u32;

/// The definition of the status code extended data header. The data will follow `HeaderSize` bytes from the
/// beginning of the structure and is Size bytes long.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section III-6.6.2.1
#[repr(C)]
pub struct EfiStatusCodeData {
    /// Size of the status code data header.
    pub header_size: u16,
    /// Size of the status code data.
    pub size: u16,
    /// GUID identifying the type of status code data.
    pub r#type: efi::Guid,
}

/// Provides an interface that a software module can call to report a status code.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-14.2.1
pub type ReportStatusCode =
    extern "efiapi" fn(u32, u32, u32, *const efi::Guid, *const EfiStatusCodeData) -> efi::Status;

/// Provides the service required to report a status code to the platform firmware.
/// This protocol must be produced by a runtime DXE driver.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-14.2.1
#[repr(C)]
pub struct StatusCodeProtocol {
    /// Function to report status codes.
    pub report_status_code: ReportStatusCode,
}

// SAFETY: StatusCodeProtocol implements the UEFI Status Code protocol interface.
unsafe impl crate::base::protocol::ProtocolInterface for StatusCodeProtocol {
    const PROTOCOL_GUID: crate::BinaryGuid = PROTOCOL_GUID;
}

// Non-spec defined wrappers on top of the StatusCodeProtocol to make it easier to use in Rust.
impl StatusCodeProtocol {
    /// Reports a status code to the platform firmware with data.
    #[cfg(feature = "alloc")]
    pub fn report_status_code_with_data<T>(
        &self,
        status_code_type: EfiStatusCodeType,
        status_code_value: EfiStatusCodeValue,
        instance: u32,
        caller_id: &efi::Guid,
        data_type: efi::Guid,
        data: T,
    ) -> Result<(), efi::Status>
    where
        T: Sized,
    {
        let header = EfiStatusCodeData {
            header_size: mem::size_of::<EfiStatusCodeData>() as u16,
            size: mem::size_of::<T>() as u16,
            r#type: data_type,
        };

        let mut data_buffer = [any_as_u8_slice(&header), any_as_u8_slice(&data)].concat();
        let data_ptr: *mut EfiStatusCodeData = data_buffer.as_mut_ptr() as *mut EfiStatusCodeData;

        let status = (self.report_status_code)(status_code_type, status_code_value, instance, caller_id, data_ptr);

        if status.is_error() { Err(status) } else { Ok(()) }
    }

    /// Reports a status code to the platform firmware without data.
    pub fn report_status_code(
        &self,
        status_code_type: EfiStatusCodeType,
        status_code_value: EfiStatusCodeValue,
        instance: u32,
        caller_id: &efi::Guid,
    ) -> Result<(), efi::Status> {
        let status =
            (self.report_status_code)(status_code_type, status_code_value, instance, caller_id, ptr::null_mut());
        if status.is_error() { Err(status) } else { Ok(()) }
    }
}

#[cfg(feature = "alloc")]
fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    // SAFETY: P is a ref thus a valid pointer and since the type is sized, the memory boundary of this type is known.
    unsafe { slice::from_raw_parts(core::ptr::from_ref::<T>(p) as *const u8, mem::size_of::<T>()) }
}
