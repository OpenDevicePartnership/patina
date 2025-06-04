//! ACPI Components
//!
//! This library provides two components, `AcpiProviderManager` and `AcpiSystemTableProtocolManager`.
//! `AcpiProviderManager` initializes necessary context to install, uninstall, and retrieve ACPI tables.
//! `AcpiSystemTableProtocolManager` publishes the ACPI Table and ACPI SDT protocols.
//!
//! This library also provides a service interface, `AcpiProvider`, which can be consumed by other components to perform ACPI operations.
//!
//! ## Examples and Usage
//!
//! To initialize the `AcpiProviderManager`, the configuration should be customized with the correct platform values (`oem_id`, etc).
//! In the platform start routine, provide these configuration values and initialize a new `AcpiProviderManager` instance.
//!
//! ```
//!  Core::default()
//!         ...
//!         .with_config(AcpiProviderInit {
//!                         version: 1,
//!                         should_reclaim_memory: true,
//!                         oem_id: [0; 6],
//!                         oem_table_id: [0; 8],
//!                         creator_id: 0x50415449,
//!                         creator_revision: 1,
//!                     })
//!         .with_component(AcpiProviderManager::new())
//! ```
//!
//! A similar pattern can be followed to create the `AcpiSystemTableProtocolManager`.
//!
//! For examples of how to use the service interface, see `service.rs`.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#![no_std]
extern crate alloc;

pub mod component;
pub mod config;
pub mod error;
pub mod service;

mod acpi;
mod acpi_protocol;
mod acpi_table;
mod integration_test;
mod signature;
