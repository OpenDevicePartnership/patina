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
pub struct PerfTimer {
    frequency: AtomicU64,
}

impl ArchTimerFunctionality for PerfTimer {
    /// Value of the counter (ticks).
    fn cpu_count(&self) -> u64 {
        arch_cpu_count()
    }

    /// Frequency of `cpu_count` increments (in Hz).
    /// If a platform has provided a custom frequency via `PERF_FREQUENCY`, that value is used.
    /// Otherwise, an architecture-specific method is attempted to determine the frequency.
    fn perf_frequency(&self) -> u64 {
        if self.frequency.load(Ordering::Relaxed) == 0 {
            self.frequency.store(arch_perf_frequency(), Ordering::Relaxed);
        }
        self.frequency.load(Ordering::Relaxed)
    }
}

impl PerfTimer {
    /// Creates a new `PerfTimer` instance.
    pub fn new() -> Self {
        Self { frequency: AtomicU64::new(0) }
    }

    /// Creates a new `PerfTimer` instance with a specified frequency.
    pub fn with_frequency(frequency: u64) -> Self {
        Self { frequency: AtomicU64::new(frequency) }
    }

    /// Sets the performance frequency.
    pub fn set_frequency(&self, frequency: u64) {
        self.frequency.store(frequency, Ordering::Relaxed);
    }

    /// Gets the stored performance frequency.
    pub fn get_stored_frequency(&self) -> u64 {
        self.frequency.load(Ordering::Relaxed)
    }
}

impl Default for PerfTimer {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the current CPU count using architecture-specific methods.
///
/// Skip coverage as any value could be valid, including 0.
#[coverage(off)]
pub(crate) fn arch_cpu_count() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64;
        unsafe { x86_64::_rdtsc() }
    }
    #[cfg(target_arch = "aarch64")]
    {
        use aarch64_cpu::registers::{self, Readable};
        registers::CNTPCT_EL0.get()
    }
}

/// Returns the performance frequency using architecture-specific methods.
/// In general, the performance frequency is a configurable value that may be
/// provided by the platform. This function is a fallback when no
/// platform-specific configuration is provided.
///
/// Skip coverage as any value could be valid, including 0.
#[coverage(off)]
pub(crate) fn arch_perf_frequency() -> u64 {
    let mut frequency = 0u64;

    // Try to get TSC frequency from CPUID (most Intel and AMD platforms).
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::{x86_64, x86_64::CpuidResult};

        let CpuidResult { eax, ebx, ecx, .. } = unsafe { x86_64::__cpuid(0x15) };
        if eax != 0 && ebx != 0 && ecx != 0 {
            // CPUID 0x15 gives TSC_frequency = (ECX * EAX) / EBX.
            // Most modern x86 platforms support this leaf.
            frequency = (ecx as u64 * ebx as u64) / eax as u64;
        } else {
            // CPUID 0x16 gives base frequency in MHz in EAX.
            // This is supported on some older x86 platforms.
            // This is a nominal frequency and is less accurate for reflecting actual operating conditions.
            let CpuidResult { eax, .. } = unsafe { x86_64::__cpuid(0x16) };
            if eax != 0 {
                frequency = (eax * 1_000_000) as u64;
            }
        }
    }

    // Use CNTFRQ_EL0 for aarch64 platforms.
    #[cfg(target_arch = "aarch64")]
    {
        use patina::read_sysreg;
        frequency = read_sysreg!(CNTFRQ_EL0);
    }

    frequency
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use patina::component::service::perf_timer::ArchTimerFunctionality as _;

    use crate::perf_timer::PerfTimer;

    #[test]
    fn test_set_frequency() {
        let frequency = 19191919;
        let timer = PerfTimer::new();
        assert_eq!(timer.get_stored_frequency(), 0);
        timer.set_frequency(frequency);
        assert_eq!(timer.perf_frequency(), 19191919);

        let frequency = 20252025;
        let timer = PerfTimer::with_frequency(frequency);
        assert_eq!(timer.get_stored_frequency(), frequency);

        let timer = PerfTimer::default();
        assert_eq!(timer.get_stored_frequency(), 0);
    }
}
