//! Management Mode Interrupt (MMI) Handlers
//!
//! This module provides standardized MMI handler implementations for testing.
//!
//! Note: Most of the file refers to these as "MM handlers" for brevity.
//!
//! ## Logging
//!
//! - The `echo_handler` log target is used for logging within the echo handler.
//! - The `version_handler` log target is used for logging within the version info handler.
//! - The `supervisor_handler` log target is used for logging within the MM supervisor handler
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::patina_mm_integration::common::constants::*;
use r_efi::efi;

extern crate alloc;
use alloc::{string::String, vec::Vec};
pub use zerocopy::IntoBytes;

pub use patina::management_mode::protocol::{
    mm_supervisor_request,
    mm_supervisor_request::{MmSupervisorRequestHeader, MmSupervisorVersionInfo, RequestType, ResponseType},
};

/// Standardized error type for MM handlers
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MmHandlerError {
    /// Invalid input data format
    InvalidInput(String),
    /// Processing failed
    ProcessingFailed(String),
    /// Unsupported operation
    UnsupportedOperation(String),
}

impl core::fmt::Display for MmHandlerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MmHandlerError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            MmHandlerError::ProcessingFailed(msg) => write!(f, "Processing failed: {}", msg),
            MmHandlerError::UnsupportedOperation(msg) => write!(f, "Unsupported operation: {}", msg),
        }
    }
}

impl std::error::Error for MmHandlerError {}

/// Result type for MM handler operations
pub type MmHandlerResult<T> = Result<T, MmHandlerError>;

/// A trait that represents a MM handler
pub trait MmHandler: Send + Sync {
    /// Handle an MM request and return a response
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>>;

    /// Get a description of what this handler does
    #[allow(dead_code)] // Reserved for future debugging and introspection
    fn description(&self) -> &str;
}

/// Simple echo handler that returns the input data
pub struct EchoHandler {
    #[allow(dead_code)] // Usage not recognized
    description: String,
}

impl EchoHandler {
    pub fn new() -> Self {
        Self { description: "Echo handler - returns input data unchanged".to_string() }
    }
}

impl MmHandler for EchoHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        log::debug!(target: "echo_handler", "Echoing {} bytes of data", data.len());
        Ok(data.to_vec())
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// Version information handler
pub struct VersionInfoHandler {
    version_string: String,
    #[allow(dead_code)] // Usage not recognized
    description: String,
}

impl VersionInfoHandler {
    pub fn new(version: &str) -> Self {
        Self {
            version_string: version.to_string(),
            description: format!("Version info handler - returns version: {}", version),
        }
    }
}

impl MmHandler for VersionInfoHandler {
    fn handle_request(&self, _data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        log::debug!(target: "version_handler", "Returning version info: {}", self.version_string);
        Ok(self.version_string.as_bytes().to_vec())
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// MM Supervisor handler for testing supervisor communication patterns
pub struct MmSupervisorHandler {
    #[allow(dead_code)] // Usage not recognized
    description: String,
}

impl MmSupervisorHandler {
    pub fn new() -> Self {
        Self { description: "MM Supervisor handler - handles supervisor protocol requests".to_string() }
    }

    fn handle_get_info_request(&self) -> MmHandlerResult<Vec<u8>> {
        let response_header = MmSupervisorRequestHeader {
            signature: mm_supervisor_request::SIGNATURE,
            revision: mm_supervisor_request::REVISION,
            request: RequestType::VersionInfo as u32,
            reserved: 0,
            result: 0, // Success
        };

        let version_info = MmSupervisorVersionInfo {
            version: mm_supv::VERSION,
            patch_level: mm_supv::PATCH_LEVEL,
            max_supervisor_request_level: RequestType::MAX_REQUEST_TYPE,
        };

        let mut response = Vec::new();
        response.extend_from_slice(response_header.as_bytes());
        response.extend_from_slice(version_info.as_bytes());

        log::debug!(target: "supervisor_handler", "Generated get info response: {} bytes", response.len());
        Ok(response)
    }

    fn handle_get_capabilities_request(&self) -> MmHandlerResult<Vec<u8>> {
        let response_header = MmSupervisorRequestHeader {
            signature: mm_supervisor_request::SIGNATURE,
            revision: mm_supervisor_request::REVISION,
            request: RequestType::FetchPolicy as u32,
            reserved: 0,
            result: 0, // Success
        };

        let capabilities: u64 = 0x00000007; // Mock capabilities value

        let mut response = Vec::new();
        response.extend_from_slice(response_header.as_bytes());
        response.extend_from_slice(&capabilities.to_le_bytes());

        log::debug!(target: "supervisor_handler", "Generated get capabilities response: {} bytes", response.len());
        Ok(response)
    }

    fn handle_comm_update_request(&self) -> MmHandlerResult<Vec<u8>> {
        let response_header = MmSupervisorRequestHeader {
            signature: mm_supervisor_request::SIGNATURE,
            revision: mm_supervisor_request::REVISION,
            request: RequestType::CommUpdate as u32,
            reserved: 0,
            result: 0, // Success
        };

        // Mock communication buffer update response
        let update_result: u32 = 0x00000001; // Success status

        let mut response = Vec::new();
        response.extend_from_slice(response_header.as_bytes());
        response.extend_from_slice(&update_result.to_le_bytes());

        log::debug!(target: "supervisor_handler", "Generated comm update response: {} bytes", response.len());
        Ok(response)
    }

    fn handle_unblock_mem_request(&self) -> MmHandlerResult<Vec<u8>> {
        let response_header = MmSupervisorRequestHeader {
            signature: mm_supervisor_request::SIGNATURE,
            revision: mm_supervisor_request::REVISION,
            request: RequestType::UnblockMem as u32,
            reserved: 0,
            result: 0, // Success
        };

        // Mock memory unblock response
        let unblock_status: u64 = 0x0000000000000001; // Success - memory regions unblocked

        let mut response = Vec::new();
        response.extend_from_slice(response_header.as_bytes());
        response.extend_from_slice(&unblock_status.to_le_bytes());

        log::debug!(target: "supervisor_handler", "Generated unblock mem response: {} bytes", response.len());
        Ok(response)
    }
}

impl MmHandler for MmSupervisorHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        log::debug!(target: "supervisor_handler", "Processing MM supervisor request: {} bytes", data.len());

        if data.len() < MmSupervisorRequestHeader::SIZE {
            return Err(MmHandlerError::InvalidInput(format!(
                "Request too small: {} bytes, expected at least {}",
                data.len(),
                MmSupervisorRequestHeader::SIZE
            )));
        }

        let request_header = MmSupervisorRequestHeader::from_bytes(data)
            .ok_or_else(|| MmHandlerError::InvalidInput("Failed to parse header from bytes".to_string()))?;

        // Validate signature
        if request_header.signature != mm_supervisor_request::SIGNATURE {
            return Err(MmHandlerError::InvalidInput(format!(
                "Invalid signature: 0x{:08X}, expected 0x{:08X}",
                request_header.signature,
                mm_supervisor_request::SIGNATURE
            )));
        }

        // Validate revision
        if request_header.revision != mm_supervisor_request::REVISION {
            return Err(MmHandlerError::InvalidInput(format!(
                "Invalid revision: 0x{:08X}, expected 0x{:08X}",
                request_header.revision,
                mm_supervisor_request::REVISION
            )));
        }

        // Process based on request type
        match RequestType::try_from(request_header.request) {
            Ok(RequestType::VersionInfo) => {
                log::debug!(target: "supervisor_handler", "Processing get info request");
                self.handle_get_info_request()
            }
            Ok(RequestType::FetchPolicy) => {
                log::debug!(target: "supervisor_handler", "Processing fetch policy request");
                self.handle_get_capabilities_request()
            }
            Ok(RequestType::CommUpdate) => {
                log::debug!(target: "supervisor_handler", "Processing comm update request");
                self.handle_comm_update_request()
            }
            Ok(RequestType::UnblockMem) => {
                log::debug!(target: "supervisor_handler", "Processing unblock mem request");
                self.handle_unblock_mem_request()
            }
            Err(_) => {
                log::warn!(target: "supervisor_handler", "Unsupported request type: 0x{:08X}", request_header.request);

                // Return error response
                let error_header = MmSupervisorRequestHeader {
                    signature: mm_supervisor_request::SIGNATURE,
                    revision: mm_supervisor_request::REVISION,
                    request: request_header.request,
                    reserved: 0,
                    result: efi::Status::from(ResponseType::InvalidRequest).as_usize() as u64, // Error
                };

                let mut response = Vec::new();
                response.extend_from_slice(error_header.as_bytes());
                Ok(response)
            }
        }
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// Error injection handler that fails based on patterns in the data
pub struct ErrorInjectionHandler {
    #[allow(dead_code)]
    description: String,
}

impl ErrorInjectionHandler {
    pub fn new() -> Self {
        Self { description: "Error injection handler - fails on specific data patterns".to_string() }
    }
}

impl MmHandler for ErrorInjectionHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        // Fail if data starts with 0xFF
        if !data.is_empty() && data[0] == 0xFF {
            return Err(MmHandlerError::ProcessingFailed("Intentional failure on 0xFF pattern".to_string()));
        }

        // Fail if data contains specific failure pattern
        if data.len() >= 4 && &data[0..4] == b"FAIL" {
            return Err(MmHandlerError::UnsupportedOperation("FAIL pattern detected".to_string()));
        }

        // Success case - return modified data
        let mut response = data.to_vec();
        response.push(0xAA); // Add success marker
        Ok(response)
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// Buffer size stress handler that tests various buffer size scenarios
pub struct BufferSizeHandler {
    #[allow(dead_code)]
    description: String,
}

impl BufferSizeHandler {
    pub fn new() -> Self {
        Self { description: "Buffer size handler - returns data of varying sizes".to_string() }
    }
}

impl MmHandler for BufferSizeHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        if data.is_empty() {
            return Err(MmHandlerError::InvalidInput("Empty data not allowed".to_string()));
        }

        match data[0] % 5 {
            0 => Ok(Vec::new()),       // Empty response
            1 => Ok(vec![0x42]),       // Single byte
            2 => Ok(vec![0x12; 256]),  // Medium buffer
            3 => Ok(vec![0x34; 1024]), // Large buffer
            4 => Ok(data.to_vec()),    // Echo back
            _ => unreachable!(),
        }
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// Computational stress handler that performs work proportional to data size
pub struct ComputationHandler {
    #[allow(dead_code)]
    description: String,
}

impl ComputationHandler {
    pub fn new() -> Self {
        Self { description: "Computation handler - performs work proportional to input size".to_string() }
    }
}

impl MmHandler for ComputationHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        // Simulate computational work by calculating checksums
        let mut checksum: u32 = 0;
        for &byte in data {
            checksum = checksum.wrapping_add(byte as u32);
            checksum = checksum.wrapping_mul(17); // Simple hash function
        }

        // Return checksum as response
        Ok(checksum.to_le_bytes().to_vec())
    }

    fn description(&self) -> &str {
        &self.description
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_handler() {
        let handler = EchoHandler::new();
        let test_data = b"Hello, world!";

        let result = handler.handle_request(test_data);
        assert!(result.is_ok(), "Echo handler should succeed");
        assert_eq!(result.unwrap(), test_data, "Echo should return same data");
    }

    #[test]
    fn test_version_handler() {
        let version = "Test Version 1.0";
        let handler = VersionInfoHandler::new(version);

        let result = handler.handle_request(b"");
        assert!(result.is_ok(), "Version handler should succeed");
        assert_eq!(result.unwrap(), version.as_bytes(), "Should return version string");
    }

    #[test]
    fn test_supervisor_handler_invalid_input() {
        let handler = MmSupervisorHandler::new();

        // Test with too small buffer
        let result = handler.handle_request(b"small");
        assert!(result.is_err(), "Should fail with small buffer");

        if let Err(MmHandlerError::InvalidInput(msg)) = result {
            assert!(msg.contains("Request too small"), "Should indicate buffer too small");
        } else {
            panic!("Expected InvalidInput error");
        }
    }

    #[test]
    fn test_supervisor_header_conversion() {
        let original = MmSupervisorRequestHeader {
            signature: 0x12345678,
            revision: 0x1,
            request: 0x2,
            reserved: 0x0,
            result: 0x123456789ABCDEF0,
        };

        let bytes = original.as_bytes();
        assert_eq!(bytes.len(), MmSupervisorRequestHeader::SIZE);

        let recovered = MmSupervisorRequestHeader::from_bytes(bytes);
        assert!(recovered.is_some(), "Should successfully parse the header");

        let recovered = recovered.unwrap();
        assert_eq!(recovered.signature, original.signature);
        assert_eq!(recovered.revision, original.revision);
        assert_eq!(recovered.request, original.request);
        assert_eq!(recovered.reserved, original.reserved);
        assert_eq!(recovered.result, original.result);
    }
}
