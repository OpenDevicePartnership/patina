use r_efi::efi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    AllocationFailed,
    FacsUefiNot64BAligned,
    InvalidSignature,
    FadtAlreadyInstalled,
    InstallTableFailed,
    InvalidTableKey,
    InvalidTableIndex,
    InvalidNotifyUnregister,
    FreeFailed,
    XsdtNotInitialized,
    InvalidTableFormat,
    HobTableNotInstalled,
    InvalidTableLength,
    InvalidXsdtEntry,
    TableNotifyFailed,
    NullRsdpFromHob,
    XsdtNotInitializedFromHob,
}

impl Into<efi::Status> for AcpiError {
    fn into(self) -> efi::Status {
        match self {
            AcpiError::AllocationFailed => efi::Status::OUT_OF_RESOURCES,
            AcpiError::FacsUefiNot64BAligned => efi::Status::UNSUPPORTED,
            AcpiError::InvalidSignature => efi::Status::INVALID_PARAMETER,
            AcpiError::FadtAlreadyInstalled => efi::Status::ALREADY_STARTED,
            AcpiError::InstallTableFailed => efi::Status::UNSUPPORTED,
            AcpiError::InvalidTableKey => efi::Status::INVALID_PARAMETER,
            AcpiError::InvalidTableIndex => efi::Status::INVALID_PARAMETER,
            AcpiError::InvalidNotifyUnregister => efi::Status::INVALID_PARAMETER,
            AcpiError::FreeFailed => efi::Status::OUT_OF_RESOURCES,
            AcpiError::XsdtNotInitialized => efi::Status::UNSUPPORTED,
            AcpiError::InvalidTableFormat => efi::Status::INVALID_PARAMETER,
            AcpiError::HobTableNotInstalled => efi::Status::UNSUPPORTED,
            AcpiError::InvalidTableLength => efi::Status::INVALID_PARAMETER,
            AcpiError::InvalidXsdtEntry => efi::Status::INVALID_PARAMETER,
            AcpiError::TableNotifyFailed => efi::Status::INVALID_PARAMETER,
            AcpiError::NullRsdpFromHob => efi::Status::NOT_FOUND,
            AcpiError::XsdtNotInitializedFromHob => efi::Status::NOT_FOUND,
        }
    }
}
