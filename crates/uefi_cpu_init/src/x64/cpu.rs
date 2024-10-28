//! x86_86 CPU initialization implementation
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use uefi_core::interface::CpuInitializer;

use crate::x64::gdt;

/// [CpuInitializer] trait implementation for the x86_64 architecture.
///
/// TODO: Explain the initialization process this provides.
#[derive(Default)]
pub struct X64CpuInitializer;
impl CpuInitializer for X64CpuInitializer {
    fn initialize(&mut self) {
        gdt::init();
        x86_64::instructions::interrupts::enable();
    }
}
