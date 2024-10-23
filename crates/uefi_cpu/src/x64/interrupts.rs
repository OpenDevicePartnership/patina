//! x86_86 Interrupt initialization
//! 
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use super::gdt;
use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

//pub const PIC_1_OFFSET: u8 = 32;
//pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

// #[derive(Debug, Clone, Copy)]
// #[repr(u8)]
// pub enum InterruptIndex {
//     Timer = PIC_1_OFFSET,
//     Keyboard,
// }

lazy_static! {
  static ref IDT: InterruptDescriptorTable = {
    let mut idt = InterruptDescriptorTable::new();
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.debug.set_handler_fn(debug_int_handler);
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    unsafe {
      idt.double_fault.set_handler_fn(double_fault_handler).set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt.invalid_tss.set_handler_fn(invalid_tss_handler);
    idt.segment_not_present.set_handler_fn(segment_not_present_handler);
    idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);
    idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt
  };
}

pub fn init_idt() {
  IDT.load();
  log::info!("Loaded IDT");
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
  panic!("EXCEPTION: Divide Error\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn debug_int_handler(stack_frame: InterruptStackFrame) {
  panic!("EXCEPTION: DEBUG INT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
  log::info!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
  panic!("EXCEPTION: INVALID OPCODE\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
  panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
  panic!("EXCEPTION: INVALID TSS\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn segment_not_present_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
  panic!("EXCEPTION: SEGMENT NOT PRESENT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn stack_segment_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
  panic!("EXCEPTION: STACK SEGMENT FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn general_protection_fault_handler(stack_frame: InterruptStackFrame, error_code: u64) {
  panic!("EXCEPTION: GP FAULT\nStackFrame:\n{:#?}\nErrorCode:\n{:#?}", stack_frame, error_code);
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
  use x86_64::registers::control::Cr2;

  log::info!("EXCEPTION: PAGE FAULT");
  log::info!("Accessed Address: {:?}", Cr2::read());
  log::info!("Error Code: {:?}", error_code);
  log::info!("{:#?}", stack_frame);
  loop {
    x86_64::instructions::hlt();
  }
}
