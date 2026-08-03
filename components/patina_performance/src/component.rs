//! Patina Performance Components
//!
//! Performance measurement is split into four independent, single-responsibility components, each dispatched
//! only when the DXE Core's [`PerformanceManager`](patina::component::service::performance::PerformanceManager)
//! service is available (i.e. only when performance measurement is enabled):
//!
//! - [`protocol::MeasurementProtocolPublisher`] installs the EDK II measurement protocol for C drivers.
//! - [`property::PropertyPublisher`] publishes the performance-counter properties configuration table.
//! - [`fbpt::FbptPublisher`] publishes the Firmware Basic Boot Performance Table (FBPT) at End of DXE.
//! - [`mm_records::MmRecordCollector`] collects Management Mode performance records at Ready to Boot, when an MM
//!   communication region is available.
//!
//! A platform registers the components it needs. See each component's documentation for its own dependencies.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod fbpt;
pub mod mm_records;
pub mod property;
pub mod protocol;

// Re-export of the Measurement enum for easier access.
pub use patina::performance::Measurement;
