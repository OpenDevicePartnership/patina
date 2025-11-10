//! Error types for SMBIOS operations
//!
//! This module defines the error types returned by SMBIOS service operations.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// SMBIOS operation errors
///
/// This enum represents all possible errors that can occur during SMBIOS operations,
/// including string validation, record management, handle allocation, and resource management.
#[derive(Debug, Clone, PartialEq)]
pub enum SmbiosError {
    // String validation errors
    /// String exceeds maximum allowed length (64 bytes)
    StringTooLong,
    /// String contains null terminator (not allowed - terminators are added during serialization)
    StringContainsNull,
    /// Empty string in string pool (consecutive null bytes)
    EmptyStringInPool,

    // Record format errors
    /// Record buffer is too small to contain a valid SMBIOS header
    RecordTooSmall,
    /// Record header is malformed or cannot be parsed
    MalformedRecordHeader,
    /// String pool is missing required double-null termination
    InvalidStringPoolTermination,
    /// String pool area is too small (must be at least 2 bytes)
    StringPoolTooSmall,

    // Handle management errors
    /// All available handles have been exhausted (reached 0xFFFE limit)
    HandleExhausted,
    /// The specified handle was not found in the record table
    HandleNotFound,
    /// String index is out of range for the specified record
    StringIndexOutOfRange,

    // Resource allocation errors
    /// Failed to allocate memory for SMBIOS table or entry point
    AllocationFailed,
    /// No SMBIOS records available to install into configuration table
    NoRecordsAvailable,

    // State errors
    /// SMBIOS manager has already been initialized
    AlreadyInitialized,
    /// SMBIOS manager has not been initialized yet
    NotInitialized,

    // Version errors
    /// SMBIOS version is not supported (only 3.0 and above are supported)
    UnsupportedVersion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smbios_error_all_variants() {
        extern crate std;
        use std::vec;

        // Test all error variants for completeness
        let errors = vec![
            SmbiosError::StringTooLong,
            SmbiosError::StringContainsNull,
            SmbiosError::EmptyStringInPool,
            SmbiosError::RecordTooSmall,
            SmbiosError::MalformedRecordHeader,
            SmbiosError::InvalidStringPoolTermination,
            SmbiosError::StringPoolTooSmall,
            SmbiosError::HandleExhausted,
            SmbiosError::HandleNotFound,
            SmbiosError::StringIndexOutOfRange,
            SmbiosError::AllocationFailed,
            SmbiosError::NoRecordsAvailable,
            SmbiosError::AlreadyInitialized,
            SmbiosError::NotInitialized,
            SmbiosError::UnsupportedVersion,
        ];

        // Each should be cloneable and comparable
        for err in errors {
            let cloned = err.clone();
            assert_eq!(err, cloned);
        }
    }

    #[test]
    fn test_smbios_error_clone_and_eq() {
        let err1 = SmbiosError::StringTooLong;
        let err2 = err1.clone();
        assert_eq!(err1, err2);

        let err3 = SmbiosError::AllocationFailed;
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_smbios_error_types() {
        // Verify we can construct each error type
        let _e1 = SmbiosError::StringTooLong;
        let _e2 = SmbiosError::HandleExhausted;
        let _e3 = SmbiosError::AllocationFailed;
        let _e4 = SmbiosError::NotInitialized;
        let _e5 = SmbiosError::UnsupportedVersion;
        let _e6 = SmbiosError::StringIndexOutOfRange;
        let _e7 = SmbiosError::RecordTooSmall;
        let _e8 = SmbiosError::InvalidStringPoolTermination;
    }
}
