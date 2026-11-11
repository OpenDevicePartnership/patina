//! x64-specific architectural helpers for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::{SystemContextDiagnostics, system_context::log_registers};
use crate::{
    UEFI_PAGE_MASK, UEFI_PAGE_SIZE, bit, error::EfiError, pi::protocol::cpu_arch::CpuFlushType, standard::efi,
};
use core::arch::asm;
use core::num::NonZeroU64;

mod tdx;

impl SystemContextDiagnostics for efi::protocols::debug_support::SystemContextX64 {
    fn dump_exception(&self, exception_type: usize) {
        match exception_type {
            8 => {
                log::error!("EXCEPTION: DOUBLE FAULT");
                log::error!("Instruction Pointer: {:#X?}", self.rip);
                log::error!("Code Segment: {:#X?}", self.cs);
                log::error!("RFLAGS: {:#X?}", self.rflags);
                log::error!("Stack Segment: {:#X?}", self.ss);
                log::error!("Stack Pointer: {:#X?}", self.rsp);
                log::error!("Data Segment: {:#X?}", self.ds);
                log::error!("Paging Enable: {}", self.cr0 & 0x80000000 != 0);
                log::error!("Protection Enable: {}", self.cr0 & 0x00000001 != 0);
                log::error!("Page Directory Base: {:#X?}", self.cr3);
                log::error!("Control Flags (cr4): {:#X?}", self.cr4);
                log::error!("");
                self.dump_system_context_registers();
            }
            13 => {
                log::error!("EXCEPTION: GP FAULT");
                log::error!("Instruction Pointer: {:#X?}", self.rip);
                log::error!("Code Segment: {:#X?}", self.cs);
                log::error!("RFLAGS: {:#X?}", self.rflags);
                log::error!("Stack Segment: {:#X?}", self.ss);
                log::error!("Stack Pointer: {:#X?}", self.rsp);
                log::error!("Data Segment: {:#X?}", self.ds);
                log::error!("Paging Enable: {}", self.cr0 & 0x80000000 != 0);
                log::error!("Protection Enable: {}", self.cr0 & 0x00000001 != 0);
                log::error!("Page Directory Base: {:#X?}", self.cr3);
                log::error!("Control Flags (cr4): {:#X?}", self.cr4);
                interpret_gp_fault_exception_data(self.exception_data);
                log::error!("");
                self.dump_system_context_registers();
            }
            14 => {
                let rsp_page = self.rsp & !UEFI_PAGE_MASK as u64;
                let cr2_page = self.cr2 & !UEFI_PAGE_MASK as u64;
                let same_or_below =
                    (cr2_page == rsp_page) || (cr2_page == (rsp_page.wrapping_sub(UEFI_PAGE_SIZE as u64)));
                let data_access = (self.exception_data & bit!(4)) == 0;

                // Stack overflows cause page faults, let's give a hint if it looks like one.
                // The heuristic we are using is that stack overflow will either have CR2 on the same page
                // as RSP or directly below it. Sometimes RSP is decremented first, sometimes second. Stack overflows
                // are always data access, so ensure that is the case as well.
                if same_or_below && data_access {
                    log::error!("EXCEPTION: PAGE FAULT STACK OVERFLOW");
                } else {
                    log::error!("EXCEPTION: PAGE FAULT");
                }
                log::error!("Accessed Address: {:#X?}", self.cr2);
                log::error!("Paging Enabled: {}", self.cr0 & 0x80000000 != 0);
                log::error!("Instruction Pointer: {:#X?}", self.rip);
                log::error!("Code Segment: {:#X?}", self.cs);
                log::error!("RFLAGS: {:#X?}", self.rflags);
                log::error!("Stack Segment: {:#X?}", self.ss);
                log::error!("Data Segment: {:#X?}", self.ds);
                log::error!("Stack Pointer: {:#X?}", self.rsp);
                log::error!("Page Directory Base: {:#X?}", self.cr3);
                log::error!("Paging Features (cr4): {:#X?}", self.cr4);
                interpret_page_fault_exception_data(self.exception_data);
                log::error!("");
                self.dump_system_context_registers();
            }
            _ => {
                log::error!("EXCEPTION: {exception_type}");
                self.dump_system_context_registers();
            }
        }
    }

    fn dump_system_context_registers(&self) {
        log::error!("Control Registers:");
        log_registers!(
            "CR0",
            self.cr0,
            "CR2",
            self.cr2,
            "CR3",
            self.cr3,
            "CR4",
            self.cr4,
            "RIP",
            self.rip,
            "CS",
            self.cs,
            "SS",
            self.ss,
            "DS",
            self.ds,
            "RSP",
            self.rsp,
            "RFLAGS",
            self.rflags
        );
        log::error!("");
        log::error!("General-Purpose Registers:");
        log_registers!(
            "RAX", self.rax, "RBX", self.rbx, "RCX", self.rcx, "RDX", self.rdx, "RSI", self.rsi, "RDI", self.rdi,
            "RBP", self.rbp, "R8", self.r8, "R9", self.r9, "R10", self.r10, "R11", self.r11, "R12", self.r12, "R13",
            self.r13, "R14", self.r14, "R15", self.r15
        );
        log::debug!("Full Context: {self:#X?}");
    }

    fn page_fault_address(&self, exception_type: usize) -> Option<u64> {
        (exception_type == 14).then_some(self.cr2)
    }
}

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

    if (exception_data & bit!(2)) != 0 {
        log::error!("User-mode access violation");
    } else {
        log::error!("Supervisor-mode access violation");
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

// see Intel SDM Vol 3A section 7.15
fn interpret_gp_fault_exception_data(exception_data: u64) {
    if exception_data != 0 {
        log::error!("Segment descriptor or IDT vector number: {exception_data:#X?}");
    } else {
        log::error!("Not from loading a segment descriptor");
    }
}

/// Cache writeback granule for `x86_64`, using 4 bytes following precedence set by Tianocore.
const CACHE_WRITEBACK_GRANULE: u32 = 4;

pub use tdx::is_td_enabled;

/// Returns whether the current environment is an Intel TDX guest.
///
/// Platform exception handlers must be installed before calling this function
/// so that any virtualization exceptions raised during detection are properly
/// handled.
pub fn is_cc_guest() -> bool {
    is_td_enabled()
}

/// Returns the Current Privilege Level (CPL) from the CS selector.
fn current_privilege_level() -> u16 {
    let cs: u16;
    // SAFETY: Reading the CS register has no side effects.
    unsafe {
        asm!("mov {0:x}, cs", out(reg) cs, options(nostack, nomem));
    }
    cs & 0b11
}

/// Returns whether the executing code runs at CPL 0, for helpers named `name` that execute an
/// instruction that faults outside CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0, so a caller at the wrong privilege level is caught instead
/// of the helper silently doing nothing.
#[track_caller]
fn check_cpl0(name: &str) -> bool {
    let cpl = current_privilege_level();
    debug_assert_eq!(cpl, 0, "{name}() requires CPL 0");
    cpl == 0
}

/// Returns the active virtual address width selected by `CR4.LA57`. Must execute at CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0.
#[track_caller]
pub fn virtual_address_bits() -> u32 {
    check_cpl0("virtual_address_bits");
    let cr4: u64;
    // SAFETY: Reading CR4 has no unsafe side effects
    unsafe {
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem, preserves_flags));
    }
    if cr4 & (1 << 12) != 0 { 57 } else { 48 }
}

/// Writes a byte to an x64 I/O port.
///
/// # Safety
///
/// The caller must ensure `port` is valid for byte writes on this platform and
/// that the side effects are safe in the current execution context.
pub unsafe fn io_out8(port: u16, value: u8) {
    // SAFETY: Guaranteed by caller.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nostack, nomem, preserves_flags)
        );
    }
}

/// Reads a 64-bit Model-Specific Register (MSR).
///
/// # Safety
///
/// Must execute at CPL 0. The caller must ensure `msr` is a valid, readable MSR on this
/// processor and in this execution context. Reading some MSRs has side effects.
///
/// # Panics
///
/// Debug builds panic outside CPL 0.
#[track_caller]
pub unsafe fn read_msr(msr: u32) -> u64 {
    check_cpl0("read_msr");
    let low: u32;
    let high: u32;
    // SAFETY: Guaranteed by caller; `rdmsr` reads the MSR in ECX into EDX:EAX.
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nostack, nomem, preserves_flags),
        );
    }
    (u64::from(high) << 32) | u64::from(low)
}

/// Writes a 64-bit Model-Specific Register (MSR).
///
/// # Safety
///
/// Must execute at CPL 0. The caller must ensure `msr` and `value` are valid for this processor
/// and in this execution context. Writing MSRs can have wide-ranging side effects.
///
/// # Panics
///
/// Debug builds panic outside CPL 0.
#[track_caller]
pub unsafe fn write_msr(msr: u32, value: u64) {
    check_cpl0("write_msr");
    let low = value as u32;
    let high = (value >> 32) as u32;
    // SAFETY: Guaranteed by caller; `wrmsr` writes EDX:EAX to the MSR in ECX.
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nostack, nomem, preserves_flags),
        );
    }
}

/// Enables interrupts with `sti`. Must execute at CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0. Release builds leave interrupts unchanged because `sti` would
/// fault.
#[track_caller]
pub(crate) fn enable_interrupts() {
    if !check_cpl0("enable_interrupts") {
        return;
    }
    // SAFETY: Enabling interrupts via `sti` does not violate memory safety; the caller is
    // responsible for ensuring the system is ready to service interrupts. This operation
    // preserves flags even though it sets the IF flag, because preserves_flags is only about
    // status flags, not control flags.
    unsafe {
        asm!("sti", options(nostack, nomem, preserves_flags));
    }
}

/// Disables interrupts with `cli`. Must execute at CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0. Release builds leave interrupts unchanged because `cli` would
/// fault.
#[track_caller]
pub(crate) fn disable_interrupts() {
    if !check_cpl0("disable_interrupts") {
        return;
    }
    // SAFETY: Disabling interrupts via `cli` does not violate memory safety.
    unsafe {
        asm!("cli", options(nostack, nomem, preserves_flags));
    }
}

/// Returns whether the interrupt flag (IF) is set in RFLAGS.
pub fn interrupts_enabled() -> bool {
    let eflags: u64;
    const IF: u64 = 0x200;
    // SAFETY: Reading RFLAGS via a push and pop has no side effects.
    unsafe {
        asm!("pushfq; pop {}", out(reg) eflags);
    }
    eflags & IF != 0
}

/// Enables interrupts and halts the CPU until the next interrupt (`sti; hlt`). Must execute at
/// CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0. Release builds return without halting because `sti` and
/// `hlt` would fault.
#[track_caller]
pub fn enable_interrupts_and_sleep() {
    if !check_cpl0("enable_interrupts_and_sleep") {
        return;
    }
    // SAFETY: This halts the CPU until the next interrupt, which has no memory safety implications. This operation
    // preserves flags even though it sets the IF flag, because preserves_flags is only about status flags, not
    // control flags.
    unsafe {
        asm!("sti; hlt", options(nostack, nomem, preserves_flags));
    }
}

/// Flushes the CPU data cache. x64 flushes the entire cache, so `start` and `length` are ignored.
///
/// ## Errors
///
/// Returns [`EfiError::Unsupported`] for [`CpuFlushType::EfiCpuFlushTypeWriteBack`].
#[track_caller]
pub fn flush_data_cache(_start: efi::PhysicalAddress, _length: u64, flush_type: CpuFlushType) -> Result<(), EfiError> {
    match flush_type {
        CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => {
            asm_wbinvd();
            Ok(())
        }
        CpuFlushType::EfiCpuFlushTypeInvalidate => {
            asm_invd();
            Ok(())
        }
        CpuFlushType::EfiCpuFlushTypeWriteBack => Err(EfiError::Unsupported),
    }
}

/// Returns the cache writeback granule size in bytes.
///
/// This value is used to populate the `dma_buffer_alignment` field in the `EFI_CPU_ARCH_PROTOCOL`.
pub fn cache_writeback_granule() -> u32 {
    CACHE_WRITEBACK_GRANULE
}

/// Returns the current time-stamp counter value, in ticks.
pub fn get_timer_value() -> u64 {
    rdtsc()
}

/// Returns the time-stamp counter frequency, in Hz, derived from CPUID leaf `0x15` or `0x16`, or
/// `None` if neither leaf reports it.
pub fn get_timer_frequency() -> Option<NonZeroU64> {
    // The maximum supported standard CPUID leaf is reported in EAX of leaf 0. A leaf must not
    // be queried unless it falls within this supported range.
    //
    // `#[allow(unused_unsafe)]` is used here to simultaneously support Rust <= 1.93 toolchains
    // that consider __cpuid unsafe and Rust >= 1.94 (or >= nightly-2025-12-27) toolchains that
    // consider __cpuid safe.
    #[allow(unused_unsafe)]
    // SAFETY: Calling cpuid does not violate memory safety
    let max_leaf = unsafe { core::arch::x86_64::__cpuid(0) }.eax;

    // CPUID 0x15 gives TSC_frequency = (ECX * EBX) / EAX. Most modern x86 platforms support it.
    if max_leaf >= 0x15 {
        #[allow(unused_unsafe)]
        // SAFETY: Calling cpuid does not violate memory safety
        let core::arch::x86_64::CpuidResult { eax, ebx, ecx, .. } = unsafe { core::arch::x86_64::__cpuid(0x15) };
        if eax != 0 && ebx != 0 && ecx != 0 {
            return NonZeroU64::new((u64::from(ecx) * u64::from(ebx)) / u64::from(eax));
        }
    }

    // CPUID 0x16 gives base frequency in MHz in EAX. This is supported on some older x86
    // platforms. It is a nominal frequency and is less accurate for reflecting actual operating
    // conditions.
    if max_leaf >= 0x16 {
        #[allow(unused_unsafe)]
        // SAFETY: Calling cpuid does not violate memory safety
        let core::arch::x86_64::CpuidResult { eax, .. } = unsafe { core::arch::x86_64::__cpuid(0x16) };
        if eax != 0 {
            return NonZeroU64::new(u64::from(eax * 1_000_000));
        }
    }

    None
}

/// Writes back and invalidates all caches of the executing CPU (`wbinvd`). Must execute at CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0. Release builds leave the caches unchanged because `wbinvd`
/// would fault.
#[track_caller]
fn asm_wbinvd() {
    if !check_cpl0("wbinvd") {
        return;
    }
    // SAFETY: Writing back and invalidating the cache has no memory safety implications.
    unsafe {
        asm!("wbinvd");
    }
}

/// Invalidates all caches of the executing CPU without writing them back (`invd`). Must execute
/// at CPL 0.
///
/// # Panics
///
/// Debug builds panic outside CPL 0. Release builds leave the caches unchanged because `invd`
/// would fault.
#[track_caller]
fn asm_invd() {
    if !check_cpl0("invd") {
        return;
    }
    // SAFETY: Invalidating the cache has no memory safety implications.
    unsafe {
        asm!("invd");
    }
}

/// Reads the time-stamp counter.
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` reads a CPU counter and does not violate memory safety.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem)) };
    (u64::from(hi) << 32) | u64::from(lo)
}
