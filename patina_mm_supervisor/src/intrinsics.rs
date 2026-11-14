//! Architectural Intrinsics for the MM Supervisor Core
//!
//! Wrappers for the MSR, CPUID, control register, descriptor table and SMAP instructions the
//! supervisor uses.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::x86_64::{__cpuid, CpuidResult};

use patina_paging::x64::{disable_write_protection, enable_write_protection};

/// Pointer structure used by the SIDT and LIDT (and SGDT and LGDT) instructions.
///
/// Layout matches the Intel SDM: a 16-bit limit followed by a 64-bit base.
/// `packed(2)` gives the expected 10-byte in-memory layout with no padding between `limit` and
/// `base`.
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

/// CPUID leaf `0x8000_0000`: the highest extended leaf the processor supports, in EAX.
const CPUID_EXTENDED_FUNCTION: u32 = 0x8000_0000;

/// CPUID leaf `0x8000_0008`: virtual and physical address sizes. EAX[7:0] is MAXPHYADDR.
const CPUID_VIR_PHY_ADDRESS_SIZE: u32 = 0x8000_0008;

/// Physical address width to assume when the processor does not report CPUID leaf `0x8000_0008`,
/// as the producer assumes.
const DEFAULT_PHYSICAL_ADDRESS_BITS: u8 = 36;

/// MSR index for `IA32_APIC_BASE`.
const IA32_APIC_BASE_MSR_INDEX: u32 = 0x1B;

/// BSP flag bit in `IA32_APIC_BASE` MSR (bit 8).
const IA32_APIC_BSP: u64 = 1 << 8;

/// CR4.LA57 (bit 12): 5-level paging is enabled.
pub(crate) const CR4_LA57: u64 = 1 << 12;

/// Reads a Model-Specific Register (MSR) by index.
///
/// ## Safety
///
/// The caller must ensure the MSR index is valid and readable on the current
/// platform. Executes the privileged `rdmsr` instruction, which faults outside
/// ring 0 and cannot run in a host-based unit test.
pub(crate) unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: the caller guarantees the MSR index is valid and readable, and `rdmsr` touches no
    //         memory. Reading can still disclose the MSR's contents, so Ring 3 access should stay
    //         behind the syscall gate and be limited to specific MSRs.
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
/// platform. Executes the privileged `wrmsr` instruction, which faults outside
/// ring 0 and cannot run in a host-based unit test.
pub(crate) unsafe fn write_msr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    // SAFETY: the caller guarantees the MSR index is valid and writable (this function's
    //         `unsafe` contract). `wrmsr` writes only the selected MSR from EDX:EAX and
    //         touches no memory (nomem, nostack).
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

/// Returns the raw CPUID leaf 1 (version information) result for the current CPU.
///
/// The initial APIC ID is in EBX[31:24] of the returned [`CpuidResult`]; callers extract it.
/// Depends on the running processor's `cpuid` state, which cannot be exercised
/// deterministically in a host-based unit test.
pub(crate) fn cpuid_version_info() -> CpuidResult {
    // CPUID is always available on x86_64, and `__cpuid` is a safe intrinsic.
    __cpuid(CPUID_VERSION_INFO)
}

/// Returns the processor's physical address width (MAXPHYADDR) in bits, from CPUID leaf
/// `0x8000_0008`, or 36 when the processor does not report that leaf.
///
/// Depends on the running processor, so a host-based unit test cannot pin its value.
pub(crate) fn cpuid_physical_address_bits() -> u8 {
    // CPUID is always available on x86_64, and `__cpuid` is a safe intrinsic.
    if __cpuid(CPUID_EXTENDED_FUNCTION).eax < CPUID_VIR_PHY_ADDRESS_SIZE {
        return DEFAULT_PHYSICAL_ADDRESS_BITS;
    }
    let [bits, ..] = __cpuid(CPUID_VIR_PHY_ADDRESS_SIZE).eax.to_le_bytes();
    bits
}

/// Returns the APIC ID of the processor executing this call.
///
/// The initial APIC ID is EBX[31:24] of CPUID leaf 1.
pub(crate) fn current_apic_id() -> u32 {
    (cpuid_version_info().ebx >> 24) & 0xff
}

/// Returns whether the current processor is the Bootstrap Processor (BSP).
///
/// Reads the BSP flag (bit 8) of the `IA32_APIC_BASE` MSR, which hardware sets at reset on the
/// bootstrap processor. `rdmsr` faults outside ring 0, so this cannot run in a host-based unit
/// test.
pub(crate) fn is_bsp() -> bool {
    // SAFETY: IA32_APIC_BASE is an architectural MSR on every x86_64 CPU, and reading it has no
    // side effects.
    let apic_base = unsafe { read_msr(IA32_APIC_BASE_MSR_INDEX) };
    (apic_base & IA32_APIC_BSP) != 0
}

/// Reads the CR3 register. Returns 0 in unit tests.
pub(crate) fn read_cr3() -> u64 {
    let value: u64;

    #[cfg(test)]
    {
        value = 0;
    }

    #[cfg(not(test))]
    {
        // SAFETY: reading CR3 has no side effects and touches no memory.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) value, options(nostack, preserves_flags));
        }
    }

    value
}

/// Reads the CR4 register. Returns 0 in unit tests.
pub(crate) fn read_cr4() -> u64 {
    let value: u64;

    #[cfg(test)]
    {
        value = 0;
    }

    #[cfg(not(test))]
    {
        // SAFETY: reading CR4 has no side effects and touches no memory.
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) value, options(nostack, preserves_flags));
        }
    }

    value
}

/// Reads the IDT register (IDTR) with SIDT.
///
/// Returns a zeroed [`DescriptorTablePointer`] in unit tests.
pub(crate) fn read_idtr() -> DescriptorTablePointer {
    let rt_descriptor = DescriptorTablePointer { limit: 0, base: 0 };

    // Outside unit tests, fill it with SIDT. Test builds keep the zeroed value, so no mutable
    // binding is left unused there.
    #[cfg(not(test))]
    let rt_descriptor = {
        let mut descriptor = rt_descriptor;
        // SAFETY: SIDT writes the 10-byte IDTR pseudo-descriptor to `descriptor`, a local with
        // exactly that layout, and only reads CPU state.
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

/// Sets EFLAGS.AC with `stac`, which lets supervisor code access user pages despite SMAP.
///
/// ## Safety
///
/// Disabling SMAP removes the hardware barrier that stops the supervisor (Ring 0) from
/// reading or writing user-owned (Ring 3) memory. The caller must re-enable SMAP via
/// [`enable_smap`](crate::intrinsics::enable_smap) once the user-memory access completes, and must ensure every access
/// performed while SMAP is lifted targets valid, correctly-owned user memory. Prefer
/// [`with_user_access`](crate::user_access_guard::with_user_access), which guarantees the disable/enable pair is balanced.
pub(crate) unsafe fn disable_smap() {
    // SAFETY: `stac` sets EFLAGS.AC and changes nothing else. `preserves_flags` is still valid
    // because AC is not one of the flags Rust tracks, and `nomem` is left out on purpose so the
    // instruction also acts as a compiler barrier for the user-memory accesses after it. It is a
    // privileged instruction that is valid in the Ring 0 supervisor context this code always
    // runs in.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "stac", // Set AC flag to enable access to user memory
            options(nostack, preserves_flags)
        );
    }
}

/// Clears EFLAGS.AC with `clac`, restoring SMAP protection after a user-memory access.
///
/// ## Safety
///
/// This mutates the privileged EFLAGS.AC state and must only be called to close a region opened by
/// [`disable_smap`](crate::intrinsics::disable_smap). Callers must ensure no further user-memory
/// access that relies on SMAP being lifted happens after this returns. Prefer
/// [`with_user_access`](crate::user_access_guard::with_user_access), which guarantees the
/// disable/enable pair is balanced.
pub(crate) unsafe fn enable_smap() {
    // SAFETY: `clac` clears EFLAGS.AC and changes nothing else. `preserves_flags` is still
    // valid because AC is not one of the flags Rust tracks, and `nomem` is left out on purpose
    // so the instruction also acts as a compiler barrier for the user-memory accesses before it.
    // It is a privileged instruction that is valid in the Ring 0 supervisor context this code
    // always runs in.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "clac", // Clear AC flag to re-enable SMAP protections
            options(nostack, preserves_flags)
        );
    }
}

/// Runs `write` with CR0.WP cleared so Ring 0 can store to read-only pages, and restores CR0.WP
/// to its prior state when `write` returns.
///
/// ## Safety
///
/// Must run in Ring 0 on the CPU whose CR0 is changed. Every store `write` makes to a read-only
/// page must be one the caller intends, and nothing `write` runs may rely on read-only
/// protection, because no page is write protected until it returns.
pub(crate) unsafe fn with_write_protection_disabled<R>(write: impl FnOnce() -> R) -> R {
    /// Restores CR0.WP from the value saved when write protection was cleared.
    struct RestoreWriteProtection(u64);

    impl Drop for RestoreWriteProtection {
        fn drop(&mut self) {
            // SAFETY: the saved value came from the paired `disable_write_protection` on this CPU,
            // so this puts CR0.WP back to what it was. The caller guarantees Ring 0.
            unsafe { enable_write_protection(self.0) };
        }
    }

    // SAFETY: the caller guarantees Ring 0, and the guard restores CR0.WP when this returns.
    let _restore = RestoreWriteProtection(unsafe { disable_write_protection() });
    write()
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_with_write_protection_disabled_runs_the_closure() {
        // SAFETY: the closures store to no memory, so no read-only page is changed. On the host
        // the CR0 accessors do nothing.
        unsafe {
            assert_eq!(with_write_protection_disabled(|| 42), 42);
            // The guard restores the prior state on drop, so the helper can be used again.
            assert_eq!(with_write_protection_disabled(|| 7), 7);
        }
    }

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
