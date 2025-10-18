//! # ESRT (EFI System Resource Table) Component
//!
//! Provides firmware update support via UEFI System Resource Table.
//!
//! ## Integration Example
//!
//! Add the ESRT component to your Patina DXE Core:
//!
//! ```rust,ignore
//! use patina_esrt::Esrt;
//!
//! Core::default()
//!     // ... other components ...
//!     .with_component(Esrt)
//!     .start()
//!     .unwrap();
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// Enable coverage attribute for scaffold exclusion
#![feature(coverage_attribute)]
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod component;
pub mod service;

mod config;
mod error;
mod fmp;
mod repository;
mod table;
mod types;

// Public API exports
pub use component::Esrt;
pub use config::EsrtConfig;
pub use error::EsrtError;
pub use service::EsrtRecords;
pub use types::{FirmwareType, LastAttemptStatus, SystemResourceEntry};
