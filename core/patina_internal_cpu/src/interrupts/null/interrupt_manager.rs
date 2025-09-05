//! Null Interrupt initialization
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina_sdk::{component::service::IntoService, error::EfiError};

use crate::interrupts::InterruptManager;

/// Null Implementation of the InterruptManager.
#[derive(Default, Copy, Clone, IntoService)]
#[service(dyn InterruptManager)]
pub struct InterruptsNull {}

impl InterruptsNull {
    /// Creates a new instance of the null implementation of the InterruptManager.
    pub const fn new() -> Self {
        Self {}
    }

    /// A do-nothing initialization function for the null implementation.
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
}

impl InterruptManager for InterruptsNull {}
