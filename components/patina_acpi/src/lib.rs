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
//! ```rust,ignore
//! use patina_acpi::component::AcpiProviderManager;
//!
//!  #[derive(Default, Clone, Copy)]
//!  struct SectionExtractExample;
//!  impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
//!      fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
//!  }
//!
//!  let physical_hob_list = core::ptr::null();
//!
//!  patina_dxe_core::Core::default()
//!         .with_section_extractor(SectionExtractExample::default())
//!         .init_memory(physical_hob_list)
//!         .with_component(AcpiProviderManager::new([0; 6], [0; 8], 0x12345678, 0x87654321, 0xDEADBEEF))
//!         .start().unwrap();
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
mod aml;
mod integration_test;
mod signature;
