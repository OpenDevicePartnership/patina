//! Debugger Transport Implementations.
//!
//! This modules contains the implementation Connection traits for a SerialIO
//! debugger transport as well as other related implementations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::result::Result;
use gdbstub::conn::{Connection, ConnectionExt};
use patina::serial::SerialIO;

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
    type Error = patina::error::EfiError;

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

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use mockall::mock;

    mock! {
        Serial {}

        impl SerialIO for Serial {
            fn init(&self);
            fn write(&self, buffer: &[u8]);
            fn read(&self) -> u8;
            fn try_read(&self) -> Option<u8>;
        }
    }

    #[test]
    fn test_connection_write() {
        let mut mock = MockSerial::new();

        // Set up expectations for each byte write
        mock.expect_write().with(mockall::predicate::eq([0x01])).times(1).returning(|_| ());
        mock.expect_write().with(mockall::predicate::eq([0x02])).times(1).returning(|_| ());
        mock.expect_write().with(mockall::predicate::eq([0x03])).times(1).returning(|_| ());

        let mut connection = SerialConnection::new(&mock);

        // Test writing multiple bytes
        for &byte in &[0x01, 0x02, 0x03] {
            let result = connection.write(byte);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_connection_flush() {
        let mock = MockSerial::new();
        let mut connection = SerialConnection::new(&mock);

        // Flush should always succeed and do nothing for SerialIO
        let result = connection.flush();
        assert!(result.is_ok());
    }

    #[test]
    fn test_connection_read() {
        let mut mock = MockSerial::new();

        // Set up expectations for reading each byte
        mock.expect_read().times(1).returning(|| 0xAA);
        mock.expect_read().times(1).returning(|| 0xBB);
        mock.expect_read().times(1).returning(|| 0xCC);

        let mut connection = SerialConnection::new(&mock);

        // Read the data back
        for expected_byte in [0xAA, 0xBB, 0xCC] {
            let result = connection.read();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), expected_byte);
        }
    }

    #[test]
    fn test_connection_peek() {
        let mut mock = MockSerial::new();

        // Set up expectations for try_read calls from peek
        mock.expect_try_read().times(1).returning(|| Some(0xDD));
        mock.expect_try_read().times(1).returning(|| Some(0xEE));
        mock.expect_try_read().times(1).returning(|| None);

        let mut connection = SerialConnection::new(&mock);

        // First peek should return the first byte
        let result = connection.peek();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(0xDD));

        // Second peek should return the same byte (still peeked)
        let result = connection.peek();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(0xDD));

        // Read should return the peeked byte (without calling transport.read())
        let result = connection.read();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0xDD);

        // Now peek should return the next byte
        let result = connection.peek();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(0xEE));

        // Read should return the next byte
        let result = connection.read();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0xEE);

        // Further peeks should return None (no more data)
        let result = connection.peek();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_logging_suspender() {
        // Get current log level
        let original_level = log::max_level();

        {
            // Create suspender - should turn off logging
            let _suspender = LoggingSuspender::suspend();
            assert_eq!(log::max_level(), log::LevelFilter::Off);
        }

        // After suspender is dropped, logging should be restored
        assert_eq!(log::max_level(), original_level);
    }

    #[test]
    fn test_logging_suspender_nested() {
        let original_level = log::max_level();

        {
            let _suspender1 = LoggingSuspender::suspend();
            assert_eq!(log::max_level(), log::LevelFilter::Off);

            {
                let _suspender2 = LoggingSuspender::suspend();
                assert_eq!(log::max_level(), log::LevelFilter::Off);
            }

            // Still suspended after inner suspender drops
            assert_eq!(log::max_level(), log::LevelFilter::Off);
        }

        // Restored after outer suspender drops
        assert_eq!(log::max_level(), original_level);
    }
}
