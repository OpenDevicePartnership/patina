//! Arch-specific timer functionality
//! By default, this module attempts to determine the timer frequency via architecture specific methods.
//! (cpuid for x86, `CNTFRQ_EL0` for aarch64)
//!
//! Platforms can override this with a custom performance frequency by providing the Core with the correct frequency:
//!
//! <!-- (The below test has to be ignore because `patna` cannot depend on `patina_dxe_core` - circular dependency.) -->
//! ```rust,ignore
//!     let frequency_hz: u64 = 1_000_000_000; // Compute with platform-specific methods.
//!
//!     Core::default()
//!        .init_timer_frequency(Some(frequency_hz))
//!```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Trait that provides architecture-specific timer functionality.
/// Components that need timing functionality can request this service.
pub trait ArchTimerFunctionality: Send + Sync {
    /// Value of the counter (ticks).
    fn cpu_count(&self) -> u64;
    /// Value in Hz of how often the counter increment.
    fn perf_frequency(&self) -> u64;
    /// Value that the performance counter starts with.
    fn cpu_count_start(&self) -> u64 {
        0
    }
    /// Value that the performance counter ends with before it rolls over.
    fn cpu_count_end(&self) -> u64 {
        u64::MAX
    }
}
