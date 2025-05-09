//! Serial Traits and Implementations for the [SerialIO] interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

/// A Trait for a Rust-UEFI serial IO access.
pub trait SerialIO: Sync {
    /// Initialize the serial port.
    fn init(&self);
    /// Write a buffer to the serial port.
    fn write(&self, buffer: &[u8]);
    /// Read a byte from the serial port, blocking until a byte is available.
    fn read(&self) -> u8;
    /// Try to read a byte from the serial port, returning `None` if no byte is available.
    fn try_read(&self) -> Option<u8>;
}

pub mod uart;

#[cfg(feature = "std")]
mod std;
#[cfg(feature = "std")]
pub use std::Terminal;
