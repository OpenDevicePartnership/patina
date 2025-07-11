use alloc::{string::String, vec::Vec};
use patina_sdk::{
    component::service::{IntoService, Service},
    runtime_services::variable_services::{GetVariableStatus, VariableInfo},
};
use r_efi::efi;

use crate::{error::VariableError, variable};

use core::ffi::c_void;

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
    ) -> Result<(&T, u32), VariableError> {
        let (var_bytes, var_attr) = self.storage_provider.get_variable(name, namespace, size_hint)?;
        let typed_var = var_bytes.as_ptr() as *const T;
        Ok((unsafe { &*typed_var }, var_attr))
    }

    fn iter_variable_names(&self) -> Result<Vec<String>, VariableError> {
        self.storage_provider.iter_variable_names()
    }

    fn set_variable<T>(
        &self,
        name: &str,
        namespace: &efi::Guid,
        attributes: u32,
        data: &mut T,
    ) -> Result<(), VariableError> {
        let data_ptr: *mut c_void = data as *mut T as *mut c_void;
        // SHERRY tangent: i don't hate the idea of using bytemuck + Pod here
        // the requirement of Pod are 1) repr(C) + 2) Copy - which we are basically requiring by doing the pointer conversion above
        // so it doesn't add any additional requirements on T
        // and makes the VariableStorage.set_variable a bit nicer
        self.storage_provider.set_variable(name, namespace, attributes, data_ptr)
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
        data: *mut c_void,
    ) -> Result<(), VariableError>;

    fn query_variables_info(&self, attributes: u32) -> Result<VariableInfo, VariableError>;
}
