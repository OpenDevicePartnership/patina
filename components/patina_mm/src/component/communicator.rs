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
use patina::Guid;
use patina::component::{
    IntoComponent, Storage,
    service::{IntoService, Service},
};
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

use core::cell::RefCell;
use core::fmt::{self, Debug};

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Trait for handling MM execution behavior.
///
/// This trait abstracts the actual MM execution logic so testing can
/// be performed without invoking real MM transitions.
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
    #[coverage(off)]
    fn execute_mm(&self, _comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
        log::debug!(target: "mm_comm", "Triggering SW MMI for MM communication");
        self.sw_mmi_trigger_service.trigger_sw_mmi(0xFF, 0).map_err(|err| {
            log::error!(target: "mm_comm", "SW MMI trigger failed: {:?}", err);
            Status::SwMmiFailed
        })
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
    /// use patina::Guid;
    ///
    /// fn component(comm_service: Service<dyn MmCommunication>) {
    ///     let data = [0x01, 0x02, 0x03];
    ///     let recipient = efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);
    ///     let result = comm_service.communicate(0, &data, Guid::from_ref(&recipient));
    ///
    ///     match result {
    ///         Ok(response) => println!("Received response: {:?}", response),
    ///         Err(status) => println!("Error occurred: {:?}", status),
    ///     }
    /// }
    /// ```
    fn communicate<'a>(&self, id: u8, data_buffer: &[u8], recipient: Guid<'a>) -> Result<Vec<u8>, Status>;
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
    fn communicate<'a>(&self, id: u8, data_buffer: &[u8], recipient: Guid<'a>) -> Result<Vec<u8>, Status> {
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
        comm_buffer.set_message_info(recipient.clone()).map_err(|err| {
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
    use crate::config::{CommunicateBuffer, MmCommunicationConfiguration};
    use patina::component::{IntoComponent, Storage};

    use core::cell::RefCell;
    use core::pin::Pin;
    use r_efi::efi;

    extern crate alloc;
    use alloc::vec::Vec;

    /// Simple MM Executor for unit tests that simulates MM handlers echoing request data back as the response
    struct EchoMmExecutor;

    impl MmExecutor for EchoMmExecutor {
        fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
            // Get the current message data to echo back
            let request_data = comm_buffer.get_message().map_err(|_| Status::InvalidDataBuffer)?;

            // Simulate MM handler processing by echoing the data back
            let recipient_bytes = comm_buffer
                .get_header_guid()
                .map_err(|_| Status::CommBufferInitError)?
                .ok_or(Status::CommBufferInitError)?
                .as_bytes();
            comm_buffer.reset();
            let recipient = patina::Guid::from_bytes(&recipient_bytes);
            comm_buffer.set_message_info(recipient).map_err(|_| Status::CommBufferInitError)?;
            comm_buffer.set_message(&request_data).map_err(|_| Status::CommBufferInitError)?;

            Ok(())
        }
    }

    /// Transform MM Executor that simulates MM handlers transforming request data
    struct TransformMmExecutor {
        transform_fn: fn(&[u8]) -> Vec<u8>,
    }

    impl TransformMmExecutor {
        fn new(transform_fn: fn(&[u8]) -> Vec<u8>) -> Self {
            Self { transform_fn }
        }
    }

    impl MmExecutor for TransformMmExecutor {
        fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
            // Get the current message data
            let request_data = comm_buffer.get_message().map_err(|_| Status::InvalidDataBuffer)?;

            // Transform the data using the provided function
            let response_data = (self.transform_fn)(&request_data);

            // Set the transformed response back in the buffer
            let recipient_bytes = comm_buffer
                .get_header_guid()
                .map_err(|_| Status::CommBufferInitError)?
                .ok_or(Status::CommBufferInitError)?
                .as_bytes();
            comm_buffer.reset();
            let recipient = patina::Guid::from_bytes(&recipient_bytes);
            comm_buffer.set_message_info(recipient).map_err(|_| Status::CommBufferInitError)?;
            comm_buffer.set_message(&response_data).map_err(|_| Status::CommBufferInitError)?;

            Ok(())
        }
    }

    static TEST_DATA: [u8; 3] = [0x01, 0x02, 0x03];
    static TEST_RECIPIENT: efi::Guid =
        efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

    fn test_recipient() -> Guid<'static> {
        Guid::from_ref(&TEST_RECIPIENT)
    }

    macro_rules! get_test_communicator {
        ($size:expr, $mock_executor:expr) => {{
            let buffer: &'static mut [u8; $size] = Box::leak(Box::new([0u8; $size]));
            MmCommunicator {
                comm_buffers: RefCell::new(vec![CommunicateBuffer::new(Pin::new(buffer), 0)]),
                mm_executor: Some(Box::new($mock_executor)),
            }
        }};
    }

    fn create_communicator_with_buffers(
        buffers: Vec<CommunicateBuffer>,
        executor: Box<dyn MmExecutor>,
    ) -> MmCommunicator {
        MmCommunicator { comm_buffers: RefCell::new(buffers), mm_executor: Some(executor) }
    }

    #[test]
    fn test_communicator_runs_with_deps_satisfied() {
        let mut storage = Storage::new();
        storage.add_config(MmCommunicationConfiguration::default());
        storage.add_service(SwMmiManager::new());

        let mut communicator = MmCommunicator::new().into_component();

        communicator.initialize(&mut storage);
        assert_eq!(communicator.run(&mut storage), Ok(true));
    }

    #[test]
    fn test_communicate_no_comm_buffer() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator =
            MmCommunicator { comm_buffers: RefCell::new(vec![]), mm_executor: Some(Box::new(mock_executor)) };
        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert_eq!(result, Err(Status::NoCommBuffer));
    }

    #[test]
    fn test_communicate_empty_data_buffer() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator = get_test_communicator!(1024, mock_executor);
        let result = communicator.communicate(0, &[], test_recipient());
        assert_eq!(result, Err(Status::InvalidDataBuffer));
    }

    #[test]
    fn test_communicate_no_mm_executor() {
        let communicator = MmCommunicator {
            comm_buffers: RefCell::new(vec![CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 0)]),
            mm_executor: None,
        };
        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert_eq!(result, Err(Status::SwMmiServiceNotAvailable));
    }

    #[test]
    fn test_communicate_buffer_not_found() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator = get_test_communicator!(1024, mock_executor);
        let result = communicator.communicate(99, &TEST_DATA, test_recipient()); // ID 99 doesn't exist
        assert_eq!(result, Err(Status::CommBufferNotFound));
    }

    #[test]
    fn test_communicate_buffer_too_small() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        // Create a buffer that's too small for header + data
        let communicator = get_test_communicator!(10, mock_executor);
        let large_data = vec![0x42; 100];
        let result = communicator.communicate(0, &large_data, test_recipient());
        assert_eq!(result, Err(Status::CommBufferTooSmall));
    }

    #[test]
    fn test_communicate_successful_echo() {
        let communicator = get_test_communicator!(1024, EchoMmExecutor);

        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert!(result.is_ok(), "Communication should succeed: {:?}", result.err());
        assert_eq!(result.unwrap(), TEST_DATA.to_vec());
    }

    #[test]
    fn test_communicate_successful_transform() {
        // Create a transform function that reverses the data
        let reverse_transform = |data: &[u8]| -> Vec<u8> {
            let mut reversed = data.to_vec();
            reversed.reverse();
            reversed
        };

        let communicator = get_test_communicator!(1024, TransformMmExecutor::new(reverse_transform));

        let test_data = vec![1, 2, 3, 4, 5];
        let expected_response = vec![5, 4, 3, 2, 1];

        let result = communicator.communicate(0, &test_data, test_recipient());
        assert!(result.is_ok(), "Communication should succeed: {:?}", result.err());
        assert_eq!(result.unwrap(), expected_response);
    }

    #[test]
    fn test_communicate_mm_executor_error() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().times(1).returning(|_| Err(Status::SwMmiFailed));

        let communicator = get_test_communicator!(1024, mock_executor);
        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert_eq!(result, Err(Status::SwMmiFailed));
    }

    #[test]
    fn test_communicate_with_multiple_buffers() {
        // Create multiple buffers with different IDs
        let buffers = vec![
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 512]))), 1),
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 5),
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 256]))), 10),
        ];

        let communicator = create_communicator_with_buffers(buffers, Box::new(EchoMmExecutor));

        // Test communication with each buffer
        let test_data1 = b"Buffer 1 test";
        let result1 = communicator.communicate(1, test_data1, test_recipient());
        assert_eq!(result1.unwrap(), test_data1.to_vec());

        let test_data5 = b"Buffer 5 test with more data";
        let result5 = communicator.communicate(5, test_data5, test_recipient());
        assert_eq!(result5.unwrap(), test_data5.to_vec());

        let test_data10 = b"Buffer 10";
        let result10 = communicator.communicate(10, test_data10, test_recipient());
        assert_eq!(result10.unwrap(), test_data10.to_vec());
    }

    #[test]
    fn test_communicate_large_message() {
        let communicator = get_test_communicator!(4096, EchoMmExecutor);

        // Test with maximum size message (buffer size - header size)
        let max_message_size = 4096 - EfiMmCommunicateHeader::size();
        let large_data = vec![0x55; max_message_size];

        let result = communicator.communicate(0, &large_data, test_recipient());
        assert!(result.is_ok(), "Large message communication should succeed");
        assert_eq!(result.unwrap(), large_data);
    }

    #[test]
    fn test_communicate_buffer_state_tracking() {
        let communicator = get_test_communicator!(1024, EchoMmExecutor);

        // First communication
        let data1 = b"First message";
        let result1 = communicator.communicate(0, data1, test_recipient());
        assert_eq!(result1.unwrap(), data1.to_vec());

        // Second communication with different data
        let data2 = b"Second different message";
        let result2 = communicator.communicate(0, data2, test_recipient());
        assert_eq!(result2.unwrap(), data2.to_vec());

        // Verify buffer was properly reset between communications
        let buffer = &communicator.comm_buffers.borrow()[0];
        let current_message = buffer.get_message().unwrap();
        assert_eq!(current_message, data2.to_vec());
    }

    #[test]
    fn test_communicate_verifies_buffer_consistency() {
        // Test that the communicate method properly verifies buffer state consistency
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().times(1).returning(|comm_buffer| {
            // Simulate MM handler corrupting the buffer state by directly writing to memory
            // This should be caught by the state verification
            // SAFETY: Test intentionally corrupts buffer to verify error detection
            unsafe {
                let ptr = comm_buffer.as_ptr();
                *ptr = 0xFF; // Corrupt the first byte of the header
            }
            Ok(())
        });

        let communicator = get_test_communicator!(1024, mock_executor);
        let result = communicator.communicate(0, &TEST_DATA, test_recipient());

        // Should return an error because the buffer state is inconsistent after MM execution
        assert!(result.is_err(), "Should detect buffer corruption");
        assert_eq!(result.unwrap_err(), Status::InvalidResponse);
    }
}
