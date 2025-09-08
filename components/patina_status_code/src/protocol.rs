use patina_sdk::{boot_services::StandardBootServices, uefi_protocol::ProtocolInterface};
use r_efi::efi;

use crate::{
    service::{RscHandler, StatusCodeType, StatusCodeValue},
    status_code::StandardRscHandler,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EfiStatusCodeHeader {
    pub header_size: u16,
    pub size: u16,
    pub data_type: efi::Guid,
}

// Register and unregister

// sherry: u probably need a global :(

pub(crate) static GLOBAL_RSC_HANDLER: StandardRscHandler<StandardBootServices> = StandardRscHandler::new_uninit();

pub(crate) type EfiRscHandlerCallback =
    extern "efiapi" fn(StatusCodeType, StatusCodeValue, u32, efi::Guid, &EfiStatusCodeHeader);
type EfiRscHandlerRegister = extern "efiapi" fn(EfiRscHandlerCallback, efi::Tpl) -> efi::Status;
type EfiRscHandlerUnregister = extern "efiapi" fn(EfiRscHandlerCallback) -> efi::Status;

#[repr(C)]
pub struct RscHandlerProtocol {
    pub register: EfiRscHandlerRegister,
    pub unregister: EfiRscHandlerUnregister,
}

unsafe impl ProtocolInterface for RscHandlerProtocol {
    const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x86212936, 0xe76, 0x41c8, 0xa0, 0x3a, &[0x2a, 0xf2, 0xfc, 0x1c, 0x39, 0xe2]);
}

impl RscHandlerProtocol {
    fn new() -> Self {
        Self { register: Self::rsc_handler_register, unregister: Self::rsc_handler_unregister }
    }

    extern "efiapi" fn rsc_handler_register(callback: EfiRscHandlerCallback, _tpl: efi::Tpl) -> efi::Status {
        let result = GLOBAL_RSC_HANDLER.register(
            crate::callback::RscHandlerCallback::Efi(callback),
            patina_sdk::boot_services::tpl::Tpl::APPLICATION,
        );
        match result {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }

    extern "efiapi" fn rsc_handler_unregister(callback: EfiRscHandlerCallback) -> efi::Status {
        let result = GLOBAL_RSC_HANDLER.unregister(crate::callback::RscHandlerCallback::Efi(callback));
        match result {
            Ok(()) => efi::Status::SUCCESS,
            Err(e) => e.into(),
        }
    }
}
