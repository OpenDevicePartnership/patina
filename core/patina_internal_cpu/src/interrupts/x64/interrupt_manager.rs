//! X64 Interrupt manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    base::{UEFI_PAGE_MASK, UEFI_PAGE_SIZE},
    bit,
    component::service::IntoService,
    error::EfiError,
    pi::protocols::cpu_arch::EfiSystemContext,
};
#[cfg(target_arch = "x86_64")]
use patina_mtrr::Mtrr;
use patina_paging::{PageTable, PagingType};
use patina_stacktrace::{StackFrame, StackTrace};

use crate::interrupts::{EfiExceptionStackTrace, HandlerType, InterruptManager, x64::ExceptionContextX64};

/// X64 Implementation of the InterruptManager.
///
/// An x64 version of the InterruptManager for managing IDT based interrupts.
///
#[derive(Default, Copy, Clone, IntoService)]
#[service(dyn InterruptManager)]
pub struct InterruptsX64 {}

impl InterruptsX64 {
    /// Creates a new instance of the x64 implementation of the InterruptManager.
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
        // Initialize the IDT.
        #[cfg(target_os = "uefi")]
        crate::interrupts::x64::idt::initialize_idt();

        // Register some default handlers.
        self.register_exception_handler(13, HandlerType::UefiRoutine(general_protection_fault_handler))
            .expect("Failed to install default exception handler!");
        self.register_exception_handler(14, HandlerType::UefiRoutine(page_fault_handler))
            .expect("Failed to install default exception handler!");

        Ok(())
    }
}

impl InterruptManager for InterruptsX64 {}

#[coverage(off)]
/// Default handler for GP faults.
extern "efiapi" fn general_protection_fault_handler(_exception_type: isize, context: EfiSystemContext) {
    // SAFETY: We don't have any choice here, we are in an exception and have to do our best
    // to report. The system is dead anyway.
    let x64_context = unsafe { context.system_context_x64.as_ref().unwrap() };
    log::error!("EXCEPTION: GP FAULT");
    log::error!("Instruction Pointer: {:#X?}", x64_context.rip);
    log::error!("Code Segment: {:#X?}", x64_context.cs);
    log::error!("RFLAGS: {:#X?}", x64_context.rflags);
    log::error!("Stack Segment: {:#X?}", x64_context.ss);
    log::error!("Stack Pointer: {:#X?}", x64_context.rsp);
    log::error!("Data Segment: {:#X?}", x64_context.ds);
    log::error!("Paging Enable: {}", x64_context.cr0 & 0x80000000 != 0);
    log::error!("Protection Enable: {}", x64_context.cr0 & 0x00000001 != 0);
    log::error!("Page Directory Base: {:#X?}", x64_context.cr3);
    log::error!("Control Flags (cr4): {:#X?}", x64_context.cr4);
    interpret_gp_fault_exception_data(x64_context.exception_data);

    log::error!("");

    (x64_context as &ExceptionContextX64).dump_system_context_registers();

    log::error!("Dumping Exception Stack Trace:");
    let stack_frame = StackFrame { pc: x64_context.rip, sp: x64_context.rsp, fp: x64_context.rbp };
    // SAFETY: Called during exception handling with CPU context registers. The exception context
    // is considered valid to dump at this time.
    if let Err(err) = unsafe { StackTrace::dump_with(stack_frame) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: GP FAULT");
}

#[coverage(off)]
/// Default handler for page faults.
extern "efiapi" fn page_fault_handler(_exception_type: isize, context: EfiSystemContext) {
    // SAFETY: We don't have any choice here, we are in an exception and have to do our best
    // to report. The system is dead anyway.
    let x64_context = unsafe { context.system_context_x64.as_ref().unwrap() };

    log::error!("EXCEPTION: PAGE FAULT");
    log::error!("Accessed Address: {:#X?}", x64_context.cr2);
    log::error!("Paging Enabled: {}", x64_context.cr0 & 0x80000000 != 0);
    log::error!("Instruction Pointer: {:#X?}", x64_context.rip);
    log::error!("Code Segment: {:#X?}", x64_context.cs);
    log::error!("RFLAGS: {:#X?}", x64_context.rflags);
    log::error!("Stack Segment: {:#X?}", x64_context.ss);
    log::error!("Data Segment: {:#X?}", x64_context.ds);
    log::error!("Stack Pointer: {:#X?}", x64_context.rsp);
    log::error!("Page Directory Base: {:#X?}", x64_context.cr3);
    log::error!("Paging Features (cr4): {:#X?}", x64_context.cr4);
    interpret_page_fault_exception_data(x64_context.exception_data);

    log::error!("");

    (x64_context as &ExceptionContextX64).dump_system_context_registers();

    let paging_type =
        { if x64_context.cr4 & (1 << 12) != 0 { PagingType::Paging5Level } else { PagingType::Paging4Level } };

    // SAFETY: CR3 and the paging type are correct as they are from the current context.
    unsafe { dump_pte(x64_context.cr2, x64_context.cr3, paging_type) };

    log::error!("Dumping Exception Stack Trace:");
    let stack_frame = StackFrame { pc: x64_context.rip, sp: x64_context.rsp, fp: x64_context.rbp };
    // SAFETY: Called during page fault exception handling with CPU context registers. The exception context
    // is considered valid to dump at this time.
    if let Err(err) = unsafe { StackTrace::dump_with(stack_frame) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: PAGE FAULT");
}

#[coverage(off)]
// see Intel SDM Vol 3A section 7.15
fn interpret_page_fault_exception_data(exception_data: u64) {
    log::error!("Error Code: {exception_data:#X?}");
    if (exception_data & bit!(0)) == 0 {
        log::error!("Page not present");
    } else {
        log::error!("Page-level protection violation");
    }

    if (exception_data & bit!(1)) == 0 {
        log::error!("R/W: Read");
    } else {
        log::error!("R/W: Write");
    }

    if (exception_data & bit!(3)) != 0 {
        log::error!("Reserved bit violation");
    }

    if (exception_data & bit!(4)) == 0 {
        log::error!("Data access");
    } else {
        log::error!("Instruction fetch access");
    }
}

#[coverage(off)]
// see Intel SDM Vol 3A section 7.15
fn interpret_gp_fault_exception_data(exception_data: u64) {
    if exception_data != 0 {
        log::error!("Segment descriptor or IDT vector number: {exception_data:#X?}");
    } else {
        log::error!("Not from loading a segment descriptor");
    }
}

// There is no value in coverage for this function.
#[coverage(off)]
/// Dumps the page table entries for the given CR2 and CR3 values.
///
/// ## Safety
///
/// The caller is responsible for ensuring that the CR3 value is a valid and well-formed page table base address and
/// matches the paging type requested.
unsafe fn dump_pte(cr2: u64, cr3: u64, paging_type: PagingType) {
    // SAFETY: Caller must ensure cr3 & paging type are correct.
    if let Ok(pt) = unsafe {
        patina_paging::x64::X64PageTable::from_existing(
            cr3,
            patina_paging::page_allocator::PageAllocatorStub,
            paging_type,
        )
    } {
        let _ = pt.dump_page_tables(cr2 & !(UEFI_PAGE_MASK as u64), UEFI_PAGE_SIZE as u64);
    }

    // we don't carry the caching attributes in the page table, so get them from the MTRRs
    #[cfg(target_arch = "x86_64")]
    {
        let mtrr = patina_mtrr::create_mtrr_lib(0);
        log::error!("");
        log::error!("MTRR Cache Attribute: {}", mtrr.get_memory_attribute(cr2));
        log::error!("");
    }
}

#[coverage(off)]
#[cfg(test)]
mod test {
    extern crate std;

    use serial_test::serial;

    use super::*;

    #[test]
    #[serial(exception_handlers)]
    fn test_interrupts_x64() {
        let mut interrupts = InterruptsX64::new();
        assert!(interrupts.initialize().is_ok());
        assert!(interrupts.unregister_exception_handler(13).is_ok());
        assert!(interrupts.unregister_exception_handler(14).is_ok());
    }
}
