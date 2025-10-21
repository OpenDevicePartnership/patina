//! MM Communication Stress Tests
//!
//! Runs a lot of testa against the real MM Communicator code.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::patina_mm_integration::common::{constants::*, framework::*};

extern crate alloc;
use alloc::{boxed::Box, format, vec, vec::Vec};
use core::pin::Pin;
use patina::component::Storage;
use patina::component::service::Service;
use patina::{Guid, base::SIZE_4KB};
use patina_mm::component::communicator::{MmCommunication, MmCommunicator, MmExecutor};
use patina_mm::config::CommunicateBuffer;

/// Additional test GUIDs for stress testing
mod stress_guids {
    use r_efi::efi;

    pub const ERROR_INJECTION: efi::Guid =
        efi::Guid::from_fields(0xdeadbeef, 0x1111, 0x2222, 0x33, 0x44, &[0x55, 0x66, 0x77, 0x88, 0x99, 0xaa]);

    pub const BUFFER_SIZE_TEST: efi::Guid =
        efi::Guid::from_fields(0xcafebabe, 0x3333, 0x4444, 0x55, 0x66, &[0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc]);

    pub const COMPUTATION_TEST: efi::Guid =
        efi::Guid::from_fields(0xfeedf00d, 0x5555, 0x6666, 0x77, 0x88, &[0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]);
}

/// Mock MM Executor for stress testing that uses the test framework
struct StressTestMmExecutor {
    framework: MmTestFramework,
}

impl StressTestMmExecutor {
    fn new(framework: MmTestFramework) -> Self {
        Self { framework }
    }
}

impl MmExecutor for StressTestMmExecutor {
    fn execute_mm(
        &self,
        comm_buffer: &mut CommunicateBuffer,
    ) -> Result<(), patina_mm::component::communicator::Status> {
        // Extract the GUID and data from the communication buffer
        let data =
            comm_buffer.get_message().map_err(|_| patina_mm::component::communicator::Status::InvalidDataBuffer)?;

        // Extract the GUID from the header and convert to an owned efi::Guid
        let guid = comm_buffer
            .get_header_guid()
            .map_err(|_| patina_mm::component::communicator::Status::InvalidDataBuffer)?
            .ok_or(patina_mm::component::communicator::Status::InvalidDataBuffer)?
            .to_efi_guid(); // Convert to owned efi::Guid to avoid borrowing issues

        // Use the test framework to process the message
        match self.framework.communicate(&guid, &data) {
            Ok(response) => {
                // Set the response back in the buffer
                comm_buffer.reset();
                comm_buffer
                    .set_message_info(Guid::from_ref(&guid))
                    .map_err(|_| patina_mm::component::communicator::Status::CommBufferInitError)?;
                comm_buffer
                    .set_message(&response)
                    .map_err(|_| patina_mm::component::communicator::Status::CommBufferInitError)?;
                Ok(())
            }
            Err(status) => Err(status),
        }
    }
}

/// Test data generator for creating various test scenarios
struct TestDataGenerator {
    counter: usize,
}

impl TestDataGenerator {
    fn new() -> Self {
        Self { counter: 0 }
    }

    /// Generate test data for various edge cases
    fn generate_test_data(&mut self) -> Vec<u8> {
        let data = match self.counter % 10 {
            0 => vec![],                                                 // Empty data
            1 => vec![0x00],                                             // Single null byte
            2 => vec![0xFF],                                             // Single 0xFF byte (triggers error injection)
            3 => b"FAIL".to_vec(),                                       // Failure pattern
            4 => vec![0x42; 16],                                         // Small buffer
            5 => vec![0x42; 64],                                         // Medium buffer
            6 => (0..=255).collect::<Vec<u8>>(),                         // Sequential bytes
            7 => b"Hello, MM World!".to_vec(),                           // Text data
            8 => format!("Test message #{}", self.counter).into_bytes(), // Dynamic text
            9 => vec![0xAA, 0xBB, 0xCC, 0xDD],                           // Fixed pattern
            _ => unreachable!(),
        };

        self.counter += 1;
        data
    }
}

/// Create a configured MM Communicator for stress testing
fn create_stress_test_communicator() -> (MmCommunicator, MmTestFramework) {
    // Create a comprehensive test framework with all handler types
    let framework = MmTestFramework::builder()
        .with_echo_handler()
        .with_mm_supervisor_handler()
        .with_error_injection_handler(stress_guids::ERROR_INJECTION)
        .with_buffer_size_handler(stress_guids::BUFFER_SIZE_TEST)
        .with_computation_handler(stress_guids::COMPUTATION_TEST)
        .build()
        .expect("Framework creation should succeed");

    // Create MM Communicator with our test executor
    let executor = Box::new(StressTestMmExecutor::new(framework.clone()));
    let communicator = MmCommunicator::with_executor(executor);

    // Create communication buffers for testing
    let buffers = vec![
        CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; SIZE_4KB]))), 0),
        CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; SIZE_4KB]))), 1),
        CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; SIZE_4KB * 2]))), 2), // Larger buffer
    ];

    communicator.set_test_comm_buffers(buffers);

    (communicator, framework)
}

/// Test basic trigger counting functionality
#[test]
fn test_framework_trigger_counting_basic() {
    let framework = MmTestFramework::builder().with_echo_handler().build().expect("Framework creation should succeed");

    // Initially should be zero
    assert_eq!(framework.get_trigger_count(), 0);

    // Test a few communication calls
    let test_data = b"test data";
    let _result1 = framework.communicate(&TEST_COMMUNICATION_GUID, test_data);
    assert_eq!(framework.get_trigger_count(), 1);

    let _result2 = framework.communicate(&TEST_COMMUNICATION_GUID, test_data);
    assert_eq!(framework.get_trigger_count(), 2);

    // Reset should work
    framework.reset_trigger_count();
    assert_eq!(framework.get_trigger_count(), 0);
}

/// Test MM communication stress with 1000 calls
#[test]
fn test_mm_communication_stress_thousand_calls() {
    const NUM_ITERATIONS: usize = 1000;

    let (communicator, framework) = create_stress_test_communicator();

    // Reset trigger count before starting
    framework.reset_trigger_count();

    // Wrap communicator in Service for testing
    let mut storage = Storage::new();
    storage.add_service(communicator);
    let comm_service: Service<dyn MmCommunication> = storage.get_service().expect("Service should be available");

    let mut data_generator = TestDataGenerator::new();
    let mut success_count = 0;
    let mut error_count = 0;
    let mut various_errors = std::collections::HashMap::new();

    // Test GUIDs to cycle through
    let test_guids = [
        TEST_COMMUNICATION_GUID,        // Echo handler
        test_guids::MM_SUPERVISOR,      // MM Supervisor
        stress_guids::ERROR_INJECTION,  // Error injection
        stress_guids::BUFFER_SIZE_TEST, // Buffer size tests
        stress_guids::COMPUTATION_TEST, // Computation tests
    ];

    println!("Starting stress test with {} iterations...", NUM_ITERATIONS);

    for i in 0..NUM_ITERATIONS {
        // Cycle through different GUIDs and buffer IDs
        let guid = &test_guids[i % test_guids.len()];
        let buffer_id = (i % 3) as u8; // Cycle through the 3 buffers
        let test_data = data_generator.generate_test_data();

        // Perform MM communication
        let result = comm_service.communicate(buffer_id, &test_data, Guid::from_ref(guid));

        match result {
            Ok(response) => {
                success_count += 1;

                // Basic validation for echo handler
                if guid == &TEST_COMMUNICATION_GUID {
                    // Echo handler should return the same data for non-empty inputs
                    if !test_data.is_empty() {
                        assert_eq!(response, test_data, "Echo handler should return input data at iteration {}", i);
                    }
                }
            }
            Err(status) => {
                error_count += 1;
                *various_errors.entry(format!("{:?}", status)).or_insert(0) += 1;
            }
        }

        // Progress reporting every 100 iterations
        if (i + 1) % 100 == 0 {
            println!("Completed {} iterations (Success: {}, Errors: {})", i + 1, success_count, error_count);
        }
    }

    // Verify framework trigger counting behavior
    // Note: Some communications may fail early validation in MmCommunicator before reaching the framework
    let trigger_count = framework.get_trigger_count();
    let total_attempted = success_count + error_count;

    // Trigger count should be <= total attempted (since some may fail early validation)
    assert!(
        trigger_count <= total_attempted,
        "Framework trigger count ({}) should not exceed total attempted communications ({})",
        trigger_count,
        total_attempted
    );

    // Most communications should reach the framework (at least 80% for this test)
    let min_expected = (total_attempted as f64 * 0.8) as usize;
    assert!(
        trigger_count >= min_expected,
        "Framework trigger count ({}) should be at least 80% of attempted communications ({})",
        trigger_count,
        min_expected
    );

    // Ensure we had a reasonable mix of successes and errors
    assert!(success_count > 0, "Should have some successful communications");
    assert!(error_count > 0, "Should have some error scenarios for comprehensive testing");

    // Verify we had some early validation failures (showing the full MM communication pipeline)
    let early_failures = total_attempted - trigger_count;
    assert!(early_failures > 0, "Should have had at least some early validation failure");

    // Print final statistics
    println!("Stress test completed!");
    println!("Total iterations: {}", NUM_ITERATIONS);
    println!("Successful communications: {}", success_count);
    println!("Error communications: {}", error_count);
    println!("Communications that reached framework: {}", trigger_count);
    println!("Early validation failures: {}", early_failures);
    println!(
        "Trigger count validation: Framework correctly tracked {} out of {} communications ({:.1}%)",
        trigger_count,
        total_attempted,
        (trigger_count as f64 / total_attempted as f64) * 100.0
    );
    println!("Failed communications: {}", error_count);
    println!("Success rate: {:.2}%", (success_count as f64 / NUM_ITERATIONS as f64) * 100.0);

    if !various_errors.is_empty() {
        println!("Error breakdown:");
        for (error_type, count) in various_errors {
            println!("  {}: {}", error_type, count);
        }
    }

    // We expect at least some communications to succeed
    assert!(success_count > 0, "At least some communications should succeed");

    // Verify the framework handled all our requests
    assert!(trigger_count > 0, "Trigger count should be greater than 0");

    println!("Stress test passed with {} successful communications out of {}", success_count, NUM_ITERATIONS);
}
