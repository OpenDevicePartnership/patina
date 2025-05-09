//! Null Interrupt module - For doc tests
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

mod interrupt_manager;
pub use interrupt_manager::InterruptsNull;
use mu_pi::protocols::cpu_arch::EfiSystemContext;
use uefi_sdk::error::EfiError;

#[derive(Debug)]
pub struct ExceptionContextNull;

impl super::EfiSystemContextFactory for ExceptionContextNull {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        // Pointer being set is arbitrary, but EBC is architecture agnostic.
        EfiSystemContext { system_context_ebc: core::ptr::null_mut() }
    }
}

impl super::EfiExceptionStackTrace for ExceptionContextNull {
    fn dump_stack_trace(&self) {}
}

#[allow(unused)]
pub fn enable_interrupts() {}

#[allow(unused)]
pub fn disable_interrupts() {}

#[allow(unused)]
pub fn get_interrupt_state() -> Result<bool, EfiError> {
    Ok(false)
}
