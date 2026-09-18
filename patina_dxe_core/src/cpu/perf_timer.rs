//! Performance timer implementation for DXE core and components.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU64, Ordering};

use patina::component::service::{IntoService, perf_timer::ArchTimerFunctionality};

/// Performance timer implementation.
#[derive(IntoService)]
#[service(dyn ArchTimerFunctionality)]
pub(crate) struct PerfTimer {
    frequency: AtomicU64,
}

impl ArchTimerFunctionality for PerfTimer {
    /// Value of the counter (ticks).
    #[cfg_attr(coverage, coverage(off))]
    fn cpu_count(&self) -> u64 {
        patina::arch::get_timer_value()
    }

    /// Frequency of `cpu_count` increments (in Hz).
    /// If a platform has provided a custom frequency via `PERF_FREQUENCY`, that value is used.
    /// Otherwise, an architecture-specific method is attempted to determine the frequency.
    fn perf_frequency(&self) -> u64 {
        if self.frequency.load(Ordering::Relaxed) == 0 {
            let frequency = patina::arch::get_timer_frequency().map_or(0, core::num::NonZero::get);
            self.frequency.store(frequency, Ordering::Relaxed);
        }
        self.frequency.load(Ordering::Relaxed)
    }
}

impl PerfTimer {
    /// Creates a new `PerfTimer` instance.
    pub const fn new() -> Self {
        Self { frequency: AtomicU64::new(0) }
    }

    /// Creates a new `PerfTimer` instance with a specified frequency.
    pub const fn with_frequency(frequency: u64) -> Self {
        Self { frequency: AtomicU64::new(frequency) }
    }

    /// Initializes the frequency of the performance timer to the specified value.
    pub(crate) fn set_frequency(&self, frequency: u64) {
        self.frequency.store(frequency, Ordering::Relaxed);
    }
}

impl Default for PerfTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_set_non_zero_frequency_forces_that_frequency() {
        let frequency = 19191919;
        let timer = PerfTimer::with_frequency(frequency);
        assert_eq!(timer.perf_frequency(), frequency);
    }

    #[test]
    fn test_zero_frequency_forces_arch_perf_frequency() {
        let expected = patina::arch::get_timer_frequency().map_or(0, std::num::NonZero::get);

        let timer = PerfTimer::default();
        assert_eq!(timer.perf_frequency(), expected);

        let timer = PerfTimer::new();
        assert_eq!(timer.perf_frequency(), expected);

        let timer = PerfTimer::with_frequency(0);
        assert_eq!(timer.perf_frequency(), expected);
    }
}
