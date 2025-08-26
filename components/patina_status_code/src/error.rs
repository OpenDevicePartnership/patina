use r_efi::efi;

pub enum ReportStatusCodeError {
    MissingDataType,
    ProtocolFailed(efi::Status),
    ProtocolNotFound,
}

impl From<ReportStatusCodeError> for efi::Status {
    fn from(report_status_code_error: ReportStatusCodeError) -> Self {
        match report_status_code_error {
            ReportStatusCodeError::MissingDataType => efi::Status::INVALID_PARAMETER,
            ReportStatusCodeError::ProtocolFailed(status) => status,
            ReportStatusCodeError::ProtocolNotFound => efi::Status::NOT_FOUND,
            _ => efi::Status::UNSUPPORTED,
        }
    }
}

pub enum RuntimeStatusCodeError {
    CallbackAlreadyRegistered,
    CallbackNotRegistered,
    InvalidCallback,
    InvalidTpl,
}
