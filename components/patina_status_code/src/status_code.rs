use r_efi::efi;

use crate::{
    error::ReportStatusCodeHandlerError,
    service::{
        ReportStatusCode, ReportStatusCodeHandlerCallback, RuntimeStatusCode, StatusCodeData, StatusCodeType,
        StatusCodeValue,
    },
};
struct RuntimeStatusCodeHandler {}

impl RuntimeStatusCode for RuntimeStatusCodeHandler {
    fn register(
        &self,
        callback: ReportStatusCodeHandlerCallback,
        tpl: efi::Tpl,
    ) -> Result<(), ReportStatusCodeHandlerError> {
        // Implementation for registering the callback
        Ok(())
    }

    fn unregister(&self, callback: ReportStatusCodeHandlerCallback) -> Result<(), ReportStatusCodeHandlerError> {
        // Implementation for unregistering the callback
        Ok(())
    }
}
struct ReportStatusCodeHandler {}

impl ReportStatusCode for ReportStatusCodeHandler {
    fn report_status_code(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: efi::Guid,
        data: &StatusCodeData,
    ) -> Result<(), ReportStatusCodeHandlerError> {
        // Implementation for reporting the status code
        Ok(())
    }
}
