//! MM Communication Core Functionality Tests
//!
//! Tests core MM communication functionality using a lightweight test framework
//! that exercises the fundamental patterns used in MM communication.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::Guid;
use patina_mm::component::communicator::{MmCommunication, MmCommunicator, MmExecutor, Status};
use patina_mm::config::{CommunicateBuffer, EfiMmCommunicateHeader};

use core::pin::Pin;
use std::collections::HashMap;

extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

/// Lightweight MM handler used for testing
struct TestHandler {
    guid: r_efi::efi::Guid, //The r_efi GUID is kept for the internal GUID here and concerted as needed
    response_data: Vec<u8>,
}

impl TestHandler {
    fn new(guid: r_efi::efi::Guid, response_data: Vec<u8>) -> Self {
        Self { guid, response_data }
    }
}

/// Simple MM executor used for testing
struct CoreTestExecutor {
    handlers: HashMap<r_efi::efi::Guid, TestHandler>,
}

impl CoreTestExecutor {
    fn new() -> Self {
        Self { handlers: HashMap::new() }
    }

    fn add_handler(&mut self, handler: TestHandler) {
        self.handlers.insert(handler.guid, handler);
    }
}

impl MmExecutor for CoreTestExecutor {
    fn execute_mm(&self, comm_buffer: &mut CommunicateBuffer) -> Result<(), Status> {
        let recipient_guid = comm_buffer
            .get_header_guid()
            .map_err(|_| Status::CommBufferInitError)?
            .ok_or(Status::CommBufferInitError)?;

        let handler = self.handlers.get(&recipient_guid.to_efi_guid()).ok_or(Status::CommBufferNotFound)?;

        // Set response (need to clone the recipient_guid to avoid borrow conflicts)
        let recipient_copy = Guid::from_bytes(&recipient_guid.as_bytes());
        comm_buffer.reset();
        comm_buffer.set_message_info(recipient_copy).map_err(|_| Status::CommBufferInitError)?;
        comm_buffer.set_message(&handler.response_data).map_err(|_| Status::CommBufferInitError)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_GUID: r_efi::efi::Guid =
        r_efi::efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

    fn create_test_communicator() -> MmCommunicator {
        let mut executor = CoreTestExecutor::new();
        executor.add_handler(TestHandler::new(TEST_GUID, b"test response".to_vec()));

        let buffers = vec![CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 0)];

        let communicator = MmCommunicator::with_executor(Box::new(executor));
        communicator.set_test_comm_buffers(buffers);
        communicator
    }

    #[test]
    fn test_basic_communication() {
        let communicator = create_test_communicator();
        let recipient = Guid::from_ref(&TEST_GUID);

        let result = communicator.communicate(0, b"test data", recipient);

        assert!(result.is_ok(), "Basic communication should succeed");
        assert_eq!(result.unwrap(), b"test response".to_vec());
    }

    #[test]
    fn test_communication_with_different_data_sizes() {
        let communicator = create_test_communicator();

        // Test with various data sizes (skip empty since it should fail)
        let test_cases = vec![
            b"a".to_vec(),     // Single byte
            b"small".to_vec(), // Small
            vec![0x42; 100],   // Medium
            vec![0x55; 500],   // Large (but fits in buffer)
        ];

        for test_data in test_cases {
            let recipient = Guid::from_ref(&TEST_GUID);
            let result = communicator.communicate(0, &test_data, recipient);
            assert!(result.is_ok(), "Communication should succeed for data size: {}", test_data.len());
            assert_eq!(result.unwrap(), b"test response".to_vec());
        }
    }

    #[test]
    fn test_communication_too_large_for_buffer() {
        let communicator = create_test_communicator();
        let recipient = Guid::from_ref(&TEST_GUID);

        // Create data that's too large for the buffer
        let max_size = 1024 - EfiMmCommunicateHeader::size();
        let too_large_data = vec![0x99; max_size + 1];

        let result = communicator.communicate(0, &too_large_data, recipient);

        assert!(result.is_err(), "Communication should fail for data that is too large");
        assert_eq!(result.unwrap_err(), Status::CommBufferTooSmall);
    }

    #[test]
    fn test_communication_unknown_handler() {
        let communicator = create_test_communicator();
        let unknown_guid = r_efi::efi::Guid::from_fields(
            0xDEADBEEF,
            0xCAFE,
            0xABCD,
            0xAA,
            0xBB,
            &[0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11],
        );

        let result = communicator.communicate(0, b"test", Guid::from_ref(&unknown_guid));

        assert!(result.is_err(), "Communication should fail for an unknown handler");
        assert_eq!(result.unwrap_err(), Status::CommBufferNotFound);
    }

    #[test]
    fn test_communication_simple_error_conditions() {
        let communicator = create_test_communicator();

        // Test empty data
        let result = communicator.communicate(0, &[], Guid::from_ref(&TEST_GUID));
        assert!(result.is_err(), "Empty data should fail");
        assert_eq!(result.unwrap_err(), Status::InvalidDataBuffer);

        // Test non-existent buffer
        let result = communicator.communicate(99, b"test", Guid::from_ref(&TEST_GUID));
        assert!(result.is_err(), "Non-existent buffer should fail");
        assert_eq!(result.unwrap_err(), Status::CommBufferNotFound);
    }

    #[test]
    fn test_multiple_sequential_communications() {
        let communicator = create_test_communicator();

        // Perform multiple communications in sequence
        for i in 0..5 {
            let test_data = format!("test data {}", i);
            let result = communicator.communicate(0, test_data.as_bytes(), Guid::from_ref(&TEST_GUID));
            assert!(result.is_ok(), "Sequential communication {} should succeed", i);
            assert_eq!(result.unwrap(), b"test response".to_vec());
        }
    }

    #[test]
    fn test_buffer_state_consistency() {
        let communicator = create_test_communicator();

        let result1 = communicator.communicate(0, b"first", Guid::from_ref(&TEST_GUID));
        assert!(result1.is_ok(), "First communication should succeed");

        let result2 = communicator.communicate(0, b"second", Guid::from_ref(&TEST_GUID));
        assert!(result2.is_ok(), "Second communication should succeed");

        // Both should return the same response data
        assert_eq!(result1.unwrap(), result2.unwrap());
    }

    #[test]
    fn test_safe_message_parsing() {
        // Basic test to verify the framework works with message parsing
        let test_guid = r_efi::efi::Guid::from_fields(
            0x12345678,
            0x1234,
            0x5678,
            0x12,
            0x34,
            &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef],
        );
        let test_data = b"Integration test";

        // This test validates that GUIDs and data can be safely handled
        assert_eq!(test_guid.as_bytes().len(), 16, "GUID should be 16 bytes");
        assert_eq!(test_data.len(), 16, "Test data length should match");
    }
}
