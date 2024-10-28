//! Null CPU Initialization
//!
//! This module provides a default implementation for the [CpuInitializer] trait that does nothing.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use uefi_core::interface::CpuInitializer;

/// [CpuInitializer] trait implementation that does nothing.
///
/// This trait implementation is available for any platforms that do not require any CPU
/// initialization.
#[derive(Default)]
pub struct NullCpuInitializer;
impl CpuInitializer for NullCpuInitializer {
    fn initialize(&mut self) {
        // Do nothing
    }
}
