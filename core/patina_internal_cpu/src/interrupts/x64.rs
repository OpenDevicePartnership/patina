//! X64 Interrupt module
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::interrupts::ExceptionContextX64;
use patina::pi::protocol::cpu_arch::EfiSystemContext;
use patina_stacktrace::{StackFrame, StackTrace};

#[cfg_attr(coverage, coverage(off))]
#[cfg(all(target_arch = "x86_64", target_os = "uefi"))]
mod apic;
#[cfg(target_arch = "x86_64")]
pub(crate) mod idt;
mod interrupt_manager;

#[allow(unused)]
pub use interrupt_manager::InterruptsX64;

impl super::EfiSystemContextFactory for ExceptionContextX64 {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        EfiSystemContext { system_context_x64: core::ptr::from_mut(self) }
    }
}

impl super::EfiExceptionInfoDump for ExceptionContextX64 {
    fn dump_stack_trace(&self) {
        let stack_frame = StackFrame { pc: self.rip, sp: self.rsp, fp: self.rbp };
        // SAFETY: Called during exception handling with CPU context registers. The exception context
        // is considered valid to dump at this time.
        if let Err(err) = unsafe { StackTrace::dump_with(stack_frame) } {
            log::error!("StackTrace: {err}");
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    fn dump_system_context_registers(&self) {
        patina::arch::SystemContextDiagnostics::dump_system_context_registers(self);
    }
}
