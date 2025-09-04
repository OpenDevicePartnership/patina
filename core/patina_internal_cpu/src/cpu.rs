//! UEFI CPU Module
//!
//! This module provides implementation for Cpu. The [EfiCpu] struct is the only accessible struct when using this
//! module. The other structs are architecture specific implementations and replace the [EfiCpu] struct at compile time
//! based on the target architecture.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "x86_64"))] {
        mod x64;
        pub type EfiCpu = x64::EfiCpuX64;
    } else if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        mod aarch64;
        pub type EfiCpu = aarch64::EfiCpuAarch64;
    } else if #[cfg(feature = "doc")] {
        mod x64;
        mod aarch64;
        mod null;
        pub use x64::EfiCpuX64;
        pub use aarch64::EfiCpuAarch64;
        pub use null::EfiCpuNull;

        /// Type alias whose implementation is [EfiCpuX64], [EfiCpuAarch64], or [EfiCpuNull] depending on the compilation target.
        ///
        /// This struct is for documentation purposes only. Please refer to the individual implementations for specific details.
        pub type EfiCpu = EfiCpuNull;
    } else {
        mod x64;
        mod aarch64;
        mod null;
        pub type EfiCpu = null::EfiCpuNull;
        pub use x64::EfiCpuX64;
        pub use aarch64::EfiCpuAarch64;
        pub use null::EfiCpuNull;
    }
}

use mu_pi::protocols::cpu_arch::{CpuFlushType, CpuInitType};
use patina_sdk::error::EfiError;
use r_efi::efi;

/// A trait to facilitate architecture-specific implementations.
/// TODO: This trait will be further broken down in future.
pub trait Cpu {
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
    fn flush_data_cache(
        &self,
        start: efi::PhysicalAddress,
        length: u64,
        flush_type: CpuFlushType,
    ) -> Result<(), EfiError>;

    /// Generates an INIT to the CPU.
    ///
    /// init_type          Type of CPU INIT to perform
    ///
    /// ## Errors
    ///
    /// Success       If CPU INIT occurred. This value should never be seen.
    /// DeviceError   If CPU INIT failed.
    /// Unsupported   Requested type of CPU INIT not supported.
    fn init(&self, init_type: CpuInitType) -> Result<(), EfiError>;

    /// Returns a timer value from one of the CPU's internal timers. There is no
    /// inherent time interval between ticks but is a function of the CPU frequency.
    ///
    /// timer_index          - Specifies which CPU timer is requested.
    ///
    /// ## Errors
    ///
    /// Success          - If the CPU timer count was returned.
    /// Unsupported      - If the CPU does not have any readable timers.
    /// DeviceError      - If an error occurred while reading the timer.
    /// InvalidParameter - timer_index is not valid or TimerValue is NULL.
    fn get_timer_value(&self, timer_index: u32) -> Result<(u64, u64), EfiError>;
}
