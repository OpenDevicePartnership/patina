//! AArch64 Interrupt module
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::log_registers;
use patina::error::EfiError;
use patina::pi::protocols::cpu_arch::EfiSystemContext;
use patina_stacktrace::StackTrace;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        mod interrupt_manager;
        pub mod gic_manager;
        pub use interrupt_manager::InterruptsAarch64;
        use patina::{read_sysreg, write_sysreg};
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
        // SAFETY: This is called from the exception context. We have no choice but to trust the ELR and SP values.
        // the stack trace module does its best to not cause recursive exceptions.
        if let Err(err) = unsafe { StackTrace::dump_with(self.elr, self.sp) } {
            log::error!("StackTrace: {err}");
        }
    }

    fn dump_system_context_registers(&self) {
        log::error!("Exception Registers:");
        log_registers!("ESR", self.esr, "ELR", self.elr, "SPSR", self.spsr, "FAR", self.far,);

        log::error!("");

        log::error!("General-Purpose Registers:");
        log_registers!(
            "x0", self.x0, "x1", self.x1, "x2", self.x2, "x3", self.x3, "x4", self.x4, "x5", self.x5, "x6", self.x6,
            "x7", self.x7, "x8", self.x8, "x9", self.x9, "x10", self.x10, "x11", self.x11, "x12", self.x12, "x13",
            self.x13, "x14", self.x14, "x15", self.x15, "x16", self.x16, "x17", self.x17, "x18", self.x18, "x19",
            self.x19, "x20", self.x20, "x21", self.x21, "x22", self.x22, "x23", self.x23, "x24", self.x24, "x25",
            self.x25, "x26", self.x26, "x27", self.x27, "x28", self.x28, "fp", self.fp, "lr", self.lr, "sp", self.sp
        );

        log::debug!("Full Context: {self:#X?}");
    }
}

#[allow(unused)]
pub fn enable_interrupts() {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        write_sysreg!(reg daifclr, imm 0x02, "isb sy");
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
        write_sysreg!(reg daifset, imm 0x02, "isb sy");
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
