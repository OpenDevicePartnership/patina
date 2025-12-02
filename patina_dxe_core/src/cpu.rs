//! DXE Core subsystem for all CPU-related functionality.
//!
//! This subsystem is responsible for initializing the CPU and managing CPU-specific functionality
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
mod cpu_arch_protocol;
#[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
mod hw_interrupt_protocol;
mod perf_timer;

pub(crate) use cpu_arch_protocol::CpuArchProtocolInstaller;
#[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
pub(crate) use hw_interrupt_protocol::HwInterruptProtocolInstaller;
pub(crate) use perf_timer::PerfTimer;

use patina_internal_cpu::{cpu::EfiCpu, interrupts::Interrupts};

/// A configuration struct containing the GIC bases (gic_d, gic_r) for AARCH64 systems.
///
/// ## Invariants
///
/// - `self.0` (GIC Distributor Base) points to the GIC Distributor register space.
/// - `self.1` (GIC Redistributor Base) points to the GIC Redistributor register space.
/// - Access to these registers are exclusive to this GicBases instance.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
///
/// struct PlatformConfig;
/// # impl ComponentInfo for PlatformConfig {}
/// # impl MemoryInfo for PlatformConfig {}
///
/// impl CpuInfo for PlatformConfig {
///   #[cfg(target_arch = "aarch64")]
///   fn gic_bases() -> GicBases {
///     /// SAFETY: gicd and gicr bases correctly point to the register spaces.
///     /// SAFETY: Access to these registers is exclusive to this struct instance.
///     unsafe { GicBases::new(0x1E000000, 0x1E010000) }
///   }
/// }
///
/// # impl PlatformInfo for PlatformConfig {
/// #   type MemoryInfo = Self;
/// #   type Extractor = patina_ffs_extractors::NullSectionExtractor;
/// #   type ComponentInfo = Self;
/// #   type CpuInfo = Self;
/// # }
/// ```
#[derive(Debug, PartialEq)]
pub struct GicBases {
    /// The GIC Distributor base address.
    pub(crate) gicd: u64,
    /// The GIC Redistributor base address.
    pub(crate) gicr: u64,
}

impl GicBases {
    /// Creates a new instance of the GicBases struct with the provided GIC Distributor and Redistributor base addresses.
    ///
    /// ## Safety
    ///
    /// `gicd_base` must point to the GIC Distributor register space.
    ///
    /// `gicr_base` must point to the GIC Redistributor register space.
    ///
    /// Access to these registers are exclusive to this GicBases instance.
    ///
    /// Caller must guarantee that access to these registers is exclusive to this GicBases instance.
    #[coverage(off)]
    pub unsafe fn new(gicd_base: u64, gicr_base: u64) -> Self {
        GicBases { gicd: gicd_base, gicr: gicr_base }
    }
}

/// A trait to be implemented by the platform to provide configuration values and types related to the CPU.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
///
/// struct ExamplePlatform;
///
/// impl CpuInfo for ExamplePlatform {
///   #[cfg(target_arch = "aarch64")]
///   fn gic_bases() -> GicBases {
///     /// SAFETY: gicd and gicr bases correctly point to the register spaces.
///     /// SAFETY: Access to these registers is exclusive to this struct instance.
///     unsafe { GicBases::new(0x1E000000, 0x1E010000) }
///   }
/// }
/// ```
#[cfg_attr(test, mockall::automock)]
pub trait CpuInfo {
    /// Informs the core of the GIC base addresses for AARCH64 systems.
    #[cfg(target_arch = "aarch64")]
    fn gic_bases() -> GicBases;

    /// Returns the performance timer frequency for the platform.
    ///
    /// By default, this returns `None`, indicating that the core should attempt to determine the frequency
    /// automatically using cpu architecture-specific methods.
    #[inline(always)]
    fn perf_timer_frequency() -> Option<u64> {
        None
    }
}

#[coverage(off)]
pub fn initialize_cpu_subsystem() -> crate::error::Result<(EfiCpu, Interrupts)> {
    let mut cpu = EfiCpu::default();
    cpu.initialize().inspect_err(|err| {
        log::error!("Failed to initialize CPU subsystem: {:?}", err);
    })?;

    let mut interrupt_manager = Interrupts::new();
    interrupt_manager.initialize().inspect_err(|err| {
        log::error!("Failed to initialize Interrupt Manager: {:?}", err);
    })?;

    Ok((cpu, interrupt_manager))
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_info_trait_defaults_do_not_change() {
        // A simple test to acknowledge that the default implementations of the CpuInfo trait should not change without
        // a conscious decision, which requires updating this test.
        struct TestPlatform;

        impl CpuInfo for TestPlatform {
            #[cfg(target_arch = "aarch64")]
            fn gic_bases() -> GicBases {
                unsafe { GicBases::new(0, 0) }
            }
        }

        assert!(<TestPlatform as CpuInfo>::perf_timer_frequency().is_none());
    }
}
