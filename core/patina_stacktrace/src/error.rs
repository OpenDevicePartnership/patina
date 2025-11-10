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
    /// Attempted to read past the end of a backing buffer while decoding structured data.
    OutOfBoundsRead {
        /// Optional module name associated with the buffer.
        module: Option<&'static str>,
        /// Position in the buffer that triggered the out-of-bounds access.
        index: usize,
    },

    /// Encountered invalid or unsupported data while interpreting unwind metadata.
    Malformed {
        /// Optional module name tied to the malformed data.
        module: Option<&'static str>,
        /// Static description of why the data is considered malformed.
        reason: &'static str,
    },

    /// Unable to find a loaded PE image that contains the requested instruction pointer.
    ImageNotFound {
        /// Instruction pointer used to locate the PE image.
        rip: u64,
    },

    /// The target PE image does not expose an exception directory.
    ExceptionDirectoryNotFound {
        /// Optional module name for the PE image lacking an exception directory.
        module: Option<&'static str>,
    },

    /// No runtime function entry matches the provided relative instruction pointer for the module.
    RuntimeFunctionNotFound {
        /// Optional module name associated with the lookup.
        module: Option<&'static str>,
        /// Relative virtual address used to search for the runtime function.
        rip_rva: u32,
    },

    /// The runtime function references unwind info that cannot be located at the supplied image base.
    UnwindInfoNotFound {
        /// Optional module name tied to the missing unwind info.
        module: Option<&'static str>,
        /// Module image base used to resolve the unwind info.
        image_base: u64,
        /// RVA of the unwind information that could not be found.
        unwind_info: u32,
    },

    /// Encountered an unexpected unwind opcode sequence while decoding the stack frame.
    UnexpectedUnwindCode {
        /// Optional module name for the frame that failed to unwind.
        module: Option<&'static str>,
    },

    /// Attempted to read past the end of the unwind code buffer during opcode decoding.
    UnwindCodeOutOfBounds {
        /// Optional module name tied to the unwind code buffer.
        module: Option<&'static str>,
        /// Offset that exceeded the available bytes.
        requested: usize,
        /// Total bytes available in the buffer.
        available: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let no_module_str = "<no module>";
        match self {
            Error::OutOfBoundsRead { index, .. } => {
                write!(fmt, "Attempted to read past buffer bounds at index {index}")
            }
            Error::Malformed { reason, .. } => write!(fmt, "Malformed entity: {reason}"),
            Error::ImageNotFound { rip } => {
                write!(fmt, "Failed to locate a PE Image in memory with rip: {rip:X}")
            }
            Error::ExceptionDirectoryNotFound { module } => {
                write!(
                    fmt,
                    "Exception directory not found for module {}. Make sure to build with RUSTFLAGS=-Cforce-unwind-tables",
                    module.as_ref().unwrap_or(&no_module_str)
                )
            }
            Error::RuntimeFunctionNotFound { module, rip_rva } => {
                write!(
                    fmt,
                    "Runtime function not found for module {} with rip(rva): {:X}",
                    module.as_ref().unwrap_or(&no_module_str),
                    rip_rva
                )
            }
            Error::UnwindInfoNotFound { module, image_base, unwind_info } => {
                write!(
                    fmt,
                    "Failed to locate unwind info({:X}) for module {} at image base({:X})",
                    unwind_info,
                    module.as_ref().unwrap_or(&no_module_str),
                    image_base
                )
            }
            Error::UnexpectedUnwindCode { module } => {
                write!(
                    fmt,
                    "Encountered unexpected unwind opcode while decoding for module {}",
                    module.as_ref().unwrap_or(&no_module_str)
                )
            }
            Error::UnwindCodeOutOfBounds { requested, available, .. } => {
                write!(
                    fmt,
                    "Unwind code read exceeded buffer bounds (requested offset {}, available {})",
                    requested, available
                )
            }
        }
    }
}

/// A specialized result type for the patina_stacktrace crate.
pub type StResult<T> = Result<T, Error>;

impl Error {
    /// Propagate the provided module name onto errors that carry optional module context.
    pub fn with_module(self, fallback: Option<&'static str>) -> Self {
        match self {
            Error::OutOfBoundsRead { module, index } => Error::OutOfBoundsRead { module: module.or(fallback), index },
            Error::Malformed { module, reason } => Error::Malformed { module: module.or(fallback), reason },
            Error::ExceptionDirectoryNotFound { module } => {
                Error::ExceptionDirectoryNotFound { module: module.or(fallback) }
            }
            Error::RuntimeFunctionNotFound { module, rip_rva } => {
                Error::RuntimeFunctionNotFound { module: module.or(fallback), rip_rva }
            }
            Error::UnwindInfoNotFound { module, image_base, unwind_info } => {
                Error::UnwindInfoNotFound { module: module.or(fallback), image_base, unwind_info }
            }
            Error::UnexpectedUnwindCode { module } => Error::UnexpectedUnwindCode { module: module.or(fallback) },
            Error::UnwindCodeOutOfBounds { module, requested, available } => {
                Error::UnwindCodeOutOfBounds { module: module.or(fallback), requested, available }
            }
            other => other,
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::Error;

    fn assert_display(err: Error, expected: &str) {
        assert_eq!(format!("{err}"), expected);
    }

    #[test]
    fn buffer_too_short_display() {
        assert_display(
            Error::OutOfBoundsRead { module: Some("image"), index: 5 },
            "Attempted to read past buffer bounds at index 5",
        );
    }

    #[test]
    fn malformed_display() {
        assert_display(Error::Malformed { module: Some("image"), reason: "bad" }, "Malformed entity: bad");
    }

    #[test]
    fn image_not_found_display() {
        assert_display(Error::ImageNotFound { rip: 0x1234 }, "Failed to locate a PE Image in memory with rip: 1234");
    }

    #[test]
    fn exception_directory_display_with_module() {
        assert_display(
            Error::ExceptionDirectoryNotFound { module: Some("mod") },
            "Exception directory not found for module mod. Make sure to build with RUSTFLAGS=-Cforce-unwind-tables",
        );
    }

    #[test]
    fn exception_directory_display_without_module() {
        assert_display(
            Error::ExceptionDirectoryNotFound { module: None },
            "Exception directory not found for module <no module>. Make sure to build with RUSTFLAGS=-Cforce-unwind-tables",
        );
    }

    #[test]
    fn runtime_function_not_found_display() {
        assert_display(
            Error::RuntimeFunctionNotFound { module: Some("image"), rip_rva: 0x10 },
            "Runtime function not found for module image with rip(rva): 10",
        );
    }

    #[test]
    fn unwind_info_not_found_display() {
        assert_display(
            Error::UnwindInfoNotFound { module: Some("image"), image_base: 0x1000, unwind_info: 0x20 },
            "Failed to locate unwind info(20) for module image at image base(1000)",
        );
    }

    #[test]
    fn unexpected_unwind_code_display() {
        assert_display(
            Error::UnexpectedUnwindCode { module: Some("image") },
            "Encountered unexpected unwind opcode while decoding for module image",
        );
    }

    #[test]
    fn unwind_code_out_of_bounds_display() {
        assert_display(
            Error::UnwindCodeOutOfBounds { module: Some("image"), requested: 5, available: 3 },
            "Unwind code read exceeded buffer bounds (requested offset 5, available 3)",
        );
    }

    #[test]
    fn with_module_exercises_all_paths() {
        let fallback = Some("fallback");

        // Validate fallback propagation for every module-carrying variant and ensure others remain untouched.
        let err = Error::OutOfBoundsRead { module: None, index: 7 }.with_module(fallback);
        match err {
            Error::OutOfBoundsRead { module, index } => {
                assert_eq!(module, fallback);
                assert_eq!(index, 7);
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::OutOfBoundsRead { module: Some("explicit"), index: 9 }.with_module(fallback);
        match err {
            Error::OutOfBoundsRead { module, index } => {
                assert_eq!(module, Some("explicit"));
                assert_eq!(index, 9);
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::Malformed { module: None, reason: "bad" }.with_module(fallback);
        match err {
            Error::Malformed { module, reason } => {
                assert_eq!(module, fallback);
                assert_eq!(reason, "bad");
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::Malformed { module: Some("explicit"), reason: "bad" }.with_module(fallback);
        match err {
            Error::Malformed { module, reason } => {
                assert_eq!(module, Some("explicit"));
                assert_eq!(reason, "bad");
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::ExceptionDirectoryNotFound { module: None }.with_module(fallback);
        match err {
            Error::ExceptionDirectoryNotFound { module } => assert_eq!(module, fallback),
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::RuntimeFunctionNotFound { module: None, rip_rva: 0x10 }.with_module(fallback);
        match err {
            Error::RuntimeFunctionNotFound { module, rip_rva } => {
                assert_eq!(module, fallback);
                assert_eq!(rip_rva, 0x10);
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err =
            Error::UnwindInfoNotFound { module: None, image_base: 0x1000, unwind_info: 0x20 }.with_module(fallback);
        match err {
            Error::UnwindInfoNotFound { module, image_base, unwind_info } => {
                assert_eq!(module, fallback);
                assert_eq!(image_base, 0x1000);
                assert_eq!(unwind_info, 0x20);
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::UnwindInfoNotFound { module: None, image_base: 0x2000, unwind_info: 0x30 }.with_module(None);
        match err {
            Error::UnwindInfoNotFound { module, image_base, unwind_info } => {
                assert_eq!(module, None);
                assert_eq!(image_base, 0x2000);
                assert_eq!(unwind_info, 0x30);
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::UnexpectedUnwindCode { module: None }.with_module(fallback);
        match err {
            Error::UnexpectedUnwindCode { module } => assert_eq!(module, fallback),
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::UnwindCodeOutOfBounds { module: None, requested: 5, available: 3 }.with_module(fallback);
        match err {
            Error::UnwindCodeOutOfBounds { module, requested, available } => {
                assert_eq!(module, fallback);
                assert_eq!(requested, 5);
                assert_eq!(available, 3);
            }
            other => panic!("Unexpected variant: {other:?}"),
        };

        let err = Error::ImageNotFound { rip: 0x1234 }.with_module(fallback);
        match err {
            Error::ImageNotFound { rip } => assert_eq!(rip, 0x1234),
            other => panic!("Unexpected variant: {other:?}"),
        };
    }
}
