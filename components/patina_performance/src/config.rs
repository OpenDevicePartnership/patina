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

/// The configuration for the Patina Performance component.
#[derive(Debug, Default)]
pub struct PerfConfig {
    /// Indicates whether the Patina Performance component is enabled.
    pub enable_component: bool,
    /// A wrapper to generate a mask of all enabled measurements.
    pub enabled_measurements: u32,
}
