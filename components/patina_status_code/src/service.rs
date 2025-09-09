use core::mem;

use alloc::boxed::Box;
use patina_sdk::{
    boot_services::tpl,
    component::service::{IntoService, Service},
};
use r_efi::efi::{self, Status};

use crate::{callback::RscHandlerCallback, error::RscHandlerError, protocol::EfiStatusCodeHeader};

// typedef
// EFI_STATUS
// (EFIAPI *EFI_RSC_HANDLER_CALLBACK) (
//   IN EFI_STATUS_CODE_TYPE     CodeType,
//   IN EFI_STATUS_CODE_VALUE    Value,
//   IN UINT32                   Instance,
//   IN EFI_GUID                 *CallerId,
//   IN EFI_STATUS_CODE_DATA     *Data
//   );
pub(crate) type StatusCodeType = u32;
pub(crate) type StatusCodeValue = u32;

// sherry: i am so worried about this and don't think it's the best idea tbh
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StatusCodeData {
    pub data_header: EfiStatusCodeHeader,
    pub data_bytes: Box<[u8]>,
}

pub trait RscHandler {
    fn register_callback(&self, callback: RscHandlerCallback, tpl: tpl::Tpl) -> Result<(), RscHandlerError>;

    fn unregister_callback(&self, callback: RscHandlerCallback) -> Result<(), RscHandlerError>;

    fn report_status_code(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: Option<efi::Guid>,
        data_header: Option<StatusCodeData>,
    ) -> Result<(), RscHandlerError>;
}
