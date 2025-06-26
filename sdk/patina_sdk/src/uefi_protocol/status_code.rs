//! Status Code Protocol
//!
//! Provides the protocol required to report a status code to the platform firmware.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Runtime_Protocols.html#efi-status-code-protocol>
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

extern crate alloc;

use core::{mem, slice};

use r_efi::efi;

use mu_pi::protocols::status_code::{self, EfiStatusCodeData, EfiStatusCodeType, EfiStatusCodeValue, ReportStatusCode};

use super::ProtocolInterface;

/// Rust definition of the UEFI Status Code Protocol.
///
/// <https://uefi.org/specs/PI/1.9/V2_DXE_Runtime_Protocols.html#status-code-runtime-protocol>
#[repr(transparent)]
pub struct StatusCodeRuntimeProtocol {
    protocol: status_code::Protocol,
}

unsafe impl ProtocolInterface for StatusCodeRuntimeProtocol {
    const PROTOCOL_GUID: efi::Guid = status_code::PROTOCOL_GUID;
}

impl StatusCodeRuntimeProtocol {
    /// Creates a new instance of the Status Code Runtime Protocol with the given implementation.
    pub fn new(report_status_code: ReportStatusCode) -> Self {
        Self { protocol: status_code::Protocol { report_status_code } }
    }

    /// Reports a status code to the platform firmware.
    pub fn report_status_code<T>(
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

        let status =
            (self.protocol.report_status_code)(status_code_type, status_code_value, instance, caller_id, data_ptr);

        if status.is_error() {
            Err(status)
        } else {
            Ok(())
        }
    }
}

fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    // SAFETY: P is a ref thus a valid pointer and since the type is sized, the memory boundary of this type is known.
    unsafe { slice::from_raw_parts((p as *const T) as *const u8, mem::size_of::<T>()) }
}
