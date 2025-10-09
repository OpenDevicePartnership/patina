//! Null Interrupt module - For doc tests
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod interrupt_manager;
pub use interrupt_manager::InterruptsNull;
use patina::error::EfiError;
use patina_pi::protocols::cpu_arch::EfiSystemContext;

/// Null implementation of the EfiSystemContextFactory and EfiExceptionStackTrace traits.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ExceptionContextNull;

impl super::EfiSystemContextFactory for ExceptionContextNull {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        // Pointer being set is arbitrary, but EBC is architecture agnostic.
        EfiSystemContext { system_context_ebc: core::ptr::null_mut() }
    }
}

impl super::EfiExceptionStackTrace for ExceptionContextNull {
    fn dump_stack_trace(&self) {}
    fn dump_system_context_registers(&self) {}
}

/// A function that does nothing as this is a null implementation.
#[allow(unused)]
pub fn enable_interrupts() {}

/// A function that does nothing as this is a null implementation.
#[allow(unused)]
pub fn disable_interrupts() {}

/// A function that always returns `false` as this is a null implementation.
#[allow(unused)]
pub fn get_interrupt_state() -> Result<bool, EfiError> {
    Ok(false)
}
