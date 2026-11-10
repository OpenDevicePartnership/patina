//! Performance Timer for the MM Supervisor Core
//!
//! TSC-based timeouts for the mailbox, AP arrival and AP exit polling, using the counter and
//! frequency from `patina::arch`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::num::NonZeroU64;

/// Converts microseconds to ticks at the counter frequency the CPU reports, or returns `None`
/// when the CPU reports no usable frequency.
pub fn us_to_ticks(us: u64) -> Option<u64> {
    patina::arch::get_timer_frequency().map(|freq| us_to_ticks_at(us, freq))
}

/// Converts `us` microseconds to ticks of a counter running at `freq` Hz.
fn us_to_ticks_at(us: u64, freq: NonZeroU64) -> u64 {
    ((u128::from(freq.get()) * u128::from(us)) / 1_000_000) as u64
}

/// Spins until `condition` returns `true` or `timeout_us` microseconds pass, and returns whether
/// the condition was met. Without a known counter frequency it polls `timeout_us * 10` times
/// instead.
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

/// Polls `condition` `timeout_us * 10` times, for when the CPU reports no usable counter
/// frequency.
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
