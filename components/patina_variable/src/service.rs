use patina_sdk::{
    component::service::{IntoService, Service},
    runtime_services::variable_services::GetVariableStatus,
};
use r_efi::efi;

use crate::{error::VariableError, variable};

#[derive(IntoService)]
#[service(VariableServices)]
pub struct VariableServices {
    storage_provider: Service<dyn VariableStorage>,
}

impl VariableServices {
    fn get_variable<T>(
        &self,
        name: &str,
        namespace: &efi::Guid,
        size_hint: Option<usize>,
    ) -> Result<(T, u32), VariableError>
    where
        T: Copy + 'static,
    {
        let (var_bytes, var_attr) = self.storage_provider.get_variable(name, namespace, size_hint)?;
        let typed_var = var_bytes.as_ptr() as *const T;
        Ok((unsafe { *typed_var }, var_attr))
    }
}

pub trait VariableStorage {
    fn get_variable(
        &self,
        name: &str,
        namespace: &efi::Guid,
        data_size: Option<usize>,
    ) -> Result<(&[u8], u32), VariableError>;
}
