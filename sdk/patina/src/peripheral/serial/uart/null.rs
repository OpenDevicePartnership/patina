//! Null (stub) [`SerialIO`](crate::peripheral::serial::SerialIO) implementation.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// A null (stub) device that does nothing.
#[derive(Debug)]
pub struct UartNull {}

impl crate::peripheral::serial::SerialIO for UartNull {
    fn init(&mut self) {}

    fn write(&mut self, _buffer: &[u8]) {}

    fn read(&mut self) -> u8 {
        // PANIC: Would loop forever, better to panic.
        panic!();
    }

    fn try_read(&mut self) -> Option<u8> {
        None
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::peripheral::serial::SerialIO;

    #[test]
    fn test_init() {
        let mut uart = UartNull {};
        uart.init();
    }

    #[test]
    fn test_write() {
        let mut uart = UartNull {};
        uart.write(b"nothing happens!");
    }

    #[test]
    fn test_try_read() {
        let mut uart = UartNull {};
        assert_eq!(uart.try_read(), None);
    }

    #[test]
    #[should_panic]
    fn test_read_panics() {
        let mut uart = UartNull {};
        uart.read();
    }
}
