//! x64-specific architectural helpers for Patina.
//!
//! - Port I/O
//! - Model-specific registers
//! - Processor identification
//! - Control registers
//! - Descriptor tables
//! - Debug registers
//! - SMAP
//! - Debugger
//! - Halt and wait
//! - Interrupts
//! - Cache management (including TLB)
//! - Timer
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{error::EfiError, pi::protocol::cpu_arch::CpuFlushType, standard::efi};
use core::arch::{
    asm,
    x86_64::{__cpuid_count, CpuidResult},
};
use core::num::NonZeroU64;

//
// Port I/O
//

/// Writes a byte to I/O port `port` (`OUT`).
///
/// # Safety
///
/// `port` must be safe to write on this platform in the current context.
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

//
// Model-specific registers
//

/// Reads model-specific register `msr` (`RDMSR`).
///
/// # Safety
///
/// `msr` must be a valid, readable MSR on this processor. Some reads have side effects.
pub unsafe fn read_msr(msr: u32) -> u64 {
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

/// Writes `value` to model-specific register `msr` (`WRMSR`).
///
/// # Safety
///
/// `msr` and `value` must be valid for this processor. Writes can change any processor state.
pub unsafe fn write_msr(msr: u32, value: u64) {
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

//
// Processor identification
//

/// CPUID leaf 0x1: version information.
pub const CPUID_VERSION_INFO: u32 = 0x01;

/// MSR index for `IA32_APIC_BASE`.
const IA32_APIC_BASE_MSR_INDEX: u32 = 0x1B;

/// BSP flag bit in the `IA32_APIC_BASE` MSR (bit 8).
const IA32_APIC_BASE_BSP: u64 = 1 << 8;

/// Executes `CPUID` for `leaf` and `subleaf`. Leaves above the supported maximum return
/// processor-specific data.
pub fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    // `#[allow(unused_unsafe)]` supports toolchains that consider `__cpuid_count` unsafe as well
    // as those that consider it safe.
    #[allow(unused_unsafe)]
    // SAFETY: CPUID has no memory safety implications.
    unsafe {
        __cpuid_count(leaf, subleaf)
    }
}

/// Returns CPUID leaf 0x1 ([`CPUID_VERSION_INFO`]) for the executing processor.
pub fn cpuid_version_info() -> CpuidResult {
    cpuid(CPUID_VERSION_INFO, 0)
}

/// Returns the initial APIC ID of the executing processor (`CPUID.01H:EBX[31:24]`).
pub fn current_apic_id() -> u32 {
    (cpuid_version_info().ebx >> 24) & 0xff
}

/// Returns whether the executing processor is the BSP, from the `IA32_APIC_BASE` BSP flag. Must
/// execute at CPL 0.
pub fn is_bsp() -> bool {
    // SAFETY: IA32_APIC_BASE is an architectural MSR that is always readable at CPL 0 and has no
    // read side effects.
    let apic_base = unsafe { read_msr(IA32_APIC_BASE_MSR_INDEX) };
    (apic_base & IA32_APIC_BASE_BSP) != 0
}

//
// Control registers
//

/// Reads CR0 (operating mode and state flags). Must execute at CPL 0.
pub fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY: Reading CR0 has no side effects and does not access memory. `preserves_flags` is
    // omitted because the SDM leaves status flags undefined after MOV from a control register.
    unsafe {
        asm!("mov {}, cr0", out(reg) value, options(nostack, nomem));
    }
    value
}

/// Reads CR3 (page table root and flags). Must execute at CPL 0.
pub fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: Reading CR3 has no side effects and does not access memory. `preserves_flags` is
    // omitted because the SDM leaves status flags undefined after MOV from a control register.
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nostack, nomem));
    }
    value
}

/// Reads CR4 (feature enable flags). Must execute at CPL 0.
pub fn read_cr4() -> u64 {
    let value: u64;
    // SAFETY: Reading CR4 has no side effects and does not access memory. `preserves_flags` is
    // omitted because the SDM leaves status flags undefined after MOV from a control register.
    unsafe {
        asm!("mov {}, cr4", out(reg) value, options(nostack, nomem));
    }
    value
}

/// Returns the virtual address width in bits (57 or 48), selected by `CR4.LA57`.
pub fn virtual_address_bits() -> u32 {
    let cr4: u64;
    // SAFETY: Reading CR4 has no unsafe side effects
    unsafe {
        asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem, preserves_flags));
    }
    if cr4 & (1 << 12) != 0 { 57 } else { 48 }
}

//
// Descriptor tables
//

/// Pseudo-descriptor used by `SGDT`/`LGDT` and `SIDT`/`LIDT`.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DescriptorTablePointer {
    /// Size of the descriptor table in bytes, minus 1.
    pub limit: u16,
    /// Linear address of the descriptor table.
    pub base: u64,
}

const _: () = assert!(core::mem::size_of::<DescriptorTablePointer>() == 10);

/// Reads the GDT register (`SGDT`).
pub fn read_gdtr() -> DescriptorTablePointer {
    let mut descriptor = DescriptorTablePointer { limit: 0, base: 0 };
    // SAFETY: SGDT stores the 10-byte GDTR pseudo-descriptor into `descriptor`, which is a
    // valid, writable local of exactly that size.
    unsafe {
        asm!("sgdt [{}]", in(reg) &raw mut descriptor, options(nostack, preserves_flags));
    }
    descriptor
}

/// Reads the IDT register (`SIDT`).
pub fn read_idtr() -> DescriptorTablePointer {
    let mut descriptor = DescriptorTablePointer { limit: 0, base: 0 };
    // SAFETY: SIDT stores the 10-byte IDTR pseudo-descriptor into `descriptor`, which is a
    // valid, writable local of exactly that size.
    unsafe {
        asm!("sidt [{}]", in(reg) &raw mut descriptor, options(nostack, preserves_flags));
    }
    descriptor
}

/// Loads the GDT register from `descriptor` (`LGDT`). Must execute at CPL 0, and does not reload
/// segment registers.
///
/// # Safety
///
/// `descriptor` must describe a valid GDT that stays in memory while loaded. Every loaded selector
/// must stay valid in the new table until it is reloaded.
pub unsafe fn load_gdtr(descriptor: &DescriptorTablePointer) {
    // SAFETY: Guaranteed by caller. LGDT reads the 10-byte pseudo-descriptor and affects no flags.
    unsafe {
        asm!("lgdt [{}]", in(reg) descriptor, options(nostack, preserves_flags));
    }
}

/// Loads the IDT register from `descriptor` (`LIDT`). Must execute at CPL 0.
///
/// # Safety
///
/// `descriptor` must describe a valid IDT that stays in memory, unmodified except through valid
/// gate updates, while loaded.
pub unsafe fn load_idtr(descriptor: &DescriptorTablePointer) {
    // SAFETY: Guaranteed by caller. LIDT reads the 10-byte pseudo-descriptor and affects no flags.
    unsafe {
        asm!("lidt [{}]", in(reg) descriptor, options(nostack, preserves_flags));
    }
}

//
// Debug registers
//

/// Reads debug register `index` (DR0 to DR3, DR6 or DR7), or returns `None` for any other index.
/// Must execute at CPL 0.
pub fn read_debug_register(index: usize) -> Option<u64> {
    let value: u64;
    // SAFETY: Reading a debug register has no memory safety implications. `preserves_flags` is
    // omitted because the SDM leaves status flags undefined after MOV from a debug register.
    unsafe {
        match index {
            0 => asm!("mov {}, dr0", out(reg) value, options(nostack, nomem)),
            1 => asm!("mov {}, dr1", out(reg) value, options(nostack, nomem)),
            2 => asm!("mov {}, dr2", out(reg) value, options(nostack, nomem)),
            3 => asm!("mov {}, dr3", out(reg) value, options(nostack, nomem)),
            6 => asm!("mov {}, dr6", out(reg) value, options(nostack, nomem)),
            7 => asm!("mov {}, dr7", out(reg) value, options(nostack, nomem)),
            _ => return None,
        }
    }
    Some(value)
}

/// Writes `value` to debug register `index` (DR0 to DR3, DR6 or DR7), or returns `false` for any
/// other index. Must execute at CPL 0.
///
/// # Safety
///
/// For DR6 and DR7, bits 63:32 of `value` must be zero. A `#DB` handler must be installed before
/// DR7 enables a breakpoint.
pub unsafe fn write_debug_register(index: usize, value: u64) -> bool {
    // SAFETY: Guaranteed by caller. `nomem` is omitted so that memory accesses are not moved
    // across a change to the active watchpoints.
    unsafe {
        match index {
            0 => asm!("mov dr0, {}", in(reg) value, options(nostack)),
            1 => asm!("mov dr1, {}", in(reg) value, options(nostack)),
            2 => asm!("mov dr2, {}", in(reg) value, options(nostack)),
            3 => asm!("mov dr3, {}", in(reg) value, options(nostack)),
            6 => asm!("mov dr6, {}", in(reg) value, options(nostack)),
            7 => asm!("mov dr7, {}", in(reg) value, options(nostack)),
            _ => return false,
        }
    }
    true
}

//
// SMAP
//

/// Disables Supervisor Mode Access Prevention (`STAC`), so CPL 0 code can access user pages.
///
/// # Safety
///
/// The caller must call [`enable_smap`] once the user memory access completes. Must execute at
/// CPL 0 on a processor that supports SMAP.
pub unsafe fn disable_smap() {
    // SAFETY: Guaranteed by caller; `stac` only sets EFLAGS.AC and does not access memory.
    unsafe {
        asm!("stac", options(nostack, nomem, preserves_flags));
    }
}

/// Re-enables Supervisor Mode Access Prevention (`CLAC`).
///
/// # Safety
///
/// Must close a region opened by [`disable_smap`]. Must execute at CPL 0 on a processor that
/// supports SMAP.
pub unsafe fn enable_smap() {
    // SAFETY: Guaranteed by caller; `clac` only clears EFLAGS.AC and does not access memory.
    unsafe {
        asm!("clac", options(nostack, nomem, preserves_flags));
    }
}

//
// Debugger
//

/// Raises a breakpoint exception (`INT3`). Inlined so the exception is reported at the caller.
#[inline(always)]
pub fn breakpoint() {
    // SAFETY: This is the architecturally defined breakpoint instruction.
    unsafe { asm!("int 3") };
}

/// Length in bytes of the load that [`probe_read`] executes (`mov rax, [rcx]`).
pub const PROBE_READ_INSTRUCTION_LEN: u64 = 3;

/// Reads 8 bytes at `address` and discards them, to test whether the address is readable.
///
/// # Safety
///
/// If the read faults, the caller's exception handler must resume execution
/// [`PROBE_READ_INSTRUCTION_LEN`] bytes past the faulting instruction. Reading `address` must have no side effects.
pub unsafe fn probe_read(address: u64) {
    // SAFETY: Guaranteed by caller. The registers are fixed so the encoding is always
    // `48 8B 01`, which matches `PROBE_READ_INSTRUCTION_LEN`. The value read is discarded.
    unsafe { asm!("mov rax, [rcx]", in("rcx") address, out("rax") _, options(nostack)) };
}

//
// Halt and wait
//

/// Halts the executing CPU until the next interrupt, NMI, SMI or reset (`HLT`). Must execute at
/// CPL 0.
pub fn hlt() {
    // SAFETY: HLT only pauses execution. `nomem` is omitted so that memory accesses are not moved
    // across the halt.
    unsafe {
        asm!("hlt", options(nostack, preserves_flags));
    }
}

/// Arms address monitoring on the range containing `addr` for [`mwait`] (`MONITOR`). Check the
/// waited-on condition again before calling [`mwait`].
///
/// Must execute at CPL 0 on a processor that reports MONITOR/MWAIT (`CPUID.01H:ECX[3]`).
pub fn monitor<T>(addr: &T) {
    // SAFETY: `addr` is a reference, so the address is mapped and canonical and cannot fault.
    // ECX (extensions) is 0, the only defined value. `nomem` is omitted so that the caller's
    // re-check load is not moved above the arm.
    unsafe {
        asm!(
            "monitor",
            in("rax") core::ptr::from_ref(addr),
            in("rcx") 0,
            in("rdx") 0,
            options(nostack, preserves_flags),
        );
    }
}

/// Waits until a store to the range armed by [`monitor`] or another break event such as an
/// interrupt (`MWAIT`).
///
/// Must execute at CPL 0 on a processor that reports MONITOR/MWAIT (`CPUID.01H:ECX[3]`).
pub fn mwait() {
    // SAFETY: MWAIT only pauses execution. EAX (hints) and ECX (extensions) are 0, which every
    // implementation supports. `nomem` is omitted so that memory accesses are not moved across
    // the wait.
    unsafe {
        asm!("mwait", in("rax") 0, in("rcx") 0, options(nostack, preserves_flags));
    }
}

//
// Interrupts
//

/// Returns the Current Privilege Level (CPL) from the CS selector.
fn current_privilege_level() -> u16 {
    let cs: u16;
    // SAFETY: Reading the CS register has no side effects.
    unsafe {
        asm!("mov {0:x}, cs", out(reg) cs, options(nostack, nomem));
    }
    cs & 0b11
}

/// Enables interrupts (`STI`), or does nothing outside CPL 0. Components should use TPLs.
pub fn enable_interrupts() {
    if current_privilege_level() != 0 {
        return;
    }
    // SAFETY: Enabling interrupts does not violate memory safety. `preserves_flags` only covers
    // status flags, not the IF control flag.
    unsafe {
        asm!("sti", options(nostack, nomem, preserves_flags));
    }
}

/// Disables interrupts (`CLI`), or does nothing outside CPL 0. Components should use TPLs.
pub fn disable_interrupts() {
    if current_privilege_level() != 0 {
        return;
    }
    // SAFETY: Disabling interrupts via `cli` does not violate memory safety.
    unsafe {
        asm!("cli", options(nostack, nomem, preserves_flags));
    }
}

/// Returns whether interrupts are enabled (`RFLAGS.IF`).
pub fn interrupts_enabled() -> bool {
    let eflags: u64;
    const IF: u64 = 0x200;
    // SAFETY: Reading RFLAGS via a push and pop has no side effects.
    unsafe {
        asm!("pushfq; pop {}", out(reg) eflags);
    }
    eflags & IF != 0
}

/// Enables interrupts and halts until the next one (`STI; HLT`).
pub fn enable_interrupts_and_sleep() {
    // SAFETY: Halting until the next interrupt does not violate memory safety. `preserves_flags`
    // only covers status flags, not the IF control flag.
    unsafe {
        asm!("sti; hlt", options(nostack, nomem, preserves_flags));
    }
}

/// Runs `f` with interrupts disabled, then restores their previous state. Components should use
/// TPLs instead.
pub(crate) fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    if !interrupts_enabled() {
        return f();
    }
    disable_interrupts();
    let result = f();
    enable_interrupts();
    result
}

//
// Cache management
//

/// Cache writeback granule in bytes, following the value used by Tianocore.
const CACHE_WRITEBACK_GRANULE: u32 = 4;

/// Flushes the whole data cache of the executing CPU. x64 has no ranged flush, so the range is
/// ignored.
///
/// ## Errors
///
/// Returns [`EfiError::Unsupported`] for [`CpuFlushType::EfiCpuFlushTypeWriteBack`].
pub fn flush_data_cache(_start: efi::PhysicalAddress, _length: u64, flush_type: CpuFlushType) -> Result<(), EfiError> {
    match flush_type {
        CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => {
            wbinvd();
            Ok(())
        }
        CpuFlushType::EfiCpuFlushTypeInvalidate => {
            invd();
            Ok(())
        }
        CpuFlushType::EfiCpuFlushTypeWriteBack => Err(EfiError::Unsupported),
    }
}

/// Returns the cache writeback granule in bytes, used as the CPU Arch Protocol DMA buffer
/// alignment.
pub fn cache_writeback_granule() -> u32 {
    CACHE_WRITEBACK_GRANULE
}

/// Writes back and invalidates all caches of the executing CPU (`WBINVD`). Very expensive, and
/// must execute at CPL 0.
pub fn wbinvd() {
    // SAFETY: Writing back and invalidating the cache has no memory safety implications.
    unsafe {
        asm!("wbinvd");
    }
}

/// Invalidates all caches of the executing CPU without writing them back (`INVD`).
fn invd() {
    // SAFETY: Invalidating the cache has no memory safety implications.
    unsafe {
        asm!("invd");
    }
}

/// Invalidates the executing CPU's non-global TLB entries by reloading CR3. Must execute at CPL 0.
pub fn flush_tlb() {
    // SAFETY: CR3 is written back with its current value, so the active translation is
    // unchanged. `nomem` is omitted so prior page table writes are ordered before the reload.
    unsafe {
        asm!("mov {cr3}, cr3", "mov cr3, {cr3}", cr3 = out(reg) _, options(nostack));
    }
}

//
// Timer
//

/// Reads the time stamp counter (`RDTSC`).
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` reads a CPU counter and does not violate memory safety.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem)) };
    (u64::from(hi) << 32) | u64::from(lo)
}

/// Returns the current value of the time stamp counter, in ticks.
pub fn get_timer_value() -> u64 {
    rdtsc()
}

/// Returns the time stamp counter frequency in Hz from CPUID leaf 0x15 or 0x16, or `None` if
/// neither reports it.
pub fn get_timer_frequency() -> Option<NonZeroU64> {
    // A leaf must not be queried above the maximum supported leaf, which leaf 0 reports in EAX.
    let max_leaf = cpuid(0, 0).eax;

    // CPUID 0x15 gives TSC_frequency = (ECX * EBX) / EAX. Most modern x86 platforms support it.
    if max_leaf >= 0x15 {
        let CpuidResult { eax, ebx, ecx, .. } = cpuid(0x15, 0);
        if eax != 0 && ebx != 0 && ecx != 0 {
            return NonZeroU64::new((u64::from(ecx) * u64::from(ebx)) / u64::from(eax));
        }
    }

    // CPUID 0x16 gives the nominal base frequency in MHz in EAX, on some older platforms.
    if max_leaf >= 0x16 {
        let CpuidResult { eax, .. } = cpuid(0x16, 0);
        if eax != 0 {
            return NonZeroU64::new(u64::from(eax * 1_000_000));
        }
    }

    None
}
