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
pub mod config;
pub mod error;
pub mod guid;
pub mod measurement;
pub mod record;

// Re-export the Measurement enum for easier access.
pub use measurement::Measurement;
