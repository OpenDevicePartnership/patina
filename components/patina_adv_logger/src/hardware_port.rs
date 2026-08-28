//! UEFI Advanced Logger Hardware Port Support
//!
//! This module provides the [`AdvancedLoggerHardwarePort`] abstraction used by the advanced logger
//! to emit log messages to a hardware port, along with [`SerialHardwarePort`], the default
//! implementation that writes to a [`SerialIO`] instance.
//!
//! Platforms that need to gate, redirect, or otherwise customize hardware port output can provide
//! their own implementation of [`AdvancedLoggerHardwarePort`] instead of using
//! [`SerialHardwarePort`]. This is the Patina equivalent of the EDK II `AdvancedLoggerHdwPortLib`
//! library class.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    error::EfiError,
    peripheral::serial::{SerialIO, shared::SharedSerial},
};

/// The hardware port used by the advanced logger to emit log messages.
///
/// The advanced logger applies the memory log's hardware print level filtering before invoking the
/// port, so implementations are only called for messages that the logger has already determined
/// should reach the hardware port. `error_level` is still provided so that implementations may
/// apply additional platform specific filtering.
///
/// Implementations must be usable from a `static` and are expected to provide their own
/// synchronization, as the advanced logger holds the port behind a shared reference.
///
/// ## Errors
///
/// Returns an error if the message could not be emitted. Implementations that intentionally
/// suppress output should return `Ok(())` rather than an error, as a suppressed message is not a
/// failure.
#[cfg_attr(test, mockall::automock)]
pub trait AdvancedLoggerHardwarePort: Send + Sync {
    /// Writes the provided message bytes to the hardware port.
    fn write(&self, error_level: u32, buffer: &[u8]) -> Result<(), EfiError>;
}

/// The default [`AdvancedLoggerHardwarePort`] implementation, which writes every message it
/// receives to the provided serial port.
pub struct SerialHardwarePort<S: SerialIO> {
    serial: SharedSerial<S>,
}

impl<S: SerialIO> SerialHardwarePort<S> {
    /// Creates a new hardware port that writes to the provided serial port.
    pub const fn new(serial: S) -> Self {
        Self { serial: SharedSerial::new(serial) }
    }

    /// Enables blocking (spinning) acquisition of the serial port, consuming and returning `self`.
    ///
    /// See [`SharedSerial::with_blocking`] for the tradeoffs of enabling this behavior.
    #[must_use]
    pub fn with_blocking(self) -> Self {
        Self { serial: self.serial.with_blocking() }
    }
}

impl<S: SerialIO> AdvancedLoggerHardwarePort for SerialHardwarePort<S> {
    fn write(&self, _error_level: u32, buffer: &[u8]) -> Result<(), EfiError> {
        self.serial.write(buffer)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::peripheral::serial::uart::UartNull;

    #[test]
    fn test_serial_hardware_port_forwards_writes() {
        let port = SerialHardwarePort::new(UartNull {});
        assert_eq!(port.write(0x8000_0000, b"hello"), Ok(()));
    }

    #[test]
    fn test_serial_hardware_port_blocking_forwards_writes() {
        let port = SerialHardwarePort::new(UartNull {}).with_blocking();
        assert_eq!(port.write(0, b"hello"), Ok(()));
    }
}
