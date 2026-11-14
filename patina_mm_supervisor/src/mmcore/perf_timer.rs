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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    fn hz(freq: u64) -> NonZeroU64 {
        NonZeroU64::new(freq).expect("a test frequency is not zero")
    }

    #[test]
    fn test_us_to_ticks_at_a_known_frequency() {
        // At 1 MHz a tick is exactly one microsecond.
        assert_eq!(us_to_ticks_at(1_000, hz(1_000_000)), 1_000);
        assert_eq!(us_to_ticks_at(1_000, hz(2_400_000_000)), 2_400_000);
        // Sub-microsecond resolution truncates rather than rounding.
        assert_eq!(us_to_ticks_at(0, hz(2_400_000_000)), 0);
        // The intermediate product is computed in 128 bits, so a long timeout at a high
        // frequency does not overflow.
        assert_eq!(us_to_ticks_at(u64::MAX / 1_000_000, hz(1_000_000)), u64::MAX / 1_000_000);
    }

    #[test]
    fn test_us_to_ticks_follows_the_frequency_the_cpu_reports() {
        // A virtualized host need not enumerate the CPUID frequency leaves at all, and host
        // builds use the SDK's stub, in which case no conversion is possible and `None` is the
        // correct answer.
        match patina::arch::get_timer_frequency() {
            None => assert_eq!(us_to_ticks(1_000), None),
            Some(freq) => assert_eq!(us_to_ticks(1_000), Some(((u128::from(freq.get()) * 1_000) / 1_000_000) as u64)),
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
    fn test_spin_ticks_stops_on_the_condition() {
        let mut polls = 0;
        assert!(spin_ticks(
            100,
            || 0,
            || {
                polls += 1;
                polls == 3
            }
        ));
        assert_eq!(polls, 3);
    }

    #[test]
    fn test_spin_ticks_times_out_at_the_deadline() {
        let mut clock = 0_u64;
        let mut polls = 0;
        assert!(!spin_ticks(
            30,
            || {
                clock += 10;
                clock
            },
            || {
                polls += 1;
                false
            }
        ));
        // The first reading is 10, so the deadline passes on the third poll, at 40.
        assert_eq!(polls, 3);
    }

    #[test]
    fn test_spin_ticks_handles_a_counter_that_wraps() {
        let mut clock = u64::MAX - 5;
        let mut polls = 0;
        assert!(!spin_ticks(
            10,
            || {
                clock = clock.wrapping_add(4);
                clock
            },
            || {
                polls += 1;
                false
            }
        ));
        // The first reading is u64::MAX - 1. The counter then wraps to 2, 6 and 10, so the elapsed
        // ticks are 4, 8 and 12, and the deadline passes on the third poll.
        assert_eq!(polls, 3);
        assert_eq!(clock, 10);
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
