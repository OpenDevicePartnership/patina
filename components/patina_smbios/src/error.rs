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

    // Record type errors
    /// Type 127 End-of-Table marker is automatically managed and cannot be added manually
    Type127Managed,

    // Table integrity errors
    /// Published SMBIOS table was modified directly instead of using protocol APIs
    /// Use Remove() + Add() to modify records, or UpdateString() for string fields
    TableDirectlyModified,
}

impl From<SmbiosError> for patina::error::EfiError {
    fn from(error: SmbiosError) -> Self {
        match error {
            // Resource allocation errors map to OUT_OF_RESOURCES
            SmbiosError::AllocationFailed | SmbiosError::HandleExhausted => patina::error::EfiError::OutOfResources,

            // Invalid parameters map to INVALID_PARAMETER
            SmbiosError::StringTooLong
            | SmbiosError::StringContainsNull
            | SmbiosError::EmptyStringInPool
            | SmbiosError::RecordTooSmall
            | SmbiosError::MalformedRecordHeader
            | SmbiosError::InvalidStringPoolTermination
            | SmbiosError::StringPoolTooSmall
            | SmbiosError::StringIndexOutOfRange
            | SmbiosError::Type127Managed => patina::error::EfiError::InvalidParameter,

            // Not found errors map to NOT_FOUND
            SmbiosError::HandleNotFound | SmbiosError::NoRecordsAvailable => patina::error::EfiError::NotFound,

            // Version and initialization errors map to UNSUPPORTED
            SmbiosError::UnsupportedVersion | SmbiosError::AlreadyInitialized | SmbiosError::NotInitialized => {
                patina::error::EfiError::Unsupported
            }

            // Table integrity errors map to DEVICE_ERROR (indicates corrupted/invalid state)
            SmbiosError::TableDirectlyModified => patina::error::EfiError::DeviceError,
        }
    }
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
            SmbiosError::Type127Managed,
            SmbiosError::TableDirectlyModified,
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

    #[test]
    fn test_smbios_error_to_efi_error_conversion() {
        // Test resource allocation errors map to OUT_OF_RESOURCES
        let efi_err: patina::error::EfiError = SmbiosError::AllocationFailed.into();
        assert_eq!(efi_err, patina::error::EfiError::OutOfResources);

        let efi_err: patina::error::EfiError = SmbiosError::HandleExhausted.into();
        assert_eq!(efi_err, patina::error::EfiError::OutOfResources);

        // Test invalid parameters map to INVALID_PARAMETER
        let efi_err: patina::error::EfiError = SmbiosError::StringTooLong.into();
        assert_eq!(efi_err, patina::error::EfiError::InvalidParameter);

        let efi_err: patina::error::EfiError = SmbiosError::Type127Managed.into();
        assert_eq!(efi_err, patina::error::EfiError::InvalidParameter);

        // Test not found errors map to NOT_FOUND
        let efi_err: patina::error::EfiError = SmbiosError::HandleNotFound.into();
        assert_eq!(efi_err, patina::error::EfiError::NotFound);

        let efi_err: patina::error::EfiError = SmbiosError::NoRecordsAvailable.into();
        assert_eq!(efi_err, patina::error::EfiError::NotFound);

        // Test version and initialization errors map to UNSUPPORTED
        let efi_err: patina::error::EfiError = SmbiosError::UnsupportedVersion.into();
        assert_eq!(efi_err, patina::error::EfiError::Unsupported);

        let efi_err: patina::error::EfiError = SmbiosError::NotInitialized.into();
        assert_eq!(efi_err, patina::error::EfiError::Unsupported);

        // Test table integrity errors map to DEVICE_ERROR
        let efi_err: patina::error::EfiError = SmbiosError::TableDirectlyModified.into();
        assert_eq!(efi_err, patina::error::EfiError::DeviceError);
    }
}
