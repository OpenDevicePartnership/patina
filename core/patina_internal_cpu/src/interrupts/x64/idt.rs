//! IDT Management for x64
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
use x86_64::{
    VirtAddr,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
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
            // SAFETY: We are constructing the well known IDT handlers which must be present in the IDT
            unsafe { idt[vector].set_handler_addr(get_vector_address(vector.into())) };
        }

        // Intentionally use direct function for double fault. This allows for
        // more robust diagnostics of the exception stack. Currently this also
        // means external caller cannot register for double fault call backs.
        // Fix it: Below line is excluded from std builds because rustc fails to
        //        compile with following error "offset is not a multiple of 16"
        // SAFETY: We are adding a double fault handler to our static IDT that already exists
        unsafe { idt.double_fault.set_handler_addr(VirtAddr::new(double_fault_handler as *const () as u64)).set_stack_index(0); }

        // Initialize the error code vectors. the x86_64 crate does not allow these
        // to be indexed.
        // SAFETY: We are using a static IDT and configuring all exception handlers
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
            // SAFETY: We are using a static IDT and configuring the expected list of generic interrupts
            unsafe { idt[vector].set_handler_addr(get_vector_address(vector.into())) };
        }

        idt
    };
}

/// Gets the address of the assembly entry point for the given vector index.
fn get_vector_address(index: usize) -> VirtAddr {
    // Verify the index is in [0-255]
    if index >= 256 {
        panic!("Invalid vector index! 0x{index:#X?}");
    }

    // SAFETY: We have validated we are using the architecturally guaranteed indices
    unsafe { VirtAddr::from_ptr(AsmGetVectorAddress(index) as *const ()) }
}

pub fn initialize_idt() {
    if &IDT as *const _ as usize >= SIZE_4GB {
        // TODO: Come back and ensure the IDT is below 4GB
        panic!("IDT above 4GB, MP services will fail");
    }
    #[cfg(target_os = "uefi")]
    IDT.load();
    log::info!("Loaded IDT");
}

/// Handler for double faults.
///
/// Handler for double faults that is configured to run as a direct interrupt
/// handler without using the normal handler assembly or stack. This is done to
/// increase the diagnosability of faults in the interrupt handling code.
///
extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
    panic!("EXCEPTION: DOUBLE FAULT\n{stack_frame:#X?}");
}
