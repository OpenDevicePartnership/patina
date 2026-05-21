//! UEFI targeted logging implementations
//!
//! ## Examples
//!
//! ```rust ignore
//! use patina::log::SerialLogger;
//! use patina::serial::SerialIO;
//! use serial_writer::*;
//!
//! let terminal_logger = SerialLogger::new(
//!    Format::Standard,
//!    &[("crate1::module", log::LevelFilter::Off)],
//!    log::LevelFilter::Trace,
//!    Terminal,
//! );
//!
//! let uart_16550_logger = SerialLogger::new(
//!    Format::Standard,
//!    &[("crate1::module", log::LevelFilter::Off)],
//!    log::LevelFilter::Trace,
//!    Uart16550::new(Interface::Io(0x3F8)),
//! );
//!
//! let uart_pl011_logger = SerialLogger::new(
//!    Format::Standard,
//!    &[("crate1::module", log::LevelFilter::Off)],
//!    log::LevelFilter::Trace,
//!    UartPl011::new(0x3F8_0000),
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
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {:?}", output);
        assert!(!output.ends_with("\n\r\n"), "should not have bare LF before CRLF");
    }

    #[test]
    fn test_format_standard_trace_ends_with_crlf() {
        let output = format_record(&Format::Standard, log::Level::Trace, "test", "verbose");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {:?}", output);
        assert!(output.starts_with("TRACE - test.rs:42:"));
    }

    #[test]
    fn test_format_json_ends_with_crlf() {
        let output = format_record(&Format::Json, log::Level::Warn, "test", "warning");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {:?}", output);
    }

    #[test]
    fn test_format_verbose_json_ends_with_crlf() {
        let output = format_record(&Format::VerboseJson, log::Level::Error, "my_target", "oops");
        assert!(output.ends_with("\r\n"), "expected CRLF line ending, got: {:?}", output);
        assert!(output.contains("my_target"));
        assert!(output.contains("test.rs"));
    }
}
