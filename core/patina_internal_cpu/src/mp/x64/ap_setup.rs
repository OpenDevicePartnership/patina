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
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use patina::arch::x64::read_msr;

use super::ApContext;
use crate::gdt::{CODE_SELECTOR, DATA_SELECTOR, DescriptorTablePointer};
use crate::interrupts::x64::idt::{Idt, IdtEntry};

#[unsafe(no_mangle)]
static AP_SETUP: ApSetupHolder = ApSetupHolder(UnsafeCell::new(ApSetup {
    contexts: 0,
    context_count: 0,
    started_count: AtomicU32::new(0),
    idtr: DescriptorTablePointer { limit: 0, base: 0 },
    cr3: 0,
    cr0: 0,
    cr4: 0,
    efer: 0,
}));

#[repr(C)]
struct ApFailure {
    reason: AtomicU32,
    apic_id: AtomicU32,
    detail: AtomicU64,
}

#[unsafe(no_mangle)]
static AP_FAILURE: ApFailure =
    ApFailure { reason: AtomicU32::new(FAILURE_NONE), apic_id: AtomicU32::new(0), detail: AtomicU64::new(0) };

pub(super) const FAILURE_NONE: u32 = 0;
pub(super) const FAILURE_CONTEXT_NOT_FOUND: u32 = 1;
const FAILURE_EXCEPTION: u32 = 2;
const FAILURE_DIVIDE_ERROR: u32 = 3;
const FAILURE_BREAKPOINT: u32 = 4;
const FAILURE_INVALID_OPCODE: u32 = 5;
const FAILURE_DOUBLE_FAULT: u32 = 6;
const FAILURE_GENERAL_PROTECTION: u32 = 7;
const FAILURE_PAGE_FAULT: u32 = 8;
pub(super) const FAILURE_START_REJECTED: u32 = 9;
pub(super) const FAILURE_MTRR_SETUP: u32 = 10;
const FAILURE_ENTRY_RETURNED: u32 = 11;
const FAILURE_RECORDING: u32 = u32::MAX;

/// The AP IDT, used to handle exceptions before terminal parking.
static AP_IDT: spin::LazyLock<Idt> = spin::LazyLock::new(|| {
    let generic = IdtEntry::interrupt_gate(ap_exception_generic as *const () as u64, CODE_SELECTOR, 0);
    let mut idt = Idt::filled(generic);
    *idt.entry_mut(0).expect("divide-error vector must be in range") =
        IdtEntry::interrupt_gate(ap_exception_divide_error as *const () as u64, CODE_SELECTOR, 0);
    *idt.entry_mut(3).expect("breakpoint vector must be in range") =
        IdtEntry::interrupt_gate(ap_exception_breakpoint as *const () as u64, CODE_SELECTOR, 0);
    *idt.entry_mut(6).expect("invalid-opcode vector must be in range") =
        IdtEntry::interrupt_gate(ap_exception_invalid_opcode as *const () as u64, CODE_SELECTOR, 0);
    *idt.entry_mut(8).expect("double-fault vector must be in range") =
        IdtEntry::interrupt_gate(ap_exception_double_fault as *const () as u64, CODE_SELECTOR, 1);
    *idt.entry_mut(13).expect("general-protection vector must be in range") =
        IdtEntry::interrupt_gate(ap_exception_general_protection as *const () as u64, CODE_SELECTOR, 0);
    *idt.entry_mut(14).expect("page-fault vector must be in range") =
        IdtEntry::interrupt_gate(ap_exception_page_fault as *const () as u64, CODE_SELECTOR, 1);
    idt
});

/// State published to the assembly AP entry point before processors are woken.
#[repr(C)]
struct ApSetup {
    contexts: u64,
    context_count: u32,
    started_count: AtomicU32,
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
const AP_CONTEXT_GDTR_OFFSET: usize = core::mem::offset_of!(ApContext, gdtr);
global_asm!(
    include_str!("ap_entry.asm"),
    setup_contexts_off = const core::mem::offset_of!(ApSetup, contexts),
    setup_context_count_off = const core::mem::offset_of!(ApSetup, context_count),
    setup_idtr_off = const core::mem::offset_of!(ApSetup, idtr),
    setup_cr3_off = const core::mem::offset_of!(ApSetup, cr3),
    setup_cr0_off = const core::mem::offset_of!(ApSetup, cr0),
    setup_cr4_off = const core::mem::offset_of!(ApSetup, cr4),
    setup_efer_off = const core::mem::offset_of!(ApSetup, efer),
    failure_context_not_found = const FAILURE_CONTEXT_NOT_FOUND,
    failure_entry_returned = const FAILURE_ENTRY_RETURNED,
    ap_context_size = const AP_CONTEXT_SIZE,
    ap_ctx_stack_off = const AP_CONTEXT_STACK_OFFSET,
    ap_ctx_apic_off = const AP_CONTEXT_APIC_ID_OFFSET,
    ap_ctx_gdtr_off = const AP_CONTEXT_GDTR_OFFSET,
    code64_sel = const CODE_SELECTOR,
    data64_sel = const DATA_SELECTOR,
    tss_selector = const crate::gdt::TSS_SELECTOR,
    ap_entry = sym ap_entry,
);
global_asm!(
    include_str!("ap_exception.asm"),
    failure_exception = const FAILURE_EXCEPTION,
    failure_divide_error = const FAILURE_DIVIDE_ERROR,
    failure_breakpoint = const FAILURE_BREAKPOINT,
    failure_invalid_opcode = const FAILURE_INVALID_OPCODE,
    failure_double_fault = const FAILURE_DOUBLE_FAULT,
    failure_general_protection = const FAILURE_GENERAL_PROTECTION,
    failure_page_fault = const FAILURE_PAGE_FAULT,
    failure_recording = const FAILURE_RECORDING,
    failure_reason_off = const core::mem::offset_of!(ApFailure, reason),
    failure_apic_id_off = const core::mem::offset_of!(ApFailure, apic_id),
    failure_detail_off = const core::mem::offset_of!(ApFailure, detail),
);

unsafe extern "C" {
    fn ap_entry_64();
    fn ap_exception_generic();
    fn ap_exception_divide_error();
    fn ap_exception_breakpoint();
    fn ap_exception_invalid_opcode();
    fn ap_exception_double_fault();
    fn ap_exception_general_protection();
    fn ap_exception_page_fault();
}

unsafe extern "efiapi" {
    pub(super) fn ap_record_failure(reason: u32, detail: u64, apic_id: u32) -> !;
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
        idtr,
        cr3,
        cr0,
        cr4,
        efer,
    };

    AP_FAILURE.detail.store(0, Ordering::Relaxed);
    AP_FAILURE.apic_id.store(0, Ordering::Relaxed);
    AP_FAILURE.reason.store(FAILURE_NONE, Ordering::Release);

    // SAFETY: The BSP initializes the complete setup before publishing the AP
    // entry point or waking any AP. The setup is not replaced after publication.
    unsafe { AP_SETUP.0.get().write(setup) };
}

pub(super) fn started_count() -> u32 {
    // SAFETY: `started_count` is atomic and remains valid for the firmware lifetime.
    unsafe { (*AP_SETUP.0.get()).started_count.load(Ordering::Acquire) }
}

pub(super) fn increment_started_count() {
    // SAFETY: `started_count` remains initialized and atomic for the firmware lifetime.
    unsafe { (*AP_SETUP.0.get()).started_count.fetch_add(1, Ordering::Release) };
}

fn context_count() -> u32 {
    // SAFETY: The setup structure is initialized before any AP can enter Rust.
    unsafe { (*AP_SETUP.0.get()).context_count }
}

pub(super) fn ap_entry_addr() -> usize {
    ap_entry_64 as *const () as usize
}

pub(super) fn decrement_started_count() -> bool {
    // SAFETY: `started_count` remains initialized and atomic for the firmware lifetime.
    let started_count = unsafe { &(*AP_SETUP.0.get()).started_count };
    started_count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_sub(1)).is_ok()
}

pub(super) fn check_for_failures() {
    let reason = loop {
        let reason = AP_FAILURE.reason.load(Ordering::Acquire);
        if reason != FAILURE_RECORDING {
            break reason;
        }
        core::hint::spin_loop();
    };

    if reason != FAILURE_NONE {
        panic_for_failure(
            reason,
            AP_FAILURE.apic_id.load(Ordering::Relaxed),
            AP_FAILURE.detail.load(Ordering::Relaxed),
        );
    }
}

fn panic_for_failure(reason: u32, apic_id: u32, detail: u64) -> ! {
    match reason {
        FAILURE_CONTEXT_NOT_FOUND => {
            panic!("AP {apic_id:#x} could not find its context among {detail} entries")
        }
        FAILURE_EXCEPTION => panic!("AP {apic_id:#x} raised an unrecognized exception"),
        FAILURE_DIVIDE_ERROR => panic!("AP {apic_id:#x} raised a divide error"),
        FAILURE_BREAKPOINT => panic!("AP {apic_id:#x} raised a breakpoint exception"),
        FAILURE_INVALID_OPCODE => panic!("AP {apic_id:#x} raised an invalid-opcode exception"),
        FAILURE_DOUBLE_FAULT => panic!("AP {apic_id:#x} raised a double fault with error code {detail:#x}"),
        FAILURE_GENERAL_PROTECTION => {
            panic!("AP {apic_id:#x} raised a general-protection fault with error code {detail:#x}")
        }
        FAILURE_PAGE_FAULT => panic!("AP {apic_id:#x} raised a page fault at CR2={detail:#x}"),
        FAILURE_START_REJECTED => panic!("AP {apic_id:#x} could not enter its dispatch loop"),
        FAILURE_MTRR_SETUP => panic!("AP {apic_id:#x} could not apply the BSP MTRR state"),
        FAILURE_ENTRY_RETURNED => panic!("AP {apic_id:#x} unexpectedly returned from its entry point"),
        _ => panic!("AP {apic_id:#x} reported unknown failure {reason:#x} with detail {detail:#x}"),
    }
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
/// switched to its per-processor GDT.
extern "efiapi" fn ap_entry(context: *const ApContext) -> ! {
    // SAFETY: `ap_entry_64` selected this pointer from the published context array,
    // which remains allocated for the lifetime of the MP subsystem.
    let Some(context) = (unsafe { context.as_ref() }) else {
        super::MpSupport::fail_ap(FAILURE_CONTEXT_NOT_FOUND, u64::from(context_count()))
    };
    super::MpSupport::apply_mtrrs_or_fail();
    super::MpSupport::ap_run_dispatch_loop(context);
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "AP 0x2a raised a page fault at CR2=0xdeadbeef")]
    fn ap_failure_panics_with_diagnostics() {
        panic_for_failure(FAILURE_PAGE_FAULT, 0x2A, 0xDEAD_BEEF);
    }
}
