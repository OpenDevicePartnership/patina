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

/// Pseudo-descriptor used by SGDT, SIDT, LGDT and LIDT: a 16-bit limit followed by a 64-bit
/// base, 10 bytes with no padding.
#[repr(C, packed(2))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, zerocopy_derive::FromBytes, zerocopy_derive::Immutable)]
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

/// CPUID leaf `0x8000_0008`: address sizes. EAX bits 7:0 hold MAXPHYADDR.
const CPUID_VIR_PHY_ADDRESS_SIZE: u32 = 0x8000_0008;

/// Physical address width assumed when CPUID leaf `0x8000_0008` is not available.
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
/// The MSR index must be valid and readable on this processor. `rdmsr` faults outside Ring 0.
#[cfg_attr(coverage, coverage(off))]
pub(crate) unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: the caller guarantees the MSR index is valid and readable; `rdmsr` touches no
    // memory.
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
/// The MSR index must be valid and writable on this processor, and the value must be safe to
/// write. `wrmsr` faults outside Ring 0.
#[cfg_attr(coverage, coverage(off))]
pub(crate) unsafe fn write_msr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    // SAFETY: the caller guarantees the MSR index and value are valid; `wrmsr` touches no
    // memory.
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
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn cpuid_version_info() -> CpuidResult {
    __cpuid(CPUID_VERSION_INFO)
}

/// Returns the physical address width (MAXPHYADDR) in bits, or
/// [`DEFAULT_PHYSICAL_ADDRESS_BITS`] when CPUID leaf `0x8000_0008` is not available.
pub(crate) fn cpuid_physical_address_bits() -> u8 {
    if __cpuid(CPUID_EXTENDED_FUNCTION).eax < CPUID_VIR_PHY_ADDRESS_SIZE {
        return DEFAULT_PHYSICAL_ADDRESS_BITS;
    }
    let [bits, ..] = __cpuid(CPUID_VIR_PHY_ADDRESS_SIZE).eax.to_le_bytes();
    bits
}

/// Returns the initial APIC ID of the current processor (CPUID leaf 1, EBX bits 31:24).
///
/// Returns 0 in unit tests. The host can move a test thread between cores, so the real value can
/// change between two calls.
pub(crate) fn current_apic_id() -> u32 {
    if cfg!(test) { 0 } else { (cpuid_version_info().ebx >> 24) & 0xff }
}

/// Returns whether the current processor is the BSP, from the BSP flag in `IA32_APIC_BASE`.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn is_bsp() -> bool {
    // SAFETY: IA32_APIC_BASE is architectural on every x86_64 CPU, and reading it has no side
    // effects.
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

    // Test builds keep the zeroed value.
    #[cfg(not(test))]
    let rt_descriptor = {
        let mut descriptor = rt_descriptor;
        // SAFETY: SIDT writes the 10-byte IDTR to `descriptor`, a local with that layout.
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

/// Reads the GDT register (GDTR) with SGDT.
///
/// Returns a zeroed [`DescriptorTablePointer`] in unit tests.
pub(crate) fn read_gdtr() -> DescriptorTablePointer {
    let rt_descriptor = DescriptorTablePointer { limit: 0, base: 0 };

    // Test builds keep the zeroed value.
    #[cfg(not(test))]
    let rt_descriptor = {
        let mut descriptor = rt_descriptor;
        // SAFETY: SGDT writes the 10-byte GDTR to `descriptor`, a local with that layout.
        unsafe {
            core::arch::asm!(
                "sgdt [{}]",
                in(reg) &raw mut descriptor,
                options(nostack, preserves_flags)
            );
        }
        descriptor
    };

    rt_descriptor
}

/// Reads the live LDTR selector. Returns 0 in unit tests, where UMIP can prohibit SLDT.
pub(crate) fn read_ldtr() -> u16 {
    #[cfg(test)]
    {
        0
    }

    #[cfg(not(test))]
    {
        let selector: u16;
        // SAFETY: the supervisor runs at CPL0. SLDT only copies the selector into a register.
        unsafe {
            core::arch::asm!("sldt {0:x}", out(reg) selector, options(nomem, nostack, preserves_flags));
        }
        selector
    }
}

/// Sets EFLAGS.AC with `stac`, which lets Ring 0 access user pages despite SMAP.
///
/// ## Safety
///
/// Every access made while SMAP is lifted must target valid user memory, and the caller must
/// call [`enable_smap`] afterwards. Prefer
/// [`with_user_access`](crate::user_access_guard::with_user_access), which keeps the pair
/// balanced.
pub(crate) unsafe fn disable_smap() {
    // SAFETY: `stac` only sets EFLAGS.AC, which Rust does not track, so `preserves_flags`
    // holds. `nomem` is omitted so it also acts as a compiler barrier for the accesses after it.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!("stac", options(nostack, preserves_flags));
    }
}

/// Clears EFLAGS.AC with `clac`, restoring SMAP protection.
///
/// ## Safety
///
/// Must only close a region opened by [`disable_smap`], and no access that relies on SMAP being
/// lifted may follow. Prefer [`with_user_access`](crate::user_access_guard::with_user_access).
pub(crate) unsafe fn enable_smap() {
    // SAFETY: `clac` only clears EFLAGS.AC, which Rust does not track, so `preserves_flags`
    // holds. `nomem` is omitted so it also acts as a compiler barrier for the accesses before it.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!("clac", options(nostack, preserves_flags));
    }
}

/// Runs `write` with CR0.WP cleared so Ring 0 can store to read-only pages, then restores CR0.WP.
///
/// ## Safety
///
/// Must run in Ring 0. Every store `write` makes to a read-only page must be intended, and
/// nothing `write` runs may rely on write protection.
pub(crate) unsafe fn with_write_protection_disabled<R>(write: impl FnOnce() -> R) -> R {
    /// Restores CR0.WP from the value saved when write protection was cleared.
    struct RestoreWriteProtection(u64);

    impl Drop for RestoreWriteProtection {
        fn drop(&mut self) {
            // SAFETY: restores the CR0.WP value saved on this CPU; the caller guarantees Ring 0.
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
    fn test_read_descriptor_table_registers_are_zeroed_in_unit_tests() {
        for descriptor in [read_idtr(), read_gdtr()] {
            let base = descriptor.base;
            let limit = descriptor.limit;

            assert_eq!(base, 0);
            assert_eq!(limit, 0);
        }
    }

    #[test]
    fn test_cpuid_physical_address_bits_reports_an_x64_width() {
        // The exact width depends on the host, but x64 defines MAXPHYADDR as at most 52 bits,
        // and every processor reporting the leaf supports at least a 32-bit physical space.
        let bits = cpuid_physical_address_bits();

        assert!((32..=52).contains(&bits), "unexpected MAXPHYADDR {bits}");
    }

    #[test]
    fn test_descriptor_table_pointer_layout_matches_c_abi() {
        assert_eq!(core::mem::size_of::<DescriptorTablePointer>(), 10);
    }
}
