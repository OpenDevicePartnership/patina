//! Error codes for the patina_stacktrace crate
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
use core::fmt;

/// The error type for stacktrace operations.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// Error during parsing the PE
    BufferTooShort(usize),

    /// Unexpected values during parsing the PE
    Malformed(&'static str),

    /// Failed to locate a PE Image in memory
    ImageNotFound(u64),

    /// Unable to locate the runtime function for the given rip(rva)
    ExceptionDirectoryNotFound(Option<&'static str>),

    /// Unable to locate the runtime function for the given rip(rva)
    RuntimeFunctionNotFound(Option<&'static str>, u32),

    /// Failed to locate unwind info at the given image base
    UnwindInfoNotFound(Option<&'static str>, u64, u32),

    /// Failed to decode stack frame unwind codes
    StackFrameUnwindDecodeFailed(Option<&'static str>),

    /// Failed to dump all the frames in the stack trace
    StackTraceDumpFailed(Option<&'static str>),
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let no_module_str = "<no module>";
        match self {
            Error::BufferTooShort(index) => write!(fmt, "Buffer is too short {index}"),
            Error::Malformed(msg) => write!(fmt, "Malformed entity: {msg}"),
            Error::ImageNotFound(rva) => {
                write!(fmt, "Failed to locate a PE Image in memory with rip: {rva:X}")
            }
            Error::ExceptionDirectoryNotFound(module) => {
                write!(
                    fmt,
                    "Exception directory not found for module {}. Make sure to build with RUSTFLAGS=-Cforce-unwind-tables",
                    module.as_ref().unwrap_or(&no_module_str)
                )
            }
            Error::RuntimeFunctionNotFound(module, rip_rva) => {
                write!(
                    fmt,
                    "Runtime function not found for module {} with rip(rva): {:X}",
                    module.as_ref().unwrap_or(&no_module_str),
                    rip_rva
                )
            }
            Error::UnwindInfoNotFound(module, image_base, unwind_info) => {
                write!(
                    fmt,
                    "Failed to locate unwind info({:X}) for module {} at image base({:X})",
                    unwind_info,
                    module.as_ref().unwrap_or(&no_module_str),
                    image_base
                )
            }
            Error::StackFrameUnwindDecodeFailed(module) => {
                write!(
                    fmt,
                    "Failed to decode stack frame unwind codes for  {}",
                    module.as_ref().unwrap_or(&no_module_str)
                )
            }
            Error::StackTraceDumpFailed(module) => {
                write!(
                    fmt,
                    "Failed to dump all the frames in the stack trace for module {}",
                    module.as_ref().unwrap_or(&no_module_str)
                )
            }
        }
    }
}

/// A specialized result type for the patina_stacktrace crate.
pub type StResult<T> = Result<T, Error>;

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::Error;

    fn assert_display(err: Error, expected: &str) {
        assert_eq!(format!("{err}"), expected);
    }

    #[test]
    fn buffer_too_short_display() {
        assert_display(Error::BufferTooShort(5), "Buffer is too short 5");
    }

    #[test]
    fn malformed_display() {
        assert_display(Error::Malformed("bad"), "Malformed entity: bad");
    }

    #[test]
    fn image_not_found_display() {
        assert_display(Error::ImageNotFound(0x1234), "Failed to locate a PE Image in memory with rip: 1234");
    }

    #[test]
    fn exception_directory_display_with_module() {
        assert_display(
            Error::ExceptionDirectoryNotFound(Some("mod")),
            "Exception directory not found for module mod. Make sure to build with RUSTFLAGS=-Cforce-unwind-tables",
        );
    }

    #[test]
    fn exception_directory_display_without_module() {
        assert_display(
            Error::ExceptionDirectoryNotFound(None),
            "Exception directory not found for module <no module>. Make sure to build with RUSTFLAGS=-Cforce-unwind-tables",
        );
    }

    #[test]
    fn runtime_function_not_found_display() {
        assert_display(
            Error::RuntimeFunctionNotFound(Some("image"), 0x10),
            "Runtime function not found for module image with rip(rva): 10",
        );
    }

    #[test]
    fn unwind_info_not_found_display() {
        assert_display(
            Error::UnwindInfoNotFound(Some("image"), 0x1000, 0x20),
            "Failed to locate unwind info(20) for module image at image base(1000)",
        );
    }

    #[test]
    fn stack_frame_unwind_decode_failed_display_without_module() {
        assert_display(
            Error::StackFrameUnwindDecodeFailed(None),
            "Failed to decode stack frame unwind codes for  <no module>",
        );
    }

    #[test]
    fn stack_trace_dump_failed_display() {
        assert_display(
            Error::StackTraceDumpFailed(Some("image")),
            "Failed to dump all the frames in the stack trace for module image",
        );
    }
}
