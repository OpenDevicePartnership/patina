//! Patina SDK ACPI Module
//!
//! This module provides functionality for managing ACPI components.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

/// Errors associated with operation of the ACPI protocol.
pub mod error;

pub mod acpi_table;
pub mod signature;
pub mod standard;
