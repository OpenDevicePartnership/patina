//! IDT Management for x64
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(target_os = "uefi")]
use core::{arch::global_asm, cell::UnsafeCell};
#[cfg(target_os = "uefi")]
use patina::SIZE_4GB;

use crate::gdt::DescriptorTablePointer;

#[cfg(target_os = "uefi")]
global_asm!(include_str!("interrupt_handler.asm"));
// Use efiapi for the consistent calling convention.
#[cfg(target_os = "uefi")]
unsafe extern "efiapi" {
    fn AsmGetVectorAddress(index: usize) -> u64;
}

/// A single 16-byte gate descriptor in the IDT.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    #[cfg(target_os = "uefi")]
    pub(crate) const fn empty() -> Self {
        Self { offset_low: 0, selector: 0, ist: 0, type_attr: 0, offset_mid: 0, offset_high: 0, reserved: 0 }
    }

    /// Creates a present interrupt gate (DPL 0) pointing to `address`.
    pub(crate) const fn interrupt_gate(address: u64, selector: u16, ist_index: u8) -> Self {
        Self {
            offset_low: address as u16,
            selector,
            ist: ist_index & 0x7,
            type_attr: 0x8E,
            offset_mid: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }
}

/// The 256-entry x86-64 Interrupt Descriptor Table.
#[repr(C, align(16))]
pub(crate) struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    pub(crate) const fn filled(entry: IdtEntry) -> Self {
        Self { entries: [entry; 256] }
    }

    pub(crate) fn entry_mut(&mut self, vector: usize) -> Option<&mut IdtEntry> {
        self.entries.get_mut(vector)
    }

    pub(crate) fn descriptor(&self) -> DescriptorTablePointer {
        DescriptorTablePointer {
            limit: (core::mem::size_of::<Self>() - 1) as u16,
            base: core::ptr::from_ref(self) as u64,
        }
    }
}

const _: () = assert!(core::mem::size_of::<IdtEntry>() == 16);
const _: () = assert!(core::mem::size_of::<Idt>() == 4096);

#[cfg(target_os = "uefi")]
struct StaticIdt(UnsafeCell<Idt>);

// SAFETY: IDT initialization and loading is serialized during early CPU init and concurrent
// access is not possible
#[cfg(target_os = "uefi")]
unsafe impl Sync for StaticIdt {}

/// Gets the address of the assembly entry point for the given vector index.
#[cfg(target_os = "uefi")]
fn get_vector_address(index: usize) -> u64 {
    assert!(index < 256, "Invalid vector index! 0x{index:#X?}");
    // SAFETY: Index has been validated to be in [0, 255].
    unsafe { AsmGetVectorAddress(index) }
}

#[cfg(target_os = "uefi")]
static IDT: StaticIdt = StaticIdt(UnsafeCell::new(Idt::filled(IdtEntry::empty())));

#[cfg(target_os = "uefi")]
pub fn initialize_idt() {
    let cs = crate::gdt::CODE_SELECTOR;
    // SAFETY: There is only path to access the IDT and it is not possible to have concurrent access.
    let idt = unsafe { &mut *IDT.0.get() };

    // Point every vector at its corresponding assembly handler.
    for vector in 0..=255usize {
        // Use IST 1 for double fault (vector 8) and page fault (vector 14) to ensure they have a valid stack.
        let ist_index = u8::from(vector == 8 || vector == 14);
        *idt.entry_mut(vector).expect("IDT vector must be in range") =
            IdtEntry::interrupt_gate(get_vector_address(vector), cs, ist_index);
    }

    assert!((IDT.0.get() as usize) < SIZE_4GB, "IDT above 4GB, MP services will fail");
    let idtr = idt.descriptor();
    // SAFETY: Loading our fully initialized IDT.
    unsafe { core::arch::asm!("lidt [{}]", in(reg) core::ptr::addr_of!(idtr), options(nostack)) };
    log::info!("Loaded IDT");
}
