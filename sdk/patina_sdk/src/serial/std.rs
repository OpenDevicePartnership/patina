//! This module provides a serial IO implementation that uses the std input/output streams.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use std::io::{Read, Write};

/// An interface for writing to the std input/output streams
pub struct Terminal {}

impl super::SerialIO for Terminal {
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
