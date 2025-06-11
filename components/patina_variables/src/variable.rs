use patina_sdk::component::service::IntoService;
use spin::mutex::Mutex;

use crate::error::VariableServiceError;
use crate::service::{VariableAttributes, VariableService};
use crate::variable_store::VariableModule;

#[derive(IntoService)]
#[service(StandardVariableService)]
pub(crate) struct StandardVariableService {
    variable_module: Mutex<VariableModule>,
}

impl VariableService for StandardVariableService {
    fn get_variable<T>(&self, variable_name: String, guid: r_efi::efi::Guid) -> Result<(T, u32), VariableServiceError> {
    let variable_module = self.variable_module.lock();
        Err(VariableServiceError::GenericError)
    }

    fn set_variable<T>(
        &self,
        variable_name: String,
        guid: r_efi::efi::Guid,
        attributes: VariableAttributes,
        data: &T,
    ) -> Result<(), VariableServiceError> {
        // Implementation goes here
        unimplemented!()
    }
}
