//! Real Component Integration Test Framework
//!
//! This framework integrates the actual patina_mm components for comprehensive integration testing.
//! It mocks hardware and other external dependencies while exercising the real communication logic.
//!
//! ## Logging
//!
//! - The `real_test_framework` log target is used for logging within the real component test framework.
//! - The `test_mm_executor` log target is used for logging within the test MM executor.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::patina_mm_integration::common::{constants::*, handlers::*};

extern crate alloc;
use alloc::{boxed::Box, vec::Vec};
use patina::Guid;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// Import the real patina_mm components and services
use patina_mm::component::communicator::{MmCommunication, MmCommunicator, MmExecutor, Status};
use patina_mm::config::CommunicateBuffer;

/// Test MM Executor for integration testing
///
/// Test MM executor that integrates with the real MmCommunicator component
///
/// This executor simulates MM execution by calling test handlers directly, enabling
/// integration testing of the actual `MmCommunicator` component without requiring
/// real MM mode execution. It processes communication buffers and routes requests
/// to the appropriate test handlers.
pub struct TestMmExecutor {
    /// Test handlers that simulate MM handler execution
    handlers: Arc<Mutex<BTreeMap<Guid<'static>, Box<dyn MmHandler>>>>,
}

impl TestMmExecutor {
    pub fn new(handlers: Arc<Mutex<BTreeMap<Guid<'static>, Box<dyn MmHandler>>>>) -> Self {
        Self { handlers }
    }
}

impl MmExecutor for TestMmExecutor {
    fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
        log::debug!(target: "test_mm_executor", "Executing MM with test handlers");

        // Get the recipient and request data from the communication buffer
        let recipient = comm_buffer
            .get_header_guid()
            .map_err(|_| Status::CommBufferInitError)?
            .ok_or(Status::CommBufferInitError)?;
        let request_data = comm_buffer.get_message().map_err(|_| Status::InvalidDataBuffer)?;

        log::debug!(target: "test_mm_executor", "Processing MM request: recipient={:?}, data_len={}", recipient, request_data.len());

        let recipient_bytes = recipient.as_bytes();
        let handlers = self.handlers.lock().map_err(|_| Status::SwMmiFailed)?;

        // Find handler by comparing GUID bytes
        let handler_result = handlers.iter().find(|(handler_guid, _)| handler_guid.as_bytes() == recipient_bytes);

        if let Some((handler_guid, handler)) = handler_result {
            // Clone to use later
            let response_guid = handler_guid.clone();

            // Execute the handler (simulating MM execution)
            match handler.handle_request(&request_data) {
                Ok(response) => {
                    log::debug!(target: "test_mm_executor", "Handler executed successfully, response_len={}", response.len());

                    // Release the handlers lock before modifying the buffer
                    drop(handlers);

                    // Update the communication buffer with the response
                    // Reset and set the response data (simulating MM handler updating the buffer)
                    comm_buffer.reset();
                    comm_buffer.set_message_info(response_guid).map_err(|_| Status::CommBufferInitError)?;
                    comm_buffer.set_message(&response).map_err(|_| Status::CommBufferInitError)?;

                    Ok(())
                }
                Err(e) => {
                    log::error!(target: "test_mm_executor", "Handler execution failed: {:?}", e);
                    Err(Status::InvalidDataBuffer)
                }
            }
        } else {
            log::warn!(target: "test_mm_executor", "No handler found for recipient: {:?}", recipient);
            Err(Status::CommBufferNotFound)
        }
    }
}

/// Real Component MM Test Framework
///
/// This framework orchestrates real MM components while mocking hardware dependencies,
/// providing integration testing that exercises the actual `MmCommunicator` component
/// and its dependencies. Unlike the simpler `MmTestFramework`, this framework uses
/// the complete patina_mm component stack, including real `CommunicateBuffer` operations.
pub struct RealComponentMmTestFramework {
    /// Real MM Communicator service using actual communication logic
    mm_communicator: MmCommunicator,
}

impl RealComponentMmTestFramework {
    /// Create a new framework builder
    #[allow(dead_code)]
    pub fn builder() -> RealComponentMmTestFrameworkBuilder {
        RealComponentMmTestFrameworkBuilder::new()
    }

    /// Communicate with MM using the real communicator service
    pub fn communicate(&self, guid: &Guid, data: &[u8]) -> Result<Vec<u8>, Status> {
        log::debug!(target: "real_test_framework", "Real component communication request: guid={:?}, data_len={}", guid, data.len());

        // Use the real MM communicator service which will internally handle the flow
        let result = self.mm_communicator.communicate(0, data, guid.clone());

        log::debug!(target: "real_test_framework", "Real component communication result: {:?}",
                   result.as_ref().map(|r| r.len()).map_err(|e| format!("{:?}", e)));
        result
    }
}

/// Builder for the Real Component MM Test Framework
pub struct RealComponentMmTestFrameworkBuilder {
    handlers: BTreeMap<Guid<'static>, Box<dyn MmHandler>>,
}

impl RealComponentMmTestFrameworkBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self { handlers: BTreeMap::new() }
    }

    /// Add a custom handler
    pub fn with_handler(mut self, guid: Guid<'static>, handler: Box<dyn MmHandler>) -> Self {
        self.handlers.insert(guid, handler);
        self
    }

    /// Add the echo handler
    pub fn with_echo_handler(self) -> Self {
        self.with_handler(Guid::from_ref(&TEST_COMMUNICATION_GUID), Box::new(EchoHandler::new()))
    }

    /// Add the MM supervisor handler
    #[allow(dead_code)]
    pub fn with_mm_supervisor_handler(self) -> Self {
        self.with_handler(Guid::from_ref(&test_guids::MM_SUPERVISOR), Box::new(MmSupervisorHandler::new()))
    }

    /// Build the framework with real components
    pub fn build(self) -> Result<RealComponentMmTestFramework, Box<dyn std::error::Error>> {
        log::debug!(target: "real_test_framework", "Building real component MM test framework with {} handlers", self.handlers.len());

        let handlers = Arc::new(Mutex::new(self.handlers));

        // Create test communication buffer for testing
        const TEST_BUFFER_SIZE: usize = 4096;
        let test_buffer: &'static mut [u8; TEST_BUFFER_SIZE] = Box::leak(Box::new([0u8; TEST_BUFFER_SIZE]));
        let comm_buffer = CommunicateBuffer::new(core::pin::Pin::new(test_buffer), 0);

        // Create the test MM executor with our test handlers
        let test_executor = TestMmExecutor::new(handlers);

        // Create real MM communicator with test executor and test communication buffer
        let mm_communicator = MmCommunicator::with_executor(Box::new(test_executor));

        // Set up the communication buffer in the communicator
        mm_communicator.set_test_comm_buffers(vec![comm_buffer]);

        Ok(RealComponentMmTestFramework { mm_communicator })
    }
}

impl Default for RealComponentMmTestFrameworkBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_component_framework_creation() {
        let framework = RealComponentMmTestFrameworkBuilder::new()
            .with_echo_handler()
            .build()
            .expect("Framework creation should succeed");

        // Test that we can use the framework
        let test_data = b"Hello, Real Components!";
        let result = framework.communicate(&Guid::from_ref(&TEST_COMMUNICATION_GUID), test_data);

        assert!(result.is_ok(), "Real component communication should succeed");
        assert_eq!(result.unwrap(), test_data, "Echo should return the same data");
    }
}
