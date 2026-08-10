//! Serial Traits and Implementations for the [SerialIO] interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use spin::Mutex;

use crate::{arch::with_interrupts_disabled, base::error::EfiError, peripheral::serial::SerialIO};

/// A wrapper that provides guaranteed single threaded exclusive access to the serial port **in the patina environment**.
/// This is a special implementation because of unique use-case of serial ports that sit outside of the general
/// Patina TPL based locking model.
///
/// This structure makes the assumption that except for error scenarios, that serial port implementations will not be
/// reentrant. Otherwise, spurious errors may be observed.
pub struct SharedSerial<T: SerialIO> {
    serial: Mutex<T>,
    /// When `true`, acquisition blocks (spins) until the port is free instead of failing
    /// fast on contention. See [`SharedSerial::with_blocking`].
    blocking: bool,
}

impl<T: SerialIO> SharedSerial<T> {
    /// Creates a new shared serial port wrapper.
    ///
    /// Acquisition is non-blocking: if the port is already held, the operation returns
    /// [`EfiError::DeviceError`] rather than waiting. Use [`SharedSerial::with_blocking`] for
    /// lossless output under contention.
    pub const fn new(serial: T) -> Self {
        SharedSerial { serial: Mutex::new(serial), blocking: false }
    }

    /// Enables blocking (spinning) acquisition, consuming and returning `self`.
    ///
    /// When blocking is enabled, operations spin until the port is available rather than
    /// returning [`EfiError::DeviceError`] on contention. This guarantees no output is dropped
    /// under multi-core contention, at the cost of reintroducing a self-deadlock hazard if the
    /// same core re-enters the port while already holding it (e.g. logging from a panic handler
    /// mid-write). Prefer [`SharedSerial::new`] unless you specifically need lossless output.
    pub fn with_blocking(mut self) -> Self {
        self.blocking = true;
        self
    }

    /// Acquire the underlying serial port, honoring the configured blocking behavior.
    ///
    /// In blocking mode this spins until the port is free; otherwise it fails fast with
    /// [`EfiError::DeviceError`] when the port is already held.
    fn acquire(&self) -> Result<spin::MutexGuard<'_, T, spin::Spin>, EfiError> {
        if self.blocking { Ok(self.serial.lock()) } else { self.serial.try_lock().ok_or(EfiError::DeviceError) }
    }
    /// Initialize the serial port.
    pub fn init(&self) -> Result<(), EfiError> {
        with_interrupts_disabled(|| {
            let mut serial = self.acquire()?;
            serial.init();
            Ok(())
        })
    }

    /// Write a buffer to the serial port.
    pub fn write(&self, buffer: &[u8]) -> Result<(), EfiError> {
        with_interrupts_disabled(|| {
            let mut serial = self.acquire()?;
            serial.write(buffer);
            Ok(())
        })
    }

    /// Read a byte from the serial port, blocking until a byte is available.
    ///
    /// Interrupts will be disabled while waiting for data to be available. This may cause delays in servicing
    /// interrupts. [`SharedSerial::try_read`] should be used in most scenarios.
    pub fn read(&self) -> Result<u8, EfiError> {
        with_interrupts_disabled(|| {
            let mut serial = self.acquire()?;
            Ok(serial.read())
        })
    }

    /// Try to read a byte from the serial port, returning `None` if no byte is available.
    pub fn try_read(&self) -> Result<Option<u8>, EfiError> {
        with_interrupts_disabled(|| {
            let mut serial = self.acquire()?;
            Ok(serial.try_read())
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::peripheral::serial::MockSerialIO;
    use mockall::predicate::eq;

    #[test]
    fn test_shared_serial_forwards_operations() {
        let mut mock = MockSerialIO::new();
        mock.expect_init().times(1).returning(|| ());
        mock.expect_write().with(eq(*b"hi")).times(1).returning(|_| ());
        mock.expect_read().times(1).returning(|| 0xAB);
        mock.expect_try_read().times(1).returning(|| None);

        let shared = SharedSerial::new(mock);
        shared.init().unwrap();
        shared.write(b"hi").unwrap();
        assert_eq!(shared.read().unwrap(), 0xAB);
        assert_eq!(shared.try_read().unwrap(), None);
    }

    #[test]
    fn test_shared_serial_contested_access_returns_err() {
        let shared = SharedSerial::new(MockSerialIO::new());
        // Simulate the port already being locked (e.g. re-entrant access from the same core).
        let _guard = shared.serial.try_lock().expect("lock should be available");
        assert_eq!(shared.init(), Err(EfiError::DeviceError));
        assert_eq!(shared.write(b"x"), Err(EfiError::DeviceError));
        assert_eq!(shared.try_read(), Err(EfiError::DeviceError));
        assert_eq!(shared.read(), Err(EfiError::DeviceError));
    }
}
