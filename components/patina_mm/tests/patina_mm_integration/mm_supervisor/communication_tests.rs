//! MM Supervisor Communication Tests
//!
//! This module contains comprehensive integration tests for the MM Supervisor
//! communication pattern using the unified test framework.
//!
//! ## Logging
//!
//! - The `comm_update_test` log target is used for logging within the communication update test.
//! - The `unblock_mem_test` log target is used for logging within the unblock memory test.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::patina_mm_integration::common::*;

#[test]
fn test_mm_supervisor_version_request_integration() {
    let framework = MmTestFramework::builder()
        .with_mm_supervisor_handler()
        .build()
        .expect("Framework should initialize successfully");

    // Create MM Supervisor version request using safe operations
    let version_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::VERSION_INFO,
        reserved: 0,
        result: 0,
    };

    let request_bytes = version_request.to_bytes();

    // Send the request using framework
    let result = framework.communicate(&test_guids::MM_SUPERVISOR, &request_bytes);
    assert!(result.is_ok(), "MM Supervisor communication should succeed: {:?}", result.err());

    let response = result.unwrap();

    // Verify response size
    let expected_response_size =
        core::mem::size_of::<MmSupervisorRequestHeader>() + core::mem::size_of::<MmSupervisorVersionInfo>();
    assert_eq!(response.len(), expected_response_size, "Response should have correct size");

    // Parse response header safely
    let response_header = MmSupervisorRequestHeader::from_bytes(&response).expect("Should parse response header");

    assert_eq!(response_header.signature, version_request.signature, "Signature should match");
    assert_eq!(response_header.revision, version_request.revision, "Revision should match");
    assert_eq!(response_header.request, version_request.request, "Request type should match");
    assert_eq!(response_header.result, mm_supv::responses::SUCCESS, "Result should be success");

    // Parse version info safely
    let version_info_offset = core::mem::size_of::<MmSupervisorRequestHeader>();
    let version_info =
        MmSupervisorVersionInfo::from_bytes(&response[version_info_offset..]).expect("Should parse version info");

    assert_eq!(version_info.version, 0x00130008, "Version should match");
    assert_eq!(version_info.patch_level, 0x00010001, "Patch level should match");
    assert_eq!(version_info.max_supervisor_request_level, 0x0000000000000004, "Max request level should match");

    // Verify SW MMI was triggered
    assert_eq!(framework.get_trigger_count(), 1, "SW MMI should be triggered once");
}

#[test]
fn test_mm_supervisor_capabilities_request() {
    let framework = MmTestFramework::builder()
        .with_mm_supervisor_handler()
        .build()
        .expect("Framework should initialize successfully");

    // Create capabilities request using safe operations
    let capabilities_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::FETCH_POLICY,
        reserved: 0,
        result: 0,
    };

    let request_bytes = capabilities_request.to_bytes();

    // Send the request using framework
    let result = framework.communicate(&test_guids::MM_SUPERVISOR, &request_bytes);
    assert!(result.is_ok(), "MM Supervisor capabilities communication should succeed");

    let response = result.unwrap();

    // Verify response contains header + capabilities
    let expected_size = core::mem::size_of::<MmSupervisorRequestHeader>() + 8; // + u64 capabilities
    assert_eq!(response.len(), expected_size, "Capabilities response should have correct size");

    // Parse response header safely
    let response_header = MmSupervisorRequestHeader::from_bytes(&response).expect("Should parse response header");

    assert_eq!(response_header.result, mm_supv::responses::SUCCESS, "Capabilities request should succeed");

    // Parse capabilities safely
    let capabilities_offset = core::mem::size_of::<MmSupervisorRequestHeader>();
    let capabilities = u64::from_le_bytes([
        response[capabilities_offset],
        response[capabilities_offset + 1],
        response[capabilities_offset + 2],
        response[capabilities_offset + 3],
        response[capabilities_offset + 4],
        response[capabilities_offset + 5],
        response[capabilities_offset + 6],
        response[capabilities_offset + 7],
    ]);

    assert_eq!(capabilities, 0x00000007, "Capabilities should match expected value");
}

#[test]
fn test_mm_supervisor_invalid_request() {
    let framework = MmTestFramework::builder()
        .with_mm_supervisor_handler()
        .build()
        .expect("Framework should initialize successfully");

    // Create invalid request using safe operations
    let invalid_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: 0xFFFF, // Invalid request type
        reserved: 0,
        result: 0,
    };

    let request_bytes = invalid_request.to_bytes();

    // Send the request using framework
    let result = framework.communicate(&test_guids::MM_SUPERVISOR, &request_bytes);
    assert!(result.is_ok(), "Communication should succeed even with invalid request");

    let response = result.unwrap();

    // Parse response header safely
    let response_header = MmSupervisorRequestHeader::from_bytes(&response).expect("Should parse response header");

    assert_eq!(response_header.result, mm_supv::responses::ERROR, "Invalid request should return error");
}

#[test]
fn test_mm_supervisor_invalid_signature() {
    // Test handler directly with invalid signature
    let mm_supervisor = MmSupervisorHandler::new();

    // Create request with invalid signature using safe operations
    let invalid_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes([b'I', b'N', b'V', b'D']), // Invalid signature
        revision: mm_supv::REVISION,
        request: mm_supv::requests::VERSION_INFO,
        reserved: 0,
        result: 0,
    };

    let request_bytes = invalid_request.to_bytes();

    // Test handler directly
    let result = mm_supervisor.handle_request(&request_bytes);
    assert!(result.is_err(), "Invalid signature should cause handler to fail");

    let error_msg = format!("{}", result.unwrap_err());
    assert!(
        error_msg.contains("Invalid signature") || error_msg.contains("Invalid request"),
        "Error message should mention invalid signature or request: {}",
        error_msg
    );
}

#[test]
fn test_mm_supervisor_small_request() {
    let mm_supervisor = MmSupervisorHandler::new();

    // Create request that's too small
    let small_request = [0u8; 10]; // Smaller than MmSupervisorRequestHeader

    let result = mm_supervisor.handle_request(&small_request);
    assert!(result.is_err(), "Small request should cause handler to fail");

    let error_msg = format!("{}", result.unwrap_err());
    assert!(
        error_msg.contains("Request too small") || error_msg.contains("too small"),
        "Error message should mention request size: {}",
        error_msg
    );
}

#[test]
fn test_mm_supervisor_builder_integration() {
    // Test using the fluent builder API for MM Supervisor scenarios
    let framework = MmTestFramework::builder()
        .with_mm_supervisor_handler()
        .with_echo_handler()
        .build()
        .expect("Builder should create framework successfully");

    // Test basic MM communication with the framework
    let test_data = b"Hello MM!";
    let echo_result = framework.communicate(&test_guids::ECHO_HANDLER, test_data);
    assert!(echo_result.is_ok(), "Echo handler should work");

    // Test MM Supervisor handler as well
    let version_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::VERSION_INFO,
        reserved: 0,
        result: 0,
    };

    // Use framework directly instead of mm_comm_service
    let request_data = version_request.to_bytes(); // Convert to bytes
    let supervisor_result = framework.communicate(&test_guids::MM_SUPERVISOR, &request_data);
    assert!(supervisor_result.is_ok(), "MM Supervisor should work");

    // Verify both triggers worked (we made 2 communication calls: echo + supervisor)
    assert_eq!(framework.get_trigger_count(), 2, "Framework should count both communications");
}

#[test]
fn test_safe_message_parsing_with_mm_supervisor() {
    // Test that our safe message parser works correctly with MM Supervisor messages
    let mut buffer = vec![0u8; TEST_BUFFER_SIZE];

    let version_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::VERSION_INFO,
        reserved: 0,
        result: 0,
    };

    let request_data = version_request.to_bytes();

    // Test writing MM Supervisor message safely
    let mut parser = MmMessageParser::new(&mut buffer);
    parser
        .write_message(&test_guids::MM_SUPERVISOR, &request_data)
        .expect("Should write MM Supervisor message successfully");

    // Test parsing the message back safely
    let (parsed_guid, parsed_data) = parser.parse_message().expect("Should parse MM Supervisor message successfully");

    assert_eq!(parsed_guid, test_guids::MM_SUPERVISOR);
    assert_eq!(parsed_data, &request_data);

    // Verify we can parse the MM Supervisor request from the parsed data
    let parsed_request =
        MmSupervisorRequestHeader::from_bytes(parsed_data).expect("Should parse MM Supervisor request header");

    assert_eq!(parsed_request.signature, version_request.signature);
    assert_eq!(parsed_request.request, version_request.request);
}

#[test]
fn test_mm_supervisor_comm_update_request() {
    let framework = MmTestFramework::builder()
        .with_mm_supervisor_handler()
        .build()
        .expect("Framework should initialize successfully");

    // Create communication buffer update request using COMM_UPDATE constant
    let comm_update_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::COMM_UPDATE,
        reserved: 0,
        result: 0,
    };

    let request_bytes = comm_update_request.to_bytes();

    // Send the request using framework
    let result = framework.communicate(&test_guids::MM_SUPERVISOR, &request_bytes);
    assert!(result.is_ok(), "MM Supervisor comm update communication should succeed");

    let response = result.unwrap();

    // Verify response contains header + update result
    let expected_size = core::mem::size_of::<MmSupervisorRequestHeader>() + 4; // + u32 update result
    assert_eq!(response.len(), expected_size, "Comm update response should have correct size");

    // Parse response header safely
    let response_header = MmSupervisorRequestHeader::from_bytes(&response).expect("Should parse response header");

    assert_eq!(response_header.request, mm_supv::requests::COMM_UPDATE, "Response should be for COMM_UPDATE request");
    assert_eq!(response_header.result, mm_supv::responses::SUCCESS, "Comm update request should succeed");

    // Parse update result safely
    let update_result_offset = core::mem::size_of::<MmSupervisorRequestHeader>();
    let update_result = u32::from_le_bytes([
        response[update_result_offset],
        response[update_result_offset + 1],
        response[update_result_offset + 2],
        response[update_result_offset + 3],
    ]);

    assert_eq!(update_result, 0x00000001, "Update result should indicate success");

    log::info!(target: "comm_update_test", "MM Supervisor comm update test completed successfully");
}

#[test]
fn test_mm_supervisor_unblock_mem_request() {
    let framework = MmTestFramework::builder()
        .with_mm_supervisor_handler()
        .build()
        .expect("Framework should initialize successfully");

    // Create memory unblock request using UNBLOCK_MEM constant
    let unblock_mem_request = MmSupervisorRequestHeader {
        signature: u32::from_le_bytes(mm_supv::SIGNATURE),
        revision: mm_supv::REVISION,
        request: mm_supv::requests::UNBLOCK_MEM, // This uses the constant!
        reserved: 0,
        result: 0,
    };

    let request_bytes = unblock_mem_request.to_bytes();

    // Send the request using framework
    let result = framework.communicate(&test_guids::MM_SUPERVISOR, &request_bytes);
    assert!(result.is_ok(), "MM Supervisor unblock mem communication should succeed");

    let response = result.unwrap();

    // Verify response contains header + unblock status
    let expected_size = core::mem::size_of::<MmSupervisorRequestHeader>() + 8; // + u64 unblock status
    assert_eq!(response.len(), expected_size, "Unblock mem response should have correct size");

    // Parse response header safely
    let response_header = MmSupervisorRequestHeader::from_bytes(&response).expect("Should parse response header");

    assert_eq!(response_header.request, mm_supv::requests::UNBLOCK_MEM, "Response should be for UNBLOCK_MEM request");
    assert_eq!(response_header.result, mm_supv::responses::SUCCESS, "Unblock mem request should succeed");

    // Parse unblock status safely
    let unblock_status_offset = core::mem::size_of::<MmSupervisorRequestHeader>();
    let unblock_status = u64::from_le_bytes([
        response[unblock_status_offset],
        response[unblock_status_offset + 1],
        response[unblock_status_offset + 2],
        response[unblock_status_offset + 3],
        response[unblock_status_offset + 4],
        response[unblock_status_offset + 5],
        response[unblock_status_offset + 6],
        response[unblock_status_offset + 7],
    ]);

    assert_eq!(unblock_status, 0x0000000000000001, "Unblock status should indicate success");

    log::info!(target: "unblock_mem_test", "MM Supervisor unblock mem test completed successfully");
}
