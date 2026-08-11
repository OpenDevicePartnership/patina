//! Architectural Intrinsics for the MM Supervisor Core
//!
//! Provides thin, architecture-specific wrappers around low-level x86_64
//! instructions used by the supervisor: `rdmsr`/`wrmsr` for Model-Specific
//! Registers and `cpuid`/MSR reads for CPU identification (APIC ID and BSP
//! detection). Access to individual MSRs is expected to be gated by the syscall
//! policy layer.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::x86_64::{__cpuid, CpuidResult};

/// CPUID leaf 0x1: Version Information (Type, Family, Model, and Stepping ID).
pub(crate) const CPUID_VERSION_INFO: u32 = 0x01;

/// MSR index for IA32_APIC_BASE.
const IA32_APIC_BASE_MSR_INDEX: u32 = 0x1B;

/// BSP flag bit in IA32_APIC_BASE MSR (bit 8).
const IA32_APIC_BSP: u64 = 1 << 8;

/// Reads a Model-Specific Register (MSR) by index.
///
/// ## Safety
///
/// The caller must ensure the MSR index is valid and readable on the current
/// platform.
// Executes the privileged `rdmsr` instruction, which faults outside ring 0 and
// cannot run in a host-based unit test.
#[cfg_attr(coverage, coverage(off))]
pub unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: Reading the MSR is memory safe as long as the caller ensures the
    //         MSR index is valid. But this could also reveal the contents of
    //         the MSR, which is why we should guard this behind the syscall
    //         gate and only allow access to certain MSRs.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Writes a 64-bit value to a Model-Specific Register (MSR).
///
/// ## Safety
///
/// The caller must ensure the MSR index is valid and writable on the current
/// platform.
// Executes the privileged `wrmsr` instruction, which faults outside ring 0 and
// cannot run in a host-based unit test.
#[cfg_attr(coverage, coverage(off))]
pub unsafe fn write_msr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    // SAFETY: Writing the MSR is memory safe as long as the caller ensures the
    //         MSR index is valid and writable (guaranteed by this function's
    //         `unsafe` contract). `wrmsr` writes only the selected MSR from
    //         EDX:EAX and touches no memory (nomem, nostack).
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nomem, nostack),
        );
    }
}

/// Gets the current CPU's APIC ID.
///
/// On x86_64, this reads the APIC ID from the Local APIC or CPUID.
// Depends on the running processor's `cpuid` state, which cannot be exercised
// deterministically in a host-based unit test.
#[cfg_attr(coverage, coverage(off))]
pub fn get_current_cpu_id() -> CpuidResult {
    // Use CPUID to get the initial APIC ID
    // CPUID function 0x01

    // CPUID is always available on x86_64 and `__cpuid` is a safe intrinsic.
    __cpuid(CPUID_VERSION_INFO)
}

/// Checks if the current processor is the Bootstrap Processor (BSP).
///
/// This reads the IA32_APIC_BASE MSR and checks the BSP flag (bit 8).
/// The BSP flag is set by hardware during reset and indicates which
/// processor is the bootstrap processor.
// Reads the IA32_APIC_BASE MSR via the privileged `rdmsr` instruction, which
// faults outside ring 0 and cannot run in a host-based unit test.
#[cfg_attr(coverage, coverage(off))]
pub fn is_bsp() -> bool {
    // SAFETY: The IA32_APIC_BASE MSR is safe to read on x86_64.
    let apic_base = unsafe { read_msr(IA32_APIC_BASE_MSR_INDEX) };
    (apic_base & IA32_APIC_BSP) != 0
}

/// Read CR3 register.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn read_cr3() -> u64 {
    let mut _value = 0u64;

    #[cfg(not(test))]
    {
        // SAFETY: inline asm is inherently unsafe because Rust can't reason about it.
        // In this case we are reading the CR3 register, which is a safe operation.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) _value, options(nostack, preserves_flags));
        }
    }

    _value
}
