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
            Error::DebugAssert { msg, file, line } => write!(f, "Assertion at {file}:{line}: {msg}"),
        }
    }
}

impl core::error::Error for Error {}
