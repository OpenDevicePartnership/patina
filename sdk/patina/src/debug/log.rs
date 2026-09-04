//! UEFI targeted logging implementations
//!
//! ## Examples
//!
//! ```rust ignore
//! use patina::debug::log::{Format, SerialLogger};
//! use patina::peripheral::serial::Terminal;
//! use patina::peripheral::serial::uart::{Uart16550, UartPl011};
//!
//! let terminal_logger = SerialLogger::new(
//!    Format::Standard,
//!    &[("crate1::module", log::LevelFilter::Off)],
//!    log::LevelFilter::Trace,
//!    Terminal {},
//! );
//!
//! // SAFETY: 0x3F8 is the standard COM1 I/O port, exclusively owned for the logger's lifetime.
//! let uart_16550_logger = SerialLogger::new(
//!    Format::Standard,
//!    &[("crate1::module", log::LevelFilter::Off)],
//!    log::LevelFilter::Trace,
//!    unsafe { Uart16550::new_io(0x3F8) },
//! );
//!
//! // SAFETY: 0x3F8_0000 is a valid, exclusively-owned PL011 MMIO base address on this platform.
//! let uart_pl011_logger = SerialLogger::new(
//!    Format::Standard,
//!    &[("crate1::module", log::LevelFilter::Off)],
//!    log::LevelFilter::Trace,
//!    unsafe { UartPl011::new(0x3F8_0000) },
//! );
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod serial_logger;
pub use serial_logger::Logger as SerialLogger;

use crate::writelncrlf;

// Debug Print Error Levels. These match the EDK II definitions as
// C drivers will send these messages to the logger.

/// Initialization messages.
pub const DEBUG_INIT: u32 = 0x0000_0001;
/// Warning messages.
pub const DEBUG_WARN: u32 = 0x0000_0002;
/// Load event messages.
pub const DEBUG_LOAD: u32 = 0x0000_0004;
/// EFI file system messages.
pub const DEBUG_FS: u32 = 0x0000_0008;
/// Pool allocation and free messages.
pub const DEBUG_POOL: u32 = 0x0000_0010;
/// Page allocation and free messages.
pub const DEBUG_PAGE: u32 = 0x0000_0020;
/// Informational debug messages.
pub const DEBUG_INFO: u32 = 0x0000_0040;
/// PEI, DXE, and SMM dispatcher messages.
pub const DEBUG_DISPATCH: u32 = 0x0000_0080;
/// Variable service messages.
pub const DEBUG_VARIABLE: u32 = 0x0000_0100;
/// Boot manager messages.
pub const DEBUG_BM: u32 = 0x0000_0400;
/// Block I/O driver messages.
pub const DEBUG_BLKIO: u32 = 0x0000_1000;
/// Network I/O driver messages.
pub const DEBUG_NET: u32 = 0x0000_4000;
/// UNDI driver messages.
pub const DEBUG_UNDI: u32 = 0x0001_0000;
/// Load File protocol messages.
pub const DEBUG_LOADFILE: u32 = 0x0002_0000;
/// Event messages.
pub const DEBUG_EVENT: u32 = 0x0008_0000;
/// Global Coherency Database change messages.
pub const DEBUG_GCD: u32 = 0x0010_0000;
/// Memory range cacheability change messages.
pub const DEBUG_CACHE: u32 = 0x0020_0000;
/// Detailed messages that may significantly impact boot performance.
pub const DEBUG_VERBOSE: u32 = 0x0040_0000;
/// Detailed manageability and payload messages for modules such as Redfish, IPMI, and MCTP.
pub const DEBUG_MANAGEABILITY: u32 = 0x0080_0000;
/// Security and security hardware messages, such as TPM messages.
pub const DEBUG_SECURITY: u32 = 0x0100_0000;
/// Page table update messages, including memory encryption and page attribute changes.
pub const DEBUG_PAGING: u32 = 0x0200_0000;
/// Error messages.
pub const DEBUG_ERROR: u32 = 0x8000_0000;

/// Enum to describe the format of the log message.
pub enum Format {
    /// Standard text format containing the log level and message.
    Standard,
    /// JSON blob containing the log level and message.
    Json,
    /// Verbose JSON blob containing the log level, message, target, and file path and line number.
    VerboseJson,
}

impl Format {
    /// Formats the log message and writes it to the target.
    pub fn write<T: core::fmt::Write>(&self, target: &mut T, record: &log::Record) {
        // Note: This function may be called before memory allocation is fully initialized. Therefore, it should not
        //       depend on any heap allocation. In particular, the `format!()` macro creates a `String` which is
        //       allocated on the heap. It is avoided below in favor of directly writing to the target or preparing
        //       the formatting arguments with `format_args!()` to pass to another function that performs the actual
        //       writing.
        match self {
            Format::Standard if record.level() == log::Level::Trace => {
                writelncrlf!(
                    target,
                    "TRACE - {}:{}: {}",
                    record.file().unwrap_or("unknown"),
                    record.line().unwrap_or(0),
                    record.args()
                )
                .expect("Printing to serial failed");
            }
            Format::Standard => {
                writelncrlf!(target, "{} - {}", record.level(), record.args()).expect("Printing to serial failed");
            }
            Format::Json => {
                writelncrlf!(
                    target,
                    "{}",
                    format_args!("{{\"level\": \"{}\" \"message\": \"{}\"}}", record.level(), record.args())
                )
                .expect("Printing to serial failed");
            }
            Format::VerboseJson => {
                writelncrlf!(
                    target,
                    "{}",
                    format_args!(
                        "{{\"level\": \"{}\", \"target\": \"{}\", \"message\": \"{}\", \"file\": \"{}\", \"line\": \"{}\"}}",
                        record.level(),
                        record.target(),
                        record.args(),
                        record.file().unwrap_or("unknown"),
                        record.line().unwrap_or(0)
                    )
                )
                .expect("Printing to serial failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Format;
    use alloc::string::String;

    /// Writes a log record through the given Format into a String buffer and returns it.
    fn format_record(format: &Format, level: log::Level, target: &str, message: &str) -> String {
        let mut buf = String::new();
        let args = format_args!("{message}");
        let record =
            log::Record::builder().args(args).level(level).target(target).file(Some("test.rs")).line(Some(42)).build();
        format.write(&mut buf, &record);
        buf
    }

    #[test]
    fn test_format_standard_ends_with_crlf() {
        let output = format_record(&Format::Standard, log::Level::Info, "test", "hello");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {output:?}");
        assert!(!output.ends_with("\n\r\n"), "should not have bare LF before CRLF");
    }

    #[test]
    fn test_format_standard_trace_ends_with_crlf() {
        let output = format_record(&Format::Standard, log::Level::Trace, "test", "verbose");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {output:?}");
        assert!(output.starts_with("TRACE - test.rs:42:"));
    }

    #[test]
    fn test_format_json_ends_with_crlf() {
        let output = format_record(&Format::Json, log::Level::Warn, "test", "warning");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {output:?}");
    }

    #[test]
    fn test_format_verbose_json_ends_with_crlf() {
        let output = format_record(&Format::VerboseJson, log::Level::Error, "my_target", "oops");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {output:?}");
        assert!(output.contains("my_target"));
        assert!(output.contains("test.rs"));
    }
}
