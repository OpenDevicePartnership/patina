use r_efi::efi;

use crate::{
    error::ReportStatusCodeError,
    service::{
        ReportStatusCode, ReportStatusCodeHandlerCallback, RscHandler, RuntimeStatusCode, StatusCodeData,
        StatusCodeType, StatusCodeValue,
    },
};

use patina_sdk::{
    boot_services::{BootServices, StandardBootServices},
    component::service::IntoService,
    uefi_protocol::status_code::StatusCodeRuntimeProtocol,
};

#[derive(IntoService)]
#[service(dyn RuntimeStatusCode)]
struct StandardRuntimeStatusCodeHandler {}

impl RuntimeStatusCode for StandardRuntimeStatusCodeHandler {
    fn register(&self, callback: ReportStatusCodeHandlerCallback, tpl: efi::Tpl) -> Result<(), ReportStatusCodeError> {
        // Implementation for registering the callback
        Ok(())
    }

    fn unregister(&self, callback: ReportStatusCodeHandlerCallback) -> Result<(), ReportStatusCodeError> {
        // Implementation for unregistering the callback
        Ok(())
    }
}

#[derive(IntoService)]
#[service(dyn RscHandler)]
struct StandardReportStatusCodeHandler<B: BootServices + 'static> {
    boot_services: B,
}

impl<B> StandardReportStatusCodeHandler<B>
where
    B: BootServices,
{
    fn new(boot_services: B) -> Self {
        StandardReportStatusCodeHandler { boot_services }
    }
}

impl<B> RscHandler for StandardReportStatusCodeHandler<B>
where
    B: BootServices,
{
    fn report_status_code(
        &self,
        code_type: StatusCodeType,
        value: StatusCodeValue,
        instance: u32,
        caller_id: Option<&efi::Guid>,
        data_type: Option<efi::Guid>,
        data: &StatusCodeData,
    ) -> Result<(), ReportStatusCodeError> {
        let protocol_exists = unsafe { self.boot_services.locate_protocol::<StatusCodeRuntimeProtocol>(None) };
        match protocol_exists {
            Ok(protocol) => {
                let result = protocol.report_status_code(
                    code_type,
                    value,
                    instance,
                    caller_id.unwrap(),
                    data_type.unwrap(),
                    data,
                );
                match result {
                    Ok(_) => Ok(()),
                    Err(status) => Err(ReportStatusCodeError::ProtocolFailed(status)),
                }
            }
            Err(_) => Err(ReportStatusCodeError::ProtocolNotFound),
        }
    }
}
