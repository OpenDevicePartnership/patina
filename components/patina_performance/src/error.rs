use core::fmt::Display;

use patina_sdk::error::EfiError;

#[macro_export]
macro_rules! error_debug_assert {
    ($expression:expr, $msg:literal) => {{
        debug_assert!($expression, $msg);
        Err($crate::error::Error::DebugAssert { msg: $msg, file: file!(), line: line!() })
    }};
    ($msg:literal) => {
        error_debug_assert!(false, $msg)
    };
}

#[derive(Debug)]
pub enum Error {
    // FBPT full, can't add more performance records.
    OutOfResources,
    // Buffer too small to allocate fbpt.
    BufferTooSmall,
    Efi(EfiError),
    /// Error returned when `debug_assert` is disabled.
    DebugAssert {
        msg: &'static str,
        file: &'static str,
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
