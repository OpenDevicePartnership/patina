use r_efi::efi;

use crate::error::ReportStatusCodeHandlerError;

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
pub(crate) type ReportStatusCodeHandlerCallback =
    fn(StatusCodeType, StatusCodeValue, u32, efi::Guid, &StatusCodeData) -> efi::Status;

pub(crate) struct StatusCodeData {
    pub header_size: u16,
    pub size: u16,
    pub data_type: efi::Guid,
}

pub trait ReportStatusCode {
    fn report_status_code(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: efi::Guid,
        data: &StatusCodeData,
    ) -> Result<(), ReportStatusCodeHandlerError>;
}

pub trait RuntimeStatusCode {
    fn register(
        &self,
        callback: ReportStatusCodeHandlerCallback,
        tpl: efi::Tpl,
    ) -> Result<(), ReportStatusCodeHandlerError>;

    fn unregister(&self, callback: ReportStatusCodeHandlerCallback) -> Result<(), ReportStatusCodeHandlerError>;
}
