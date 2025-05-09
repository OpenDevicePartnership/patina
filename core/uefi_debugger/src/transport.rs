//! Debugger Transport Implementations.
//!
//! This modules contains the implementation Connection traits for a SerialIO
//! debugger transport as well as other related implementations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::result::Result;
use gdbstub::conn::{Connection, ConnectionExt};
use uefi_sdk::serial::SerialIO;

/// Serial Connection for use with GdbStub
///
/// Wraps the SerialIO interface for use with GdbStub.
///
pub(crate) struct SerialConnection<'a, T: SerialIO> {
    /// Serial IO transport for connecting to the debugger.
    transport: &'a T,
    /// Peeked byte for use with the GdbStub peek method.
    peeked_byte: Option<u8>,
}

impl<'a, T: SerialIO> SerialConnection<'a, T> {
    /// Create a new SerialConnection
    pub fn new(transport: &'a T) -> Self {
        SerialConnection { transport, peeked_byte: None }
    }
}

impl<T: SerialIO> Connection for SerialConnection<'_, T> {
    type Error = uefi_sdk::error::EfiError;

    /// Write a byte to the serial transport.
    fn write(&mut self, byte: u8) -> Result<(), Self::Error> {
        let buff = [byte];
        self.transport.write(&buff);
        Ok(())
    }

    /// Flush the serial transport.
    fn flush(&mut self) -> Result<(), Self::Error> {
        // Nothing to do for SerialIO.
        Ok(())
    }
}

impl<T: SerialIO> ConnectionExt for SerialConnection<'_, T> {
    /// Read a byte from the serial transport.
    fn read(&mut self) -> Result<u8, Self::Error> {
        if let Some(byte) = self.peeked_byte {
            self.peeked_byte = None;
            return Ok(byte);
        }

        Ok(self.transport.read())
    }

    /// Peek a byte from the serial transport.
    fn peek(&mut self) -> Result<Option<u8>, Self::Error> {
        if self.peeked_byte.is_some() {
            return Ok(self.peeked_byte);
        }

        match self.transport.try_read() {
            Some(byte) => {
                self.peeked_byte = Some(byte);
                Ok(Some(byte))
            }
            None => Ok(None),
        }
    }
}

/// Structure for suspending logging within a given scope.
pub struct LoggingSuspender {
    level: log::LevelFilter,
}

impl LoggingSuspender {
    /// Suspend logging within the current scope. When the returned LoggingSuspender
    /// goes out of scope, logging will be restored to the previous level.
    pub fn suspend() -> Self {
        let level = log::max_level();
        log::set_max_level(log::LevelFilter::Off);
        LoggingSuspender { level }
    }
}

impl Drop for LoggingSuspender {
    fn drop(&mut self) {
        log::set_max_level(self.level);
    }
}

/// Buffer for monitor command output. This is needed since the out provided
/// by gdbstub will write immediately and this might confuse the debugger.
pub struct BufferWriter<'a> {
    buffer: &'a mut [u8],
    pos: usize,
    truncated: usize,
}

impl<'a> BufferWriter<'a> {
    pub fn new(buffer: &'a mut [u8]) -> Self {
        BufferWriter { buffer, pos: 0, truncated: 0 }
    }

    pub fn flush_to_console(&mut self, out: &mut gdbstub::target::ext::monitor_cmd::ConsoleOutput<'_>) {
        if self.pos > 0 {
            if self.truncated > 0 {
                log::error!("Truncated monitor output by {} bytes", self.truncated);
            }

            out.write_raw(&self.buffer[..self.pos]);
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        self.pos = 0;
        self.truncated = 0;
    }
}

impl core::fmt::Write for BufferWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let len = bytes.len().min(self.buffer.len() - self.pos);
        if len < bytes.len() {
            self.truncated += bytes.len() - len;
        }

        self.buffer[self.pos..self.pos + len].copy_from_slice(&bytes[0..len]);
        self.pos += len;
        Ok(())
    }
}
