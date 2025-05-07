//! X64 Interrupt module
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::arch::asm;
use mu_pi::protocols::cpu_arch::EfiSystemContext;
use stacktrace::StackTrace;
use uefi_sdk::error::EfiError;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "x86_64"))] {
        mod interrupt_manager;
        pub use interrupt_manager::InterruptManagerX64;
    } else if #[cfg(feature = "doc")] {
        pub use interrupt_manager::InterruptManagerX64;
        mod interrupt_manager;
    }
}

pub type ExceptionContextX64 = r_efi::protocols::debug_support::SystemContextX64;

impl super::EfiSystemContextFactory for ExceptionContextX64 {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        EfiSystemContext { system_context_x64: self as *mut _ }
    }
}

impl super::EfiExceptionStackTrace for ExceptionContextX64 {
    fn dump_stack_trace(&self) {
        if let Err(err) = unsafe { StackTrace::dump_with(self.rip, self.rsp) } {
            log::error!("StackTrace: {}", err);
        }
    }
}

pub fn enable_interrupts() {
    unsafe {
        asm!("sti", options(preserves_flags, nostack));
    }
}

pub fn disable_interrupts() {
    unsafe {
        asm!("cli", options(preserves_flags, nostack));
    }
}

pub fn get_interrupt_state() -> Result<bool, EfiError> {
    let eflags: u64;
    const IF: u64 = 0x200;
    unsafe {
        asm!("pushfq; pop {}", out(reg)eflags);
    }
    Ok(eflags & IF != 0)
}
