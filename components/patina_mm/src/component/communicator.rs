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

mod comm_buffer_update;

use crate::{
    config::{CommunicateBuffer, EfiMmCommunicateHeader, MmCommunicationConfiguration},
    service::SwMmiTrigger,
};
use patina::{
    Guid,
    boot_services::StandardBootServices,
    component::{
        IntoComponent, Storage,
        service::{IntoService, Service},
    },
};
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

use core::{
    cell::RefCell,
    fmt::{self, Debug},
};

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
    #[coverage(off)]
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
    /// Configured communication buffers
    comm_buffers: RefCell<Vec<CommunicateBuffer>>,
    /// The MM Executor actively handling MM execution
    mm_executor: Option<Box<dyn MmExecutor>>,
    /// Context shared with protocol callback for pending buffer updates
    notify_context: Option<&'static comm_buffer_update::ProtocolNotifyContext>,
}

impl MmCommunicator {
    /// Create a new `MmCommunicator` instance for testing.
    pub fn new() -> Self {
        Self { comm_buffers: RefCell::new(Vec::new()), mm_executor: None, notify_context: None }
    }

    /// Create a new `MmCommunicator` instance with a custom MM executor (for testing).
    pub fn with_executor(executor: Box<dyn MmExecutor>) -> Self {
        Self { comm_buffers: RefCell::new(Vec::new()), mm_executor: Some(executor), notify_context: None }
    }

    /// Set communication buffers for testing purposes.
    #[coverage(off)]
    pub fn set_test_comm_buffers(&self, buffers: Vec<CommunicateBuffer>) {
        *self.comm_buffers.borrow_mut() = buffers;
    }

    /// Component entry point
    ///
    /// # Coverage
    ///
    /// This function is marked with `#[coverage(off)]` because it requires StandardBootServices
    /// which is not available in unit tests. It is tested through integration tests.
    #[coverage(off)]
    fn entry_point(
        mut self,
        storage: &mut Storage,
        sw_mmi_trigger: Service<dyn SwMmiTrigger>,
        boot_services: StandardBootServices,
    ) -> patina::error::Result<()> {
        log::info!(target: "mm_comm", "MM Communicator entry...");

        // Create the real MM executor
        self.mm_executor = Some(Box::new(RealMmExecutor::new(sw_mmi_trigger)));

        let (comm_buffers, enable_buffer_updates, updatable_buffer_id) = {
            let config = storage
                .get_config::<MmCommunicationConfiguration>()
                .expect("Failed to get MM Configuration Config from storage");

            log::trace!(
                target: "mm_comm",
                "Retrieved MM configuration: comm_buffers_count={}, enable_buffer_updates={}, updatable_buffer_id={:?}",
                config.comm_buffers.len(),
                config.enable_comm_buffer_updates,
                config.updatable_buffer_id
            );
            (config.comm_buffers.clone(), config.enable_comm_buffer_updates, config.updatable_buffer_id)
        };

        self.comm_buffers = RefCell::new(comm_buffers);

        let buffer_count = self.comm_buffers.borrow().len();
        log::info!(target: "mm_comm", "MM Communicator initialized with {} communication buffers", buffer_count);

        // Only setup a protocol notify callback if buffer updates are enabled and a buffer ID was given
        if enable_buffer_updates {
            if let Some(buffer_id) = updatable_buffer_id {
                log::info!(
                    target: "mm_comm",
                    "MM comm buffer updates enabled for buffer ID {}",
                    buffer_id
                );

                // SAFETY: The communicator reference remains valid as a stored service
                let self_ptr = &self as *const MmCommunicator;
                let context = comm_buffer_update::register_buffer_update_notify(boot_services, buffer_id, self_ptr)?;

                // Store context reference for checking pending updates in communicate()
                self.notify_context = Some(context);
            } else {
                log::warn!(
                    target: "mm_comm",
                    "MM comm buffer updates enabled but no updatable_buffer_id is configured"
                );
            }
        } else {
            log::info!(target: "mm_comm", "MM comm buffer updates disabled");
        }

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

        // Check for and apply any pending buffer updates from a potential protocol callback
        if let Some(context) = self.notify_context {
            let mut comm_buffers = self.comm_buffers.borrow_mut();
            comm_buffer_update::apply_pending_buffer_update(context, &mut comm_buffers);
        }

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
        let comm_buffer: &mut CommunicateBuffer =
            comm_buffers.iter_mut().find(|x| x.id() == id && x.is_enabled()).ok_or_else(|| {
                log::warn!(target: "mm_comm", "Communication buffer not found or it is disabled: id={}", id);
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

        // Set the mailbox status to indicate buffer is valid before triggering MMI
        // For MM environments with a mailbox, this is required for the MM core to process
        // the communication buffer
        if comm_buffer.has_status_mailbox() {
            log::trace!(target: "mm_comm", "Setting comm buffer status to valid before triggering MMI");
            comm_buffer.set_comm_buffer_valid().map_err(|_| {
                log::error!(target: "mm_comm", "Failed to set comm buffer valid status");
                Status::CommBufferInitError
            })?;
        } else {
            log::warn!(target: "mm_comm", "Buffer {} has no status mailbox - MM communication may not work correctly", id);
        }

        log::debug!(target: "mm_comm", "Executing MM communication");
        mm_executor.execute_mm(comm_buffer)?;

        // Read the return status from MM if a mailbox is available
        if comm_buffer.has_status_mailbox() {
            let (return_status, return_buffer_size) = comm_buffer.get_mm_return_status().map_err(|_| {
                log::error!(target: "mm_comm", "Failed to get MM return status");
                Status::InvalidResponse
            })?;
            log::trace!(target: "mm_comm", "MM return status: 0x{:X}, buffer size: 0x{:X}", return_status, return_buffer_size);

            // Check if MM communication was successful (EFI_SUCCESS = 0)
            if return_status != 0 {
                log::warn!(target: "mm_comm", "MM handler returned error status: 0x{:X}", return_status);
            }
        }

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
    use crate::{
        component::{
            communicator::{MmCommunicator, MockMmExecutor},
            sw_mmi_manager::SwMmiManager,
        },
        config::{CommunicateBuffer, MmCommBufferStatus, MmCommunicationConfiguration},
    };
    use patina::component::{IntoComponent, Storage};

    use core::{cell::RefCell, pin::Pin};
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
                notify_context: None,
            }
        }};
    }

    fn create_communicator_with_buffers(
        buffers: Vec<CommunicateBuffer>,
        executor: Box<dyn MmExecutor>,
    ) -> MmCommunicator {
        MmCommunicator { comm_buffers: RefCell::new(buffers), mm_executor: Some(executor), notify_context: None }
    }

    #[test]
    fn test_communicator_runs_with_deps_satisfied() {
        let mut storage = Storage::new();
        storage.add_config(MmCommunicationConfiguration::default());
        storage.add_service(SwMmiManager::new());

        let mut communicator = MmCommunicator::new().into_component();

        communicator.initialize(&mut storage);
        // Component requires StandardBootServices which is not available in unit tests,
        // so it should return Ok(false) indicating it cannot run yet
        assert_eq!(communicator.run(&mut storage), Ok(false));
    }

    #[test]
    fn test_communicate_no_comm_buffer() {
        let mut mock_executor = MockMmExecutor::new();
        mock_executor.expect_execute_mm().never();

        let communicator = MmCommunicator {
            comm_buffers: RefCell::new(vec![]),
            mm_executor: Some(Box::new(mock_executor)),
            notify_context: None,
        };
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
            notify_context: None,
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

    #[test]
    fn test_mm_communicator_debug_formatting() {
        let buffer1 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 512]))), 1);
        let buffer2 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 2);
        let buffers = vec![buffer1, buffer2];

        let communicator = create_communicator_with_buffers(buffers, Box::new(EchoMmExecutor));

        let debug_output = format!("{:?}", communicator);
        assert!(debug_output.contains("MM Communicator:"));
        assert!(debug_output.contains("Comm Buffer:"));
        assert!(debug_output.contains("MM Executor Set: true"));
    }

    #[test]
    fn test_mm_communicator_debug_no_executor() {
        let buffer = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 512]))), 0);
        let communicator =
            MmCommunicator { comm_buffers: RefCell::new(vec![buffer]), mm_executor: None, notify_context: None };

        let debug_output = format!("{:?}", communicator);
        assert!(debug_output.contains("MM Communicator:"));
        assert!(debug_output.contains("MM Executor Set: false"));
    }

    #[test]
    fn test_mm_communicator_default() {
        let communicator = MmCommunicator::default();
        assert_eq!(communicator.comm_buffers.borrow().len(), 0);
        assert!(communicator.mm_executor.is_none());
        assert!(communicator.notify_context.is_none());
    }

    #[test]
    fn test_mm_communicator_with_executor() {
        let executor = Box::new(EchoMmExecutor);
        let communicator = MmCommunicator::with_executor(executor);

        assert_eq!(communicator.comm_buffers.borrow().len(), 0);
        assert!(communicator.mm_executor.is_some());
        assert!(communicator.notify_context.is_none());
    }

    #[test]
    fn test_status_enum_debug() {
        let statuses = vec![
            Status::NoCommBuffer,
            Status::CommBufferNotFound,
            Status::CommBufferTooSmall,
            Status::CommBufferInitError,
            Status::InvalidDataBuffer,
            Status::SwMmiServiceNotAvailable,
            Status::SwMmiFailed,
            Status::InvalidResponse,
        ];

        for status in statuses {
            let debug_str = format!("{:?}", status);
            assert!(!debug_str.is_empty(), "Debug format should not be empty");
        }
    }

    #[test]
    fn test_status_enum_equality() {
        // Test PartialEq and Copy/Clone traits
        let status1 = Status::NoCommBuffer;
        let status2 = Status::NoCommBuffer;
        let status3 = Status::CommBufferNotFound;

        assert_eq!(status1, status2);
        assert_ne!(status1, status3);

        // Test Copy
        let status4 = status1;
        assert_eq!(status1, status4);
    }

    #[test]
    fn test_communicate_with_disabled_buffer() {
        // Create a buffer and disable it
        let mut buffer = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 0);
        buffer.disable();

        let communicator = create_communicator_with_buffers(vec![buffer], Box::new(EchoMmExecutor));

        // Should fail to find the buffer since it's disabled
        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert_eq!(result, Err(Status::CommBufferNotFound));
    }

    #[test]
    fn test_communicate_with_mixed_enabled_disabled_buffers() {
        // Create multiple buffers with some enabled and some disabled
        let mut buffer1 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 512]))), 1);
        buffer1.disable(); // Disabled

        let buffer2 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 2); // Enabled

        let mut buffer3 = CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 256]))), 3);
        buffer3.disable(); // Disabled

        let buffers = vec![buffer1, buffer2, buffer3];
        let communicator = create_communicator_with_buffers(buffers, Box::new(EchoMmExecutor));

        // Buffer 1 is disabled - should fail
        let result1 = communicator.communicate(1, &TEST_DATA, test_recipient());
        assert_eq!(result1, Err(Status::CommBufferNotFound));

        // Buffer 2 is enabled - should succeed
        let result2 = communicator.communicate(2, &TEST_DATA, test_recipient());
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), TEST_DATA.to_vec());

        // Buffer 3 is disabled - should fail
        let result3 = communicator.communicate(3, &TEST_DATA, test_recipient());
        assert_eq!(result3, Err(Status::CommBufferNotFound));
    }

    #[test]
    fn test_mm_communicator_new() {
        let communicator = MmCommunicator::new();

        // Verify initial state
        assert_eq!(communicator.comm_buffers.borrow().len(), 0);
        assert!(communicator.mm_executor.is_none());
        assert!(communicator.notify_context.is_none());

        // Verify it matches default
        let default_communicator = MmCommunicator::default();
        assert_eq!(communicator.comm_buffers.borrow().len(), default_communicator.comm_buffers.borrow().len());
    }

    #[test]
    fn test_communicate_non_zero_mm_return_status() {
        use std::alloc::{Layout, alloc, dealloc};

        // Create a buffer with a mailbox using from_firmware_region
        let buffer_size = 4096;
        let page_align = 4096;

        let buffer_layout = Layout::from_size_align(buffer_size, page_align).unwrap();
        // SAFETY: buffer_layout is a valid memory buffer allocated above.
        let buffer_ptr: *mut u8 = unsafe { alloc(buffer_layout) };
        assert!(!buffer_ptr.is_null(), "Failed to allocate aligned buffer");

        // SAFETY: buffer_ptr points to a valid memory buffer allocated above.
        unsafe {
            core::ptr::write_bytes(buffer_ptr, 0, buffer_size);
        }

        let status_layout = Layout::from_size_align(core::mem::size_of::<MmCommBufferStatus>(), page_align).unwrap();
        // SAFETY: status_layout is a valid memory buffer allocated above.
        let status_ptr = unsafe { alloc(status_layout) as *mut MmCommBufferStatus };
        assert!(!status_ptr.is_null(), "Failed to allocate aligned status");

        // SAFETY: status_ptr points to a valid memory buffer allocated above.
        unsafe {
            core::ptr::write(status_ptr, MmCommBufferStatus::new());
        }

        let buffer_addr = buffer_ptr as u64;
        let status_addr = status_ptr as u64;

        // SAFETY: The memory is allocated above in the test and valid for the duration of the test
        let buffer_with_mailbox = unsafe {
            CommunicateBuffer::from_firmware_region(buffer_addr, buffer_size, 0, Some(status_addr))
                .expect("Failed to create buffer with mailbox")
        };

        assert!(buffer_with_mailbox.has_status_mailbox());

        struct NonZeroReturnExecutor {
            status_ptr: *mut MmCommBufferStatus,
        }
        impl MmExecutor for NonZeroReturnExecutor {
            fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
                // Get the message and echo it back (like EchoMmExecutor)
                let request_data = comm_buffer.get_message().map_err(|_| Status::InvalidDataBuffer)?;
                let recipient_bytes = comm_buffer
                    .get_header_guid()
                    .map_err(|_| Status::CommBufferInitError)?
                    .ok_or(Status::CommBufferInitError)?
                    .as_bytes();
                comm_buffer.reset();
                let recipient = patina::Guid::from_bytes(&recipient_bytes);
                comm_buffer.set_message_info(recipient).map_err(|_| Status::CommBufferInitError)?;
                comm_buffer.set_message(&request_data).map_err(|_| Status::CommBufferInitError)?;

                // Set a non-zero return status in the mailbox to simulate a failure
                // SAFETY: The memory was allocated and owned by the test
                unsafe {
                    (*self.status_ptr).return_status = 0x8000_0000_0000_0001;
                    (*self.status_ptr).return_buffer_size = request_data.len() as u64;
                }

                Ok(())
            }
        }

        let communicator =
            create_communicator_with_buffers(vec![buffer_with_mailbox], Box::new(NonZeroReturnExecutor { status_ptr }));

        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert!(result.is_ok(), "Communication should succeed even with a non-zero MM return status");
        assert_eq!(result.unwrap(), TEST_DATA.to_vec());

        // SAFETY: Cleaning up memory allocated in the test.
        unsafe {
            dealloc(buffer_ptr, buffer_layout);
            dealloc(status_ptr as *mut u8, status_layout);
        }
    }

    #[test]
    fn test_communicate_get_message_fails_after_mm() {
        use std::alloc::{Layout, alloc, dealloc};

        // Create a buffer with a mailbox using from_firmware_region
        let buffer_size = 4096;
        let page_align = 4096;

        let buffer_layout = Layout::from_size_align(buffer_size, page_align).unwrap();
        // SAFETY: buffer_layout is a valid memory buffer allocated above.
        let buffer_ptr = unsafe { alloc(buffer_layout) };
        assert!(!buffer_ptr.is_null(), "Failed to allocate aligned buffer");

        // SAFETY: buffer_ptr points to a valid memory buffer allocated above.
        unsafe {
            core::ptr::write_bytes(buffer_ptr, 0, buffer_size);
        }

        let status_layout = Layout::from_size_align(core::mem::size_of::<MmCommBufferStatus>(), page_align).unwrap();
        // SAFETY: status_layout is a valid memory buffer allocated above.
        let status_ptr = unsafe { alloc(status_layout) as *mut MmCommBufferStatus };
        assert!(!status_ptr.is_null(), "Failed to allocate aligned status");

        // SAFETY: status_ptr points to a valid memory buffer allocated above.
        unsafe {
            core::ptr::write(status_ptr, MmCommBufferStatus::new());
        }

        let buffer_addr = buffer_ptr as u64;
        let status_addr = status_ptr as u64;

        // SAFETY: The memory is allocated above in the test and valid for the duration of the test
        let buffer_with_mailbox = unsafe {
            CommunicateBuffer::from_firmware_region(buffer_addr, buffer_size, 0, Some(status_addr))
                .expect("Failed to create buffer with mailbox")
        };

        // Create an executor that corrupts the buffer to cause get_message to fail
        struct CorruptBufferExecutor;
        impl MmExecutor for CorruptBufferExecutor {
            fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
                // Corrupt the message length (larger than the buffer size)
                let huge_length = usize::MAX;

                // SAFETY: The buffer is intentionally being corrupted for the test
                unsafe {
                    let ptr = comm_buffer.as_ptr();
                    let length_offset = 16;
                    let length_ptr = ptr.add(length_offset) as *mut usize;
                    *length_ptr = huge_length;
                }

                Ok(())
            }
        }

        let communicator = create_communicator_with_buffers(vec![buffer_with_mailbox], Box::new(CorruptBufferExecutor));

        let result = communicator.communicate(0, &TEST_DATA, test_recipient());
        assert_eq!(result, Err(Status::InvalidResponse));

        // SAFETY: Cleaning up memory allocated in the test.
        unsafe {
            dealloc(buffer_ptr, buffer_layout);
            dealloc(status_ptr as *mut u8, status_layout);
        }
    }
}
