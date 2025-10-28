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
    IntoComponent,
    hob::{FromHob, Hob},
    params::ConfigMut,
};

/// Responsible for providing performance configuration information to other performance components.
#[derive(IntoComponent)]
pub struct PerformanceConfigurationProvider;

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
            config_mut.enabled_measurements = perf_config_hob.enabled_measurements;
        }

        log::trace!("Outgoing MM Configuration: {:?}", *config_mut);

        config_mut.lock();

        Ok(())
    }
}
