//! UEFI Advanced Logger Support
//!
//! This module provides a struct that implements log::Log for writing to a SerialIO
//! and the advanced logger memory log. This module is written to be phase agnostic.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::memory_log::{self, AdvancedLog, LogEntry};
use core::marker::Send;
use log::Level;
use patina::{
    component::service::{Service, perf_timer::ArchTimerFunctionality},
    log::Format,
    serial::SerialIO,
};
use r_efi::efi;
use spin::Once;

// Exists for the debugger to find the log buffer.
#[used]
static mut DBG_ADV_LOG_BUFFER: u64 = 0;

/// The logger for memory/hardware port logging.
pub struct AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    hardware_port: S,
    target_filters: &'a [(&'a str, log::LevelFilter)],
    max_level: log::LevelFilter,
    format: Format,
    memory_log: Once<AdvancedLog<'static>>,
    pub(crate) timer: Service<dyn ArchTimerFunctionality>,
}

impl<'a, S> AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    /// Creates a new AdvancedLogger.
    ///
    /// ## Arguments
    ///
    /// * `format` - The format to use for logging.
    /// * `target_filters` - A list of target filters to apply to the logger.
    /// * `max_level` - The maximum log level to log.
    /// * `hardware_port` - The hardware port to write logs to.
    ///
    pub const fn new(
        format: Format,
        target_filters: &'a [(&'a str, log::LevelFilter)],
        max_level: log::LevelFilter,
        hardware_port: S,
    ) -> Self {
        Self { hardware_port, target_filters, max_level, format, memory_log: Once::new(), timer: Service::new_uninit() }
    }

    /// Initializes the performance timer service for timestamping log entries.
    /// Should only be called once during setup.
    pub fn init_timer(&self, timer: Service<dyn ArchTimerFunctionality>) {
        self.timer.replace(&timer);
    }

    /// Writes a log entry to the hardware port and memory log if available.
    pub(crate) fn log_write(&self, error_level: u32, data: &[u8]) {
        let mut hw_write = true;
        if let Some(memory_log) = self.memory_log.get() {
            hw_write = memory_log.hardware_write_enabled(error_level);
            let timestamp = self.timer.map_or(0, |timer| timer.cpu_count());
            let _ = memory_log.add_log_entry(LogEntry {
                phase: memory_log::ADVANCED_LOGGER_PHASE_DXE,
                level: error_level,
                timestamp,
                data,
            });
        }

        if hw_write {
            self.hardware_port.write(data);
        }
    }

    /// Sets the address of the advanced logger memory log.
    pub(crate) fn set_log_info_address(&self, address: efi::PhysicalAddress) {
        assert!(!self.memory_log.is_completed());
        // SAFETY: The caller must ensure the address is valid for an AdvancedLog.
        if let Some(log) = unsafe { AdvancedLog::adopt_memory_log(address) } {
            let memory_log = self.memory_log.call_once(|| log);
            log::info!("Advanced logger buffer initialized. Address = {:#x}", memory_log.get_address());

            // The frequency may not be initialized, if not do so now.
            if memory_log.get_frequency() == 0 {
                let frequency = self.timer.map_or(0, |timer| timer.perf_frequency());
                memory_log.set_frequency(frequency);
            }

            // SAFETY: This is only set for discoverability while debugging.
            unsafe {
                DBG_ADV_LOG_BUFFER = address;
            }
        } else {
            log::error!("Failed to initialize on existing advanced logger buffer!");
        }
    }

    pub(crate) fn get_log_address(&self) -> Option<efi::PhysicalAddress> {
        self.memory_log.get().map(|log| log.get_address())
    }
}

impl<S> log::Log for AdvancedLogger<'_, S>
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
            let level = log_level_to_debug_level(record.metadata().level());
            let mut writer = BufferedWriter::new(level, self);
            self.format.write(&mut writer, record);
            writer.flush();
        }
    }

    fn flush(&self) {
        // Do nothing
    }
}

/// Converts a log::Level to a EFI Debug Level.
const fn log_level_to_debug_level(level: Level) -> u32 {
    match level {
        Level::Error => memory_log::DEBUG_LEVEL_ERROR,
        Level::Warn => memory_log::DEBUG_LEVEL_WARNING,
        Level::Info => memory_log::DEBUG_LEVEL_INFO,
        Level::Trace => memory_log::DEBUG_LEVEL_VERBOSE,
        Level::Debug => memory_log::DEBUG_LEVEL_INFO,
    }
}

/// Size of the buffer for the buffered writer.
const WRITER_BUFFER_SIZE: usize = 128;

/// A wrapper for buffering and redirecting writes from the formatter.
struct BufferedWriter<'a, S>
where
    S: SerialIO + Send,
{
    level: u32,
    writer: &'a AdvancedLogger<'a, S>,
    buffer: [u8; WRITER_BUFFER_SIZE],
    buffer_size: usize,
}

impl<'a, S> BufferedWriter<'a, S>
where
    S: SerialIO + Send,
{
    /// Creates a new BufferedWriter with the specified log level and writer.
    const fn new(level: u32, writer: &'a AdvancedLogger<'a, S>) -> Self {
        Self { level, writer, buffer: [0; WRITER_BUFFER_SIZE], buffer_size: 0 }
    }

    /// Flushes the current buffer to the underlying writer.
    fn flush(&mut self) {
        if self.buffer_size == 0 {
            return;
        }

        let data = &self.buffer[0..self.buffer_size];
        self.writer.log_write(self.level, data);
        self.buffer_size = 0;
    }
}

impl<S> core::fmt::Write for BufferedWriter<'_, S>
where
    S: SerialIO + Send,
{
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let data = s.as_bytes();
        let len = data.len();

        // buffer the message if it will fit.
        if len < WRITER_BUFFER_SIZE {
            // If it will not fit with the current data, flush the current data.
            if len > WRITER_BUFFER_SIZE - self.buffer_size {
                self.flush();
            }
            self.buffer[self.buffer_size..self.buffer_size + len].copy_from_slice(data);
            self.buffer_size += len;
        } else {
            // this message is too big to buffer, flush then write the message.
            self.flush();
            self.writer.log_write(self.level, data);
        }

        Ok(())
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use alloc::boxed::Box;
    use patina::{
        component::service::{IntoService, perf_timer::ArchTimerFunctionality},
        log::Format,
        serial::uart::UartNull,
    };

    use crate::logger::AdvancedLogger;

    #[derive(IntoService)]
    #[service(dyn ArchTimerFunctionality)]
    struct MockTimer {}

    impl ArchTimerFunctionality for MockTimer {
        fn perf_frequency(&self) -> u64 {
            100
        }
        fn cpu_count(&self) -> u64 {
            200
        }
    }

    #[test]
    fn test_uninit() {
        let serial = UartNull {};
        let logger_uninit = AdvancedLogger::<UartNull>::new(
            Format::Standard,
            &[("test_target", log::LevelFilter::Info)],
            log::LevelFilter::Debug,
            serial,
        );
        assert!(logger_uninit.timer.map_or(0, |timer| timer.cpu_count()) == 0);
    }

    #[test]
    fn test_init() {
        let serial = UartNull {};
        let logger_uninit = AdvancedLogger::<UartNull>::new(
            Format::Standard,
            &[("test_target", log::LevelFilter::Info)],
            log::LevelFilter::Debug,
            serial,
        );
        logger_uninit.init_timer(patina::component::service::Service::mock(Box::new(MockTimer {})));
        assert!(logger_uninit.timer.cpu_count() > 0);
    }
}
