//! SMBIOS (System Management BIOS) component for Patina
//!
//! This crate provides a safe, type-safe Rust interface for working with SMBIOS tables in UEFI
//! environments. It offers structured record types with automatic serialization, making SMBIOS
//! table management simple and safe.
//!
//! # Architecture Overview
//!
//! The SMBIOS component provides a unified service interface for all SMBIOS operations:
//!
//! ## Service Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                  Platform Components                        │
//! │  (Rust code using component services)                       │
//! └──────────────────────────┬──────────────────────────────────┘
//!                            │
//!                            ▼
//!                ┌────────────────────────┐
//!                │   Service<Smbios>      │
//!                │                        │
//!                │ Type-safe operations:  │
//!                │ • add_record<T>()      │
//!                │                        │
//!                │ Record management:     │
//!                │ • update_string()      │
//!                │ • remove()             │
//!                │                        │
//!                │ Table management:      │
//!                │ • version()            │
//!                │ • publish_table()      │
//!                └────────────┬───────────┘
//!                             │
//!                             ▼
//!                  ┌──────────────────┐
//!                  │  SMBIOS Manager  │
//!                  │  (TPL_NOTIFY)    │
//!                  └──────────────────┘
//!                             ▼
//!                  ┌─────────────────────┐
//!                  │  SmbiosManager      │
//!                  │  (Global Singleton) │
//!                  │                     │
//!                  │ • Record storage    │
//!                  │ • Handle allocation │
//!                  │ • Table generation  │
//!                  └──────────┬──────────┘
//!                             │
//!                  ┌──────────┴──────────┐
//!                  ▼                     ▼
//!       ┌──────────────────┐  ┌──────────────────────┐
//!       │ UEFI Config Table│  │ C Protocol Interface │
//!       │ (SMBIOS 3.x)     │  │ (EDKII Compatible)   │
//!       └──────────────────┘  └──────────────────────┘
//! ```
//!
//! ### Key Components
//!
//! - **Smbios service**: Provides type-safe SMBIOS operations through structured records
//! - **Global Manager**: Single source of truth for SMBIOS data, protected by TplMutex
//! - **C Protocol**: EDKII-compatible protocol for legacy driver integration
//!
//! ## Thread Safety and TPL Protection
//!
//! The global SMBIOS manager is protected by a **TplMutex** at **TPL_NOTIFY** level:
//!
//! - Prevents timer interrupt reentrancy during SMBIOS operations
//! - TPL automatically raised to NOTIFY when accessing the manager
//! - TPL automatically restored when the lock guard drops
//! - Safe for use in DXE phase with timer interrupts enabled
//!
//! # Usage Examples
//!
//! ## Basic Setup in Platform DXE
//!
//! ```ignore
//! use patina::component::{Component, IntoComponent};
//! use patina_smbios::component::SmbiosProvider;
//!
//! // Register SMBIOS provider component with SMBIOS version
//! fn my_platform_init(mut commands: Commands) -> Result<()> {
//!     // Register SMBIOS provider with SMBIOS 3.9 specification
//!     commands.add_component(SmbiosProvider::new(3, 9));
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Adding SMBIOS Records
//!
//! Use the type-safe `add_record<T>()` method to add SMBIOS records. This provides
//! automatic serialization and compile-time type safety:
//!
//! ```ignore
//! use patina::component::service::Service;
//! use patina_smbios::service::Smbios;
//! use patina_smbios::smbios_record::{Type0PlatformFirmwareInformation, SmbiosTableHeader};
//! use patina_smbios::service::SMBIOS_HANDLE_PI_RESERVED;
//!
//! fn add_bios_info(
//!     smbios: Service<Smbios>
//! ) -> Result<()> {
//!     // Create a Type 0 (BIOS Information) record
//!     let bios_info = Type0PlatformFirmwareInformation {
//!         header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
//!         vendor: 1,                               // String index 1
//!         firmware_version: 2,                     // String index 2
//!         bios_starting_address_segment: 0xE000,   // Standard BIOS segment
//!         firmware_release_date: 3,                // String index 3
//!         firmware_rom_size: 0x0F,                 // 1MB
//!         characteristics: 0x08,                   // PCI supported
//!         characteristics_ext1: 0x03,              // ACPI supported
//!         characteristics_ext2: 0x01,              // UEFI supported
//!         system_bios_major_release: 2,
//!         system_bios_minor_release: 4,
//!         embedded_controller_major_release: 0xFF, // Not supported
//!         embedded_controller_minor_release: 0xFF,
//!         extended_bios_rom_size: 0x0000,
//!         string_pool: vec![
//!             String::from("ACME BIOS Corp"),
//!             String::from("v2.4.1"),
//!             String::from("09/26/2025"),
//!         ],
//!     };
//!
//!     // Add the record - serialization happens automatically!
//!     let handle = smbios.add_record(None, &bios_info)?;
//!
//!     log::info!("Added BIOS info with handle: {}", handle);
//!     Ok(())
//! }
//! ```
//!
//! ## Querying Records
//!
//! Record iteration is not currently exposed through the public API. In typical usage,
//! platform components add their SMBIOS records during initialization, then the table
//! is published for the operating system to read directly. The OS queries the SMBIOS
//! table through the UEFI Configuration Table.
//!
//! If you need to query existing records before publishing, this functionality may be
//! added to the `Service<Smbios>` interface in the future.
//!
//! ## Publishing the SMBIOS Table
//!
//! ```ignore
//! use patina::component::service::Service;
//! use patina_smbios::Smbios;
//!
//! fn publish_smbios(
//!     smbios: Service<Smbios>
//! ) -> Result<()> {
//!     // Publish SMBIOS table to UEFI Configuration Table
//!     let (table_addr, entry_point_addr) = smbios.publish_table()?;
//!     
//!     log::info!("SMBIOS table at: 0x{:X}", table_addr);
//!     log::info!("Entry point at: 0x{:X}", entry_point_addr);
//!     Ok(())
//! }
//! ```
//!
//! ## Updating Existing Records
//!
//! ```ignore
//! use patina::component::service::Service;
//! use patina_smbios::service::Smbios;
//!
//! fn update_firmware_version(
//!     smbios: Service<Smbios>,
//!     handle: u16,
//!     new_version: &str
//! ) -> Result<()> {
//!     // Update string index 2 (firmware version) in the Type 0 record
//!     smbios.update_string(handle, 2, new_version)?;
//!     Ok(())
//! }
//! ```
//!
//! # Integration Guide
//!
//! ## Step 1: Add Dependency
//!
//! Add to your platform's `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! patina_smbios = "14.0"
//! ```
//!
//! ## Step 2: Register Provider Component
//!
//! In your platform initialization code, register the SMBIOS provider with your SMBIOS version:
//!
//! ```ignore
//! use patina_smbios::component::SmbiosProvider;
//!
//! // Register with SMBIOS 3.9 specification
//! commands.add_component(SmbiosProvider::new(3, 9));
//! ```
//!
//! ## Step 3: Add Records in Your Components
//!
//! Request the SMBIOS service in your components and add records:
//!
//! ```ignore
//! use patina::component::{IntoComponent, service::Service};
//! use patina_smbios::service::Smbios;
//! use patina_smbios::smbios_record::Type1SystemInformation;
//!
//! #[derive(IntoComponent)]
//! struct MyPlatformComponent;
//!
//! impl MyPlatformComponent {
//!     fn entry_point(
//!         self,
//!         smbios: Service<Smbios>,
//!     ) -> Result<()> {
//!         // Create and add your platform's SMBIOS records
//!         let system_info = Type1SystemInformation { /* ... */ };
//!         smbios.add_record(None, &system_info)?;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Step 4: Publish Table
//!
//! After all components have added their records, publish the table:
//!
//! ```ignore
//! smbios.publish_table()?;
//! ```
//!
//! # Safety and Architecture Guarantees
//!
//! ## Type Safety
//!
//! - Structured record types with automatic serialization
//! - Compile-time verification of record structure
//! - No manual byte manipulation required
//! - Derive macro ensures correct SMBIOS format
//!
//! ## Memory Safety
//!
//! - All record data validated before storage
//! - String pools checked for proper null termination
//! - No unsafe pointer arithmetic exposed to users
//! - All allocations tracked and managed by UEFI boot services
//!
//! ## Thread Safety
//!
//! - Global manager protected by TplMutex at TPL_NOTIFY
//! - Prevents reentrancy from timer interrupts
//! - Safe for concurrent access from different components
//! - UEFI DXE model ensures single-threaded execution at same TPL
//!
//! ## Global State Justification
//!
//! The global SMBIOS manager is necessary because:
//!
//! 1. **C Protocol Requirement**: EDKII SMBIOS protocol callbacks don't receive `self` pointer,
//!    requiring global state to access the manager
//! 2. **Single Source of Truth**: All SMBIOS data (Rust + C consumers) must share one manager
//! 3. **Table Publication**: Final SMBIOS table must contain all records from all producers
//!
//! The manager is installed once during initialization and remains valid for system lifetime.
//!
//! ## Privacy and Encapsulation
//!
//! - Manager module is **private** - not exposed to platform code
//! - Platform code interacts only through the `Smbios` service
//! - Component pattern ensures proper dependency injection
//! - No direct hardcoded manager access possible
//!
//! # SMBIOS Specification Compliance
//!
//! This implementation follows **SMBIOS 3.0+** specification:
//!
//! - 64-bit table addresses (SMBIOS 3.x entry point structure)
//! - No 4GB table size limitation
//! - Standard string pool format (null-terminated, double-null terminated)
//! - Proper checksum calculation for entry point
//! - ACPI_RECLAIM_MEMORY type for table storage
//!
//! # Error Handling
//!
//! The [`error::SmbiosError`] enum provides detailed error information:
//!
//! - **String errors**: `StringTooLong`, `StringContainsNull`, `EmptyStringInPool`
//! - **Format errors**: `RecordTooSmall`, `MalformedRecordHeader`, `InvalidStringPoolTermination`
//! - **Handle errors**: `HandleExhausted`, `HandleNotFound`, `StringIndexOutOfRange`
//! - **Resource errors**: `AllocationFailed`, `NoRecordsAvailable`
//! - **State errors**: `AlreadyInitialized`, `NotInitialized`, `UnsupportedVersion`
//!
//! All errors are detailed and actionable for debugging.
//!
//! # Module Organization
//!
//! - [`component`]: Component registration and service providers
//! - [`error`]: Error types for SMBIOS operations
//! - [`service`]: Public service trait definitions and types
//! - `manager`: Private SMBIOS manager implementation (not public)
//! - `smbios_record`: Record structures and serialization (exported through `service`)
//!
//! # License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![feature(coverage_attribute)]

// Allow the derive macro to reference this crate using `::patina_smbios::`
extern crate self as patina_smbios;

pub mod component;
pub mod error;
pub mod service;
pub mod smbios_record;

mod manager;
