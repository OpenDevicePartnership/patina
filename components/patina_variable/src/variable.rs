use alloc::{string::String, vec::Vec};
use core::ffi::c_void;
use patina_sdk::{component::service::IntoService, runtime_services::{variable_services::GetVariableStatus, RuntimeServices, StandardRuntimeServices}};
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
    ) -> Result<(Vec<u8>, u32), VariableError> { 
        // Convert the name to UTF-16.
        let mut name_vec: Vec<u16> = name.encode_utf16().collect();
        let name_slice: &mut [u16] = name_vec.as_mut_slice();

        // Attempt to use size provided by caller (it may not be correct).
        let mut data = Vec::<u8>::new();
        if let Some(size_hint) = data_size {
            data.resize(size_hint, 0);
        }

        // Do at most two calls to `runtime_services.get_variable_unchecked`.
        //
        // If size_hint was provided (and the size is correct), then only one call to `get_variable_unchecked` is
        // needed. Otherwise, the first check will determine the size of the buffer to allocate for the second call.
        let mut first_attempt = true;
        loop {
            unsafe {
                let status = self.runtime_services.get_variable_unchecked(
                    name_slice,
                    namespace,
                    if data.is_empty() { None } else { Some(&mut data) },
                );

                match status {
                    GetVariableStatus::Success { data_size: _, attributes } => { return Ok((data, attributes)); },
                    // Size hint was wrong. Use the size provided by the `runtime_services` call.
                    GetVariableStatus::BufferTooSmall { data_size, attributes: _ } => {
                        if first_attempt {
                            first_attempt = false;
                            data.resize(data_size, 0);
                        } else {
                            return Err(VariableError::BufferTooSmall);
                        }
                    }
                    GetVariableStatus::Error(e) => {
                        return Err(e.into());
                    }
                }
            }
        }
    }

    fn iter_variable_names(&self) -> Result<Vec<String>, VariableError> {
        while (true) {
            match unsafe { self.runtime_services.get_next_variable_name_unchecked() } {
                Ok(name) => {
                    if name.is_empty() {
                        break;
                    }
                    return Ok(name.into_iter().map(String::from).collect());
                }
                Err(efi::Status::NOT_FOUND) => {
                    return Ok(Vec::new());
                }
                Err(status) => {
                    return Err(status.into());
                }
            }
        }
    }

    fn set_variable(
        &self,
        name: &str,
        namespace: &efi::Guid,
        attributes: u32,
        data: &[u8],
    ) -> Result<(), VariableError> {
        // Convert the name to UTF-16.
        let mut name_vec: Vec<u16> = name.encode_utf16().collect();
        let name_slice: &mut [u16] = name_vec.as_mut_slice();

        match unsafe {
            self.runtime_services.set_variable_unchecked(
                name_slice,
                namespace,
                attributes,
                data,
            )
        } {
            Ok(()) => Ok(()),
            Err(status) => Err(status.into()),
        }
    }

    fn query_variables_info(
        &self,
        attributes: u32,
    ) -> Result<patina_sdk::runtime_services::variable_services::VariableInfo, VariableError> {
        self.runtime_services.query_variable_info(attributes)
            .map_err(|e| e.into())
    }
}
