//! Arch Specific abstractions for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{error::EfiError, pi::protocols::cpu_arch::CpuFlushType};
use core::num::NonZeroU64;
use r_efi::efi;

cfg_if::cfg_if! {
    if #[cfg(test)] {
        #[cfg_attr(coverage, coverage(off))]
        mod null;
        type Arch = null::NullArch;
    } else if #[cfg(target_arch = "aarch64")] {
        #[cfg_attr(coverage, coverage(off))] // Architecture code cannot be unit tested.
        pub mod aarch64;
        type Arch = aarch64::AArch64;
    } else if #[cfg(target_arch = "x86_64")] {
        #[cfg_attr(coverage, coverage(off))] // Architecture code cannot be unit tested.
        pub mod x64;
        type Arch = x64::X64;
    }
}

#[allow(unused)]
/// A trait for architecture-specific functionality. It inherits from subtraits
/// for code organization.
trait ArchSupport: Interrupts + CacheMgmt + Timer {}

//
// Interrupts support.
//
// Only `with_interrupts_disabled` is being exposed publically for now to
// avoid misuse of direct interrupt manipulation.
//

trait Interrupts {
    fn enable_interrupts();
    fn disable_interrupts();
    fn interrupts_enabled() -> bool;

    /// Causes the CPU to enter a low power state until the next interrupt.
    fn enable_interrupts_and_sleep();
}

/// Enables CPU interrupts on the current architecture.
fn enable_interrupts() {
    <Arch as Interrupts>::enable_interrupts();
}

/// Disables CPU interrupts on the current architecture.
fn disable_interrupts() {
    <Arch as Interrupts>::disable_interrupts();
}

/// Returns whether CPU interrupts are currently enabled on the current architecture.
pub fn interrupts_enabled() -> bool {
    <Arch as Interrupts>::interrupts_enabled()
}

/// Causes the current architecture's CPU to enter a low power state until the next interrupt.
pub fn enable_interrupts_and_sleep() {
    <Arch as Interrupts>::enable_interrupts_and_sleep()
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
    let enabled = interrupts_enabled();
    if enabled {
        disable_interrupts();
    }
    let result = f();
    if enabled {
        enable_interrupts();
    }
    result
}

/// Architecture-specific cache management operations.
///
/// This trait is implemented by the per-architecture zero-sized `Arch` type. Callers should use
/// the free functions in this module rather than referencing the trait directly.
trait CacheMgmt {
    /// Flush CPU data cache. If the instruction cache is fully coherent
    /// with all DMA operations then function can just return Success.
    ///
    /// start             Physical address to start flushing from.
    /// length            Number of bytes to flush. Round up to chipset granularity.
    /// flush_type        Specifies the type of flush operation to perform.
    ///
    /// ## Errors
    ///
    /// Success       If cache was flushed
    /// Unsupported   If flush type is not supported.
    /// DeviceError   If requested range could not be flushed.
    fn flush_data_cache(start: efi::PhysicalAddress, length: u64, flush_type: CpuFlushType) -> Result<(), EfiError>;

    /// Returns the cache writeback granule size in bytes.
    ///
    /// This value is used to populate the `dma_buffer_alignment` field in the
    /// `EFI_CPU_ARCH_PROTOCOL`. DMA buffer allocations must be aligned to this
    /// boundary to prevent cache coherency issues where a writeback of an
    /// adjacent dirty cache line could corrupt DMA data.
    fn cache_writeback_granule() -> u32;
}

/// Architecture-specific timer operations.
///
/// This trait is implemented by the per-architecture zero-sized `Arch` type. Callers should use
/// the free functions in this module rather than referencing the trait directly.
trait Timer {
    /// Returns the current value of the CPU's timer counter, in ticks. There is no
    /// inherent time interval between ticks; it is a function of the CPU frequency.
    fn get_timer_value() -> u64;

    /// Returns the frequency of the CPU's counter, in Hz, or `None` if it cannot be determined.
    fn get_timer_frequency() -> Option<NonZeroU64>;
}

/// Flushes a range of the current architecture's CPU data cache.
pub fn flush_data_cache(start: efi::PhysicalAddress, length: u64, flush_type: CpuFlushType) -> Result<(), EfiError> {
    <Arch as CacheMgmt>::flush_data_cache(start, length, flush_type)
}

/// Returns the current value of the current architecture's timer counter, in ticks.
pub fn get_timer_value() -> u64 {
    <Arch as Timer>::get_timer_value()
}

/// Returns the frequency of the current architecture's timer counter, in Hz, or `None` if it
/// cannot be determined. On some platforms, this is expected to fail and the Timer Service should be queried instead.
pub fn get_timer_frequency() -> Option<NonZeroU64> {
    <Arch as Timer>::get_timer_frequency()
}

/// Returns the cache writeback granule size in bytes for the current architecture.
pub fn cache_writeback_granule() -> u32 {
    <Arch as CacheMgmt>::cache_writeback_granule()
}
