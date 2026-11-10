//! Performance Timer for the MM Supervisor Core
//!
//! TSC-based timeouts for the mailbox, AP arrival, and AP exit polling. The counter and its
//! frequency come from `patina::arch`, which the DXE core's perf timer also uses.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::num::NonZeroU64;

/// Converts a duration in microseconds to the equivalent tick count at the counter frequency the
/// CPU reports.
///
/// Returns `None` when the CPU reports no usable counter frequency.
pub fn us_to_ticks(us: u64) -> Option<u64> {
    patina::arch::get_timer_frequency().map(|freq| us_to_ticks_at(us, freq))
}

/// Converts `us` microseconds to ticks of a counter running at `freq` Hz.
fn us_to_ticks_at(us: u64, freq: NonZeroU64) -> u64 {
    ((u128::from(freq.get()) * u128::from(us)) / 1_000_000) as u64
}

/// Spins until `condition` returns `true` or `timeout_us` microseconds pass.
///
/// Returns `true` if the condition was met before the deadline and `false` on timeout. Without
/// a known counter frequency it polls `timeout_us * 10` times instead.
pub fn spin_until<F>(timeout_us: u64, condition: F) -> bool
where
    F: FnMut() -> bool,
{
    match us_to_ticks(timeout_us) {
        Some(deadline_ticks) => spin_ticks(deadline_ticks, patina::arch::get_timer_value, condition),
        None => spin_iterations(timeout_us, condition),
    }
}

/// Polls `condition` until it holds or `now` has advanced `deadline_ticks` past its first reading.
fn spin_ticks<C, F>(deadline_ticks: u64, mut now: C, mut condition: F) -> bool
where
    C: FnMut() -> u64,
    F: FnMut() -> bool,
{
    let start = now();
    loop {
        if condition() {
            return true;
        }
        if now().wrapping_sub(start) >= deadline_ticks {
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
