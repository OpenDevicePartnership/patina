//! Arch Specific abstractions for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

cfg_if::cfg_if! {
    if #[cfg(not(target_os = "uefi"))] {
        #[cfg_attr(coverage, coverage(off))]
        pub mod stub;
        use stub as Arch;

        // Republish the compiled architecture stub module so that `patina::arch::aarch64` /
        // `patina::arch::x64` paths resolve on host builds.
        #[doc(hidden)]
        pub use stub::*;
    } else if #[cfg(target_arch = "aarch64")] {
        #[cfg_attr(coverage, coverage(off))] // Architecture code cannot be unit tested.
        pub mod aarch64;
        use aarch64 as Arch;
    } else if #[cfg(target_arch = "x86_64")] {
        #[cfg_attr(coverage, coverage(off))] // Architecture code cannot be unit tested.
        pub mod x64;
        use x64 as Arch;
    }
}

// Functions that every architecture provides, re-exported so architecture neutral code can call
// `patina::arch::flush_tlb()` instead of repeating a `target_arch` check at every call site.
// Architecture-specific helpers stay under `patina::arch::x64` or `patina::arch::aarch64`.
pub use Arch::{
    breakpoint, cache_writeback_granule, disable_interrupts, enable_interrupts, enable_interrupts_and_sleep,
    flush_data_cache, flush_tlb, get_timer_frequency, get_timer_value, interrupts_enabled,
};

pub(crate) use Arch::with_interrupts_disabled;
