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
