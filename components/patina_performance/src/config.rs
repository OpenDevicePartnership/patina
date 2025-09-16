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
//! .with_config(patina_performance::config::PerfConfig {
//!     enable_component: true,
//!     enabled_measurements: {
//!        patina_sdk::performance::Measurement::DriverBindingStart         // Adds driver binding start measurements.
//!        | patina_sdk::performance::Measurement::DriverBindingStop        // Adds driver binding stop measurements.
//!        | patina_sdk::performance::Measurement::DriverBindingSupport     // Adds driver binding support measurements.
//!        | patina_sdk::performance::Measurement::LoadImage                // Adds load image measurements.
//!        | patina_sdk::performance::Measurement::StartImage               // Adds start image measurements.
//!     }
//! })
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
//! SPDX-License-Identifier: Apache-2.0
//!

/// Default: component disabled unless explicitly enabled by platform/HOB.
pub const DEFAULT_ENABLE_COMPONENT: bool = false;
/// Default: no measurements enabled (mask = 0) until configured.
pub const DEFAULT_ENABLED_MEASUREMENTS: u32 = 0;

/// The configuration for the Patina Performance component.
#[derive(Debug, Clone, Copy)]
pub struct PerfConfig {
    /// Indicates whether the Patina Performance component is enabled.
    pub enable_component: bool,
    /// Bitmask of enabled measurements (see `patina_sdk::performance::Measurement`).
    pub enabled_measurements: u32,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self { enable_component: DEFAULT_ENABLE_COMPONENT, enabled_measurements: DEFAULT_ENABLED_MEASUREMENTS }
    }
}
