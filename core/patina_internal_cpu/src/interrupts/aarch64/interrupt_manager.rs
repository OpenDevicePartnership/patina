//! AARCH64 Interrupt manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::global_asm;
use patina::{
    base::{UEFI_PAGE_MASK, UEFI_PAGE_SIZE},
    bit,
    component::service::IntoService,
    error::EfiError,
};
use patina_paging::{PageTable, PagingType};
use patina_stacktrace::StackTrace;

#[allow(unused_imports)]
use crate::interrupts::aarch64::sysreg::{read_sysreg, write_sysreg};
use crate::interrupts::{
    EfiExceptionStackTrace, EfiSystemContext, HandlerType, InterruptManager, aarch64::ExceptionContextAArch64,
};
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
        initialize_exception()?;

        self.register_exception_handler(0, HandlerType::UefiRoutine(synchronous_exception_handler))
            .expect("Failed to install default exception handler!");

        Ok(())
    }
}

impl InterruptManager for InterruptsAarch64 {}

fn enable_fiq() {
    write_sysreg!(daifclr, 0x01, "isb sy");
}

fn disable_fiq() {
    write_sysreg!(daifset, 0x01, "isb sy");
}

fn get_fiq_state() -> Result<bool, EfiError> {
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        let daif = read_sysreg!(daif);
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
        write_sysreg!(daifclr, 0x04, "isb sy");
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        unimplemented!()
    }
}

fn initialize_exception() -> Result<(), EfiError> {
    // Set the stack pointer for EL0 to be used for synchronous exceptions
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        // SAFETY: We are using the address of a symbol defined in assembly as the stack pointer for EL0.
        let mut sp_el0_reg = unsafe { &sp_el0_end as *const _ as u64 };
        sp_el0_reg &= !0x0F;
        write_sysreg!(sp_el0, sp_el0_reg);

        let mut hcr = read_sysreg!(hcr_el2) as u64;
        hcr = hcr as u64 | 1 << 27; // Enable TGE
        write_sysreg!(hcr_el2, hcr);
    }

    // Program VBar
    #[cfg(all(not(test), target_arch = "aarch64"))]
    {
        // SAFETY: We are using the address of the exception handlers as the vector base address.
        let vec_base = unsafe { &exception_handlers_start as *const _ as u64 };
        let current_el = get_current_el();
        match current_el {
            0xC => write_sysreg!(vbar_el1, vec_base, "isb sy"),
            0x08 => write_sysreg!(vbar_el2, vec_base, "isb sy"),
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

/// Default handler for synchronous exceptions.
extern "efiapi" fn synchronous_exception_handler(_exception_type: isize, context: EfiSystemContext) {
    // SAFETY: We don't have any choice here, we are in an exception and have to do our best
    // to report. The system is dead anyway.
    let aarch64_context = unsafe { context.system_context_aarch64.as_ref().unwrap() };

    log::error!("");
    log::error!("EXCEPTION: Synchronous Exception");

    log::error!("");

    // determine if this was a page fault
    let ec = (aarch64_context.esr >> 26) & 0x3F;
    let iss = aarch64_context.esr & 0xFFFFFF;
    let page_fault = ec == 0x20 || ec == 0x21 || ec == 0x24 || ec == 0x25;
    if ec == 0x20 || ec == 0x21 {
        // Instruction Abort from a lower EL or same EL
        log::error!("Page Fault (Instruction Abort)");
    } else if ec == 0x24 || ec == 0x25 {
        // Data Abort from a lower EL or same EL
        log::error!("Page Fault (Data Abort)");
    }

    log::error!("");

    (aarch64_context as &ExceptionContextAArch64).dump_system_context_registers();

    log::error!("");

    if page_fault {
        // make sure the FAR is valid before we dump the page table
        if iss & bit!(10) == 0 {
            dump_pte(aarch64_context.far);
        } else {
            log::error!("FAR not valid, not dumping PTE");
        }
    }

    log::debug!("Full Context: {aarch64_context:#X?}");

    log::error!("Dumping Exception Stack Trace:");
    // SAFETY: As before, we don't have any choice. The stacktrace module will do its best to not cause a
    // recursive exception.
    if let Err(err) = unsafe { StackTrace::dump_with(aarch64_context.elr, aarch64_context.sp) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: Synchronous Exception");
}

fn dump_pte(far: u64) {
    let ttbr0_el2 = read_sysreg!(ttbr0_el2);

    // SAFETY: TTBR0 must be valid as it is the current page table base.
    if let Ok(pt) = unsafe {
        patina_paging::aarch64::AArch64PageTable::from_existing(
            ttbr0_el2,
            patina_paging::page_allocator::PageAllocatorStub,
            PagingType::Paging4Level,
        )
    } {
        let _ = pt.dump_page_tables(far & !(UEFI_PAGE_MASK as u64), UEFI_PAGE_SIZE as u64);
        log::error!("");
    }
}
