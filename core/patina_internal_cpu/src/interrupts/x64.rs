//! X64 Interrupt module
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{interrupts::ExceptionContextX64, log_registers};
use core::arch::asm;
use patina::{error::EfiError, pi::protocols::cpu_arch::EfiSystemContext};
use patina_stacktrace::{StackFrame, StackTrace};

#[cfg(target_os = "uefi")]
mod idt;
mod interrupt_manager;

#[allow(unused)]
pub use interrupt_manager::InterruptsX64;

impl super::EfiSystemContextFactory for ExceptionContextX64 {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        EfiSystemContext { system_context_x64: self as *mut _ }
    }
}

impl super::EfiExceptionStackTrace for ExceptionContextX64 {
    fn dump_stack_trace(&self) {
        let stack_frame = StackFrame { pc: self.rip, sp: self.rsp, fp: self.rbp };
        // SAFETY: Called during exception handling with CPU context registers. The exception context
        // is considered valid to dump at this time.
        if let Err(err) = unsafe { StackTrace::dump_with(stack_frame) } {
            log::error!("StackTrace: {err}");
        }
    }

    fn dump_system_context_registers(&self) {
        log::error!("Control Registers:");
        log_registers!(
            "CR0",
            self.cr0,
            "CR2",
            self.cr2,
            "CR3",
            self.cr3,
            "CR4",
            self.cr4,
            "RIP",
            self.rip,
            "CS",
            self.cs,
            "SS",
            self.ss,
            "DS",
            self.ds,
            "RSP",
            self.rsp,
            "RFLAGS",
            self.rflags
        );

        log::error!("");

        log::error!("General-Purpose Registers:");
        log_registers!(
            "RAX", self.rax, "RBX", self.rbx, "RCX", self.rcx, "RDX", self.rdx, "RSI", self.rsi, "RDI", self.rdi,
            "RBP", self.rbp, "R8", self.r8, "R9", self.r9, "R10", self.r10, "R11", self.r11, "R12", self.r12, "R13",
            self.r13, "R14", self.r14, "R15", self.r15
        );

        log::debug!("Full Context: {self:#X?}");
    }
}

#[allow(unused)]
pub fn enable_interrupts() {
    // SAFETY: The caller must ensure the system is ready to handle interrupts at this point
    unsafe {
        asm!("sti", options(nostack));
    }
}

#[allow(unused)]
pub fn disable_interrupts() {
    // SAFETY: The caller must ensure the system is ready to disable interrupts at this point
    unsafe {
        asm!("cli", options(nostack));
    }
}

#[allow(unused)]
pub fn get_interrupt_state() -> Result<bool, EfiError> {
    let eflags: u64;
    const IF: u64 = 0x200;
    // SAFETY: The ASM below simply reads the interrupt flag to determine state, it is a safe operation
    unsafe {
        asm!("pushfq; pop {}", out(reg)eflags);
    }
    Ok(eflags & IF != 0)
}
