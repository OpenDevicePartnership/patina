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
use spin::Mutex;

use super::Format;

/// Global lock serializing whole log records so concurrent callers do not
/// interleave formatted output on the serial port.
static LOG_LOCK: Mutex<()> = Mutex::new(());

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
            // Hold the lock for the whole record so formatted output is not interleaved.
            let _guard = LOG_LOCK.lock();
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

#[cfg(test)]
mod tests {
    use super::Logger;
    use crate::log::Format;
    use crate::serial::SerialIO;
    use alloc::string::{String, ToString};
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use spin::Mutex;

    /// A [`SerialIO`] implementation that captures every byte written to it into a
    /// shared buffer, so tests can inspect the serialized output.
    struct CaptureSerial {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl SerialIO for CaptureSerial {
        fn init(&self) {}

        fn write(&self, buffer: &[u8]) {
            self.buffer.lock().extend_from_slice(buffer);
            // Yield after each fragment to widen the interleaving window. The
            // record-level `LOG_LOCK` must still keep concurrent records from
            // interleaving on the shared buffer.
            std::thread::yield_now();
        }

        fn read(&self) -> u8 {
            0
        }

        fn try_read(&self) -> Option<u8> {
            None
        }
    }

    /// Logs a single record through the given logger with the provided level, target and message.
    fn log_record<S: SerialIO + Send>(logger: &Logger<'_, S>, level: log::Level, target: &str, message: &str) {
        log::Log::log(
            logger,
            &log::Record::builder().args(format_args!("{message}")).level(level).target(target).build(),
        );
    }

    fn buffer_to_string(buffer: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buffer.lock().clone()).expect("captured output must be valid UTF-8")
    }

    #[test]
    fn test_serial_logger_writes_formatted_record() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let logger =
            Logger::new(Format::Standard, &[], log::LevelFilter::Trace, CaptureSerial { buffer: buffer.clone() });

        log_record(&logger, log::Level::Info, "test", "hello");

        assert_eq!(buffer_to_string(&buffer), "INFO - hello\r\n");
    }

    #[test]
    fn test_serial_logger_sequential_records_release_lock() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let logger =
            Logger::new(Format::Standard, &[], log::LevelFilter::Trace, CaptureSerial { buffer: buffer.clone() });

        // Multiple sequential logs must each acquire and release `LOG_LOCK`
        // without deadlocking, producing all records in order.
        log_record(&logger, log::Level::Info, "test", "first");
        log_record(&logger, log::Level::Warn, "test", "second");
        log_record(&logger, log::Level::Error, "test", "third");

        assert_eq!(buffer_to_string(&buffer), "INFO - first\r\nWARN - second\r\nERROR - third\r\n");
    }

    #[test]
    fn test_serial_logger_respects_max_level() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let logger =
            Logger::new(Format::Standard, &[], log::LevelFilter::Warn, CaptureSerial { buffer: buffer.clone() });

        // Below the max level: filtered out, nothing written and the lock is never taken.
        log_record(&logger, log::Level::Info, "test", "ignored");
        assert!(buffer.lock().is_empty());

        // At or above the max level: written.
        log_record(&logger, log::Level::Error, "test", "boom");
        assert_eq!(buffer_to_string(&buffer), "ERROR - boom\r\n");
    }

    #[test]
    fn test_serial_logger_respects_target_filters() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let filters: &[(&str, log::LevelFilter)] = &[("noisy", log::LevelFilter::Off)];
        let logger =
            Logger::new(Format::Standard, filters, log::LevelFilter::Trace, CaptureSerial { buffer: buffer.clone() });

        // The "noisy" target is silenced by its filter.
        log_record(&logger, log::Level::Info, "noisy", "dropped");
        assert!(buffer.lock().is_empty());

        // Other targets fall back to the max level and are written.
        log_record(&logger, log::Level::Info, "other", "kept");
        assert_eq!(buffer_to_string(&buffer), "INFO - kept\r\n");
    }

    #[test]
    fn test_serial_logger_concurrent_records_not_interleaved() {
        const NUM_THREADS: usize = 4;
        const RECORDS_PER_THREAD: usize = 25;

        let buffer = Arc::new(Mutex::new(Vec::new()));

        // Every logger shares the same underlying buffer. Because each log record is
        // written as several `write` fragments, only the global record-level
        // `LOG_LOCK` prevents concurrent records from interleaving on that buffer.
        std::thread::scope(|scope| {
            for t in 0..NUM_THREADS {
                let buffer = buffer.clone();
                scope.spawn(move || {
                    let logger = Logger::new(Format::Standard, &[], log::LevelFilter::Trace, CaptureSerial { buffer });
                    for i in 0..RECORDS_PER_THREAD {
                        log::Log::log(
                            &logger,
                            &log::Record::builder()
                                .args(format_args!("thread{t}-msg{i}"))
                                .level(log::Level::Info)
                                .target("test")
                                .build(),
                        );
                    }
                });
            }
        });

        let mut expected = Vec::new();
        for t in 0..NUM_THREADS {
            for i in 0..RECORDS_PER_THREAD {
                expected.push(alloc::format!("INFO - thread{t}-msg{i}"));
            }
        }
        expected.sort();

        let text = buffer_to_string(&buffer);
        let mut actual: Vec<String> =
            text.split("\r\n").filter(|line| !line.is_empty()).map(|l| l.to_string()).collect();
        actual.sort();

        // Every record must appear exactly once and intact. Any interleaving would
        // corrupt a line and fail this multiset comparison.
        assert_eq!(actual, expected);
    }
}
