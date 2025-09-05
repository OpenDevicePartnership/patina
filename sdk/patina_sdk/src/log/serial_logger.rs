//! A serial logger implementation for the `log` crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::serial::SerialIO;
use core::marker::Send;

use super::Format;

/// A Base implementation for a logger.
///
/// ## Functionality
///
/// This implementation writes log messages directly to hardware port
///
pub struct Logger<'a, S>
where
    S: SerialIO + Send,
{
    serial_port: S,
    target_filters: &'a [(&'a str, log::LevelFilter)],
    max_level: log::LevelFilter,
    format: Format,
}

impl<'a, S> Logger<'a, S>
where
    S: SerialIO + Send,
{
    /// Creates a new logger instance.
    pub const fn new(
        format: Format,
        target_filters: &'a [(&'a str, log::LevelFilter)],
        max_level: log::LevelFilter,
        serial_port: S,
    ) -> Self {
        Self { serial_port, target_filters, max_level, format }
    }
}

impl<S> log::Log for Logger<'_, S>
where
    S: SerialIO + Send,
{
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level().to_level_filter()
            <= *self
                .target_filters
                .iter()
                .find(|(name, _)| metadata.target().starts_with(name))
                .map(|(_, level)| level)
                .unwrap_or(&self.max_level)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let mut writer = LogWriter { serial_port: &self.serial_port };
            self.format.write(&mut writer, record);
        }
    }

    fn flush(&self) {
        // Do nothing
    }
}

/// A wrapper for handling log writes to a serial IO object.
struct LogWriter<'a, S>
where
    S: SerialIO + Send,
{
    serial_port: &'a S,
}

impl<S> core::fmt::Write for LogWriter<'_, S>
where
    S: SerialIO + Send,
{
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.serial_port.write(s.as_bytes());
        Ok(())
    }
}
