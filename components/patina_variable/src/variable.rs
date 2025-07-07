use crate::{error::VariableError, service::VariableStorage};

/// A simple variable services wrapper around the C protocols.
pub(crate) struct SimpleVariableStorage {}

impl VariableStorage for SimpleVariableStorage {
    fn get_variable(
        &self,
        name: &str,
        namespace: &r_efi::efi::Guid,
        data_size: Option<usize>,
    ) -> Result<(&[u8], u32), VariableError> {
        todo!()
    }
}
