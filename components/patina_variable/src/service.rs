use alloc::{string::String, vec::Vec};
use patina_sdk::{
    component::service::{IntoService, Service},
    runtime_services::variable_services::{GetVariableStatus, VariableInfo},
};
use r_efi::efi;

use crate::{error::VariableError, variable};

use core::{ffi::c_void, mem, slice};

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
        let data_ptr: *mut u8 = data as *mut T as *mut u8;
        // SHERRY tangent: i don't hate the idea of using bytemuck + Pod here
        // the requirement of Pod are 1) repr(C) + 2) Copy - which we are basically requiring by doing the pointer conversion above
        // so it doesn't add any additional requirements on T
        // and makes the VariableStorage.set_variable a bit nicer
        self.storage_provider.set_variable(name, namespace, attributes, unsafe { slice::from_raw_parts_mut(data_ptr, mem::size_of::<T>()) })
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
    ) -> Result<(Vec<u8>, u32), VariableError>;

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
