//! This module provides a serial IO implementation that uses the std input/output streams.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use std::io::{Read, Write};

/// An interface for writing to the std input/output streams
pub struct Terminal {}

impl super::SerialIO for Terminal {
    fn init(&mut self) {}

    fn write(&mut self, buffer: &[u8]) {
        std::io::stdout().write_all(buffer).unwrap();
    }

    fn read(&mut self) -> u8 {
        let buffer = &mut [0u8; 1];
        std::io::stdin().read_exact(buffer).unwrap();
        buffer[0]
    }

    fn try_read(&mut self) -> Option<u8> {
        let buffer = &mut [0u8; 1];
        match std::io::stdin().read(buffer) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(buffer[0]),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::peripheral::serial::SerialIO;

    #[test]
    fn test_terminal_init() {
        let mut terminal = Terminal {};
        terminal.init();
    }

    #[test]
    fn test_terminal_write_bytes() {
        let mut terminal = Terminal {};
        terminal.write(b"unittest output\n");
    }
}
