//! IDT Management for x64
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{arch::global_asm, cell::UnsafeCell};
use patina::base::SIZE_4GB;

global_asm!(include_str!("interrupt_handler.asm"));
// Use efiapi for the consistent calling convention.
unsafe extern "efiapi" {
    fn AsmGetVectorAddress(index: usize) -> u64;
}

/// A single 16-byte gate descriptor in the IDT.
#[derive(Clone, Copy)]
#[repr(C)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn empty() -> Self {
        Self { offset_low: 0, selector: 0, ist: 0, type_attr: 0, offset_mid: 0, offset_high: 0, reserved: 0 }
    }

    /// Configure this entry as a present interrupt gate (DPL 0) pointing to `addr`.
    fn set_handler(&mut self, addr: u64, selector: u16, ist_index: u8) {
        self.offset_low = addr as u16;
        self.offset_mid = (addr >> 16) as u16;
        self.offset_high = (addr >> 32) as u32;
        self.selector = selector;
        self.ist = ist_index & 0x7;
        self.type_attr = 0x8E; // Present | DPL 0 | Interrupt Gate
        self.reserved = 0;
    }
}

/// The 256-entry x86-64 Interrupt Descriptor Table.
#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; 256],
}

struct StaticIdt(UnsafeCell<Idt>);

// SAFETY: IDT initialization and loading is serialized during early CPU init before
// interrupts are enabled and before concurrent access is possible.
unsafe impl Sync for StaticIdt {}

/// Pointer structure passed to the `lidt` instruction.
#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

/// Gets the address of the assembly entry point for the given vector index.
fn get_vector_address(index: usize) -> u64 {
    if index >= 256 {
        panic!("Invalid vector index! 0x{index:#X?}");
    }
    // SAFETY: Index has been validated to be in [0, 255].
    unsafe { AsmGetVectorAddress(index) }
}

static IDT: StaticIdt = StaticIdt(UnsafeCell::new(Idt { entries: [IdtEntry::empty(); 256] }));

pub fn initialize_idt() {
    let cs = crate::cpu::x64::gdt::CODE_SELECTOR;
    // SAFETY: There is only path to access the IDT and it is not possible to have concurrent access.
    let idt = unsafe { &mut *IDT.0.get() };

    // Point every vector at its corresponding assembly handler.
    for vector in 0..=255usize {
        idt.entries[vector].set_handler(get_vector_address(vector), cs, 0);
    }

    // Override double fault (vector 8): use a direct Rust handler and IST 1
    // for more robust diagnostics when the normal interrupt stack is corrupt.
    idt.entries[8].set_handler(double_fault_handler as *const () as u64, cs, 1);

    if IDT.0.get() as usize >= SIZE_4GB {
        panic!("IDT above 4GB, MP services will fail");
    }
    #[cfg(target_os = "uefi")]
    {
        let idtr = Idtr { limit: (core::mem::size_of::<Idt>() - 1) as u16, base: IDT.0.get() as *mut Idt as u64 };
        // SAFETY: Loading our fully initialized IDT.
        unsafe { core::arch::asm!("lidt [{}]", in(reg) &idtr, options(nostack)) };
    }
    log::info!("Loaded IDT");
}

/// Stack frame pushed by the CPU on interrupt/exception entry.
#[repr(C)]
struct InterruptStackFrame {
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

impl core::fmt::Debug for InterruptStackFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InterruptStackFrame")
            .field("rip", &format_args!("{:#X}", self.rip))
            .field("cs", &format_args!("{:#X}", self.cs))
            .field("rflags", &format_args!("{:#X}", self.rflags))
            .field("rsp", &format_args!("{:#X}", self.rsp))
            .field("ss", &format_args!("{:#X}", self.ss))
            .finish()
    }
}

/// Handler for double faults.
///
/// Configured to run as a direct interrupt handler without using the normal
/// handler assembly or stack. This is done to increase the diagnosability of
/// faults in the interrupt handling code.
extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) {
    panic!("EXCEPTION: DOUBLE FAULT\n{stack_frame:#?}");
}
