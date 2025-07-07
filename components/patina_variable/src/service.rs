use patina_sdk::{
    component::service::{IntoService, Service},
    runtime_services::variable_services::{GetVariableStatus, VariableInfo},
};
use r_efi::efi;

use crate::{error::VariableError, variable};

#[derive(IntoService)]
#[service(VariableServices)]
pub struct VariableServices {
    pub(crate) storage_provider: Service<dyn VariableStorage>,
}

impl VariableServices {
    fn get_variable<T>(
        &self,
        name: &str,
        namespace: &efi::Guid,
        size_hint: Option<usize>,
    ) -> Result<(&T, u32), VariableError>
    where
        T: 'static,
    {
        let (var_bytes, var_attr) = self.storage_provider.get_variable(name, namespace, size_hint)?;
        let typed_var = var_bytes.as_ptr() as *const T;
        Ok((unsafe { &*typed_var }, var_attr))
    }

    fn iter_variable_names(&self) -> Result<Vec<String>, VariableError> {
        self.storage_provider.iter_variable_names()
    }

    // SHERrY: i don't think this is actually the best solution for T but idk what is
    fn set_variable<T>(&self, name: &str, namespace: &efi::Guid, attributes: u32, data: &T) -> Result<(), VariableError>
    where
        T: AsRef<[u8]> + 'static,
    {
        self.storage_provider.set_variable(name, namespace, attributes, data.as_ref())
    }

    fn query_variables_info(&self, attributes: u32) -> Result<VariableInfo, VariableError> {
        self.storage_provider.query_variables_info(attributes)
    }
}

pub trait VariableStorage {
    fn get_variable(
        &self,
        name: &str,
        namespace: &efi::Guid,
        size_hint: Option<usize>,
    ) -> Result<(&[u8], u32), VariableError>;

    fn iter_variable_names(&self) -> Result<Vec<String>, VariableError>;

    fn set_variable(
        &self,
        name: &str,
        namespace: &efi::Guid,
        attributes: u32,
        data: &[u8],
    ) -> Result<(), VariableError>;

    fn query_variables_info(&self, attributes: u32) -> Result<VariableInfo, VariableError>;
}
