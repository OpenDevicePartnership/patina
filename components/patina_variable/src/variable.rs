use patina_sdk::{component::service::IntoService, runtime_services::StandardRuntimeServices};

use crate::{error::VariableError, service::VariableStorage};

#[derive(IntoService)]
#[service(dyn VariableStorage)]
/// A simple variable services wrapper around the C protocols.
pub(crate) struct SimpleVariableStorage {
    pub(crate) runtime_services: StandardRuntimeServices,
}

impl VariableStorage for SimpleVariableStorage {
    fn get_variable(
        &self,
        name: &str,
        namespace: &r_efi::efi::Guid,
        data_size: Option<usize>,
    ) -> Result<(&[u8], u32), VariableError> {
        todo!()
    }

    fn iter_variable_names(&self) -> Result<Vec<String>, VariableError> {
        todo!()
    }

    fn set_variable(
        &self,
        name: &str,
        namespace: &r_efi::efi::Guid,
        attributes: u32,
        data: &[u8],
    ) -> Result<(), VariableError> {
        todo!()
    }

    fn query_variables_info(
        &self,
        attributes: u32,
    ) -> Result<patina_sdk::runtime_services::variable_services::VariableInfo, VariableError> {
        todo!()
    }
}
