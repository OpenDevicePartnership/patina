use core::mem;

use alloc::boxed::Box;
use patina_sdk::component::service::{IntoService, Service};
use r_efi::efi;

use crate::{error::ReportStatusCodeError, protocol::EfiStatusCodeHeader};

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
    fn(StatusCodeType, StatusCodeValue, u32, efi::Guid, &EfiStatusCodeHeader) -> efi::Status;

pub(crate) struct StatusCodeData {
    header: EfiStatusCodeHeader,
    data: Box<[u8]>,
}

// SHERRY: idk if i like this naming scheme
#[derive(IntoService)]
#[service(ReportStatusCode)]
pub struct ReportStatusCode {
    rsc_handler: Service<dyn RscHandler>,
}

impl ReportStatusCode {
    pub fn new(rsc_handler: Service<dyn RscHandler>) -> Self {
        ReportStatusCode { rsc_handler }
    }

    fn report_status_code<T>(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: Option<&efi::Guid>,
        data_type: Option<efi::Guid>,
        data: Option<T>,
    ) -> Result<(), ReportStatusCodeError> {
        match data {
            Some(d) => {
                let data_type = match data_type {
                    Some(dt) => dt,
                    None => return Err(ReportStatusCodeError::MissingDataType),
                };
                let header = EfiStatusCodeHeader {
                    header_size: core::mem::size_of::<EfiStatusCodeHeader>() as u16,
                    size: core::mem::size_of::<T>() as u16,
                    data_type: data_type,
                };
                // SHERRY: investigate if zerocopy can do this safely
                let data_bytes =
                    unsafe { core::slice::from_raw_parts(&d as *const T as *const u8, core::mem::size_of::<T>()) };
                let status_code_data = StatusCodeData { header, data: data_bytes.to_vec().into_boxed_slice() };
                self.rsc_handler.report_status_code(
                    code_type,
                    value,
                    instance,
                    caller_id,
                    Some(header.data_type),
                    &status_code_data,
                )
            }
            // No payload provided.
            None => {
                const ZERO_GUID: efi::Guid = efi::Guid::from_bytes(&[0; 16]);
                let header = EfiStatusCodeHeader {
                    header_size: mem::size_of::<EfiStatusCodeHeader>() as u16,
                    size: 0,
                    data_type: ZERO_GUID,
                };
                let status_code_data = StatusCodeData { header, data: Box::new([]) };
                self.rsc_handler.report_status_code(code_type, value, instance, caller_id, None, &status_code_data)
            }
        }
    }
}

pub(crate) trait RscHandler {
    fn report_status_code(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: Option<&efi::Guid>,
        data_type: Option<efi::Guid>,
        data: &StatusCodeData,
    ) -> Result<(), ReportStatusCodeError>;
}

pub trait RuntimeStatusCode {
    fn register(&self, callback: ReportStatusCodeHandlerCallback, tpl: efi::Tpl) -> Result<(), ReportStatusCodeError>;

    fn unregister(&self, callback: ReportStatusCodeHandlerCallback) -> Result<(), ReportStatusCodeError>;
}
