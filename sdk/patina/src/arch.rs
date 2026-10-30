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
        use self::stub as arch_impl;

        // Republish the compiled architecture stub module so that `patina::arch::aarch64` /
        // `patina::arch::x64` paths resolve on host builds.
        #[doc(hidden)]
        pub use stub::*;
    } else if #[cfg(target_arch = "aarch64")] {
        #[cfg_attr(coverage, coverage(off))] // Architecture code cannot be unit tested.
        pub mod aarch64;
        use self::aarch64 as arch_impl;
    } else if #[cfg(target_arch = "x86_64")] {
        #[cfg_attr(coverage, coverage(off))] // Architecture code cannot be unit tested.
        pub mod x64;
        use self::x64 as arch_impl;
    }
}

pub use arch_impl::{
    cache_writeback_granule, enable_interrupts_and_sleep, flush_data_cache, get_timer_frequency, get_timer_value,
    interrupts_enabled, is_cc_guest,
};

// `enable_interrupts` and `disable_interrupts` are crate-private in the architecture modules so
// that they are only reachable outside this crate when the `core` feature is enabled.

/// Enables CPU interrupts on the current architecture. This is only exposed to core modules
/// due to its inherent risk of misuse. Most components should use TPLs to manage execution priority.
#[cfg(feature = "core")]
pub fn enable_interrupts() {
    arch_impl::enable_interrupts();
}

/// Disables CPU interrupts on the current architecture. This is only exposed to core modules
/// due to its inherent risk of misuse. Most components should use TPLs to manage execution priority.
#[cfg(feature = "core")]
pub fn disable_interrupts() {
    arch_impl::disable_interrupts();
}

/// Executes a closure with CPU interrupts disabled, interrupts will be restored to their
/// previous state after the closure is executed.
///
/// ## Important
///
/// This function is only intended for low-level operations, and should not be used in
/// components. Components should use the TPL (Task Priority Level) to manage interrupts.
pub(crate) fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let enabled = arch_impl::interrupts_enabled();
    if enabled {
        arch_impl::disable_interrupts();
    }
    let result = f();
    if enabled {
        arch_impl::enable_interrupts();
    }
    result
}
