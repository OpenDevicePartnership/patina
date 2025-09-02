use core::mem;

use alloc::boxed::Box;
use patina_sdk::{
    boot_services::tpl,
    component::service::{IntoService, Service},
};
use r_efi::efi;

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

pub trait RscHandler {
    fn register(&self, callback: RscHandlerCallback, tpl: tpl::Tpl) -> Result<(), RscHandlerError>;

    fn unregister(&self, callback: RscHandlerCallback) -> Result<(), RscHandlerError>;
}
