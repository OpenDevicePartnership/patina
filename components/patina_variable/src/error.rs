use r_efi::efi;

pub enum VariableError {
    BufferTooSmall,
    InvalidUtf16Name,
}

impl From<efi::Status> for VariableError {
    fn from(status: efi::Status) -> Self {
        match status {
            efi::Status::BUFFER_TOO_SMALL => VariableError::BufferTooSmall,
            efi::Status::INVALID_PARAMETER => VariableError::InvalidUtf16Name,
            _ => panic!("Unexpected EFI status: {:?}", status),
        }
    }
}
