use alloc::{string::String, vec::Vec};
use core::ffi::c_void;
use patina_sdk::{component::service::IntoService, runtime_services::StandardRuntimeServices};
use r_efi::efi;

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
        if !name.iter().any(|&c| c == 0) {
            debug_assert!(false, "Name passed into get_variable is not null-terminated.");
            return Err(efi::Status::INVALID_PARAMETER);
        }

        // Keep a local copy of name to unburden the caller of having to pass in a mutable slice
        let mut name_vec = name.to_vec();

        // We can't simply allocate an empty buffer of size T because we can't assume
        // the TryFrom representation of T will be the same as T
        let mut data = Vec::<u8>::new();
        if let Some(size_hint) = size_hint {
            data.resize(size_hint, 0);
        }

        // Do at most two calls to get_variable_unchecked.
        //
        // If size_hint was provided (and the size is sufficient), then only call to get_variable_unchecked is
        // needed. Otherwise, the first check will determine the size of the buffer to allocate for the second
        // call.
        let mut first_attempt = true;
        loop {
            unsafe {
                let status = self.get_variable_unchecked(
                    name_vec.as_mut_slice(),
                    namespace,
                    if data.is_empty() { None } else { Some(&mut data) },
                );

                match status {
                    GetVariableStatus::Success { data_size: _, attributes } => match T::try_from(data) {
                        Ok(d) => return Ok((d, attributes)),
                        Err(_) => return Err(efi::Status::INVALID_PARAMETER),
                    },
                    GetVariableStatus::BufferTooSmall { data_size, attributes: _ } => {
                        if first_attempt {
                            first_attempt = false;
                            data.resize(data_size, 10);
                        } else {
                            return Err(efi::Status::BUFFER_TOO_SMALL);
                        }
                    }
                    GetVariableStatus::Error(e) => {
                        return Err(e);
                    }
                }
            }
        }
    }

    fn iter_variable_names(&self) -> Result<Vec<String>, VariableError> {
        todo!()
    }

    fn set_variable(
        &self,
        name: &str,
        namespace: &efi::Guid,
        attributes: u32,
        data: *mut c_void,
    ) -> Result<(), VariableError> {
        todo!();
    }

    fn query_variables_info(
        &self,
        attributes: u32,
    ) -> Result<patina_sdk::runtime_services::variable_services::VariableInfo, VariableError> {
        todo!()
    }
}
