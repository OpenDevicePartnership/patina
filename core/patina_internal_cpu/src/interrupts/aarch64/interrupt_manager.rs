//! AARCH64 Interrupt manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::{asm, global_asm};
use patina_sdk::{component::service::IntoService, error::EfiError};

use crate::interrupts::InterruptManager;
#[cfg(all(not(test), target_arch = "aarch64"))]
use crate::interrupts::aarch64::sysreg::{read_sysreg, write_sysreg};
use crate::interrupts::{disable_interrupts, enable_interrupts};

#[cfg(all(not(test), target_arch = "aarch64"))]
use crate::interrupts::aarch64::gic_manager::get_current_el;

global_asm!(include_str!("exception_handler.asm"));

#[cfg(all(not(test), target_arch = "aarch64"))]
// extern "efiapi" fn AsmGetVectorAddress(index: u64);
unsafe extern "C" {
    static exception_handlers_start: u64;
    static sp_el0_end: u64;
}

/// AARCH64 Implementation of the InterruptManager.
#[derive(Default, Copy, Clone, IntoService)]
#[service(dyn InterruptManager)]
pub struct InterruptsAarch64 {}

impl InterruptsAarch64 {
    /// Creates a new instance of the AARCH64 implementation of the InterruptManager.
    pub const fn new() -> Self {
        Self {}
    }

    /// Initializes the hardware and software structures for interrupts and exceptions.
    ///
    /// This routine will initialize the architecture and platforms specific mechanisms
    /// for interrupts and exceptions to be taken. This routine may install some
    /// architecture specific default handlers for exceptions.
    ///
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        // Initialize exception entrypoint
        initialize_exception()
    }
}

impl InterruptManager for InterruptsAarch64 {}

fn enable_fiq() {
    unsafe {
        asm!("msr   daifclr, 0x01", "isb sy", options(nostack));
    }
}

fn disable_fiq() {
    unsafe {
        asm!("msr   daifset, 0x01", "isb sy", options(nostack));
    }
}

fn get_fiq_state() -> Result<bool, EfiError> {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        let daif = unsafe { read_sysreg!(daif) };
        Ok(daif & 0x40 == 0)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        Err(EfiError::Unsupported)
    }
}

fn enable_async_abort() {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        unsafe {
            asm!("msr   daifclr, 0x04", "isb sy", options(nostack));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        unimplemented!()
    }
}

fn initialize_exception() -> Result<(), EfiError> {
    // Set the stack pointer for EL0 to be used for synchronous exceptions
    #[cfg(all(not(test), target_arch = "aarch64"))]
    unsafe {
        let mut sp_el0_reg = &sp_el0_end as *const _ as u64;
        sp_el0_reg &= !0x0F;
        asm!("msr sp_el0, {}", in(reg) sp_el0_reg, options(nostack));

        let mut hcr = read_sysreg!(hcr_el2) as u64;
        hcr = hcr as u64 | 1 << 27; // Enable TGE
        write_sysreg!(hcr_el2, hcr);
    }

    // Program VBar
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        let vec_base = unsafe { &exception_handlers_start as *const _ as u64 };
        let current_el = get_current_el();
        match current_el {
            0xC => unsafe { write_sysreg!(vbar_el1, vec_base, "isb sy") },
            0x08 => unsafe { write_sysreg!(vbar_el2, vec_base, "isb sy") },
            0x04 => unsafe { write_sysreg!(vbar_el3, vec_base, "isb sy") },
            _ => panic!("Invalid current EL {}", current_el),
        };
    }

    let fiq = get_fiq_state();

    disable_interrupts();
    disable_fiq();

    if fiq.is_ok_and(|fiq_b| fiq_b) {
        enable_fiq();
    }

    // We will always enable interrupt when initializing the exception manager.
    enable_interrupts();
    enable_async_abort();

    Ok(())
}
