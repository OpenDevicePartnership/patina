use core::mem;

use alloc::boxed::Box;
use patina_sdk::{
    boot_services::tpl,
    component::service::{IntoService, Service},
};
use r_efi::efi;

use crate::{error::RscHandlerError, protocol::EfiStatusCodeHeader};

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
// SHERRY: this may be problematic for FFI
pub(crate) type RscHandlerCallback =
    fn(StatusCodeType, StatusCodeValue, u32, efi::Guid, &EfiStatusCodeHeader) -> efi::Status;

pub trait RscHandler {
    fn register(&mut self, callback: RscHandlerCallback, tpl: tpl::Tpl) -> Result<(), RscHandlerError>;

    fn unregister(&mut self, callback: RscHandlerCallback) -> Result<(), RscHandlerError>;
}
