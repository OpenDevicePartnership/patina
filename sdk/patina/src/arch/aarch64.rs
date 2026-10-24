//! `AArch64` Specific abstractions for Patina.
//!
//! - System registers
//! - Exception levels
//! - Secure monitor calls
//! - Debugger
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
//! Portions Copyright 2023 The arm-gic Authors.
//! arm-gic is dual-licensed under Apache 2.0 and MIT terms.

use crate::{error::EfiError, pi::protocol::cpu_arch::CpuFlushType, standard::efi};
use core::num::NonZeroU64;

//
// System registers
//

/// Reads the aarch64 system register `$name`.
#[macro_export]
macro_rules! read_sysreg {
    ($name:ident) => {
        {
            let mut value: u64;
            // SAFETY: The caller must provide a valid system register name
            // and ensure that the system is in a state where reading this register is safe.
            unsafe {
                core::arch::asm!(
                    concat!("mrs {value:x}, ", core::stringify!($name)),
                    value = out(reg) value,
                    options(nomem, nostack),
                );
            }
            value
        }
    }
}

/// Writes the given value to the given aarch64 system register.
/// Usage:
/// - `write_sysreg!(reg register_name, imm value)` - Write immediate value
/// - `write_sysreg!(reg register_name, imm value, "barrier1", "barrier2", ...)` - Write immediate value with barriers
/// - `write_sysreg!(reg dest_register, reg src_register)` - Copy from one register to sysreg
/// - `write_sysreg!(reg dest_register, reg src_register, "barrier1", "barrier2", ...)` - Copy from one register to sysreg with barriers
/// - `write_sysreg!(reg register_name, value)` - Write literal value
/// - `write_sysreg!(reg register_name, value, "barrier1", "barrier2", ...)` - Write literal value with barriers
#[macro_export]
macro_rules! write_sysreg {
    (reg $dest:ident, imm $imm:expr) => {
        {
            // immediate-to-register copy, no barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", {imm}"),
                    imm = const $imm,
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $dest:ident, imm $imm:expr, $($barrier:literal),+) => {
        {
            // immediate-to-register copy, barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", {imm}"),
                    $($barrier,)+
                    imm = const $imm,
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $dest:ident, reg $src:ident) => {
        {
            // register-to-register copy, no barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", ", core::stringify!($src)),
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $dest:ident, reg $src:ident, $($barrier:literal),+) => {
        {
            // register-to-register copy, barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", ", core::stringify!($src)),
                    $($barrier,)+
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $name:ident, $value:expr) => {
        {
            // no barrier required case
            let v: u64 = $value;
            // SAFETY: The caller must provide a valid system register name
            // and ensure that the system is in a state where writing to this register with this value is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($name), ", {value:x}"),
                    value = in(reg) v,
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $name:ident, $value:expr, $($barrier:literal),+) => {
        {
            // barrier required case
            let v: u64 = $value;
            // SAFETY: The caller must provide a valid system register name
            // and ensure that the system is in a state where writing to this register with this value is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($name), ", {value:x}"),
                    $($barrier,)+
                    value = in(reg) v,
                    options(nomem, nostack),
                )
            }
        }
    };
}

//
// Exception levels
//

/// Exception levels that Patina supports.
#[derive(Debug, PartialEq, Eq)]
pub enum AArch64El {
    EL1,
    EL2,
}

/// Returns the current exception level, from `CurrentEL`.
///
/// ## Panics
///
/// Panics at EL0 or EL3.
pub fn get_current_el() -> AArch64El {
    match read_sysreg!(CurrentEL) & 0xC {
        0xC => panic!("EL3 is not supported"),
        0x8 => AArch64El::EL2,
        0x4 => AArch64El::EL1,
        0x0 => panic!("EL0 is not supported"),
        _ => unreachable!(),
    }
}

//
// Secure monitor calls
//

/// Issues a Secure Monitor Call (`SMC #0`) with `function_id` in W0 and `args` in X1 to X3, and
/// returns X0 to X3. Raises an undefined instruction exception if EL3 is not implemented.
///
/// # Safety
///
/// `function_id` and `args` must be valid for the platform's secure firmware, which runs with full
/// access to the system.
pub unsafe fn smc(function_id: u32, args: [u64; 3]) -> [u64; 4] {
    let (x0, x1, x2, x3): (u64, u64, u64, u64);
    // SAFETY: Guaranteed by caller. Every register that the SMC calling convention lets the
    // firmware overwrite is declared as an output. `nomem` is omitted because the firmware may
    // read or write memory.
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") u64::from(function_id) => x0,
            inout("x1") args[0] => x1,
            inout("x2") args[1] => x2,
            inout("x3") args[2] => x3,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack),
        );
    }
    [x0, x1, x2, x3]
}

//
// Debugger
//

/// Raises a breakpoint exception (`BRK #0`). Inlined so the exception is reported at the caller.
#[inline(always)]
pub fn breakpoint() {
    // SAFETY: Executing breakpoint instruction will cause an exception, this has
    // no direct impact on safety invariants.
    unsafe {
        core::arch::asm!("brk 0", options(nostack));
    }
}

/// Length in bytes of the load that [`probe_read`] executes (`LDR`).
pub const PROBE_READ_INSTRUCTION_LEN: u64 = 4;

/// Reads 8 bytes at `address` and discards them, to test whether the address is readable.
///
/// # Safety
///
/// If the read faults, the caller's exception handler must resume execution
/// [`PROBE_READ_INSTRUCTION_LEN`] bytes past the faulting instruction. Reading `address` must have no side effects.
pub unsafe fn probe_read(address: u64) {
    // SAFETY: Guaranteed by caller. Every A64 instruction is 4 bytes, which matches
    // `PROBE_READ_INSTRUCTION_LEN`. The value read is discarded.
    unsafe { core::arch::asm!("ldr {}, [{}]", out(reg) _, in(reg) address, options(nostack)) };
}

//
// Interrupts
//

/// IRQ mask bit in the `DAIFSet` and `DAIFClr` immediate.
const DAIF_WR_IRQ_BIT: u64 = 0x2;

/// Unmasks IRQs (`DAIFClr`). Components should use TPLs.
pub fn enable_interrupts() {
    write_sysreg!(reg daifclr, imm DAIF_WR_IRQ_BIT, "isb sy");
}

/// Masks IRQs (`DAIFSet`). Components should use TPLs.
pub fn disable_interrupts() {
    write_sysreg!(reg daifset, imm DAIF_WR_IRQ_BIT, "isb sy");
}

/// Returns whether IRQs are unmasked (`DAIF.I` clear).
pub fn interrupts_enabled() -> bool {
    let daif = read_sysreg!(daif);
    daif & 0x80 == 0
}

/// Unmasks IRQs and waits for the next interrupt (`WFI`).
pub fn enable_interrupts_and_sleep() {
    // SAFETY: Enabling interrupts and then waiting for an interrupt has no memory safety implications.
    unsafe {
        core::arch::asm!("msr daifclr, {}; wfi", const DAIF_WR_IRQ_BIT, options(nostack, nomem, preserves_flags));
    }
}

/// Runs `f` with IRQs masked, then restores their previous state. Components should use TPLs
/// instead.
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

/// Performs the `flush_type` data cache operation on `[start, start + length)` by virtual address.
///
/// ## Errors
///
/// Never fails; aarch64 supports every [`CpuFlushType`].
pub fn flush_data_cache(start: efi::PhysicalAddress, length: u64, flush_type: CpuFlushType) -> Result<(), EfiError> {
    flush_data_cache_range(start, length, flush_type);
    Ok(())
}

/// Returns the cache writeback granule in bytes from `CTR_EL0.CWG`, used as the CPU Arch Protocol
/// DMA buffer alignment.
pub fn cache_writeback_granule() -> u32 {
    let ctr_el0 = read_sysreg!(ctr_el0);

    // CWG (Cache Writeback Granule): CTR_EL0 bits [27:24]
    let cwg = ((ctr_el0 >> 24) & 0xF) as u32;

    // CWG is Log2 of the max size in words
    if cwg > 0 {
        4 << cwg
    } else {
        crate::base::SIZE_2KB as u32 // Default to 2K if register contains 0 per Armv8-A spec
    }
}

/// Invalidates the executing CPU's instruction cache, for use after modifying code. If
/// `CTR_EL0.IDC` is 0, clean the modified data cache lines first.
pub fn invalidate_instruction_cache() {
    // `dsb ish` completes prior stores and data cache maintenance before the invalidation, `dsb
    // nsh` waits for the invalidation to finish, and `isb` discards instructions already fetched.
    // SAFETY: Invalidating the instruction cache has no memory safety implications.
    unsafe {
        core::arch::asm!("dsb ish", "ic iallu", "dsb nsh", "isb sy", options(nostack, preserves_flags));
    }
}

/// Applies `op` to every data cache line in `[start, start + length)`, then issues one `DSB SY`.
fn flush_data_cache_range(start: efi::PhysicalAddress, length: u64, op: CpuFlushType) {
    if length == 0 {
        return;
    }

    let cacheline_size = data_cache_line_len();
    let cacheline_mask = cacheline_size - 1;
    let mut aligned_addr = start & !cacheline_mask;
    let Some(end_addr) = start.checked_add(length) else {
        debug_assert!(false, "Cache range overflow");
        return;
    };

    while aligned_addr < end_addr {
        match op {
            CpuFlushType::EfiCpuFlushTypeWriteBack => clean_data_entry_by_mva(aligned_addr),
            CpuFlushType::EfiCpuFlushTypeInvalidate => invalidate_data_cache_entry_by_mva(aligned_addr),
            CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => clean_and_invalidate_data_entry_by_mva(aligned_addr),
        }

        match aligned_addr.checked_add(cacheline_size) {
            Some(next) => aligned_addr = next,
            None => break,
        }
    }

    // One barrier after all lines, rather than one per line.
    // SAFETY: a data barrier has no impact on safety invariants.
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
}

/// Returns the smallest data cache line size in bytes, from `CTR_EL0`.
fn data_cache_line_len() -> u64 {
    let ctr_el0 = read_sysreg!(ctr_el0);
    4 << ((ctr_el0 >> 16) & 0xf)
}

/// Cleans the data cache line containing `mva` to the point of coherency.
fn clean_data_entry_by_mva(mva: efi::PhysicalAddress) {
    // SAFETY: Cleaning the data cache has no impact on safety invariants.
    unsafe {
        core::arch::asm!("dc cvac, {}", in(reg) mva, options(nostack, preserves_flags));
    }
}

/// Invalidates the data cache line containing `mva`, discarding any dirty data.
fn invalidate_data_cache_entry_by_mva(mva: efi::PhysicalAddress) {
    // SAFETY: Invalidating the data cache has no memory safety implications, but discards dirty
    // data, so the caller must only invalidate lines it owns.
    unsafe {
        core::arch::asm!("dc ivac, {}", in(reg) mva, options(nostack, preserves_flags));
    }
}

/// Cleans and invalidates the data cache line containing `mva`.
fn clean_and_invalidate_data_entry_by_mva(mva: efi::PhysicalAddress) {
    // SAFETY: Cleaning and invalidating the data cache does not impact safety invariants.
    unsafe {
        core::arch::asm!("dc civac, {}", in(reg) mva, options(nostack, preserves_flags));
    }
}

/// Invalidates the executing CPU's TLB entries for the current EL1 or EL2 translation regime.
///
/// ## Panics
///
/// Panics at EL0 or EL3.
pub fn flush_tlb() {
    // The first DSB orders prior page table writes before the invalidation, the second waits for
    // it to complete, and ISB resynchronizes the instruction stream.
    match get_current_el() {
        // SAFETY: Invalidating the local EL2 TLB has no memory safety implications.
        AArch64El::EL2 => unsafe {
            core::arch::asm!("dsb nshst", "tlbi alle2", "dsb nsh", "isb sy", options(nostack, preserves_flags));
        },
        // SAFETY: Invalidating the local EL1 TLB has no memory safety implications.
        AArch64El::EL1 => unsafe {
            core::arch::asm!("dsb nshst", "tlbi vmalle1", "dsb nsh", "isb sy", options(nostack, preserves_flags));
        },
    }
}

//
// Timer
//

/// Returns the current value of the physical counter (`CNTPCT_EL0`), in ticks.
pub fn get_timer_value() -> u64 {
    read_sysreg!(CNTPCT_EL0)
}

/// Returns the counter frequency in Hz from `CNTFRQ_EL0`, or `None` if firmware left it unset.
pub fn get_timer_frequency() -> Option<NonZeroU64> {
    NonZeroU64::new(read_sysreg!(CNTFRQ_EL0))
}
