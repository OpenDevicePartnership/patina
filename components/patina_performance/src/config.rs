//! Patina Performance Component Configuration
//!
//! ## Performance Configuration Usage
//!
//! The configuration can be set statically with `.with_config()` or produced dynamically during boot.
//!
//! ## Static Configuration Example
//!
//! ```rust,ignore
//! // ...
//!
//! Core::default()
//! // ...
//!
//! .with_config(patina_performance::config::EnabledMeasurement(&[
//!        patina_sdk::performance::Measurement::DriverBindingStart,     // Adds driver binding start measurements.
//!        patina_sdk::performance::Measurement::DriverBindingStop,      // Adds driver binding stop measurements.
//!        patina_sdk::performance::Measurement::DriverBindingSupport,   // Adds driver binding support measurements.
//!        patina_sdk::performance::Measurement::LoadImage,              // Adds load image measurements.
//!        patina_sdk::performance::Measurement::StartImage,             // Adds start image measurements.
//!    ]))
//! .with_component(patina_performance::component::Performance)
//! .start()
//! .unwrap();
//!
//! // ...
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use patina_sdk::performance::Measurement;

/// A wrapper to generate a mask of all enabled measurements.
#[derive(Debug, Default)]
pub struct EnabledMeasurement(pub &'static [Measurement]);

impl EnabledMeasurement {
    /// Returns a mask of all enabled measurements.
    pub fn mask(&self) -> u32 {
        self.0.iter().fold(0, |mask, m| mask | m.as_u32())
    }

    /// Returns a static slice of all available measurements.
    pub const fn all() -> &'static [Measurement] {
        &[
            patina_sdk::performance::Measurement::DriverBindingStart,
            patina_sdk::performance::Measurement::DriverBindingStop,
            patina_sdk::performance::Measurement::DriverBindingSupport,
            patina_sdk::performance::Measurement::LoadImage,
            patina_sdk::performance::Measurement::StartImage,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_empty() {
        let enabled = EnabledMeasurement(&[]);
        assert_eq!(enabled.mask(), 0);
    }

    #[test]
    fn test_mask_single_measurement() {
        let enabled = EnabledMeasurement(&[Measurement::DriverBindingStart]);
        assert_eq!(enabled.mask(), Measurement::DriverBindingStart.as_u32());
    }

    #[test]
    fn test_mask_multiple_measurements() {
        let enabled = EnabledMeasurement(&[Measurement::DriverBindingStart, Measurement::DriverBindingStop]);
        let expected = Measurement::DriverBindingStart.as_u32() | Measurement::DriverBindingStop.as_u32();
        assert_eq!(enabled.mask(), expected);
    }

    #[test]
    fn test_mask_all_measurements() {
        let enabled = EnabledMeasurement(EnabledMeasurement::all());
        let mut expected = 0;
        for m in EnabledMeasurement::all() {
            expected |= m.as_u32();
        }
        assert_eq!(enabled.mask(), expected);
    }

    #[test]
    fn test_all_returns_expected_measurements() {
        let all = EnabledMeasurement::all();
        assert!(all.contains(&Measurement::DriverBindingStart));
        assert!(all.contains(&Measurement::DriverBindingStop));
        assert!(all.contains(&Measurement::DriverBindingSupport));
        assert!(all.contains(&Measurement::LoadImage));
        assert!(all.contains(&Measurement::StartImage));
        assert_eq!(all.len(), 5);
    }
}
