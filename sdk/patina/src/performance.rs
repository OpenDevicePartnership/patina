//! Patina SDK Performance Module
//!
//! This module provides functionality for managing performance records in the Patina SDK.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
pub mod error;
pub mod globals;
pub mod logging;
pub mod measurement;
pub mod record;
pub mod table;

pub mod _smm;

// Re-export the Measurement enum for easier access.
pub use measurement::Measurement;
