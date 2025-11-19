//! Error codes for performance APIs in the Patina SDK.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
use core::fmt::Display;

use crate::error::EfiError;

/// Macro to assert an expression and return an error if the assertion fails.
#[doc(hidden)]
#[macro_export]
macro_rules! performance_debug_assert {
    ($expression:expr, $msg:literal) => {{
        debug_assert!($expression, $msg);
        Err($crate::performance::error::Error::DebugAssert { msg: $msg, file: file!(), line: line!() })
    }};
    ($msg:literal) => {
        performance_debug_assert!(false, $msg)
    };
}

/// Error type for the Patina Performance component.
#[derive(Debug)]
pub enum Error {
    /// FBPT full, can't add more performance records.
    OutOfResources,
    /// Buffer too small to allocate fbpt.
    BufferTooSmall,
    /// UEFI specification defined error type.
    Efi(EfiError),
    /// Generic serialization error while encoding a performance record or table.
    Serialization,
    /// A performance record exceeded the representable maximum size (u8::MAX bytes).
    RecordTooLarge {
        /// The actual size of the record that exceeded the limit.
        size: usize,
    },
    /// Error returned when `debug_assert` is disabled.
    DebugAssert {
        /// The message describing the assertion failure.
        msg: &'static str,
        /// The file where the assertion failed.
        file: &'static str,
        /// The line number where the assertion failed.
        line: u32,
    },
}

impl From<EfiError> for Error {
    fn from(value: EfiError) -> Self {
        Error::Efi(value)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::OutOfResources => write!(f, "FBPT buffer full, can't add more performance records."),
            Error::BufferTooSmall => write!(f, "Buffer to small to allocate FBPT table"),
            Error::Efi(efi_error) => write!(f, "{efi_error:?}"),
            Error::Serialization => write!(f, "Failed to serialize performance data"),
            Error::RecordTooLarge { size } => write!(f, "Performance record size {size} exceeds u8::MAX"),
            Error::DebugAssert { msg, file, line } => write!(f, "Assertion at {file}:{line}: {msg}"),
        }
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_out_of_resources_display() {
        let error = Error::OutOfResources;
        let display = format!("{}", error);
        assert_eq!(display, "FBPT buffer full, can't add more performance records.");
    }

    #[test]
    fn test_buffer_too_small_display() {
        let error = Error::BufferTooSmall;
        let display = format!("{}", error);
        assert_eq!(display, "Buffer to small to allocate FBPT table");
    }

    #[test]
    fn test_efi_error_display() {
        let error = Error::Efi(EfiError::InvalidParameter);
        let display = format!("{}", error);
        assert!(display.contains("InvalidParameter"));
    }

    #[test]
    fn test_serialization_display() {
        let error = Error::Serialization;
        let display = format!("{}", error);
        assert_eq!(display, "Failed to serialize performance data");
    }

    #[test]
    fn test_record_too_large_display() {
        let error = Error::RecordTooLarge { size: 512 };
        let display = format!("{}", error);
        assert_eq!(display, "Performance record size 512 exceeds u8::MAX");
    }

    #[test]
    fn test_debug_assert_display() {
        let error = Error::DebugAssert { msg: "test assertion", file: "test.rs", line: 42 };
        let display = format!("{}", error);
        assert_eq!(display, "Assertion at test.rs:42: test assertion");
    }

    #[test]
    fn test_from_efi_error() {
        let efi_error = EfiError::OutOfResources;
        let error: Error = efi_error.into();
        match error {
            Error::Efi(EfiError::OutOfResources) => {}
            _ => panic!("Expected Error::Efi(OutOfResources)"),
        }
    }

    #[test]
    fn test_debug_formatting() {
        let error = Error::OutOfResources;
        let debug = format!("{:?}", error);
        assert!(debug.contains("OutOfResources"));
    }

    #[test]
    fn test_record_too_large_debug() {
        let error = Error::RecordTooLarge { size: 300 };
        let debug = format!("{:?}", error);
        assert!(debug.contains("RecordTooLarge"));
        assert!(debug.contains("300"));
    }

    #[test]
    fn test_debug_assert_error_fields() {
        let error = Error::DebugAssert { msg: "bounds check", file: "module.rs", line: 123 };
        match error {
            Error::DebugAssert { msg, file, line } => {
                assert_eq!(msg, "bounds check");
                assert_eq!(file, "module.rs");
                assert_eq!(line, 123);
            }
            _ => panic!("Expected DebugAssert variant"),
        }
    }
}
