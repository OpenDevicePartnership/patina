//! Performance Timer for the MM Supervisor Core
//!
//! Provides real-time, TSC-based timing helpers used by mailbox timeouts and
//! AP-arrival polling.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::x86_64;

use crate::CpuInfo;

// TODO: This is copied from perf_timer.rs in patina_dxe_core
/// Returns the current CPU count using architecture-specific methods.
///
/// Skip coverage as any value could be valid, including 0.
#[cfg_attr(coverage_nightly, coverage(off))]
fn ticks() -> u64 {
    // SAFETY: _rdtsc only reads the TSC on x86_64. No invariants are required for safety.
    unsafe { x86_64::_rdtsc() }
}

/// Returns the frequency in Hz from the platform's CpuInfo, or `0` if
/// not determinable.
#[inline]
pub fn frequency<C: CpuInfo>() -> u64 {
    C::perf_timer_frequency().unwrap_or(0)
}

/// Converts a duration in microseconds to the equivalent tick count using
/// the given CpuInfo's frequency.
///
/// Returns `None` if the frequency is unknown (0).
#[inline]
pub fn us_to_ticks<C: CpuInfo>(us: u64) -> Option<u64> {
    let mut freq = frequency::<C>();
    if freq == 0 {
        freq = arch_perf_frequency();
    }
    Some(((freq as u128 * us as u128) / 1_000_000) as u64)
}

pub(crate) fn arch_perf_frequency() -> u64 {
    // Try to get TSC frequency from CPUID (most Intel and AMD platforms).
    #[cfg(target_arch = "x86_64")]
    {
        // `#[allow(unused_unsafe)]` is used here to simultaneously support Rust <= 1.93 toolchains
        // that consider __cpuid unsafe and Rust >= 1.94 (or >= nightly-2025-12-27) toolchains that
        // consider __cpuid safe.
        #[allow(unused_unsafe)]
        // SAFETY: Calling cpuid does not violate memory safety
        let core::arch::x86_64::CpuidResult { eax, ebx, ecx, .. } = unsafe { core::arch::x86_64::__cpuid(0x15) };
        if eax != 0 && ebx != 0 && ecx != 0 {
            // CPUID 0x15 gives TSC_frequency = (ECX * EBX) / EAX.
            // Most modern x86 platforms support this leaf.
            return (ecx as u64 * ebx as u64) / eax as u64;
        }

        // CPUID 0x16 gives base frequency in MHz in EAX.
        // This is supported on some older x86 platforms.
        // This is a nominal frequency and is less accurate for reflecting actual operating conditions.
        //
        // `#[allow(unused_unsafe)]` is used here to simultaneously support Rust <= 1.93 toolchains
        // that consider __cpuid unsafe and Rust >= 1.94 (or >= nightly-2025-12-27) toolchains that
        // consider __cpuid safe.
        #[allow(unused_unsafe)]
        // SAFETY: Calling cpuid does not violate memory safety
        let core::arch::x86_64::CpuidResult { eax, .. } = unsafe { core::arch::x86_64::__cpuid(0x16) };
        if eax != 0 {
            // CPUID 0x16 gives base frequency in MHz in EAX.
            // This is supported on some older x86 platforms.
            // This is a nominal frequency and is less accurate for reflecting actual operating conditions.
            return (eax * 1_000_000) as u64;
        }

        0
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    0
}

/// Spins until at least `timeout_us` microseconds have elapsed.
///
/// Returns `true` when the provided `condition` closure returns `true`
/// before the deadline, or `false` on timeout.
///
/// If the performance frequency is unknown, falls back to a conservative
/// iteration-count heuristic (`timeout_us * 10` loops).
pub fn spin_until<C: CpuInfo, F>(timeout_us: u64, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    if let Some(deadline_ticks) = us_to_ticks::<C>(timeout_us) {
        let start = ticks();
        loop {
            if condition() {
                return true;
            }
            if ticks().wrapping_sub(start) >= deadline_ticks {
                return false;
            }
            core::hint::spin_loop();
        }
    } else {
        // Fallback: iteration-count approximation.
        let iterations = timeout_us.saturating_mul(10);
        for _ in 0..iterations {
            if condition() {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCpu;
    impl CpuInfo for TestCpu {
        fn perf_timer_frequency() -> Option<u64> {
            None
        }
    }

    /// A CPU to make timer test happy.
    struct FixedFreqCpu;
    impl CpuInfo for FixedFreqCpu {
        fn perf_timer_frequency() -> Option<u64> {
            Some(1_000_000)
        }
    }

    #[test]
    fn test_us_to_ticks_basic() {
        // With a known 1 MHz frequency, 1000 us converts to exactly 1000 ticks.
        assert_eq!(us_to_ticks::<FixedFreqCpu>(1000), Some(1000));
    }

    #[test]
    fn test_spin_until_immediate_true() {
        let result = spin_until::<TestCpu, _>(1_000, || true);
        assert!(result);
    }
}
