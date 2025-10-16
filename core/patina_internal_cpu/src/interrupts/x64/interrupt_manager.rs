//! X64 Interrupt manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::global_asm;
use lazy_static::lazy_static;
use patina::base::SIZE_4GB;
use patina::base::{UEFI_PAGE_MASK, UEFI_PAGE_SIZE};
use patina::{component::service::IntoService, error::EfiError};
use patina_paging::{PageTable, PagingType};
use patina_pi::protocols::cpu_arch::EfiSystemContext;
use patina_stacktrace::StackTrace;
use x86_64::VirtAddr;
use x86_64::structures::idt::InterruptDescriptorTable;
use x86_64::structures::idt::InterruptStackFrame;

use crate::interrupts::{
    EfiExceptionStackTrace, HandlerType, InterruptManager, exception_handling::FaultAllocator, x64::ExceptionContextX64,
};

global_asm!(include_str!("interrupt_handler.asm"));

// Use efiapi for the consistent calling convention.
unsafe extern "efiapi" {
    fn AsmGetVectorAddress(index: usize) -> u64;
}

// The x86_64 crate requires the IDT to be static, which makes sense as the IDT
// can live beyond any code lifetime.
lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // Initialize all of the index-able well-known entries.
        for vector in [0, 1, 2, 3, 4, 5, 6, 7, 9, 16, 19, 20, 28] {
            unsafe { idt[vector].set_handler_addr(get_vector_address(vector.into())) };
        }

        // Intentionally use direct function for double fault. This allows for
        // more robust diagnostics of the exception stack. Currently this also
        // means external caller cannot register for double fault call backs.
        // Fix it: Below line is excluded from std builds because rustc fails to
        //        compile with following error "offset is not a multiple of 16"
        unsafe { idt.double_fault.set_handler_addr(VirtAddr::new(double_fault_handler as *const () as u64)).set_stack_index(0) };

        // Initialize the error code vectors. the x86_64 crate does not allow these
        // to be indexed.
        unsafe {
            idt.invalid_tss.set_handler_addr(get_vector_address(10));
            idt.segment_not_present.set_handler_addr(get_vector_address(11));
            idt.stack_segment_fault.set_handler_addr(get_vector_address(12));
            idt.general_protection_fault.set_handler_addr(get_vector_address(13));
            idt.page_fault.set_handler_addr(get_vector_address(14));
            idt.alignment_check.set_handler_addr(get_vector_address(17));
            idt.cp_protection_exception.set_handler_addr(get_vector_address(19));
            idt.vmm_communication_exception.set_handler_addr(get_vector_address(29));
            idt.security_exception.set_handler_addr(get_vector_address(30));
        }

        // Initialize generic interrupts.
        for vector in 32..=255 {
            unsafe { idt[vector].set_handler_addr(get_vector_address(vector.into())) };
        }

        idt
    };
}

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
        if &IDT as *const _ as usize >= SIZE_4GB {
            // TODO: Come back and ensure the GDT is below 4GB
            panic!("GDT above 4GB, MP services will fail");
        }
        IDT.load();
        log::info!("Loaded IDT");

        // Register some default handlers.
        self.register_exception_handler(13, HandlerType::UefiRoutine(general_protection_fault_handler))
            .expect("Failed to install default exception handler!");
        self.register_exception_handler(14, HandlerType::UefiRoutine(page_fault_handler))
            .expect("Failed to install default exception handler!");

        Ok(())
    }
}

impl InterruptManager for InterruptsX64 {}

/// Handler for double faults.
///
/// Handler for double faults that is configured to run as a direct interrupt
/// handler without using the normal handler assembly or stack. This is done to
/// increase the diagnosability of faults in the interrupt handling code.
///
extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
    panic!("EXCEPTION: DOUBLE FAULT\n{stack_frame:#X?}");
}

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
    // SAFETY: As before, we don't have any choice. The stacktrace module will do its best to not cause a
    // recursive exception.
    if let Err(err) = unsafe { StackTrace::dump_with(x64_context.rip, x64_context.rsp) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: GP FAULT");
}

/// Default handler for page faults.
extern "efiapi" fn page_fault_handler(_exception_type: isize, context: EfiSystemContext) {
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

    dump_pte(x64_context.cr2, x64_context.cr3, paging_type);

    log::error!("Dumping Exception Stack Trace:");
    if let Err(err) = unsafe { StackTrace::dump_with(x64_context.rip, x64_context.rsp) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: PAGE FAULT");
}

/// Gets the address of the assembly entry point for the given vector index.
fn get_vector_address(index: usize) -> VirtAddr {
    // Verify the index is in [0-255]
    if index >= 256 {
        panic!("Invalid vector index! 0x{index:#X?}");
    }

    unsafe { VirtAddr::from_ptr(AsmGetVectorAddress(index) as *const ()) }
}

fn interpret_page_fault_exception_data(exception_data: u64) {
    log::error!("Error Code: {exception_data:#X?}");
    if (exception_data & 0x1) == 0 {
        log::error!("Page not present");
    } else {
        log::error!("Page-level protection violation");
    }

    if (exception_data & 0x2) == 0 {
        log::error!("R/W: Read");
    } else {
        log::error!("R/W: Write");
    }

    if (exception_data & 0x4) == 0 {
        log::error!("Mode: Supervisor");
    } else {
        log::error!("Mode: User");
    }

    if (exception_data & 0x8) == 0 {
        log::error!("Reserved bit violation");
    }

    if (exception_data & 0x10) == 0 {
        log::error!("Instruction fetch access");
    }
}

fn interpret_gp_fault_exception_data(exception_data: u64) {
    log::error!("Error Code: {exception_data:#X?}");
    if (exception_data & 0x1) != 0 {
        log::error!("Invalid segment");
    }

    if (exception_data & 0x2) != 0 {
        log::error!("Invalid write access");
    }

    if (exception_data & 0x4) == 0 {
        log::error!("Mode: Supervisor");
    } else {
        log::error!("Mode: User");
    }
}

fn dump_pte(cr2: u64, cr3: u64, paging_type: PagingType) {
    if let Ok(pt) = unsafe { patina_paging::x64::X64PageTable::from_existing(cr3, FaultAllocator {}, paging_type) } {
        let _ = pt.dump_page_tables(cr2 & !(UEFI_PAGE_MASK as u64), UEFI_PAGE_SIZE as u64);
    }
}
