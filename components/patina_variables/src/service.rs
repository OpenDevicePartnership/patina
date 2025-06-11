use r_efi::efi;

use crate::error::VariableServiceError;

pub trait VariableService {
    fn get_variable<T>(&self, variable_name: String, guid: efi::Guid) -> Result<(T, u32), VariableServiceError>;
    fn set_variable<T>(
        &self,
        variable_name: String,
        guid: efi::Guid,
        attributes: VariableAttributes,
        data: &T,
    ) -> Result<(), VariableServiceError>;
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VariableAttributes: u32 {
        const NON_VOLATILE = 0x00000001;
        const BOOTSERVICE_ACCESS = 0x00000002;
        const RUNTIME_ACCESS = 0x00000004;
        const HARDWARE_ERROR_RECORD = 0x00000008;
        const AUTHENTICATED_WRITE_ACCESS = 0x00000010;
        const TIME_BASED_AUTHENTICATED_WRITE_ACCESS = 0x00000020;
        const APPEND_WRITE_ACCESS = 0x00000040;
        const ENHANCED_AUTHENTICATED_ACCESS = 0x00000080;
    }
}
