//! Patina Management Mode (MM) Test Framework
//!
//! This module provides a test framework for Patina MM functionality.
//!
//! At this time, that is primarily focused on MM Communication testing
//! using the Patina MM Communication service against mock handlers.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
use crate::patina_mm_integration::common::{constants::*, handlers::*, message_parser::*};

extern crate alloc;
use alloc::{boxed::Box, string::String, vec::Vec};
use r_efi::efi;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Test framework error types
///
/// Represents errors that can occur during MM Communication test framework operations.
///
/// ## Integration with EFI Error Model
///
/// `From<TestFrameworkError> for EfiError` is implemented to convert between
/// these framework-specific errors and the EFI errors in the `patina` crate.
#[derive(Debug)]
pub enum TestFrameworkError {
    /// Handler registration failed
    HandlerRegistrationFailed(String),
    /// Service creation failed
    ServiceCreationFailed(String),
    /// Buffer operation failed
    BufferError(String),
    /// Handler execution failed
    HandlerError(String),
}

impl core::fmt::Display for TestFrameworkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TestFrameworkError::HandlerRegistrationFailed(msg) => write!(f, "Handler registration failed: {}", msg),
            TestFrameworkError::ServiceCreationFailed(msg) => write!(f, "Service creation failed: {}", msg),
            TestFrameworkError::BufferError(msg) => write!(f, "Buffer error: {}", msg),
            TestFrameworkError::HandlerError(msg) => write!(f, "Handler error: {}", msg),
        }
    }
}

impl std::error::Error for TestFrameworkError {}

impl From<TestFrameworkError> for patina::error::EfiError {
    fn from(_err: TestFrameworkError) -> Self {
        patina::error::EfiError::Aborted // Convert to appropriate EFI error
    }
}

/// Simple test framework for MM communication testing
///
/// A lightweight testing infrastructure for Patina MM Communication functionality. This
/// framework is used for unit and integration testing. Integration testing is used to
/// simulate MM handling without requiring a real MM execution environment.
///
/// A registry of test handlers is maintained, each handler processes a MM request
/// and returns a response. The framework simulates the core communication flow by
/// formatting messages, delegating to handlers, and returning results. New handlers
/// can be added easily when the framework is constructed in different testing scenarios.
#[derive(Clone)]
pub struct MmTestFramework {
    /// A MM handler registry that is wrapped with Arc<Mutex<...>> for shared, thread-safe access
    /// to handlers.
    ///
    /// In particular, multiple parts of the test framework may need to access the same
    /// handler registry concurrently (e.g. different test threads, or different components
    /// of the framework). `Arc`is used to enable shared ownership of the handler registry,
    /// Handlers can be added during framework construction, and then transferred to the final
    /// framework instance without cloning. It also lets the same handler registry be shared
    /// between different testing contexts, without duplicating data.
    handlers: Arc<Mutex<HashMap<efi::Guid, Box<dyn MmHandler>>>>,

    /// Atomic counter tracking the number of MM communication triggers
    ///
    /// This counter is incremented every time `communicate` or `communicate_with_buffer`
    /// is called, regardless of success or failure. It provides an accurate count of
    /// MM communication attempts for testing and validation purposes.
    ///
    /// Uses `AtomicUsize` for thread-safe access without requiring mutex locks.
    trigger_count: Arc<AtomicUsize>,
}

impl MmTestFramework {
    /// Create a new framework builder
    pub fn builder() -> MmTestFrameworkBuilder {
        MmTestFrameworkBuilder::new()
    }

    /// Perform MM communication with the specified GUID and data
    ///
    /// The is a simple simulation of the core patina_mm communication flow that:
    ///
    /// 1. Creates a test buffer
    /// 2. Looks up the appropriate handler in the registry
    /// 3. Delegates request processing to the corresponding registered handler
    /// 4. Returns the handler's response / error status
    ///
    /// > Note: Operates with the resources (e.g. MMI handlers) defined in this
    /// > instance.
    ///
    /// ## Error Handling
    ///
    /// The method maps internal errors to `patina_mm::component::communicator::Status`
    /// to maintain compatibility with the real MM communication interface.
    ///
    /// ## Thread Safety
    ///
    /// The method acquires a mutex lock on the handler registry for each communication.
    ///
    /// The lock is held only during handler lookup, not during handler execution,
    /// allowing concurrent handler processing for different handler GUIDs.
    pub fn communicate(
        &self,
        guid: &efi::Guid,
        data: &[u8],
    ) -> Result<Vec<u8>, patina_mm::component::communicator::Status> {
        // Increment trigger count at the start of every communication attempt
        self.trigger_count.fetch_add(1, Ordering::Relaxed);

        // Create a buffer for the message
        let mut buffer = vec![0u8; TEST_BUFFER_SIZE];

        // Write the message to the buffer
        let mut parser = MmMessageParser::new(&mut buffer);
        parser.write_message(guid, data).map_err(|_| patina_mm::component::communicator::Status::InvalidDataBuffer)?;

        // Process the message with our handlers
        let handlers =
            self.handlers.lock().map_err(|_| patina_mm::component::communicator::Status::CommBufferNotFound)?;

        if let Some(handler) = handlers.get(guid) {
            match handler.handle_request(data) {
                Ok(response) => Ok(response),
                Err(_) => Err(patina_mm::component::communicator::Status::InvalidDataBuffer),
            }
        } else {
            Err(patina_mm::component::communicator::Status::CommBufferNotFound)
        }
    }

    /// Perform MM communication with a pre-allocated buffer (for testing error conditions)
    #[allow(dead_code)] // Usage in integration code is not recognized
    pub fn communicate_with_buffer(
        &self,
        _guid: &efi::Guid,
        buffer: &mut [u8],
    ) -> Result<Vec<u8>, patina_mm::component::communicator::Status> {
        // Increment trigger count at the start of every communication attempt
        self.trigger_count.fetch_add(1, Ordering::Relaxed);

        if buffer.is_empty() {
            return Err(patina_mm::component::communicator::Status::InvalidDataBuffer);
        }

        if buffer.len() < MmMessageParser::required_buffer_size(0) {
            return Err(patina_mm::component::communicator::Status::CommBufferTooSmall);
        }

        // Try to parse the message
        let parser = MmMessageParser::new(buffer);
        match parser.parse_message() {
            Ok((parsed_guid, message_data)) => {
                let handlers =
                    self.handlers.lock().map_err(|_| patina_mm::component::communicator::Status::CommBufferNotFound)?;

                if let Some(handler) = handlers.get(&parsed_guid) {
                    match handler.handle_request(message_data) {
                        Ok(response) => Ok(response),
                        Err(_) => Err(patina_mm::component::communicator::Status::InvalidDataBuffer),
                    }
                } else {
                    Err(patina_mm::component::communicator::Status::CommBufferNotFound)
                }
            }
            Err(_) => Err(patina_mm::component::communicator::Status::InvalidDataBuffer),
        }
    }

    /// Get the number of times MM communication was triggered
    #[allow(dead_code)] // Usage is not recognized
    pub fn get_trigger_count(&self) -> usize {
        self.trigger_count.load(Ordering::Relaxed)
    }

    /// Reset the trigger count (useful for testing scenarios)
    #[allow(dead_code)] // Used in stress testing scenarios
    pub fn reset_trigger_count(&self) {
        self.trigger_count.store(0, Ordering::Relaxed);
    }
}

/// A builder for the Patina MM Test Framework
///
/// Enables a test author to easily configure a specific testing environment by adding
/// handlers for specific GUIDs.
///
/// ## Design Note
///
/// The HashMap used to store handlers during the build phase is wrapped in `Arc<Mutex<...>>`
/// only when the framework is built (`.build()`). This is intended to optimize the common
/// case where the builder is used in a single-threaded context to configure the framework. This
/// avoids the overhead of synchronization primitives during the build phase, where they are not
/// needed.
pub struct MmTestFrameworkBuilder {
    /// Handler storage used during the "construction phase"
    ///
    /// This HashMap is owned exclusively by the builder and accessed only during
    /// the single-threaded configuration phase. Handlers are moved in via `Box<dyn MmHandler>`
    /// and then the entire collection is transferred to the framework's `Arc<Mutex<...>>``
    /// wrapper during the `.build()` call.
    handlers: HashMap<efi::Guid, Box<dyn MmHandler>>,
}

impl MmTestFrameworkBuilder {
    fn new() -> Self {
        Self { handlers: HashMap::new() }
    }

    /// Add a custom handler for the specified GUID
    pub fn with_handler(mut self, guid: efi::Guid, handler: Box<dyn MmHandler>) -> Self {
        self.handlers.insert(guid, handler);
        self
    }

    /// Add the standard echo handler
    pub fn with_echo_handler(self) -> Self {
        self.with_handler(TEST_COMMUNICATION_GUID, Box::new(EchoHandler::new()))
    }

    /// Add the MM supervisor handler
    pub fn with_mm_supervisor_handler(self) -> Self {
        self.with_handler(test_guids::MM_SUPERVISOR, Box::new(MmSupervisorHandler::new()))
    }

    /// Add a version info handler
    #[allow(dead_code)] // Reserved for future version handler tests
    pub fn with_version_handler(self, guid: efi::Guid, version: &str) -> Self {
        self.with_handler(guid, Box::new(VersionInfoHandler::new(version)))
    }

    /// Add an error injection handler for testing error conditions
    #[allow(dead_code)] // Used in stress testing scenarios
    pub fn with_error_injection_handler(self, guid: efi::Guid) -> Self {
        self.with_handler(guid, Box::new(ErrorInjectionHandler::new()))
    }

    /// Add a buffer size handler for testing various buffer scenarios
    #[allow(dead_code)] // Used in stress testing scenarios
    pub fn with_buffer_size_handler(self, guid: efi::Guid) -> Self {
        self.with_handler(guid, Box::new(BufferSizeHandler::new()))
    }

    /// Add a computation handler for stress testing
    #[allow(dead_code)] // Used in stress testing scenarios
    pub fn with_computation_handler(self, guid: efi::Guid) -> Self {
        self.with_handler(guid, Box::new(ComputationHandler::new()))
    }

    /// Build the test framework
    ///
    /// Tansfers the collected handlers from the  builder's simple HashMap into the
    /// framework's thread-safe `Arc<Mutex<HashMap>>`.
    ///
    /// ## Ownership Transfer Note
    ///
    /// This method is a transition point where ownership of the handler registry
    /// moves from the single-threaded builder to the multi-threaded framework instance.
    ///
    /// - Handler cloning is not used (`Box<dyn MmHandler>` are moved)
    /// - THe HashMap structure is preserved (only wrapped, not reconstructed)
    /// - Arc reference counting starts at 1 (framework owns the initial reference)
    ///
    /// After this call, the builder is consumed and cannot be reused..
    pub fn build(self) -> Result<MmTestFramework, TestFrameworkError> {
        Ok(MmTestFramework {
            handlers: Arc::new(Mutex::new(self.handlers)),
            trigger_count: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framework_builder() {
        let framework = MmTestFramework::builder().with_echo_handler().with_mm_supervisor_handler().build();

        assert!(framework.is_ok(), "Framework creation should succeed");
    }

    #[test]
    fn test_echo_communication() {
        let framework =
            MmTestFramework::builder().with_echo_handler().build().expect("Framework creation should succeed");

        let test_data = b"Hello, test!";
        let result = framework.communicate(&TEST_COMMUNICATION_GUID, test_data);

        assert!(result.is_ok(), "Communication should succeed");
        assert_eq!(result.unwrap(), test_data, "Should echo the same data");
    }

    #[test]
    fn test_message_parser_integration() {
        let mut buffer = vec![0u8; 128];
        let test_guid = TEST_COMMUNICATION_GUID;
        let test_data = b"Integration test";

        let mut parser = MmMessageParser::new(&mut buffer);
        let write_result = parser.write_message(&test_guid, test_data);
        assert!(write_result.is_ok(), "Writing message should succeed");

        let parse_result = parser.parse_message();
        assert!(parse_result.is_ok(), "Parsing message should succeed");

        let (parsed_guid, parsed_data) = parse_result.unwrap();
        assert_eq!(parsed_guid, test_guid, "GUID should match");
        assert_eq!(parsed_data, test_data, "Data should match");
    }
}
