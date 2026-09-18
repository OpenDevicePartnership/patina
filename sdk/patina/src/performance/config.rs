//! Performance measurement configuration.
//!
//! This module defines the configuration consumed by the performance measurement infrastructure. The configuration is
//! produced as a guided HOB prior to Patina DXE Core execution and read by the core to determine whether performance
//! measurement is enabled and which measurements to collect.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{BinaryGuid, component::hob::FromHob, performance::Measurement};

/// The configuration for performance measurement.
#[derive(Debug, Clone, Copy, zerocopy_derive::FromBytes)]
#[repr(C, packed)]
pub struct PerformanceConfig {
    /// Indicates whether performance measurement is enabled.
    pub enabled: u8,
    /// Bitmask of enabled measurements (see [`crate::performance::Measurement`]).
    pub enabled_measurements: u32,
}

impl PerformanceConfig {
    /// Constant value indicating that performance measurement is enabled.
    pub const ENABLED: u8 = 1;
    /// Constant value indicating that performance measurement is disabled.
    pub const DISABLED: u8 = 0;
    /// Constant value indicating that no performance measurements are enabled.
    pub const NO_MEASUREMENTS: u32 = 0;

    /// Creates a new `PerformanceConfig` that is disabled with no measurements.
    pub const fn new() -> Self {
        Self { enabled: Self::DISABLED, enabled_measurements: Self::NO_MEASUREMENTS }
    }

    /// Returns this configuration with performance measurement enabled and `measurement`
    /// added to the set of enabled  measurements. Intended for chaining from
    /// [`PerformanceConfig::new`] to build a configuration in a const context.
    #[must_use]
    pub const fn with_measurement(self, measurement: Measurement) -> Self {
        Self { enabled: Self::ENABLED, enabled_measurements: self.enabled_measurements | measurement.as_u32() }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl FromHob for PerformanceConfig {
    const HOB_GUID: BinaryGuid = BinaryGuid::from_string("fd87f2d8-112d-4640-9c00-d37d2a1fb75d");

    fn parse(bytes: &[u8]) -> Self {
        match <Self as zerocopy::FromBytes>::read_from_prefix(bytes) {
            Ok((config, _)) => config,
            Err(_) => panic!(
                "Guided Hob [{:#?}] parse failed. Buffer too small for type {}",
                Self::HOB_GUID,
                core::any::type_name::<Self>()
            ),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_performance_config_new_is_disabled_with_no_measurements() {
        let config = PerformanceConfig::new();
        assert_eq!(config.enabled, PerformanceConfig::DISABLED);
        assert_eq!({ config.enabled_measurements }, PerformanceConfig::NO_MEASUREMENTS);
    }

    #[test]
    fn test_performance_config_default_matches_new() {
        let config = PerformanceConfig::default();
        assert_eq!(config.enabled, PerformanceConfig::DISABLED);
        assert_eq!({ config.enabled_measurements }, PerformanceConfig::NO_MEASUREMENTS);
    }

    #[test]
    fn test_performance_config_with_measurement_enables_and_accumulates_mask() {
        let config =
            PerformanceConfig::new().with_measurement(Measurement::StartImage).with_measurement(Measurement::LoadImage);
        assert_eq!(config.enabled, PerformanceConfig::ENABLED);
        assert_eq!({ config.enabled_measurements }, Measurement::StartImage.as_u32() | Measurement::LoadImage.as_u32());
    }

    #[test]
    fn test_performance_config_parse_reads_packed_fields() {
        // Packed layout: enabled (u8) followed by enabled_measurements (u32, little-endian).
        let bytes: [u8; 5] = [PerformanceConfig::ENABLED, 0x0A, 0x00, 0x00, 0x00];
        let config = PerformanceConfig::parse(&bytes);
        assert_eq!(config.enabled, PerformanceConfig::ENABLED);
        assert_eq!({ config.enabled_measurements }, 0x0A);
    }

    #[test]
    fn test_performance_config_parse_ignores_trailing_bytes() {
        let bytes: [u8; 8] = [PerformanceConfig::DISABLED, 0x01, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF];
        let config = PerformanceConfig::parse(&bytes);
        assert_eq!(config.enabled, PerformanceConfig::DISABLED);
        assert_eq!({ config.enabled_measurements }, 0x01);
    }

    #[test]
    #[should_panic]
    fn test_performance_config_parse_panics_on_short_buffer() {
        let bytes: [u8; 2] = [PerformanceConfig::ENABLED, 0x00];
        let _ = PerformanceConfig::parse(&bytes);
    }
}
