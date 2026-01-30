//! Patina Performance Configuration Provider
//!
//! Produces dynamic performance configuration for performance in Patina.
//!
//! This is an optional component that can be used if Patina performance needs to be configured dynamically at runtime.
//!
//! At this time, it transfers configuration information from a HOB to configuration that is passed to any
//! components that depend on performance configuration.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use crate::config;
use patina::component::{
    component,
    hob::{FromHob, Hob},
    params::ConfigMut,
};

/// Responsible for providing performance configuration information to other performance components.
pub struct PerformanceConfigurationProvider;

/// Converts EDK II's PcdPerformanceLibraryPropertyMask format to Patina's enabled_measurements format.
///
/// EDK II uses "disable bits" (BIT1-6 set = disabled), while Patina uses "enable bits" (bit set = enabled).
///
/// ## EDK II Format (PcdPerformanceLibraryPropertyMask)
/// - BIT0: Enable Performance Measurement (master switch, not used here - handled by enable_component)
/// - BIT1: DISABLE Start Image Logging
/// - BIT2: DISABLE Load Image logging
/// - BIT3: DISABLE Binding Support logging
/// - BIT4: DISABLE Binding Start logging
/// - BIT5: DISABLE Binding Stop logging
/// - BIT6: DISABLE all other general Perfs
///
/// ## Patina Format (enabled_measurements)
/// - BIT0 (1): ENABLE StartImage
/// - BIT1 (2): ENABLE LoadImage
/// - BIT2 (4): ENABLE DriverBindingSupport
/// - BIT3 (8): ENABLE DriverBindingStart
/// - BIT4 (16): ENABLE DriverBindingStop
///
/// ## Example
/// - EDK II `0x01` (only BIT0 set, no disable bits) → Patina `0x1F` (all enabled)
/// - EDK II `0x03` (BIT0 + BIT1) → Patina `0x1E` (StartImage disabled)
fn convert_edk2_mask_to_patina(edk2_mask: u32) -> u32 {
    use patina::performance::Measurement;

    let mut patina_mask: u32 = 0;

    // EDK II BIT1 = DISABLE StartImage → if NOT set, enable in Patina
    if edk2_mask & (1 << 1) == 0 {
        patina_mask |= Measurement::StartImage as u32;
    }
    // EDK II BIT2 = DISABLE LoadImage
    if edk2_mask & (1 << 2) == 0 {
        patina_mask |= Measurement::LoadImage as u32;
    }
    // EDK II BIT3 = DISABLE BindingSupport
    if edk2_mask & (1 << 3) == 0 {
        patina_mask |= Measurement::DriverBindingSupport as u32;
    }
    // EDK II BIT4 = DISABLE BindingStart
    if edk2_mask & (1 << 4) == 0 {
        patina_mask |= Measurement::DriverBindingStart as u32;
    }
    // EDK II BIT5 = DISABLE BindingStop
    if edk2_mask & (1 << 5) == 0 {
        patina_mask |= Measurement::DriverBindingStop as u32;
    }

    patina_mask
}

/// A HOB that contains Patina Performance component configuration information.
///
/// HOB GUID values for reference:
/// - `{0xfd87f2d8, 0x112d, 0x4640, {0x9c, 0x00, 0xd3, 0x7d, 0x2a, 0x1f, 0xb7, 0x5d}}``
/// - `{fd87f2d8-112d-4640-9c00-d37d2a1fb75d}``
#[derive(FromHob, zerocopy_derive::FromBytes)]
#[hob = "fd87f2d8-112d-4640-9c00-d37d2a1fb75d"]
#[repr(C, packed)]
pub struct PerformanceConfigHob {
    /// Indicates whether the Patina Performance component is enabled.
    enable_component: u8,
    /// The enabled measurements for the Patina Performance component.
    ///
    /// This is a bitmask of `Measurement` values that indicate which performance measurements are enabled. The
    /// bits correspond to the [`patina::performance::Measurement`] enum values.
    enabled_measurements: u32,
}

#[component]
impl PerformanceConfigurationProvider {
    /// Entry point for the Patina Performance Configuration Provider.
    ///
    /// ## Parameters
    ///
    /// - `perf_config_hob`: A HOB that contains platform configuration for the Patina Performance component.
    /// - `config_mut`: A mutable reference to the Patina Performance Config instance to be populated with runtime
    ///   information.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if the entry point was successful.
    /// - `Err(patina::error::Result)` if the entry point failed.
    ///
    pub fn entry_point(
        self,
        perf_config_hob: Hob<PerformanceConfigHob>,
        mut config_mut: ConfigMut<config::PerfConfig>,
    ) -> patina::error::Result<()> {
        log::trace!("Patina Performance Configuration Provider Entry Point");

        log::trace!("Incoming Patina Performance Component Configuration: {:?}", *config_mut);

        config_mut.enable_component = perf_config_hob.enable_component != 0;
        if !config_mut.enable_component {
            log::trace!("The Patina Performance component is disabled per HOB configuration.");
        } else {
            log::trace!("The Patina Performance component is enabled per HOB configuration.");
            // Convert EDK II "disable bits" format to Patina "enable bits" format
            config_mut.enabled_measurements = convert_edk2_mask_to_patina(perf_config_hob.enabled_measurements);
        }

        log::trace!("Outgoing MM Configuration: {:?}", *config_mut);

        config_mut.lock();

        Ok(())
    }
}
