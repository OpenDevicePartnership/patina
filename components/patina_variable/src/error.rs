use r_efi::efi;

pub enum VariableError {
    BufferTooSmall,
}

impl From<efi::Status> for VariableError {
    fn from(status: efi::Status) -> Self {
        match status {
            efi::Status::BUFFER_TOO_SMALL => VariableError::BufferTooSmall,
            _ => panic!("Unexpected EFI status: {:?}", status),
        }
    }
}