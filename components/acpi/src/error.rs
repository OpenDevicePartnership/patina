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
}
