//! MM Communicator Integration Tests
//!
//! This test file focuses on testing the MmCommunicator component integration
//! with its dependencies using the actual component entry point flow.
//!
//! ## Logging
//!
//! - The `real_test_framework` log target is used for logging within the real component test framework.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::Guid;
use patina::component::{IntoComponent, Storage};
use patina_mm::component::communicator::{MmCommunication, MmCommunicator};
use patina_mm::component::sw_mmi_manager::SwMmiManager;
use patina_mm::config::{CommunicateBuffer, MmCommunicationConfiguration};
use r_efi::efi;

use core::pin::Pin;

use crate::patina_mm_integration::common::*;

static TEST_RECIPIENT: efi::Guid =
    efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

#[test]
fn test_mm_communicator_component_initialization() {
    let mut storage = Storage::new();

    // Set up required configuration with communication buffers
    let config = MmCommunicationConfiguration {
        comm_buffers: vec![
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 0),
            CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 2048]))), 1),
        ],
        ..Default::default()
    };
    storage.add_config(config);

    // Add required SW MMI manager service
    storage.add_service(SwMmiManager::new());

    // Test that the component can be initialized and run
    let mut communicator = MmCommunicator::new().into_component();
    communicator.initialize(&mut storage);
    assert_eq!(communicator.run(&mut storage), Ok(true));

    // Verify that the MmCommunication service is now available
    let service_result = storage.get_service::<dyn MmCommunication>();
    assert!(service_result.is_some(), "MmCommunication service should be available after component initialization");
}

#[test]
fn test_mm_communicator_with_empty_config() {
    let mut storage = Storage::new();

    // Set up minimal configuration with no communication buffers
    storage.add_config(MmCommunicationConfiguration::default());
    storage.add_service(SwMmiManager::new());

    let mut communicator = MmCommunicator::new().into_component();
    communicator.initialize(&mut storage);
    assert_eq!(communicator.run(&mut storage), Ok(true));

    // Service should still be available even with no buffers
    let service_result = storage.get_service::<dyn MmCommunication>();
    assert!(service_result.is_some(), "MmCommunication service should be available even with empty config");

    // Communication should fail due to no buffers
    let service = service_result.unwrap();
    let result = service.communicate(0, b"test", Guid::from_ref(&TEST_RECIPIENT));
    assert!(result.is_err(), "Communication should fail with no buffers configured");
}

#[test]
fn test_mm_communicator_without_sw_mmi_service() {
    let mut storage = Storage::new();

    // Set up configuration but missing SW MMI service
    storage.add_config(MmCommunicationConfiguration::default());
    // NOte: Deliberately not adding the SwMmiManager service

    let mut communicator = MmCommunicator::new().into_component();
    communicator.initialize(&mut storage);

    // Component should not be able to run due to missing dependency
    assert_eq!(communicator.run(&mut storage), Ok(false));
}

#[test]
fn test_mm_communicator_dependency_injection() {
    let mut storage = Storage::new();

    // Set up all required dependencies
    let config = MmCommunicationConfiguration {
        comm_buffers: vec![CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 512]))), 5)],
        ..Default::default()
    };
    storage.add_config(config);
    storage.add_service(SwMmiManager::new());

    // Initialize multiple components to test dependency injection
    let mut communicator1 = MmCommunicator::new().into_component();
    let mut communicator2 = MmCommunicator::new().into_component();

    communicator1.initialize(&mut storage);
    communicator2.initialize(&mut storage);

    // First component should run successfully
    assert_eq!(communicator1.run(&mut storage), Ok(true));

    // Second component should also run since it's a different component instance
    assert_eq!(communicator2.run(&mut storage), Ok(true));

    // The service should still be available
    let service_result = storage.get_service::<dyn MmCommunication>();
    assert!(service_result.is_some(), "MmCommunication service should be available");
}

// Integration tests using the common framework
use std::sync::Once;

static INIT: Once = Once::new();

fn init_logger() {
    INIT.call_once(|| {
        // Default to no logging unless RUST_LOG environment variable is set
        let mut builder = env_logger::Builder::from_default_env();

        // If RUST_LOG is not set, default to Off (no logging)
        if std::env::var("RUST_LOG").is_err() {
            builder.filter_level(log::LevelFilter::Off);
        }

        builder.init();
    });
}

#[test]
fn test_real_component_echo_communication() {
    init_logger();
    let framework = RealComponentMmTestFramework::builder()
        .with_echo_handler()
        .build()
        .expect("Real component framework should initialize successfully");

    let test_data = b"Hello, Real MM Components!";
    let result = framework.communicate(&Guid::from_ref(&TEST_COMMUNICATION_GUID), test_data);

    assert!(result.is_ok(), "Real component communication should succeed: {:?}", result.err());
    let response = result.unwrap();
    assert_eq!(response, test_data, "Echo handler should return the same data");
}

#[test]
fn test_real_component_mm_supervisor_version_request() {
    init_logger();
    let framework = RealComponentMmTestFramework::builder()
        .with_mm_supervisor_handler()
        .build()
        .expect("Real component framework should initialize successfully");

    // Create MM Supervisor version request using the actual structures
    let version_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::VERSION_INFO,
        reserved: 0,
        result: 0,
    };

    let request_bytes = version_request.to_bytes();

    // Send the request using the real component framework
    let result = framework.communicate(&Guid::from_ref(&test_guids::MM_SUPERVISOR), &request_bytes);
    assert!(result.is_ok(), "Real component MM Supervisor communication should succeed: {:?}", result.err());

    let response = result.unwrap();

    // Verify response size matches expected structure
    let expected_response_size =
        core::mem::size_of::<MmSupervisorRequestHeader>() + core::mem::size_of::<MmSupervisorVersionInfo>();
    assert_eq!(response.len(), expected_response_size, "Response should have correct size");

    // Parse response header safely
    let response_header =
        MmSupervisorRequestHeader::from_bytes(&response).expect("Should parse response header from real component");

    // Verify header fields
    assert_eq!(response_header.signature, mm_supv::REQUEST_SIGNATURE, "Response signature should match");
    assert_eq!(response_header.revision, mm_supv::REVISION, "Response revision should match");
    assert_eq!(response_header.request, mm_supv::requests::VERSION_INFO, "Response request type should match");
    assert_eq!(response_header.result, 0, "Response should indicate success");

    // Parse version info from response
    let version_info_offset = core::mem::size_of::<MmSupervisorRequestHeader>();
    let version_info_bytes = &response[version_info_offset..];
    let version_info = MmSupervisorVersionInfo::from_bytes(version_info_bytes)
        .expect("Should parse version info from real component response");

    // Verify version info
    assert_eq!(version_info.version, mm_supv::VERSION, "Version should match expected value");
    assert_eq!(version_info.patch_level, mm_supv::PATCH_LEVEL, "Patch level should match expected value");

    log::debug!(target: "real_test_framework", "MM Supervisor version returned: {:#X}", version_info.version);
    log::debug!(target: "real_test_framework", "MM Supervisor patch level returned: {:#X}", version_info.patch_level);

    assert_eq!(
        version_info.max_supervisor_request_level,
        mm_supv::MAX_REQUEST_LEVEL,
        "Max request level should match expected value"
    );
}

#[test]
fn test_real_component_invalid_guid_communication() {
    let framework = RealComponentMmTestFramework::builder()
        .with_echo_handler()
        .build()
        .expect("Real component framework should initialize successfully");

    // Use an unknown GUID that has no registered handler
    let unknown_guid = r_efi::efi::Guid::from_fields(0xFFFFFFFF, 0xFFFF, 0xFFFF, 0xFF, 0xFF, &[0xFF; 6]);
    let test_data = b"This should fail";

    let result = framework.communicate(&Guid::from_ref(&unknown_guid), test_data);

    // The real components should properly handle this error case
    assert!(result.is_err(), "Communication with unknown GUID should fail");

    // Verify we get the expected error type from the real communicator
    match result.unwrap_err() {
        patina_mm::component::communicator::Status::CommBufferNotFound => {
            // This is the expected error when no handler is found
        }
        other => {
            panic!("Expected CommBufferNotFound error, got: {:?}", other);
        }
    }
}

#[test]
fn test_real_component_empty_data_validation() {
    let framework = RealComponentMmTestFramework::builder()
        .with_echo_handler()
        .build()
        .expect("Real component framework should initialize successfully");

    let empty_data = &[];
    let result = framework.communicate(&Guid::from_ref(&TEST_COMMUNICATION_GUID), empty_data);

    // The real components should validate input data
    assert!(result.is_err(), "Communication with empty data should fail");

    // Verify we get the expected error type
    match result.unwrap_err() {
        patina_mm::component::communicator::Status::InvalidDataBuffer => {
            // This is the expected error for invalid input
        }
        other => {
            panic!("Expected InvalidDataBuffer error, got: {:?}", other);
        }
    }
}

#[test]
fn test_real_component_multiple_handlers() {
    init_logger();

    let framework = RealComponentMmTestFramework::builder()
        .with_echo_handler()
        .with_mm_supervisor_handler()
        .build()
        .expect("Real component framework should initialize successfully");

    // Test echo handler
    let echo_data = b"Echo test with real components";
    let echo_result = framework.communicate(&Guid::from_ref(&TEST_COMMUNICATION_GUID), echo_data);
    assert!(echo_result.is_ok(), "Echo communication should succeed");
    assert_eq!(echo_result.unwrap(), echo_data, "Echo should return same data");

    // Test MM supervisor handler
    let supervisor_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::FETCH_POLICY,
        reserved: 0,
        result: 0,
    };

    let supervisor_result =
        framework.communicate(&Guid::from_ref(&test_guids::MM_SUPERVISOR), &supervisor_request.to_bytes());
    assert!(supervisor_result.is_ok(), "Supervisor communication should succeed");

    // Both handlers should work independently through the real component infrastructure
    let supervisor_response = supervisor_result.unwrap();
    assert!(!supervisor_response.is_empty(), "Supervisor should return response data");
}
