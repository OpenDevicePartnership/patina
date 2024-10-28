//! x86_86 Interrupt initialization
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use crate::efi_system_context::EfiSystemContext;
use crate::InterruptManager;
use core::arch::global_asm;
use lazy_static::lazy_static;
use uefi_core::error::EfiError;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
use x86_64::VirtAddr;

global_asm!(include_str!("x64/interrupt_handler.asm"));

// Use efiapi for the consistent calling convention.
extern "efiapi" {
    fn AsmGetVectorAddress(index: usize) -> u64;
}

// The x86_64 crate requires the IDT to be static, which makes sense as the IDT
// can live beyond any code lifetime.
lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // Initialize all of the index-able well-known entries.
        for vector in [0, 1, 2, 3, 4, 5, 6, 7, 9, 16, 19, 20, 28] {
            unsafe { idt[vector].set_handler_addr(get_vector_address(vector)) };
        }

        // Intentionally use direct function for double fault. This allows for
        // more robust diagnostics of the exception stack. Currently this also
        // means external caller cannot register for double fault call backs.
        unsafe { idt.double_fault.set_handler_fn(double_fault_handler).set_stack_index(0) };

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
        for vector in 32..256 {
            unsafe { idt[vector].set_handler_addr(get_vector_address(vector)) };
        }

        idt
    };
}

/// X64 Implementation of the InterruptManager.
///
/// An x64 version of the InterruptManager for managing IDT based interrupts.
///
#[derive(Default, Copy, Clone)]
pub struct InterruptManagerX64 {}

impl InterruptManagerX64 {
    pub const fn new() -> Self {
        Self {}
    }
}

impl InterruptManager for InterruptManagerX64 {
    fn initialize(&mut self) -> Result<(), EfiError> {
        IDT.load();
        log::info!("Loaded IDT");

        // Register some default handlers.
        self.register_exception_handler(13, general_protection_fault_handler)
            .expect("Failed to install default exception handler!");
        self.register_exception_handler(14, page_fault_handler).expect("Failed to install default exception handler!");

        Ok(())
    }
}

/// Handler for double faults.
///
/// Handler for doubel faults that is configured to run as a direct interrupt
/// handler without using the normal handler assembly or stack. This is done to
/// increase the diagnosability of faults in the interrupt handling code.
///
extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#x?}", stack_frame);
}

/// Default handler for GP faults.
extern "efiapi" fn general_protection_fault_handler(_exception_type: u64, context: EfiSystemContext) {
    panic!("EXCEPTION: GP FAULT\n{:#x?}", context.get_arch_context());
}

/// Default handler for page faults.
extern "efiapi" fn page_fault_handler(_exception_type: u64, context: EfiSystemContext) {
    let x64_context = context.get_arch_context();

    log::error!("EXCEPTION: PAGE FAULT");
    log::error!("Accessed Address: 0x{:x?}", x64_context.cr2);
    log::error!("Error Code: 0x{:x?}", x64_context.exception_data);
    log::error!("{:#x?}", x64_context);
    panic!("EXCEPTION: PAGE FAULT");
}

/// Gets the address of the assembly entry point for the given vector index.
fn get_vector_address(index: usize) -> VirtAddr {
    // Verify the index is in [0-255]
    if index >= 256 {
        panic!("Invalid vector index! 0x{:x}", index);
    }

    unsafe { VirtAddr::from_ptr(AsmGetVectorAddress(index) as *const ()) }
}
