//! Architectural Intrinsics for the MM Supervisor Core
//!
//! Provides thin, architecture-specific wrappers around low-level `x86_64`
//! instructions used by the supervisor: `rdmsr`/`wrmsr` for Model-Specific
//! Registers, `cpuid`/MSR reads for CPU identification (APIC ID and BSP
//! detection), `sidt` for the interrupt descriptor table pointer, and `stac`/`clac`
//! for Supervisor Mode Access Prevention. Access to individual MSRs is expected to be gated by the syscall
//! policy layer.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::x86_64::{__cpuid, CpuidResult};

/// Pointer structure used by the `SIDT` / `LIDT` (and `SGDT` / `LGDT`) instructions.
///
/// Layout matches the Intel SDM: a 16-bit limit followed by a 64-bit base.
/// `packed(2)` produces the expected 10-byte on-the-wire representation with no
/// internal padding between `limit` and `base`.
#[repr(C, packed(2))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct DescriptorTablePointer {
    /// Size of the descriptor table in bytes, minus 1.
    pub(crate) limit: u16,
    /// Linear address of the descriptor table.
    pub(crate) base: u64,
}

/// CPUID leaf 0x1: Version Information (Type, Family, Model, and Stepping ID).
pub(crate) const CPUID_VERSION_INFO: u32 = 0x01;

/// MSR index for `IA32_APIC_BASE`.
const IA32_APIC_BASE_MSR_INDEX: u32 = 0x1B;

/// BSP flag bit in `IA32_APIC_BASE` MSR (bit 8).
const IA32_APIC_BSP: u64 = 1 << 8;

/// Reads a Model-Specific Register (MSR) by index.
///
/// ## Safety
///
/// The caller must ensure the MSR index is valid and readable on the current
/// platform.
// Executes the privileged `rdmsr` instruction, which faults outside ring 0 and
// cannot run in a host-based unit test.
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
    (u64::from(hi) << 32) | u64::from(lo)
}

/// Writes a 64-bit value to a Model-Specific Register (MSR).
///
/// ## Safety
///
/// The caller must ensure the MSR index is valid and writable on the current
/// platform.
// Executes the privileged `wrmsr` instruction, which faults outside ring 0 and
// cannot run in a host-based unit test.
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
/// On `x86_64`, this reads the APIC ID from the Local APIC or CPUID.
// Depends on the running processor's `cpuid` state, which cannot be exercised
// deterministically in a host-based unit test.
pub fn get_current_cpu_id() -> CpuidResult {
    // Use CPUID to get the initial APIC ID
    // CPUID function 0x01

    // CPUID is always available on x86_64 and `__cpuid` is a safe intrinsic.
    __cpuid(CPUID_VERSION_INFO)
}

/// Returns the APIC ID of the processor executing this call.
///
/// The initial APIC ID is `EBX[31:24]` of CPUID leaf 1.
pub fn current_apic_id() -> u32 {
    (get_current_cpu_id().ebx >> 24) & 0xff
}

/// Checks if the current processor is the Bootstrap Processor (BSP).
///
/// This reads the `IA32_APIC_BASE` MSR and checks the BSP flag (bit 8).
/// The BSP flag is set by hardware during reset and indicates which
/// processor is the bootstrap processor.
// Reads the IA32_APIC_BASE MSR via the privileged `rdmsr` instruction, which
// faults outside ring 0 and cannot run in a host-based unit test.
pub fn is_bsp() -> bool {
    // SAFETY: The IA32_APIC_BASE MSR is safe to read on x86_64.
    let apic_base = unsafe { read_msr(IA32_APIC_BASE_MSR_INDEX) };
    (apic_base & IA32_APIC_BSP) != 0
}

/// Read CR3 register.
pub(crate) fn read_cr3() -> u64 {
    let value: u64;

    #[cfg(test)]
    {
        value = 0;
    }

    #[cfg(not(test))]
    {
        // SAFETY: inline asm is inherently unsafe because Rust can't reason about it.
        // In this case we are reading the CR3 register, which is a safe operation.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) value, options(nostack, preserves_flags));
        }
    }

    value
}

/// Read the current IDT Register (IDTR) via the `SIDT` instruction.
///
/// Returns a [`DescriptorTablePointer`] containing the IDT base and limit.
pub(crate) fn read_idtr() -> DescriptorTablePointer {
    let rt_descriptor = DescriptorTablePointer { limit: 0, base: 0 };

    // On the real firmware target, populate it via `SIDT`. The asm-free builds
    // (tests / non-x86_64) keep the zero-initialized value, so no mutable binding
    // is introduced where it would go unused.
    #[cfg(not(test))]
    let rt_descriptor = {
        let mut descriptor = rt_descriptor;
        // SAFETY: SIDT stores the 10-byte IDTR pseudo-descriptor to the specified
        // memory location. This is a read-only operation on CPU state.
        unsafe {
            core::arch::asm!(
                "sidt [{}]",
                in(reg) &raw mut descriptor,
                options(nostack, preserves_flags)
            );
        }
        descriptor
    };

    rt_descriptor
}

/// Helper function to disable the SMAP bit in EFLAGS to allow supervisor code to access user memory when needed.
///
/// ## Safety
///
/// Disabling SMAP removes the hardware barrier that stops the supervisor (Ring 0) from
/// reading or writing user-owned (Ring 3) memory. The caller must re-enable SMAP via
/// [`enable_smap`](crate::intrinsics::enable_smap) once the user-memory access completes, and must ensure every access
/// performed while SMAP is lifted targets valid, correctly-owned user memory. Prefer
/// [`with_user_access`](crate::user_access_guard::with_user_access), which guarantees the disable/enable pair is balanced.
pub(crate) unsafe fn disable_smap() {
    // SAFETY: `stac` only sets the AC flag in EFLAGS; it touches no memory and clobbers
    // no registers (hence `nostack, preserves_flags`). It is a privileged instruction that
    // is valid in the Ring 0 supervisor context this code always runs in.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "stac", // Set AC flag to enable access to user memory
            options(nostack, preserves_flags)
        );
    }
}

/// Helper function to re-enable the SMAP bit in EFLAGS after accessing user memory.
///
/// ## Safety
///
/// This mutates the privileged EFLAGS.AC state and must only be called to close a region
/// opened by [`disable_smap`](crate::intrinsics::disable_smap). Callers must ensure no further user-memory access that
/// relies on SMAP being lifted happens after this returns. Prefer [`with_user_access`](crate::user_access_guard::with_user_access),
/// which guarantees the disable/enable pair is balanced.
pub(crate) unsafe fn enable_smap() {
    // SAFETY: `clac` only clears the AC flag in EFLAGS; it touches no memory and clobbers
    // no registers (hence `nostack, preserves_flags`). It is a privileged instruction that
    // is valid in the Ring 0 supervisor context this code always runs in.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "clac", // Clear AC flag to re-enable SMAP protections
            options(nostack, preserves_flags)
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_read_idtr_is_zeroed_in_unit_tests() {
        let idtr = read_idtr();
        let base = idtr.base;
        let limit = idtr.limit;

        assert_eq!(base, 0);
        assert_eq!(limit, 0);
    }

    #[test]
    fn test_descriptor_table_pointer_layout_matches_c_abi() {
        assert_eq!(core::mem::size_of::<DescriptorTablePointer>(), 10);
    }
}
