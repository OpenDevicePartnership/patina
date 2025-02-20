//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

extern crate alloc;

use core::{mem, ops::Deref, slice};

use alloc::vec::Vec;

use mu_pi::protocols::{
    status_code,
    status_code::{EfiStatusCodeData, EfiStatusCodeType, EfiStatusCodeValue},
};
use r_efi::efi;
use uefi_sdk::{boot_services::BootServices, protocol::Protocol};

pub struct StatusCodeRuntimeProtocol;

impl Deref for StatusCodeRuntimeProtocol {
    type Target = efi::Guid;

    fn deref(&self) -> &Self::Target {
        self.protocol_guid()
    }
}

unsafe impl Protocol for StatusCodeRuntimeProtocol {
    type Interface = status_code::Protocol;

    fn protocol_guid(&self) -> &'static efi::Guid {
        &status_code::PROTOCOL_GUID
    }
}

fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    // SAFETY: P is a ref thus a valid pointer and since the type is sized, the memory boundary of this type is known.
    unsafe { slice::from_raw_parts((p as *const T) as *const u8, mem::size_of::<T>()) }
}

/// Rust interface for Report Status Code
pub trait ReportStatusCode {
    fn report_status_code<T, B: BootServices>(
        boot_services: &B,
        status_code_type: EfiStatusCodeType,
        status_code_value: EfiStatusCodeValue,
        instance: u32,
        caller_id: Option<&efi::Guid>,
        data_type: efi::Guid,
        data: T,
    ) -> Result<(), efi::Status>;
}

impl ReportStatusCode for StatusCodeRuntimeProtocol {
    fn report_status_code<T, B: BootServices>(
        boot_services: &B,
        status_code_type: EfiStatusCodeType,
        status_code_value: EfiStatusCodeValue,
        instance: u32,
        caller_id: Option<&efi::Guid>,
        data_type: efi::Guid,
        data: T,
    ) -> Result<(), efi::Status> {
        let protocol = unsafe { boot_services.locate_protocol(&StatusCodeRuntimeProtocol, None)? };

        let header_size = mem::size_of::<EfiStatusCodeData>();
        let data_size = mem::size_of::<T>();

        let header = EfiStatusCodeData { header_size: header_size as u16, size: data_size as u16, r#type: data_type };

        let mut data_buffer = Vec::from(any_as_u8_slice(&header));
        data_buffer.extend(any_as_u8_slice(&data));

        let data_ptr: *mut EfiStatusCodeData = data_buffer.as_mut_ptr() as *mut EfiStatusCodeData;

        let caller_id = caller_id.unwrap_or(&mu_rust_helpers::guid::CALLER_ID);

        let status = (protocol.report_status_code)(status_code_type, status_code_value, instance, caller_id, data_ptr);

        if status.is_error() {
            Err(status)
        } else {
            Ok(())
        }
    }
}
