//! PEI-to-DXE AP handoff bridge.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::global_asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

use patina::arch::x64::read_msr;

use super::ApContext;
use crate::gdt::{CODE_SELECTOR, DATA_SELECTOR, DescriptorTablePointer};
use crate::interrupts::x64::idt::{Idt, IdtEntry};

#[unsafe(no_mangle)]
static AP_SETUP: ApSetupHolder = ApSetupHolder(UnsafeCell::new(ApSetup {
    contexts: 0,
    context_count: 0,
    started_count: AtomicU32::new(0),
    gdtr: DescriptorTablePointer { limit: 0, base: 0 },
    idtr: DescriptorTablePointer { limit: 0, base: 0 },
    cr3: 0,
    cr0: 0,
    cr4: 0,
    efer: 0,
}));

/// The AP park IDT, used to handle exceptions before terminal parking.
static AP_IDT: spin::LazyLock<Idt> = spin::LazyLock::new(|| {
    let park = IdtEntry::interrupt_gate(super::park::ap_park as *const () as u64, CODE_SELECTOR, 0);
    let mut idt = Idt::filled(park);
    *idt.entry_mut(2).expect("NMI vector must be in range") =
        IdtEntry::interrupt_gate(ap_nmi_abort as *const () as u64, CODE_SELECTOR, 0);
    idt
});

/// State published to the assembly AP entry point before processors are woken.
#[repr(C)]
struct ApSetup {
    contexts: u64,
    context_count: u32,
    started_count: AtomicU32,
    gdtr: DescriptorTablePointer,
    idtr: DescriptorTablePointer,
    cr3: u64,
    cr0: u64,
    cr4: u64,
    efer: u64,
}

#[repr(transparent)]
struct ApSetupHolder(UnsafeCell<ApSetup>);
// SAFETY: The BSP writes the complete setup once before waking any AP. After
// publication, only `started_count` is mutated, using atomic operations.
unsafe impl Sync for ApSetupHolder {}

const AP_CONTEXT_SIZE: usize = core::mem::size_of::<ApContext>();
const AP_CONTEXT_STACK_OFFSET: usize = core::mem::offset_of!(ApContext, stack_top);
const AP_CONTEXT_APIC_ID_OFFSET: usize = core::mem::offset_of!(ApContext, apic_id);
const RFLAGS_CLEAR_TF_IF_MASK: i32 = !((1 << 8) | (1 << 9));

global_asm!(
    include_str!("ap_entry.asm"),
    setup_contexts_off = const core::mem::offset_of!(ApSetup, contexts),
    setup_context_count_off = const core::mem::offset_of!(ApSetup, context_count),
    setup_started_count_off = const core::mem::offset_of!(ApSetup, started_count),
    setup_gdtr_off = const core::mem::offset_of!(ApSetup, gdtr),
    setup_idtr_off = const core::mem::offset_of!(ApSetup, idtr),
    setup_cr3_off = const core::mem::offset_of!(ApSetup, cr3),
    setup_cr0_off = const core::mem::offset_of!(ApSetup, cr0),
    setup_cr4_off = const core::mem::offset_of!(ApSetup, cr4),
    setup_efer_off = const core::mem::offset_of!(ApSetup, efer),
    ap_context_size = const AP_CONTEXT_SIZE,
    ap_ctx_stack_off = const AP_CONTEXT_STACK_OFFSET,
    ap_ctx_apic_off = const AP_CONTEXT_APIC_ID_OFFSET,
    rflags_clear_tf_if_mask = const RFLAGS_CLEAR_TF_IF_MASK,
    code64_sel = const CODE_SELECTOR,
    data64_sel = const DATA_SELECTOR,
    ap_entry = sym ap_entry,
);

unsafe extern "C" {
    fn ap_entry_64();
    fn ap_nmi_abort();
}

pub(super) fn setup(aps: &'static [ApContext]) {
    const IA32_EFER: u32 = 0xC000_0080;
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;

    spin::LazyLock::force(&AP_IDT);
    let idtr = AP_IDT.descriptor();

    // SAFETY: Reading control registers has no side effects.
    unsafe {
        core::arch::asm!(
            "mov {0}, cr0",
            "mov {1}, cr3",
            "mov {2}, cr4",
            out(reg) cr0,
            out(reg) cr3,
            out(reg) cr4,
            options(nomem, nostack, preserves_flags),
        );
    }

    // SAFETY: Reading IA32_EFER has no side effects.
    let efer = unsafe { read_msr(IA32_EFER) };
    let setup = ApSetup {
        contexts: aps.as_ptr() as u64,
        context_count: aps.len() as u32,
        started_count: AtomicU32::new(0),
        gdtr: crate::gdt::descriptor(),
        idtr,
        cr3,
        cr0,
        cr4,
        efer,
    };

    // SAFETY: The BSP initializes the complete setup before publishing the AP
    // entry point or waking any AP. The setup is not replaced after publication.
    unsafe { AP_SETUP.0.get().write(setup) };
}

pub(super) fn started_count() -> u32 {
    // SAFETY: `started_count` is atomic and remains valid for the firmware lifetime.
    unsafe { (*AP_SETUP.0.get()).started_count.load(Ordering::Acquire) }
}

fn ap_entry_addr() -> usize {
    ap_entry_64 as *const () as usize
}

/// Wakes the AP who is monitoring `signal_address` by writing the
/// AP entry point into `procedure_address` and then raising the signal.
///
/// # Safety
///
/// `procedure_address` and `signal_address` must be the handoff slots
/// for an AP parked in a wait loop.
pub(super) unsafe fn wake_ap(procedure_address: u64, signal_address: u64, signal_value: u32) {
    let entry = ap_entry_addr() as u64;
    // SAFETY: the caller guarantees these are the handoff slots are correctly set
    // up for the AP.
    unsafe {
        core::ptr::write_volatile(procedure_address as *mut u64, entry);
        core::sync::atomic::fence(Ordering::Release);
        core::ptr::write_volatile(signal_address as *mut u32, signal_value);
    }
}

/// AP entry point. Called by each AP after it has configured its stack and
/// switched to the BSP's GDT.
extern "efiapi" fn ap_entry(context: *const ApContext) {
    // SAFETY: `ap_entry_64` selected this pointer from the published context array,
    // which remains allocated for the lifetime of the MP subsystem.
    let Some(context) = (unsafe { context.as_ref() }) else {
        // SAFETY: The AP park routine disables interrupts and does not return.
        unsafe { super::park::ap_park() }
    };
    super::MpSupport::ap_run_dispatch_loop(context);
}
