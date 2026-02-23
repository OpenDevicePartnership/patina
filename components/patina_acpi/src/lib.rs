//! ACPI Components
//!
//! This library provides three components, `AcpiProviderManager`, `AcpiSystemProtocolManager`, and `GenericAcpiManager`.
//! `AcpiProviderManager` and `GenericAcpiManager` initialize necessary context to install, uninstall, and retrieve ACPI tables.
//! `AcpiSystemTableProtocolManager` publishes the ACPI Table and ACPI SDT protocols.
//! To simply use and consume the Rust service, only `AcpiProviderManager` and `GenericAcpiManager` are necessary.
//! To use the EDKII protocols, `AcpiSystemProtocolManager` should also be included.
//!
//! ## Examples and Usage
//!
//! To initialize the `AcpiProviderManager`, the configuration should be customized with the correct platform values (`oem_id`, etc).
//! In the platform start routine, provide these configuration values and initialize a new `AcpiProviderManager` instance.
//!
//! ```rust,ignore
//! static CORE: Core<MyPlatform> = Core::new(CompositeSectionExtractor::new());
//!
//! impl ComponentInfo for Intel {
//! fn components(mut add: Add<Component>) {
//!         add.component(AdvancedLoggerComponent::<Uart16550>::new(&LOGGER));
//!         // Other platform components...
//!         add.component(patina_acpi::component::AcpiProviderManager::new(oem_id, ... /* Other platform init. */));
//!         add.component(patina_acpi::component::AcpiSystemProtocolManager::default());
//!         add.component(patina_acpi::component::GenericAcpiManager::default());
//!     }
//! }
//! ```
//!
//! A similar pattern can be followed to create the `AcpiSystemProtocolManager`.
//!
//! For examples of how to use the service interface, see `service.rs`.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![feature(coverage_attribute)]
#![feature(allocator_api)]

extern crate alloc;

/// Component that provides initialization of ACPI functionality in the core.
pub mod component;
/// Errors associated with operation of the ACPI protocol.
pub mod error;
/// Definition for ACPI HOB, which transfers existing ACPI tables from the PEI phase through the RSDP.
pub mod hob;
/// Public service interface for the ACPI protocol.
pub mod service;

mod acpi;
mod acpi_protocol;
mod acpi_table;
mod integration_test;
mod signature;
