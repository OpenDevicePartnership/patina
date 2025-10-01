//! Management Mode (MM) Communicator Service
//!
//! Provides a MM communication service that can be used to send and receive messages to MM handlers.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{
    component::sw_mmi_manager::SwMmiTrigger,
    config::{CommunicateBuffer, EfiMmCommunicateHeader, MmCommunicationConfiguration},
};
use patina::component::{
    IntoComponent, Storage,
    service::{IntoService, Service},
};
use r_efi::efi;
extern crate alloc;
use alloc::vec::Vec;

use core::cell::RefCell;
use core::fmt::{self, Debug};

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// MM Communicator Service Status Codes
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Status {
    /// No communication buffers are available.
    NoCommBuffer,
    /// The specified communication buffer was not found.
    CommBufferNotFound,
    /// The specified communication buffer is too small for the operation.
    CommBufferTooSmall,
    /// An error occurred while initializing the communication buffer contents.
    CommBufferInitError,
    /// The given data buffer is empty or invalid.
    InvalidDataBuffer,
    /// The SW MMI Trigger service is not available.
    SwMmiServiceNotAvailable,
    /// The SW MMI Trigger failed.
    SwMmiFailed,
}

/// MM Communication Trait
///
/// Provides a mechanism for components to communicate with MM handlers.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait MmCommunication {
    /// Sends messages via a communication ("comm") buffer to a MM handler and receives a response.
    ///
    /// # Parameters
    ///
    /// - `id`: The ID of the comm buffer to use.
    /// - `data_buffer`: The data to send to the MM handler.
    /// - `recipient`: The GUID of the recipient MM handler.
    ///
    /// # Returns
    ///
    /// - `Ok(&'static [u8])`: A reference to the response data from the MM handler.
    /// - `Err(Status)`: An error status indicating the failure reason.
    ///
    /// # Example
    ///
    /// ```rust
    /// use r_efi::efi;
    /// use patina_mm::component::communicator::MmCommunication;
    /// use patina::component::service::Service;
    ///
    /// fn component(comm_service: Service<dyn MmCommunication>) {
    ///     let data = [0x01, 0x02, 0x03];
    ///     let recipient = efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);
    ///     let result = comm_service.communicate(0, &data, recipient);
    ///
    ///     match result {
    ///         Ok(response) => println!("Received response: {:?}", response),
    ///         Err(status) => println!("Error occurred: {:?}", status),
    ///     }
    /// }
    /// ```
    fn communicate(&self, id: u8, data_buffer: &[u8], recipient: efi::Guid) -> Result<Vec<u8>, Status>;
}

/// MM Communicator Service
///
/// Provides a mechanism for components to communicate with MM handlers.
///
/// Allows sending messages via a communication ("comm") buffer and receiving responses from the MM handler where
/// the response is stored in the same buffer.
#[derive(IntoComponent, IntoService)]
#[service(dyn MmCommunication)]
pub struct MmCommunicator {
    comm_buffers: RefCell<Vec<CommunicateBuffer>>,
    sw_mmi_trigger_service: Option<Service<dyn SwMmiTrigger>>,
}

impl MmCommunicator {
    /// Create a new `MmCommunicator` instance.
    pub fn new() -> Self {
        Self { comm_buffers: RefCell::new(Vec::new()), sw_mmi_trigger_service: None }
    }

    fn entry_point(
        mut self,
        storage: &mut Storage,
        sw_mmi_trigger: Service<dyn SwMmiTrigger>,
    ) -> patina::error::Result<()> {
        log::debug!("MM Communicator entry...");

        self.sw_mmi_trigger_service = Some(sw_mmi_trigger);
        self.comm_buffers = RefCell::new(
            storage
                .get_config::<MmCommunicationConfiguration>()
                .expect("Failed to get MM Configuration Config from storage")
                .comm_buffers
                .clone(),
        );

        storage.add_service(self);

        Ok(())
    }
}

impl Debug for MmCommunicator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "MM Communicator:")?;
        for buffer in self.comm_buffers.borrow().iter() {
            writeln!(f, "Comm Buffer: {buffer:?}")?;
        }
        writeln!(f, "SW MMI Trigger Service Set: {}", self.sw_mmi_trigger_service.is_some())?;
        Ok(())
    }
}

impl MmCommunication for MmCommunicator {
    fn communicate(&self, id: u8, data_buffer: &[u8], recipient: efi::Guid) -> Result<Vec<u8>, Status> {
        if self.comm_buffers.borrow().is_empty() {
            return Err(Status::NoCommBuffer);
        }

        if data_buffer.is_empty() {
            return Err(Status::InvalidDataBuffer);
        }

        let sw_smi_trigger_service = self.sw_mmi_trigger_service.as_ref().ok_or(Status::SwMmiServiceNotAvailable)?;

        let mut comm_buffers = self.comm_buffers.borrow_mut();
        let comm_buffer: &mut CommunicateBuffer =
            comm_buffers.iter_mut().find(|x| x.id() == id).ok_or(Status::CommBufferNotFound)?;

        let total_required_comm_buffer_length = EfiMmCommunicateHeader::size() + data_buffer.len();

        if comm_buffer.len() < total_required_comm_buffer_length {
            return Err(Status::CommBufferTooSmall);
        }

        comm_buffer.set_message_info(recipient).map_err(|_| Status::CommBufferInitError)?;
        comm_buffer.set_message(data_buffer).map_err(|_| Status::CommBufferInitError)?;

        unsafe { sw_smi_trigger_service.trigger_sw_mmi(0xFF, 0).map_err(|_| Status::SwMmiFailed)? };

        Ok(comm_buffer.get_message())
    }
}

impl Default for MmCommunicator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::component::communicator::MmCommunicator;
    use crate::component::sw_mmi_manager::{MockSwMmiTrigger, SwMmiManager};
    use crate::config::{
        CommunicateBuffer, CommunicateBufferStatus, EfiMmCommunicateHeader, MmCommunicationConfiguration,
    };
    use patina::component::{IntoComponent, Storage};

    use core::cell::RefCell;
    use core::pin::Pin;
    use r_efi::efi;

    extern crate alloc;
    use alloc::vec::Vec;

    static TEST_DATA: [u8; 3] = [0x01, 0x02, 0x03];
    static TEST_RESPONSE: [u8; 4] = [0x04, 0x03, 0x02, 0x1];
    static TEST_RECIPIENT: efi::Guid =
        efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

    macro_rules! get_test_communicator {
        ($size:expr, $sw_mmi_trigger_instance:expr) => {{
            let buffer: &'static mut [u8; $size] = Box::leak(Box::new([0u8; $size]));
            MmCommunicator {
                comm_buffers: RefCell::new(vec![unsafe { CommunicateBuffer::new(Pin::new(buffer), 0) }]),
                sw_mmi_trigger_service: Some(Service::mock(Box::new($sw_mmi_trigger_instance))),
            }
        }};
    }

    #[test]
    fn communicator_runs_with_deps_satisfied() {
        let mut storage = Storage::new();
        storage.add_config(MmCommunicationConfiguration::default());
        storage.add_service(SwMmiManager::new());

        // A MmCommunicationConfiguration  instance is required.
        let mut communicator = MmCommunicator::new().into_component();

        communicator.initialize(&mut storage);
        assert_eq!(communicator.run(&mut storage), Ok(true));
    }

    #[test]
    fn test_communicate_no_comm_buffer() {
        let communicator: MmCommunicator = MmCommunicator {
            comm_buffers: RefCell::new(vec![]),
            sw_mmi_trigger_service: Some(Service::mock(Box::new(SwMmiManager::new()))),
        };
        let result = communicator.communicate(0, &TEST_DATA, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::NoCommBuffer));
    }

    #[test]
    fn test_communicate_invalid_data_buffer() {
        let communicator = get_test_communicator!(0, MockSwMmiTrigger::new());
        let data = [];
        let result = communicator.communicate(0, &data, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::InvalidDataBuffer));
    }

    #[test]
    fn test_communicate_comm_buffer_too_small() {
        let communicator = get_test_communicator!(4, MockSwMmiTrigger::new());
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let result = communicator.communicate(0, &data, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::CommBufferTooSmall));
    }

    #[test]
    fn test_communicate_sw_mmi_is_triggered_once() {
        let mut mock_sw_mmi_trigger = MockSwMmiTrigger::new();

        // Verify that a software MMI is only triggered once
        mock_sw_mmi_trigger.expect_trigger_sw_mmi().once().returning(|_, _| Ok(()));

        let communicator = get_test_communicator!(1024, mock_sw_mmi_trigger);

        let result = communicator.communicate(0, &TEST_DATA, TEST_RECIPIENT);
        assert!(result.is_ok(), "Expected successful communication, but got: {:?}", result.err());
    }

    #[test]
    fn test_communicate_sw_mmi_is_returns_mmi_error() {
        let mut mock_sw_mmi_trigger = MockSwMmiTrigger::new();

        // Verify that a software MMI that fails returns `patina_mm::communicator::StatusSwMmiFailed`
        mock_sw_mmi_trigger
            .expect_trigger_sw_mmi()
            .times(1)
            .returning(|_, _| Err(patina::error::EfiError::DeviceError));

        let communicator = get_test_communicator!(1024, mock_sw_mmi_trigger);

        let result = communicator.communicate(0, &TEST_DATA, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::SwMmiFailed), "Expected `Status::SwMmiFailed`, but got: {result:?}");
    }

    #[test]
    fn test_communicate_sw_mmi_get_and_set_message_are_consistent() {
        const COMM_BUFFER_SIZE: usize = 64;
        const DATA_BUFFFER_SIZE: usize = COMM_BUFFER_SIZE - EfiMmCommunicateHeader::size();

        let mut mock_sw_mmi_trigger = MockSwMmiTrigger::new();

        mock_sw_mmi_trigger.expect_trigger_sw_mmi().returning(|_, _| Ok(()));

        let communicator = get_test_communicator!(COMM_BUFFER_SIZE, mock_sw_mmi_trigger);

        let result = communicator.comm_buffers.borrow_mut()[0].set_message(&TEST_RESPONSE);
        assert_eq!(result, Err(CommunicateBufferStatus::InvalidRecipient));
        let result = communicator.comm_buffers.borrow_mut()[0].set_message_info(TEST_RECIPIENT);
        assert_eq!(result, Ok(()), "Expected message info to be set successfully, but got: {result:?}");
        let result = communicator.comm_buffers.borrow_mut()[0].set_message(&TEST_RESPONSE);
        assert_eq!(result, Ok(()), "Expected message to be set successfully, but got: {result:?}");

        let message = communicator.comm_buffers.borrow_mut()[0].get_message();
        let mut expected_data = vec![0u8; DATA_BUFFFER_SIZE];
        expected_data[..TEST_RESPONSE.len()].copy_from_slice(&TEST_RESPONSE);
        assert!(!message.is_empty(), "Expected message to be set, but got empty message: {message:?}");
        assert_eq!(message, *expected_data, "Expected message to be set correctly, but got: {message:?}");
    }

    #[test]
    fn test_communicate_uses_correct_comm_buffer() {
        const COMM_BUFFER_SIZE: usize = 64;

        const COMM_BUFFER_1_ID: u8 = 1;
        const COMM_BUFFER_2_ID: u8 = 20;
        const COMM_BUFFER_3_ID: u8 = 30;

        const COMM_RESPONSE_TEST_BYTE_LEN: usize = 4;

        let mut mock_sw_mmi_trigger = MockSwMmiTrigger::new();

        // Verify that a software MMI is only triggered once
        mock_sw_mmi_trigger.expect_trigger_sw_mmi().once().returning(|_, _| Ok(()));

        // Note: This macro creates a comm buffer of size 0 with ID 0
        let communicator = get_test_communicator!(64, mock_sw_mmi_trigger);

        let comm_buffer_ids = [COMM_BUFFER_1_ID, COMM_BUFFER_2_ID, COMM_BUFFER_3_ID];
        let comm_buffers = [
            unsafe {
                CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]))), comm_buffer_ids[0])
            },
            unsafe {
                CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]))), comm_buffer_ids[1])
            },
            unsafe {
                CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]))), comm_buffer_ids[2])
            },
        ];

        // Cleat the buffer added by the macro and add the new buffers
        communicator.comm_buffers.borrow_mut().clear();
        comm_buffers.iter().cloned().for_each(|b| {
            communicator.comm_buffers.borrow_mut().push(b);
        });

        let mut comm_buffer_test_data: Vec<Vec<u8>> = Vec::new();
        for (i, comm_buffer) in communicator.comm_buffers.borrow_mut().iter_mut().enumerate() {
            let data = &(0..COMM_RESPONSE_TEST_BYTE_LEN).map(|x| (i + x + 1) as u8).collect::<Vec<_>>()[..];
            assert!(
                comm_buffers.iter().any(|b| b.id() == comm_buffer.id()),
                "Comm buffer ID {} is not an expected ID",
                comm_buffer.id()
            );

            let local_comm_buffer = comm_buffers.iter().find(|b| b.id() == comm_buffer.id()).unwrap();
            assert_eq!(
                local_comm_buffer.id(),
                comm_buffer.id(),
                "Comm buffer ID mismatch: expected {}, got {}",
                local_comm_buffer.id(),
                comm_buffer.id()
            );
            assert!(
                comm_buffer.set_message_info(TEST_RECIPIENT).is_ok(),
                "Failed to set message info for comm buffer with ID: {}",
                comm_buffer.id()
            );
            comm_buffer.set_message(data).unwrap();
            assert_eq!(
                comm_buffer.get_message()[..data.len()],
                *data,
                "Failed to set message for comm buffer with ID: {}",
                comm_buffer.id()
            );
            comm_buffer_test_data.push(comm_buffer.get_message());
        }

        // Verify that the correct comm buffer is used for the first ID (which matches after the comm data is written)
        let result = communicator.communicate(comm_buffer_ids[0], &TEST_DATA, TEST_RECIPIENT);
        assert_eq!(result, Ok(comm_buffer_test_data[0].clone()), "Comm buffer 1 failed to return the expected data");
    }

    #[test]
    fn test_communicate_debug_formatting() {
        let communicator = get_test_communicator!(64, MockSwMmiTrigger::new());

        let debug_output = format!("{communicator:?}");
        assert!(
            debug_output.contains("MM Communicator:"),
            "Expected debug output to contain 'MM Communicator', but got: {debug_output:?}"
        );
        assert!(
            debug_output.contains("SW MMI Trigger Service Set: true"),
            "Expected debug output to contain 'SW MMI Trigger Service Set: true', but got: {debug_output:?}",
        );
    }
}
