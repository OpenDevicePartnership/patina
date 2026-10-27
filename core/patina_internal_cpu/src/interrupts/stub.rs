//! Stub Interrupt module for tests.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{error::EfiError, pi::protocol::cpu_arch::EfiSystemContext};

use crate::interrupts::{ExceptionType, HandlerType, InterruptHandler, InterruptManager};

/// Null implementation of the `EfiSystemContextFactory` and `EfiExceptionInfoDump` traits.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ExceptionContextStub;

impl super::EfiSystemContextFactory for ExceptionContextStub {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        // Pointer being set is arbitrary, but EBC is architecture agnostic.
        EfiSystemContext { system_context_ebc: core::ptr::null_mut() }
    }
}

impl super::EfiExceptionInfoDump for ExceptionContextStub {
    fn dump_stack_trace(&self) {}
    fn dump_system_context_registers(&self) {}
}

/// Null Implementation of the `InterruptManager`.
#[derive(Default, Copy, Clone)]
pub struct InterruptsStub {}

impl InterruptsStub {
    /// Creates a new instance of the null implementation of the `InterruptManager`.
    pub const fn new() -> Self {
        Self {}
    }

    /// Registers supplied platform handlers without initializing hardware.
    pub fn initialize(
        &mut self,
        exception_handlers: &[(ExceptionType, &'static dyn InterruptHandler)],
    ) -> Result<(), EfiError> {
        for &(exception_type, handler) in exception_handlers {
            self.register_exception_handler(exception_type, HandlerType::Handler(handler))?;
        }
        Ok(())
    }
}

impl InterruptManager for InterruptsStub {}
