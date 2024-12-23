//! UEFI Advanced Logger Support
//!
//! This module provides a struct that implements log::Log for writing to a SerialIO
//! and the advanced logger memory log. This module is written to be phase agnostic.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use crate::memory_log::{self, AdvLoggerInfo, LogEntry};
use core::marker::Send;
use log::Level;
use r_efi::efi;
use spin::Once;
use uefi_sdk::{log::Format, serial::SerialIO};

/// The logger for memory/hardware port logging.
pub struct AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    hardware_port: S,
    target_filters: &'a [(&'a str, log::LevelFilter)],
    max_level: log::LevelFilter,
    format: Format,
    memory_log: Once<&'static AdvLoggerInfo>,
}

impl<'a, S> AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    pub const fn new(
        format: Format,
        target_filters: &'a [(&'a str, log::LevelFilter)],
        max_level: log::LevelFilter,
        hardware_port: S,
    ) -> Self {
        Self { hardware_port, target_filters, max_level, format, memory_log: Once::new() }
    }

    pub fn log_write(&self, error_level: u32, data: &[u8]) {
        let mut hw_write = true;
        if let Some(memory_log) = self.get_log_info() {
            hw_write = memory_log.hardware_write_enabled(error_level);
            memory_log.add_log_entry(LogEntry {
                phase: memory_log::ADVANCED_LOGGER_PHASE_DXE,
                level: error_level,
                timestamp: 0, // TODO - Lacking mu_perf_timer support for Q35.
                data,
            });
        }

        if hw_write {
            self.hardware_port.write(data);
        }
    }

    pub fn set_log_info_address(&self, address: efi::PhysicalAddress) {
        assert!(!self.memory_log.is_completed());
        if let Some(log_info) = unsafe { AdvLoggerInfo::adopt_memory_log(address) } {
            self.memory_log.call_once(|| log_info);
            log::info!("Advanced logger buffer initialized. Address = {:#p}", log_info);
        } else {
            log::error!("Failed to initialize on existing advanced logger buffer!");
        }
    }

    pub fn get_log_info(&self) -> Option<&AdvLoggerInfo> {
        match self.memory_log.get() {
            Some(log_info) => Some(*log_info),
            None => None,
        }
    }
}

impl<'a, S> log::Log for AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        return metadata.level().to_level_filter()
            <= *self
                .target_filters
                .iter()
                .find(|(name, _)| metadata.target().starts_with(name))
                .map(|(_, level)| level)
                .unwrap_or(&self.max_level);
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
        Level::Debug => memory_log::DEBUG_LEVEL_VERBOSE,
    }
}

/// Size of the buffer for the buffered writer.
const WRITER_BUFFER_SIZE: usize = 128;

/// A wrapper for buffering and redirecting writes from the formatter.
pub struct BufferedWriter<'a, S>
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
    pub const fn new(level: u32, writer: &'a AdvancedLogger<'a, S>) -> Self {
        Self { level, writer, buffer: [0; WRITER_BUFFER_SIZE], buffer_size: 0 }
    }

    pub fn flush(&mut self) {
        if self.buffer_size == 0 {
            return;
        }

        let data = &self.buffer[0..self.buffer_size];
        self.writer.log_write(self.level, data);
        self.buffer_size = 0;
    }
}

impl<'a, S> core::fmt::Write for BufferedWriter<'a, S>
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
