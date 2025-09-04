//! UEFI targeted logging implementations
//!
//! ## Examples
//!
//! ```rust ignore
//! use patina_sdk::log::SerialLogger;
//! use patina_sdk::serial::SerialIO;
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
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod serial_logger;
pub use serial_logger::Logger as SerialLogger;

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
                writeln!(
                    target,
                    "TRACE - {}:{}: {}",
                    record.file().unwrap_or("unknown"),
                    record.line().unwrap_or(0),
                    record.args()
                )
                .expect("Printing to serial failed");
            }
            Format::Standard => {
                writeln!(target, "{} - {}", record.level(), record.args()).expect("Printing to serial failed");
            }
            Format::Json => {
                write!(
                    target,
                    "{}",
                    format_args!("{{\"level\": \"{}\" \"message\": \"{}\"}}\n", record.level(), record.args())
                )
                .expect("Printing to serial failed");
            }
            Format::VerboseJson => {
                write!(
                    target,
                    "{}",
                    format_args!(
            "{{\"level\": \"{}\", \"target\": \"{}\", \"message\": \"{}\", \"file\": \"{}\", \"line\": \"{}\"}}\n",
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
