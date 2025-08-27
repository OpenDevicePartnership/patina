use r_efi::efi;

pub enum RscHandlerError {
    CallbackAlreadyRegistered,
    EventCreationFailed(efi::Status),
}

impl From<RscHandlerError> for efi::Status {
    fn from(report_status_code_error: RscHandlerError) -> Self {
        match report_status_code_error {
            RscHandlerError::CallbackAlreadyRegistered => efi::Status::ALREADY_STARTED,
            RscHandlerError::EventCreationFailed(efi_status) => efi_status,
            _ => efi::Status::INVALID_PARAMETER,
        }
    }
}
