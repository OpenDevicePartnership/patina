//! AArch64 Interrupt module
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use mu_pi::protocols::cpu_arch::EfiSystemContext;
use patina::error::EfiError;
use patina_stacktrace::StackTrace;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        mod interrupt_manager;
        mod sysreg;
        pub mod gic_manager;
        pub use interrupt_manager::InterruptsAarch64;
        use core::arch::asm;
        use crate::interrupts::aarch64::sysreg::read_sysreg;
    } else if #[cfg(feature = "doc")] {
        pub use interrupt_manager::InterruptsAarch64;
        mod interrupt_manager;
    }
}

pub type ExceptionContextAArch64 = r_efi::protocols::debug_support::SystemContextAArch64;

impl super::EfiSystemContextFactory for ExceptionContextAArch64 {
    fn create_efi_system_context(&mut self) -> EfiSystemContext {
        EfiSystemContext { system_context_aarch64: self as *mut _ }
    }
}

impl super::EfiExceptionStackTrace for ExceptionContextAArch64 {
    fn dump_stack_trace(&self) {
        if let Err(err) = unsafe { StackTrace::dump_with(self.elr, self.sp) } {
            log::error!("StackTrace: {err}");
        }
    }
}

#[allow(unused)]
pub fn enable_interrupts() {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        unsafe {
            asm!("msr   daifclr, 0x02", "isb", options(nostack));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        unimplemented!()
    }
}

#[allow(unused)]
pub fn disable_interrupts() {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        unsafe {
            asm!("msr   daifset, 0x02", "isb", options(nostack));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        unimplemented!()
    }
}

#[allow(unused)]
pub fn get_interrupt_state() -> Result<bool, EfiError> {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        let daif = unsafe { read_sysreg!(daif) };
        Ok(daif & 0x80 == 0)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        Err(EfiError::Unsupported)
    }
}
