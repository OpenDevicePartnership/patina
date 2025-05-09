//! Null Interrupt initialization
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use uefi_sdk::{component::service::IntoService, error::EfiError};

use crate::interrupts::InterruptManager;

/// Null Implementation of the InterruptManager.
#[derive(Default, Copy, Clone, IntoService)]
#[service(dyn InterruptManager)]
pub struct InterruptsNull {}

impl InterruptsNull {
    pub const fn new() -> Self {
        Self {}
    }

    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
}

impl InterruptManager for InterruptsNull {}
