//! Management Mode (MM) Message Parser
//!
//! This module provides utilities for parsing and manipulating MM communication messages.
//! It is intended for use in the patina_mm test framework to facilitate testing of MM
//! communication scenarios.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;
#[allow(unused_imports)] // Used in test module within this file
use alloc::vec::Vec;
use r_efi::efi;

/// Error types for MM message parsing operations
#[derive(Debug, PartialEq)]
pub enum MmMessageParseError {
    /// Buffer is too small to contain a valid MM header
    BufferTooSmall,
    /// Invalid header format or content
    #[allow(dead_code)] // Reserved for future header validation
    InvalidHeader,
    /// Message length extends beyond buffer bounds
    MessageTooLarge,
    /// Buffer is not properly aligned
    #[allow(dead_code)] // Reserved for future alignment validation
    InvalidAlignment,
}

impl core::fmt::Display for MmMessageParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MmMessageParseError::BufferTooSmall => write!(f, "Buffer is too small for MM header"),
            MmMessageParseError::InvalidHeader => write!(f, "Invalid MM header format"),
            MmMessageParseError::MessageTooLarge => write!(f, "Message length exceeds buffer size"),
            MmMessageParseError::InvalidAlignment => write!(f, "Buffer alignment is invalid"),
        }
    }
}

/// Represents a MM Communication header
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct MmCommunicateHeader {
    /// Recipient handler GUID
    header_guid: efi::Guid,
    /// Length of the message data (excluding header)
    message_length: u64,
}

impl MmCommunicateHeader {
    const SIZE: usize = core::mem::size_of::<Self>();

    /// Create a new header with the specified GUID and message length
    fn new(guid: &efi::Guid, message_length: u64) -> Self {
        Self { header_guid: *guid, message_length }
    }

    /// Write this header to the beginning of a buffer
    fn write_to_buffer(&self, buffer: &mut [u8]) -> Result<(), MmMessageParseError> {
        if buffer.len() < Self::SIZE {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        // SAFETY: MmCommunicateHeader is repr(C) with well-defined size and layout
        let header_bytes = unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, Self::SIZE) };
        buffer[..Self::SIZE].copy_from_slice(header_bytes);
        Ok(())
    }

    /// Read a header from the beginning of a buffer
    fn read_from_buffer(buffer: &[u8]) -> Result<Self, MmMessageParseError> {
        if buffer.len() < Self::SIZE {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        // Byte-by-byte copy to avoid alignment issues
        let mut header =
            MmCommunicateHeader { header_guid: efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]), message_length: 0 };

        // SAFETY: MmCommunicateHeader is repr(C) with well-defined size and layout
        let header_bytes = unsafe { core::slice::from_raw_parts_mut(&mut header as *mut Self as *mut u8, Self::SIZE) };
        header_bytes.copy_from_slice(&buffer[..Self::SIZE]);
        Ok(header)
    }
}

/// A MM message parser
///
/// Provides safe, bounds-checked parsing of MM Communication messages for the patina_mm
/// test framework. This parser validates MM message structure and content without using
/// unsafe operations, ensuring that test scenarios can safely examine and manipulate
/// MM Communication buffers while testing the `CommunicateBuffer` and `MmCommunicator`
/// functionality.
///
/// This intended to be more simple and direct than actual message parsing. It does not
/// rely on internal state like actual message parsing would be. This allows tests to
/// test cases like intentionally corrupting the buffer, bypassing safety checks to
/// test various conditions, and other direct manipulation of the message buffer.
pub struct MmMessageParser<'a> {
    buffer: &'a mut [u8],
}

impl<'a> MmMessageParser<'a> {
    /// Create a new message parser for the given buffer
    pub fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer }
    }

    /// Parse an MM message from the buffer, returning the GUID and message data
    pub fn parse_message(&self) -> Result<(efi::Guid, &[u8]), MmMessageParseError> {
        if self.buffer.len() < MmCommunicateHeader::SIZE {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        let header = MmCommunicateHeader::read_from_buffer(self.buffer)?;

        let message_start = MmCommunicateHeader::SIZE;
        let message_end = message_start + header.message_length as usize;

        if message_end > self.buffer.len() {
            return Err(MmMessageParseError::MessageTooLarge);
        }

        let message_data = &self.buffer[message_start..message_end];
        Ok((header.header_guid, message_data))
    }

    /// Write an MM message to the buffer with the specified GUID and data
    pub fn write_message(&mut self, guid: &efi::Guid, data: &[u8]) -> Result<(), MmMessageParseError> {
        let total_size = MmCommunicateHeader::SIZE + data.len();
        if total_size > self.buffer.len() {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        // Write the header
        let header = MmCommunicateHeader::new(guid, data.len() as u64);
        header.write_to_buffer(self.buffer)?;

        // Write the message data
        let message_start = MmCommunicateHeader::SIZE;
        let message_end = message_start + data.len();
        self.buffer[message_start..message_end].copy_from_slice(data);

        Ok(())
    }

    /// Update the message length in the header
    #[allow(dead_code)] // Part of complete message manipulation API
    pub fn update_message_length(&mut self, new_length: u64) -> Result<(), MmMessageParseError> {
        if self.buffer.len() < MmCommunicateHeader::SIZE {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        let mut header = MmCommunicateHeader::read_from_buffer(self.buffer)?;
        header.message_length = new_length;
        header.write_to_buffer(self.buffer)?;

        Ok(())
    }

    /// Get the current message length from the header
    #[allow(dead_code)] // Part of complete message manipulation API
    pub fn get_message_length(&self) -> Result<u64, MmMessageParseError> {
        if self.buffer.len() < MmCommunicateHeader::SIZE {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        let header = MmCommunicateHeader::read_from_buffer(self.buffer)?;
        Ok(header.message_length)
    }

    /// Get the GUID from the header
    #[allow(dead_code)] // Part of complete message manipulation API
    pub fn get_header_guid(&self) -> Result<efi::Guid, MmMessageParseError> {
        if self.buffer.len() < MmCommunicateHeader::SIZE {
            return Err(MmMessageParseError::BufferTooSmall);
        }

        let header = MmCommunicateHeader::read_from_buffer(self.buffer)?;
        Ok(header.header_guid)
    }

    /// Get the total size required for a message with the given data length
    pub fn required_buffer_size(data_length: usize) -> usize {
        MmCommunicateHeader::SIZE + data_length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_round_trip() {
        let mut buffer = vec![0u8; 128];
        let test_guid =
            efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let test_data = b"Hello, MM World!";

        let mut parser = MmMessageParser::new(&mut buffer);

        // Write message
        let write_result = parser.write_message(&test_guid, test_data);
        assert!(write_result.is_ok(), "Writing message should succeed");

        // Parse message back
        let parse_result = parser.parse_message();
        assert!(parse_result.is_ok(), "Parsing message should succeed");

        let (parsed_guid, parsed_data) = parse_result.unwrap();
        assert_eq!(parsed_guid, test_guid, "GUID should match");
        assert_eq!(parsed_data, test_data, "Parsed data should match original");
    }

    #[test]
    fn test_buffer_too_small() {
        let mut small_buffer = vec![0u8; 4]; // Much smaller than header size
        let test_guid =
            efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let test_data = b"Data";

        let mut parser = MmMessageParser::new(&mut small_buffer);
        let result = parser.write_message(&test_guid, test_data);

        assert!(result.is_err(), "Should fail with buffer too small");
        assert_eq!(result.unwrap_err(), MmMessageParseError::BufferTooSmall);
    }
}
