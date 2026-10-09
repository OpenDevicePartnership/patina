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
use core::arch::x86_64::{__cpuid, CpuidResult};

const CPUID_TIME_STAMP_COUNTER: u32 = 0x15;
const CPUID_PROCESSOR_FREQUENCY: u32 = 0x16;

// TODO: This is copied from perf_timer.rs in patina_dxe_core
/// Returns the current CPU count using architecture-specific methods.
///
/// Skip coverage as any value could be valid, including 0.
#[cfg_attr(coverage, coverage(off))]
fn ticks() -> u64 {
    // SAFETY: _rdtsc only reads the TSC on x86_64. No invariants are required for safety.
    unsafe { x86_64::_rdtsc() }
}

/// Converts a duration in microseconds to the equivalent tick count using the
/// CPU's detected performance-counter frequency (via CPUID).
///
/// Returns `None` when the CPU reports no usable counter frequency.
#[inline]
pub fn us_to_ticks(us: u64) -> Option<u64> {
    us_to_ticks_at(us, arch_perf_frequency())
}

/// Converts `us` microseconds to ticks of a counter running at `freq` Hz.
fn us_to_ticks_at(us: u64, freq: u64) -> Option<u64> {
    if freq == 0 {
        return None;
    }
    Some(((u128::from(freq) * u128::from(us)) / 1_000_000) as u64)
}

/// Returns the frequency of the counter [`ticks`] reads, in Hz.
///
/// Leaf 0x15 gives the TSC frequency directly and is supported by most modern Intel and AMD
/// platforms; leaf 0x16 is the less accurate nominal fallback for older ones. Returns 0 when
/// neither enumerates a frequency, which makes [`us_to_ticks`] report `None` and leaves
/// [`spin_until`] on its iteration-count heuristic.
pub(crate) fn arch_perf_frequency() -> u64 {
    // Leaf 0x15 is supported by most modern Intel and AMD platforms; leaf 0x16 is the
    // less accurate fallback for older ones. It is only queried when 0x15 comes up empty
    // so the common path issues a single (serializing) CPUID.
    tsc_frequency(__cpuid(CPUID_TIME_STAMP_COUNTER))
        .unwrap_or_else(|| base_frequency(__cpuid(CPUID_PROCESSOR_FREQUENCY)))
}

/// Derives the TSC frequency in Hz from CPUID leaf 0x15, where it is reported as the
/// ratio `ECX * EBX / EAX`.
///
/// Returns `None` when any term is zero, meaning the CPU does not enumerate the leaf.
fn tsc_frequency(leaf: CpuidResult) -> Option<u64> {
    if leaf.eax == 0 || leaf.ebx == 0 || leaf.ecx == 0 {
        return None;
    }

    Some((u64::from(leaf.ecx) * u64::from(leaf.ebx)) / u64::from(leaf.eax))
}

/// Derives the nominal base frequency in Hz from CPUID leaf 0x16, where it is reported
/// in MHz in `EAX`.
///
/// This is a nominal frequency and is less accurate for reflecting actual operating
/// conditions. Returns 0 when the CPU does not enumerate the leaf.
fn base_frequency(leaf: CpuidResult) -> u64 {
    u64::from(leaf.eax) * 1_000_000
}

/// Spins until at least `timeout_us` microseconds have elapsed.
///
/// Returns `true` when the provided `condition` closure returns `true`
/// before the deadline, or `false` on timeout.
///
/// If the performance frequency is unknown, falls back to a conservative
/// iteration-count heuristic (`timeout_us * 10` loops).
pub fn spin_until<F>(timeout_us: u64, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    let Some(deadline_ticks) = us_to_ticks(timeout_us) else {
        return spin_iterations(timeout_us, condition);
    };

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
}

/// Polls `condition` for an iteration count approximating `timeout_us`, used when the CPU
/// reports no usable counter frequency.
fn spin_iterations<F>(timeout_us: u64, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    let iterations = timeout_us.saturating_mul(10);
    for _ in 0..iterations {
        if condition() {
            return true;
        }
        core::hint::spin_loop();
    }

    false
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    fn leaf(eax: u32, ebx: u32, ecx: u32) -> CpuidResult {
        CpuidResult { eax, ebx, ecx, edx: 0 }
    }

    #[test]
    fn test_tsc_frequency_from_cpuid_ratio() {
        // A 24 MHz crystal with a 100/1 ratio yields a 2.4 GHz counter.
        assert_eq!(tsc_frequency(leaf(1, 100, 24_000_000)), Some(2_400_000_000));
    }

    #[test]
    fn test_tsc_frequency_requires_every_term() {
        // A zero in any term means the CPU does not enumerate leaf 0x15.
        assert_eq!(tsc_frequency(leaf(0, 100, 24_000_000)), None);
        assert_eq!(tsc_frequency(leaf(1, 0, 24_000_000)), None);
        assert_eq!(tsc_frequency(leaf(1, 100, 0)), None);
    }

    #[test]
    fn test_base_frequency_scales_mhz_to_hz() {
        assert_eq!(base_frequency(leaf(0, 0, 0)), 0);
        assert_eq!(base_frequency(leaf(2_400, 0, 0)), 2_400_000_000);
        // Above 4294 MHz the conversion overflows if it is done in 32-bit arithmetic.
        assert_eq!(base_frequency(leaf(5_000, 0, 0)), 5_000_000_000);
    }

    #[test]
    fn test_arch_perf_frequency_queries_the_host_cpu() {
        // The value depends on the host CPU, but the query must not panic.
        let _ = arch_perf_frequency();
    }

    #[test]
    fn test_us_to_ticks_at_a_known_frequency() {
        // At 1 MHz a tick is exactly one microsecond.
        assert_eq!(us_to_ticks_at(1_000, 1_000_000), Some(1_000));
        assert_eq!(us_to_ticks_at(1_000, 2_400_000_000), Some(2_400_000));
        // Sub-microsecond resolution truncates rather than rounding.
        assert_eq!(us_to_ticks_at(0, 2_400_000_000), Some(0));
        // The intermediate product is computed in 128 bits, so a long timeout at a high
        // frequency does not overflow.
        assert_eq!(us_to_ticks_at(u64::MAX / 1_000_000, 1_000_000), Some(u64::MAX / 1_000_000));
    }

    #[test]
    fn test_us_to_ticks_at_an_unknown_frequency() {
        assert_eq!(us_to_ticks_at(1_000, 0), None);
    }

    #[test]
    fn test_us_to_ticks_follows_the_frequency_the_cpu_reports() {
        // A virtualized host need not enumerate the CPUID frequency leaves at all, in
        // which case no conversion is possible and `None` is the correct answer.
        match arch_perf_frequency() {
            0 => assert_eq!(us_to_ticks(1_000), None),
            freq => assert_eq!(us_to_ticks(1_000), Some(((u128::from(freq) * 1_000) / 1_000_000) as u64)),
        }
    }

    #[test]
    fn test_spin_until_immediate_true() {
        let result = spin_until(1_000, || true);
        assert!(result);
    }

    #[test]
    fn test_spin_until_reports_a_satisfied_condition_after_polling() {
        let mut polls = 0;
        assert!(spin_until(1_000_000, || {
            polls += 1;
            polls == 5
        }));
        assert_eq!(polls, 5);
    }

    #[test]
    fn test_spin_until_times_out() {
        // A condition that never holds must return once the deadline passes.
        assert!(!spin_until(1, || false));
    }

    #[test]
    fn test_spin_iterations_stops_on_the_condition() {
        let mut polls = 0;
        assert!(spin_iterations(10, || {
            polls += 1;
            polls == 3
        }));
        assert_eq!(polls, 3);
    }

    #[test]
    fn test_spin_iterations_gives_up_after_its_budget() {
        let mut polls = 0;
        assert!(!spin_iterations(10, || {
            polls += 1;
            false
        }));
        // The budget is ten iterations per microsecond.
        assert_eq!(polls, 100);
    }

    #[test]
    fn test_spin_iterations_with_no_budget() {
        assert!(!spin_iterations(0, || panic!("condition must not be polled")));
        // A timeout large enough to overflow the budget saturates instead of wrapping to zero.
        let mut polled = false;
        assert!(spin_iterations(u64::MAX, || {
            polled = true;
            true
        }));
        assert!(polled);
    }
}
