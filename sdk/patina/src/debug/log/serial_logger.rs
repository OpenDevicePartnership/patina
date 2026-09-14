//! A serial logger implementation for the `log` crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::peripheral::serial::{SerialIO, shared::SharedSerial};
use core::marker::Send;
use spin::Mutex;

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
    serial_port: SharedSerial<S>,
    target_filters: &'a [(&'a str, log::LevelFilter)],
    max_level: log::LevelFilter,
    format: Format,
    /// When `true`, [`write_lock`](Self::write_lock) is engaged to serialize each record.
    blocking: bool,
    /// Serializes an entire record so the multiple `write_str` fragments of a single message
    /// cannot interleave with another core's message. Only taken when `blocking` is set.
    write_lock: Mutex<()>,
}

impl<'a, S> Logger<'a, S>
where
    S: SerialIO + Send,
{
    /// Creates a new logger instance.
    ///
    /// Serial acquisition is non-blocking by default: on contention a write fails fast rather
    /// than waiting. Call [`Logger::with_blocking`] to switch to blocking (lossless) output.
    pub const fn new(
        format: Format,
        target_filters: &'a [(&'a str, log::LevelFilter)],
        max_level: log::LevelFilter,
        serial_port: S,
    ) -> Self {
        Self {
            serial_port: SharedSerial::new(serial_port),
            target_filters,
            max_level,
            format,
            blocking: false,
            write_lock: Mutex::new(()),
        }
    }

    /// Switches the logger to blocking (spinning) serial acquisition, consuming and returning `self`.
    ///
    /// In blocking mode, each record is written under a logger-level lock so concurrent cores
    /// cannot interleave the individual `write_str` fragments of a single message, and the serial
    /// port itself spins until free instead of failing fast on contention. This guarantees lossless,
    /// uncorrupted output at the cost of a self deadlock hazard if the same core re-enters the
    /// logger while already writing a record (e.g. logging from a panic handler mid record).
    #[must_use]
    pub fn with_blocking(mut self) -> Self {
        self.serial_port = self.serial_port.with_blocking();
        self.blocking = true;
        self
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
                .map_or(&self.max_level, |(_, level)| level)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let mut writer = LogWriter { serial_port: &self.serial_port };
            // In blocking mode, hold the logger-level lock for the whole record so concurrent
            // cores cannot interleave the multiple `write_str` fragments of a single message.
            let _guard = self.blocking.then(|| self.write_lock.lock());
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
    serial_port: &'a SharedSerial<S>,
}

impl<S> core::fmt::Write for LogWriter<'_, S>
where
    S: SerialIO + Send,
{
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Best-effort: serial write failures (e.g. contested access) must not break logging.
        let _ = self.serial_port.write(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::debug::log::Format;
    use crate::peripheral::serial::MockSerialIO;
    use log::{Level, LevelFilter, Log, Metadata};
    use spin::Mutex;
    use std::thread;
    use std::{string::String, sync::Arc, vec::Vec};

    fn metadata(level: Level, target: &str) -> Metadata<'_> {
        Metadata::builder().level(level).target(target).build()
    }

    fn recording_serial() -> (MockSerialIO, Arc<Mutex<Vec<u8>>>) {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let sink = buffer.clone();
        let mut mock = MockSerialIO::new();
        mock.expect_write().returning(move |bytes| sink.lock().extend_from_slice(bytes));
        (mock, buffer)
    }

    #[test]
    fn test_serial_logger_writes_using_format() {
        for (format, level, expected) in [
            (Format::Standard, Level::Info, "INFO - hello\r\n"),
            (Format::Json, Level::Warn, "{\"level\": \"WARN\" \"message\": \"hello\"}\r\n"),
        ] {
            let (mock, buffer) = recording_serial();
            let logger = Logger::new(format, &[], LevelFilter::Trace, mock);
            let args = format_args!("hello");
            logger.log(&log::Record::builder().args(args).level(level).target("test").build());
            assert_eq!(String::from_utf8(buffer.lock().clone()).expect("valid utf8"), expected);
        }
    }

    #[test]
    fn test_serial_logger_filters() {
        let logger = Logger::new(Format::Standard, &[], LevelFilter::Warn, MockSerialIO::new());
        assert!(logger.enabled(&metadata(Level::Error, "any")));
        assert!(logger.enabled(&metadata(Level::Warn, "any")));
        assert!(!logger.enabled(&metadata(Level::Info, "any")));

        let filters = &[("noisy::module", LevelFilter::Off), ("app", LevelFilter::Debug)];
        let logger = Logger::new(Format::Standard, filters, LevelFilter::Info, MockSerialIO::new());
        assert!(!logger.enabled(&metadata(Level::Error, "noisy::module::inner")));
        assert!(logger.enabled(&metadata(Level::Debug, "app::sub")));
        assert!(!logger.enabled(&metadata(Level::Trace, "app::sub")));
        assert!(logger.enabled(&metadata(Level::Info, "other")));
        assert!(!logger.enabled(&metadata(Level::Debug, "other")));
    }

    #[test]
    fn test_serial_logger_disabled_noop() {
        let mut mock = MockSerialIO::new();
        mock.expect_write().never();
        let logger = Logger::new(Format::Standard, &[], LevelFilter::Warn, mock);

        let args = format_args!("should be filtered out");
        logger.log(&log::Record::builder().args(args).level(Level::Info).target("test").build());
        logger.flush();
    }

    #[test]
    fn test_serial_logger_blocking_writes_intact_record() {
        let (mock, buffer) = recording_serial();
        let logger = Logger::new(Format::Standard, &[], LevelFilter::Trace, mock).with_blocking();
        assert!(logger.blocking);

        let args = format_args!("hello");
        logger.log(&log::Record::builder().args(args).level(Level::Info).target("test").build());
        assert_eq!(String::from_utf8(buffer.lock().clone()).expect("valid utf8"), "INFO - hello\r\n");
    }

    #[test]
    #[ignore = "Stress test: spawns threads and logs many records; run explicitly with --ignored."]
    fn test_serial_logger_blocking_concurrent_records_not_interleaved() {
        // Each record is emitted as several `write_str` fragments (level, separator, message,
        // terminator). In blocking mode the logger-level `write_lock` must hold for the whole
        // record so two threads writing concurrently cannot interleave those fragments. We log
        // two distinct messages from two threads and assert every rendered record is intact -
        // i.e. the stream is a sequence of whole "A" or "B" records, never a garbled mix.
        const ITERATIONS: usize = 500;

        let (mock, buffer) = recording_serial();
        let logger = Logger::new(Format::Standard, &[], LevelFilter::Trace, mock).with_blocking();

        thread::scope(|scope| {
            for message in ["aaaa", "bbbb"] {
                let logger = &logger;
                scope.spawn(move || {
                    for _ in 0..ITERATIONS {
                        logger.log(
                            &log::Record::builder()
                                .args(format_args!("{message}"))
                                .level(Level::Info)
                                .target("test")
                                .build(),
                        );
                    }
                });
            }
        });

        let output = String::from_utf8(buffer.lock().clone()).expect("valid utf8");
        let (mut a_count, mut b_count) = (0usize, 0usize);
        for record in output.split_terminator("\r\n") {
            match record {
                "INFO - aaaa" => a_count += 1,
                "INFO - bbbb" => b_count += 1,
                garbled => panic!("interleaved/garbled record observed: {garbled:?}"),
            }
        }
        assert_eq!(a_count, ITERATIONS, "missing or corrupted 'aaaa' records");
        assert_eq!(b_count, ITERATIONS, "missing or corrupted 'bbbb' records");
    }
}
