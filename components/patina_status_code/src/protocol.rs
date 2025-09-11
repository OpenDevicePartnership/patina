use alloc::{borrow::ToOwned, boxed::Box, vec::Vec};
use patina_sdk::{boot_services::StandardBootServices, uefi_protocol::ProtocolInterface};
use r_efi::efi;

use crate::{
    service::{RscHandler, StatusCodeData, StatusCodeType, StatusCodeValue},
    status_code::StandardRscHandler,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EfiStatusCodeHeader {
    pub header_size: u16,
    /// Data size in bytes, excluding the header size itself.
    pub data_size: u16,
    pub data_type: efi::Guid,
}

impl EfiStatusCodeHeader {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.header_size.to_ne_bytes());
        buf.extend_from_slice(&self.data_size.to_ne_bytes());
        buf.extend_from_slice(self.data_type.as_bytes());
        buf
    }
}

// Register and unregister

pub(crate) static GLOBAL_RSC_HANDLER: StandardRscHandler<StandardBootServices> = StandardRscHandler::new_uninit();

pub(crate) type EfiRscHandlerCallback = extern "efiapi" fn(
    StatusCodeType,
    StatusCodeValue,
    u32,
    *const efi::Guid,
    *const EfiStatusCodeHeader,
) -> efi::Status;
type EfiRscHandlerRegister = extern "efiapi" fn(EfiRscHandlerCallback, efi::Tpl) -> efi::Status;
type EfiRscHandlerUnregister = extern "efiapi" fn(EfiRscHandlerCallback) -> efi::Status;
type EfiReportStatusCode = extern "efiapi" fn(
    StatusCodeType,
    StatusCodeValue,
    u32,
    *const efi::Guid,
    *const EfiStatusCodeHeader,
) -> efi::Status;

#[repr(C)]
pub struct RscHandlerProtocol {
    pub register: EfiRscHandlerRegister,
    pub unregister: EfiRscHandlerUnregister,
}

unsafe impl ProtocolInterface for RscHandlerProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x86212936, 0xe76, 0x41c8, 0xa0, 0x3a, &[0x2a, 0xf2, 0xfc, 0x1c, 0x39, 0xe2]);
}

#[repr(C)]
pub struct StatusCodeProtocol {
    pub report_status_code: EfiReportStatusCode,
}

unsafe impl ProtocolInterface for StatusCodeProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xd2b2b828, 0x826, 0x48a7, 0xb3, 0xdf, &[0x98, 0x3c, 0x0, 0x60, 0x24, 0xf0]);
}

impl RscHandlerProtocol {
    fn new() -> Self {
        Self { register: Self::rsc_handler_register, unregister: Self::rsc_handler_unregister }
    }

    extern "efiapi" fn rsc_handler_register(callback: EfiRscHandlerCallback, _tpl: efi::Tpl) -> efi::Status {
        let result = GLOBAL_RSC_HANDLER.register_callback(
            crate::callback::RscHandlerCallback::Efi(callback),
            patina_sdk::boot_services::tpl::Tpl::APPLICATION,
        );
        match result {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }

    extern "efiapi" fn rsc_handler_unregister(callback: EfiRscHandlerCallback) -> efi::Status {
        let result = GLOBAL_RSC_HANDLER.unregister_callback(crate::callback::RscHandlerCallback::Efi(callback));
        match result {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }
}

impl StatusCodeProtocol {
    fn new() -> Self {
        Self { report_status_code: Self::report_status_code }
    }

    extern "efiapi" fn report_status_code(
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: *const efi::Guid,
        data_header: *const EfiStatusCodeHeader,
    ) -> efi::Status {
        let caller_id_param = if caller_id.is_null() { None } else { Some(unsafe { *caller_id }) };
        let status_code_data_param = match data_header.is_null() {
            true => None,
            false => Some(StatusCodeData {
                data_header: unsafe { *data_header },
                data_bytes: unsafe {
                    core::slice::from_raw_parts(
                        (data_header as *const u8).add((*data_header).header_size as usize),
                        (*data_header).data_size as usize,
                    )
                    .to_owned()
                    .into_boxed_slice()
                },
            }),
        };
        match GLOBAL_RSC_HANDLER.report_status_code(code_type, value, instance, caller_id_param, status_code_data_param)
        {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }
}
