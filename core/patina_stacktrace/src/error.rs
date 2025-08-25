//! Error codes for the patina_stacktrace crate
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
use core::fmt;

/// The error type for stacktrace operations.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// Error during parsing the PE
    BufferTooShort(usize),

    /// Error thrown when buffer is not aligned
    BufferUnaligned(usize),

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

    /// Failed to calculate the stack offset
    StackOffsetNotFound(Option<&'static str>),

    /// Failed to dump all the frames in the stack trace
    StackTraceDumpFailed(Option<&'static str>),

    /// Failed to load module(mainly in tests)
    #[cfg(test)]
    ModuleLoadFailed(Option<&'static str>),
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let no_module_str = "<no module>";
        match self {
            Error::BufferTooShort(index) => write!(fmt, "Buffer is too short {index}"),
            Error::BufferUnaligned(addr) => write!(fmt, "Buffer is not aligned {addr:X}"),
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
            Error::StackOffsetNotFound(module) => {
                write!(
                    fmt,
                    "Failed to calculate the stack offset for module {}",
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
            #[cfg(test)]
            Error::ModuleLoadFailed(module) => {
                write!(fmt, "Failed to load module: {}", module.as_ref().unwrap_or(&no_module_str))
            }
        }
    }
}

/// A specialized result type for the patina_stacktrace crate.
pub type StResult<T> = Result<T, Error>;
