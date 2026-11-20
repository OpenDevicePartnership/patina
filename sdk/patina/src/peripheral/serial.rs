//! Serial Traits and Implementations for the [`SerialIO`] interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// A Trait for a Rust-UEFI serial IO access.
#[cfg_attr(any(test, feature = "mockall"), mockall::automock)]
pub trait SerialIO: Send {
    /// Initialize the serial port.
    fn init(&mut self);
    /// Write a buffer to the serial port.
    ///
    /// Bytes may still be queued for transmission when this returns.
    fn write(&mut self, buffer: &[u8]);
    /// Wait for queued output to finish transmitting, if supported by the implementation.
    ///
    /// The default implementation does nothing. Buffered implementations should override
    /// this method to drain their transmit buffers and shift registers.
    fn flush(&mut self) {}
    /// Read a byte from the serial port, blocking until a byte is available.
    fn read(&mut self) -> u8;
    /// Try to read a byte from the serial port, returning `None` if no byte is available.
    fn try_read(&mut self) -> Option<u8>;
}

pub mod shared;
pub mod uart;
pub mod virtio;

#[cfg(feature = "std")]
mod host;
#[cfg(feature = "std")]
pub use host::Terminal;
