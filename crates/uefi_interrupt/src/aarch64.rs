//! Aarch64 Interrupt Management
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use crate::InterruptManager;
use uefi_core::error::EfiError;

/// AARCH64 Implementation of the InterruptManager.
#[derive(Default, Copy, Clone)]
pub struct InterruptManagerAarch64 {}

impl InterruptManagerAarch64 {
    pub const fn new() -> Self {
        Self {}
    }
}

impl InterruptManager for InterruptManagerAarch64 {
    fn initialize(&mut self) -> Result<(), EfiError> {
        // TODO
        Ok(())
    }
}
