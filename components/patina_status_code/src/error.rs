use r_efi::efi;

#[derive(Debug, PartialEq, Eq)]
pub enum RscHandlerError {
    CallbackAlreadyRegistered,
    EventCreationFailed(efi::Status),
    UnregisterNotFound,
    MissingEvent, // tpl < tpl_high must have event. if not, error
    AlreadyInitialized,
    NotInitialized,
    ReentrantReportStatusCode,
}

impl From<RscHandlerError> for efi::Status {
    fn from(report_status_code_error: RscHandlerError) -> Self {
        match report_status_code_error {
            RscHandlerError::CallbackAlreadyRegistered => efi::Status::ALREADY_STARTED,
            RscHandlerError::EventCreationFailed(efi_status) => efi_status,
            RscHandlerError::UnregisterNotFound => efi::Status::NOT_FOUND,
            RscHandlerError::MissingEvent => efi::Status::NOT_FOUND,
            RscHandlerError::AlreadyInitialized => efi::Status::ALREADY_STARTED,
            RscHandlerError::NotInitialized => efi::Status::NOT_READY,
            RscHandlerError::ReentrantReportStatusCode => efi::Status::ALREADY_STARTED,
            _ => efi::Status::INVALID_PARAMETER,
        }
    }
}
