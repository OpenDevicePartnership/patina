//! Management Mode (MM) Communicator Service
//!
//! Provides a MM communication service that can be used to send and receive messages to MM handlers.
//!
//! ## Logging
//!
//! Detailed logging is available for this component using the `mm_comm` log target.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::config::{CommunicateBuffer, EfiMmCommunicateHeader, MmCommunicationConfiguration};
use crate::service::SwMmiTrigger;
use patina::component::{
    IntoComponent, Storage,
    service::{IntoService, Service},
};
use r_efi::efi;
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

use core::cell::RefCell;
use core::fmt::{self, Debug};

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Trait for handling MM execution behavior.
///
/// This trait abstracts the actual MM execution logic to enable testing
/// of the communication flow without requiring real MM handlers.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait MmExecutor {
    /// Execute MM with the given communication buffer.
    ///
    /// This method triggers the MM execution and allows the MM handlers
    /// to process the request in the communication buffer.
    ///
    /// # Parameters
    /// - `comm_buffer`: Mutable reference to the communication buffer containing the request
    ///
    /// # Returns
    /// - `Ok(())` if MM execution completed successfully
    /// - `Err(Status)` if MM execution failed
    fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status>;
}

/// Real MM Executor that uses the SW MMI trigger service
///
/// This is the production implementation that actually triggers MM execution
/// via the software MMI trigger service.
pub struct RealMmExecutor {
    sw_mmi_trigger_service: Service<dyn SwMmiTrigger>,
}

impl RealMmExecutor {
    /// Creates a new MM executor instance.
    pub fn new(sw_mmi_trigger_service: Service<dyn SwMmiTrigger>) -> Self {
        Self { sw_mmi_trigger_service }
    }
}

impl MmExecutor for RealMmExecutor {
    fn execute_mm(&self, _comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
        log::debug!(target: "mm_comm", "Triggering SW MMI for MM communication");
        unsafe {
            self.sw_mmi_trigger_service.trigger_sw_mmi(0xFF, 0).map_err(|err| {
                log::error!(target: "mm_comm", "SW MMI trigger failed: {:?}", err);
                Status::SwMmiFailed
            })
        }
    }
}

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
    /// Failed to retrieve a valid response from the communication buffer.
    InvalidResponse,
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
    mm_executor: Option<Box<dyn MmExecutor>>,
}

impl MmCommunicator {
    /// Create a new `MmCommunicator` instance.
    pub fn new() -> Self {
        Self { comm_buffers: RefCell::new(Vec::new()), mm_executor: None }
    }

    /// Create a new `MmCommunicator` instance with a custom MM executor (for testing).
    pub fn with_executor(executor: Box<dyn MmExecutor>) -> Self {
        Self { comm_buffers: RefCell::new(Vec::new()), mm_executor: Some(executor) }
    }

    /// Set communication buffers for testing purposes.
    pub fn set_test_comm_buffers(&self, buffers: Vec<CommunicateBuffer>) {
        *self.comm_buffers.borrow_mut() = buffers;
    }

    fn entry_point(
        mut self,
        storage: &mut Storage,
        sw_mmi_trigger: Service<dyn SwMmiTrigger>,
    ) -> patina::error::Result<()> {
        log::info!(target: "mm_comm", "MM Communicator entry...");

        // Create the real MM executor
        self.mm_executor = Some(Box::new(RealMmExecutor::new(sw_mmi_trigger)));

        let comm_buffers = {
            let config = storage
                .get_config::<MmCommunicationConfiguration>()
                .expect("Failed to get MM Configuration Config from storage");

            log::trace!(target: "mm_comm", "Retrieved MM configuration: comm_buffers_count={}", config.comm_buffers.len());
            config.comm_buffers.clone()
        };

        self.comm_buffers = RefCell::new(comm_buffers);
        log::info!(target: "mm_comm", "MM Communicator initialized with {} communication buffers", self.comm_buffers.borrow().len());

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
        writeln!(f, "MM Executor Set: {}", self.mm_executor.is_some())?;
        Ok(())
    }
}

impl MmCommunication for MmCommunicator {
    fn communicate(&self, id: u8, data_buffer: &[u8], recipient: efi::Guid) -> Result<Vec<u8>, Status> {
        log::debug!(target: "mm_comm", "Starting MM communication: buffer_id={}, data_size={}, recipient={:?}", id, data_buffer.len(), recipient);

        if self.comm_buffers.borrow().is_empty() {
            log::warn!(target: "mm_comm", "No communication buffers available");
            return Err(Status::NoCommBuffer);
        }

        if data_buffer.is_empty() {
            log::warn!(target: "mm_comm", "Invalid data buffer: empty");
            return Err(Status::InvalidDataBuffer);
        }

        let mm_executor = self.mm_executor.as_ref().ok_or_else(|| {
            log::error!(target: "mm_comm", "MM Executor not available");
            Status::SwMmiServiceNotAvailable
        })?;

        let mut comm_buffers = self.comm_buffers.borrow_mut();
        let comm_buffer: &mut CommunicateBuffer = comm_buffers.iter_mut().find(|x| x.id() == id).ok_or_else(|| {
            log::warn!(target: "mm_comm", "Communication buffer not found: id={}", id);
            Status::CommBufferNotFound
        })?;

        let total_required_comm_buffer_length = EfiMmCommunicateHeader::size() + data_buffer.len();
        log::trace!(target: "mm_comm", "Buffer validation: buffer_len={}, required_len={}", comm_buffer.len(), total_required_comm_buffer_length);

        if comm_buffer.len() < total_required_comm_buffer_length {
            log::warn!(target: "mm_comm", "Communication buffer too small: available={}, required={}", comm_buffer.len(), total_required_comm_buffer_length);
            return Err(Status::CommBufferTooSmall);
        }

        log::trace!(target: "mm_comm", "Resetting the comm buffer and internal tracking state");
        comm_buffer.reset();

        log::trace!(target: "mm_comm", "Setting up communication buffer for MM request");
        comm_buffer.set_message_info(recipient).map_err(|err| {
            log::error!(target: "mm_comm", "Failed to set message info: {:?}", err);
            Status::CommBufferInitError
        })?;
        comm_buffer.set_message(data_buffer).map_err(|err| {
            log::error!(target: "mm_comm", "Failed to set message data: {:?}", err);
            Status::CommBufferInitError
        })?;

        log::debug!(target: "mm_comm", "Outgoing MM communication request: buffer_id={}, data_size={}, recipient={:?}", id, data_buffer.len(), recipient);
        log::debug!(target: "mm_comm", "Request Data (hex): {:02X?}", &data_buffer[..core::cmp::min(data_buffer.len(), 64)]);
        log::trace!(target: "mm_comm", "Comm buffer before request: {:?}", comm_buffer);

        log::debug!(target: "mm_comm", "Executing MM communication");
        mm_executor.execute_mm(comm_buffer)?;

        log::trace!(target: "mm_comm", "MM communication completed successfully, retrieving response");
        let response = comm_buffer.get_message().map_err(|_| {
            log::error!(target: "mm_comm", "Failed to retrieve response from communication buffer");
            Status::InvalidResponse
        })?;
        log::debug!(target: "mm_comm", "MM communication response received: size={}", response.len());

        Ok(response)
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
    use crate::component::communicator::{MmCommunicator, MockMmExecutor};
    use crate::component::sw_mmi_manager::SwMmiManager;
    use crate::config::{CommunicateBuffer, CommunicateBufferStatus, MmCommunicationConfiguration};
    use patina::component::{IntoComponent, Storage};

    use core::cell::RefCell;
    use core::pin::Pin;
    use r_efi::efi;

    extern crate alloc;
    use alloc::vec::Vec;

    /// Test MM Executor that simulates successful MM execution
    /// This simply echoes back the request data as the response
    #[allow(dead_code)] // Usage in integration tests is not found
    struct TestMmExecutor;

    #[allow(dead_code)]
    impl MmExecutor for TestMmExecutor {
        fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
            // Get the current message data
            let request_data = comm_buffer.get_message().map_err(|_| Status::InvalidDataBuffer)?;

            // For test purposes, just echo the request data back as the response
            // In a real MM environment, the MM handlers would process the request and update the buffer
            // Reset and set the same data back (simulating MM handler processing)
            let recipient = comm_buffer
                .get_header_guid()
                .map_err(|_| Status::CommBufferInitError)?
                .ok_or(Status::CommBufferInitError)?;
            comm_buffer.reset();
            comm_buffer.set_message_info(recipient).map_err(|_| Status::CommBufferInitError)?;
            comm_buffer.set_message(&request_data).map_err(|_| Status::CommBufferInitError)?;

            Ok(())
        }
    }

    static TEST_DATA: [u8; 3] = [0x01, 0x02, 0x03];
    static TEST_RESPONSE: [u8; 4] = [0x04, 0x03, 0x02, 0x1];
    static TEST_RECIPIENT: efi::Guid =
        efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

    macro_rules! get_test_communicator {
        ($size:expr, $mock_executor:expr) => {{
            let buffer: &'static mut [u8; $size] = Box::leak(Box::new([0u8; $size]));
            MmCommunicator {
                comm_buffers: RefCell::new(vec![CommunicateBuffer::new(Pin::new(buffer), 0)]),
                mm_executor: Some(Box::new($mock_executor)),
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
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator: MmCommunicator =
            MmCommunicator { comm_buffers: RefCell::new(vec![]), mm_executor: Some(Box::new(mock_executor)) };
        let result = communicator.communicate(0, &TEST_DATA, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::NoCommBuffer));
    }

    #[test]
    fn test_communicate_invalid_data_buffer() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator = get_test_communicator!(0, mock_executor);
        let data = [];
        let result = communicator.communicate(0, &data, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::InvalidDataBuffer));
    }

    #[test]
    fn test_communicate_comm_buffer_too_small() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator = get_test_communicator!(4, mock_executor);
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let result = communicator.communicate(0, &data, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::CommBufferTooSmall));
    }

    #[test]
    fn test_communicate_sw_mmi_is_triggered_once() {
        let mut mock_mm_executor = MockMmExecutor::new();

        // Verify that MM execution is only triggered once
        mock_mm_executor.expect_execute_mm().once().returning(|_| Ok(()));

        let communicator = get_test_communicator!(1024, mock_mm_executor);

        let result = communicator.communicate(0, &TEST_DATA, TEST_RECIPIENT);
        assert!(result.is_ok(), "Expected successful communication, but got: {:?}", result.err());
    }

    #[test]
    fn test_communicate_sw_mmi_is_returns_mmi_error() {
        let mut mock_mm_executor = MockMmExecutor::new();

        // Verify that MM execution failure returns `Status::SwMmiFailed`
        mock_mm_executor.expect_execute_mm().times(1).returning(|_| Err(Status::SwMmiFailed));

        let communicator = get_test_communicator!(1024, mock_mm_executor);

        let result = communicator.communicate(0, &TEST_DATA, TEST_RECIPIENT);
        assert_eq!(result, Err(Status::SwMmiFailed), "Expected `Status::SwMmiFailed`, but got: {result:?}");
    }

    #[test]
    fn test_communicate_sw_mmi_get_and_set_message_are_consistent() {
        const COMM_BUFFER_SIZE: usize = 64;

        let mut mock_mm_executor = MockMmExecutor::new();

        mock_mm_executor.expect_execute_mm().returning(|_| Ok(()));

        let communicator = get_test_communicator!(COMM_BUFFER_SIZE, mock_mm_executor);

        let result = communicator.comm_buffers.borrow_mut()[0].set_message(&TEST_RESPONSE);
        assert_eq!(result, Err(CommunicateBufferStatus::InvalidRecipient));
        let result = communicator.comm_buffers.borrow_mut()[0].set_message_info(TEST_RECIPIENT);
        assert_eq!(result, Ok(()), "Expected message info to be set successfully, but got: {result:?}");
        let result = communicator.comm_buffers.borrow_mut()[0].set_message(&TEST_RESPONSE);
        assert_eq!(result, Ok(()), "Expected message to be set successfully, but got: {result:?}");

        let message = communicator.comm_buffers.borrow_mut()[0].get_message().unwrap();
        assert!(!message.is_empty(), "Expected message to be set, but got empty message: {message:?}");
        assert_eq!(message, TEST_RESPONSE, "Expected message to be set correctly, but got: {message:?}");
    }

    #[test]
    fn test_communicate_uses_correct_comm_buffer() {
        const COMM_BUFFER_SIZE: usize = 64;

        const COMM_BUFFER_1_ID: u8 = 1;
        const COMM_BUFFER_2_ID: u8 = 20;
        const COMM_BUFFER_3_ID: u8 = 30;

        const COMM_RESPONSE_TEST_BYTE_LEN: usize = 4;

        let mut mock_mm_executor = MockMmExecutor::new();

        // Verify that MM execution is only triggered once
        mock_mm_executor.expect_execute_mm().once().returning(|_| Ok(()));

        // Note: This macro creates a comm buffer of size 0 with ID 0
        let communicator = get_test_communicator!(64, mock_mm_executor);

        let comm_buffer_ids = [COMM_BUFFER_1_ID, COMM_BUFFER_2_ID, COMM_BUFFER_3_ID];
        let comm_buffers = [
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]))), comm_buffer_ids[0]),
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]))), comm_buffer_ids[1]),
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]))), comm_buffer_ids[2]),
        ];

        // Clear the buffer added by the macro and add the new buffers
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
                comm_buffer.get_message().unwrap()[..data.len()],
                *data,
                "Failed to set message for comm buffer with ID: {}",
                comm_buffer.id()
            );
            comm_buffer_test_data.push(comm_buffer.get_message().unwrap());
        }

        // Verify that the correct comm buffer is used for the first ID (which matches after the comm data is written)
        let result = communicator.communicate(comm_buffer_ids[0], &TEST_DATA, TEST_RECIPIENT);

        assert_eq!(result, Ok(TEST_DATA.to_vec()), "Comm buffer 1 failed to return the expected data");
    }

    #[test]
    fn test_communicate_debug_formatting() {
        let mut mock_mm_executor = MockMmExecutor::new();
        mock_mm_executor.expect_execute_mm().never();

        let communicator = get_test_communicator!(64, mock_mm_executor);

        let debug_output = format!("{communicator:?}");
        assert!(
            debug_output.contains("MM Communicator:"),
            "Expected debug output to contain 'MM Communicator', but got: {debug_output:?}"
        );
        assert!(
            debug_output.contains("MM Executor Set: true"),
            "Expected debug output to contain 'MM Executor Set: true', but got: {debug_output:?}",
        );
    }
}
