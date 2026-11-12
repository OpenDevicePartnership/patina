//! `AArch64` Interrupt module
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::pi::protocol::cpu_arch::EfiSystemContext;
use patina_stacktrace::{StackFrame, StackTrace, error::Error};

#[cfg(target_arch = "aarch64")]
pub mod gic_manager;
mod interrupt_manager;

#[allow(unused)]
pub use interrupt_manager::InterruptsAarch64;

pub type ExceptionContextAArch64 = patina::standard::efi::protocols::debug_support::SystemContextAArch64;

impl super::EfiSystemContextFactory for ExceptionContextAArch64 {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        EfiSystemContext { system_context_aarch64: core::ptr::from_mut(self) }
    }
}

impl super::EfiExceptionInfoDump for ExceptionContextAArch64 {
    fn dump_stack_trace(&self) {
        let stack_frame = StackFrame { pc: self.elr, sp: self.sp, fp: self.fp };
        // SAFETY: Called during exception handling with CPU context registers.
        // The exception context is considered valid to dump at this time.
        match unsafe { StackTrace::dump_with(stack_frame) } {
            Ok(()) => (),
            Err(Error::ExceptionDirectoryNotFound { module }) => {
                let no_name = "<no module>";
                let image_name = module.unwrap_or(no_name);
                log::error!(
                    "StackTrace: `{image_name}` does not contain an exception directory. Trying fp/lr based stack trace instead."
                );
                // SAFETY: Called during exception handling with PC, SP, and FP
                // captured from the exception context. The frame pointer chain is
                // walked as a best effort fallback mainly to cater binaries
                // compiled with the GCC toolchain.
                if let Err(err) = unsafe { StackTrace::dump_with_fp_chain(stack_frame) } {
                    log::error!("StackTrace: {err}");
                }
            }
            Err(err) => {
                log::error!("StackTrace: {err}");
            }
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    fn dump_system_context_registers(&self) {
        patina::arch::SystemContextDiagnostics::dump_system_context_registers(self);
    }
}
