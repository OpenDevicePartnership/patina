//! This module provides a serial IO implementation that uses the std input/output streams.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use std::io::{Read, Write};
use uefi_core::interface::SerialIO;

/// An interface for writing to the std input/output streams
pub struct Terminal {}

impl SerialIO for Terminal {
    fn init(&self) {}

    fn write(&self, buffer: &[u8]) {
        std::io::stdout().write_all(buffer).unwrap();
    }

    fn read(&self) -> u8 {
        let buffer = &mut [0u8; 1];
        std::io::stdin().read_exact(buffer).unwrap();
        buffer[0]
    }

    fn try_read(&self) -> Option<u8> {
        let buffer = &mut [0u8; 1];
        match std::io::stdin().read(buffer) {
            Ok(0) => None,
            Ok(_) => Some(buffer[0]),
            Err(_) => None,
        }
    }
}
