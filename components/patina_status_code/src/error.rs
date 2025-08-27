use r_efi::efi;

pub enum RscHandlerError {
    CallbackAlreadyRegistered,
    EventCreationFailed(efi::Status),
    UnregisterNotFound,
    MissingEvent, // tpl < tpl_high must have event. if not, error
}

impl From<RscHandlerError> for efi::Status {
    fn from(report_status_code_error: RscHandlerError) -> Self {
        match report_status_code_error {
            RscHandlerError::CallbackAlreadyRegistered => efi::Status::ALREADY_STARTED,
            RscHandlerError::EventCreationFailed(efi_status) => efi_status,
            RscHandlerError::UnregisterNotFound => efi::Status::NOT_FOUND,
            RscHandlerError::MissingEvent => efi::Status::NOT_FOUND,
            _ => efi::Status::INVALID_PARAMETER,
        }
    }
}
