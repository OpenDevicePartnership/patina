use core::mem;

use alloc::{vec, vec::Vec};
use fallible_streaming_iterator::FallibleStreamingIterator;
use r_efi::efi::{self, Guid};

use super::RuntimeServices;

/// Status information returned by [`RuntimeServices::get_variable_unchecked`]
#[derive(Debug)]
pub enum GetVariableStatus {
    /// The variable was unable to be retrieved
    Error(efi::Status),
    /// The variable was found, but the buffer provided wasn't large enough
    BufferTooSmall {
        /// The size of a buffer needed to retrieve the variable data
        data_size: usize,
        /// The attributes of the variable
        attributes: u32,
    },
    /// The variable was successfully retrieved
    Success {
        /// The size of the variable data retrieved
        data_size: usize,
        /// The attributes of the variable
        attributes: u32,
    },
}

/// Variable information returned by [`RuntimeServices::query_variable_info`]
#[derive(Debug)]
pub struct VariableInfo {
    /// The maximum size of the storage space available for the EFI variables associated with the attributes specified
    pub maximum_variable_storage_size: u64,
    /// The remaining size of the storage space available for EFI variables associated with the attributes specified
    pub remaining_variable_storage_size: u64,
    /// The maximum size of an individual EFI variable associated with the attributes specified
    pub maximum_variable_size: u64,
}

/// Uniquely identifies a UEFI variable
#[derive(Debug)]
pub struct VariableIdentifier {
    /// The name of a UEFI variable
    name: Vec<u16>,
    /// The namespace of a UEFI variable
    namespace: efi::Guid,
}

/// Provides a [`FallibleStreamingIterator`] over UEFI variable names
///
/// Produces an EFI status on error.
///
/// # Examples
///
/// ## Iterating through all UEFI variable names
/// ```ignore
/// pub static RUNTIME_SERVICES: StandardRuntimeServices =
///     StandardRuntimeServices::new(&(*runtime_services_ptr));
/// let mut iter = VariableNameIterator::new_from_first(runtime_services);
/// while let Some(variable_identifier) = iter.next()? {
///     some_function(variable_identifier.name, variable_identifier.namespace);
/// }
/// ```
///
/// ## Iterating through UEFI variable names, starting with a known one
/// ```ignore
/// let mut iter = VariableNameIterator::new_from_variable(
///     &SOME_VARIABLE_NAME,
///     &SOME_VARIABLE_NAMESPACE,
///     runtime_services
/// );
///
/// while let Some(variable_identifier) = iter.next()? {
///     some_function(variable_identifier.name, variable_identifier.namespace);
/// }
/// ```
#[derive(Debug)]
pub struct VariableNameIterator<'a, R: RuntimeServices> {
    rs: &'a R,

    current: VariableIdentifier,
    next: VariableIdentifier,
    finished: bool,
}

impl<'a, R: RuntimeServices> VariableNameIterator<'a, R> {
    /// Produce a new iterator from the beginning of the UEFI variable list
    pub fn new_from_first(runtime_services: &'a R) -> Self {
        Self {
            rs: runtime_services,
            current: VariableIdentifier {
                name: {
                    // Previous name should be an empty string to get the first variable
                    let prev_name = vec![0; 1];

                    prev_name
                },
                // When calling with an empty name, the GUID is ignored.
                // We can just set it to zero.
                namespace: Guid::from_bytes(&[0x0; 16]),
            },
            next: VariableIdentifier { name: Vec::<u16>::new(), namespace: Guid::from_bytes(&[0x0; 16]) },
            finished: false,
        }
    }

    /// Produce a new iterator, starting from a given variable
    pub fn new_from_variable(name: &[u16], namespace: &efi::Guid, runtime_services: &'a R) -> Self {
        Self {
            rs: runtime_services,
            current: VariableIdentifier { name: name.to_vec(), namespace: *namespace },
            next: VariableIdentifier { name: Vec::<u16>::new(), namespace: Guid::from_bytes(&[0x0; 16]) },
            finished: false,
        }
    }
}

impl<R: RuntimeServices> FallibleStreamingIterator for VariableNameIterator<'_, R> {
    type Item = VariableIdentifier;
    type Error = efi::Status;

    fn advance(&mut self) -> Result<(), Self::Error> {
        unsafe {
            // Don't do anything if we've reached the end already
            if self.finished {
                return Ok(());
            }

            let status = self.rs.get_next_variable_name_unchecked(
                &self.current.name,
                &self.current.namespace,
                &mut self.next.name,
                &mut self.next.namespace,
            );

            mem::swap(&mut self.current, &mut self.next);

            if status.is_err() && status.unwrap_err() == efi::Status::NOT_FOUND {
                self.finished = true;
                Ok(())
            } else {
                status
            }
        }
    }

    fn get(&self) -> Option<&Self::Item> {
        if self.finished {
            None
        } else {
            Some(&self.current)
        }
    }
}

#[cfg(test)]
mod test {
    use r_efi::efi;

    use super::*;
    use crate::runtime_services::{
        test::{
            mock_efi_get_next_variable_name, runtime_services, DUMMY_FIRST_NAME, DUMMY_FIRST_NAMESPACE,
            DUMMY_SECOND_NAME, DUMMY_SECOND_NAMESPACE,
        },
        StandardRuntimeServices,
    };
    use std::mem;

    #[test]
    fn test_variable_name_iterator_from_first() {
        let rs = runtime_services!(get_next_variable_name = mock_efi_get_next_variable_name);

        let mut iter = VariableNameIterator::new_from_first(&rs);

        // Make sure the first result corresponds to DUMMY_FIRST_NAME
        let mut status = iter.next();
        assert!(status.is_ok());
        assert!(status.unwrap().is_some());
        let mut variable_identifier = status.unwrap().unwrap();
        assert_eq!(variable_identifier.name, DUMMY_FIRST_NAME);
        assert_eq!(variable_identifier.namespace, DUMMY_FIRST_NAMESPACE);

        // Make sure the second result corresponds to DUMMY_SECOND_NAME
        status = iter.next();
        assert!(status.is_ok());
        assert!(status.unwrap().is_some());
        variable_identifier = status.unwrap().unwrap();
        assert_eq!(variable_identifier.name, DUMMY_SECOND_NAME);
        assert_eq!(variable_identifier.namespace, DUMMY_SECOND_NAMESPACE);

        // Make sure the third result indicates we've reached the end
        status = iter.next();
        assert!(status.is_ok());
        assert!(status.unwrap().is_none());
    }

    #[test]
    fn test_variable_name_iterator_from_second() {
        let rs = runtime_services!(get_next_variable_name = mock_efi_get_next_variable_name);

        let mut iter = VariableNameIterator::new_from_variable(&DUMMY_FIRST_NAME, &DUMMY_FIRST_NAMESPACE, &rs);

        // Make sure the first result corresponds to DUMMY_SECOND_NAME
        let mut status = iter.next();
        assert!(status.is_ok());
        assert!(status.unwrap().is_some());
        let variable_identifier = status.unwrap().unwrap();
        assert_eq!(variable_identifier.name, DUMMY_SECOND_NAME);
        assert_eq!(variable_identifier.namespace, DUMMY_SECOND_NAMESPACE);

        // Make sure the second result indicates we've reached the end
        status = iter.next();
        assert!(status.is_ok());
        assert!(status.unwrap().is_none());
    }
}
