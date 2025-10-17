//! SMBIOS (System Management BIOS) component for Patina
//!
//! This crate provides safe Rust abstractions for working with SMBIOS tables in UEFI environments,
//! offering both byte-level and type-safe interfaces for creating, managing, and publishing SMBIOS data.
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
//!                │ Byte-level operations: │
//!                │ • add_from_bytes()     │
//!                │ • update_string()      │
//!                │ • remove()             │
//!                │                        │
//!                │ Table management:      │

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
//! - **Smbios service**: Unified service providing all SMBIOS operations in one interface
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
//! use patina_smbios::{SmbiosConfiguration, SmbiosProvider};
//!
//! // 1. Register SMBIOS provider component
//! fn my_platform_init(mut commands: Commands) -> Result<()> {
//!     // Configure SMBIOS version
//!     commands.add_config(SmbiosConfiguration {
//!         major_version: 3,
//!         minor_version: 9,
//!     });
//!
//!     // Register SMBIOS provider (installs the Smbios service)
//!     commands.add_component(SmbiosProvider::new());
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Adding Records - Recommended Type-Safe API
//!
//! **This is the recommended way to add SMBIOS records.** The `add_record<T>()` method
//! provides automatic serialization and type safety:
//!
//! ```ignore
//! use patina::component::service::Service;
//! use patina_smbios::Smbios;
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
//!     // Just pass the record - serialization is automatic!
//!     let handle = smbios.add_record(None, &bios_info)?;
//!
//!     log::info!("Added BIOS info with handle: {}", handle);
//!     Ok(())
//! }
//! ```
//!
//! ## Advanced: Byte-Level Interface
//!
//! For custom record types or advanced use cases, you can use the byte-level API.
//! **Most users should prefer `add_record<T>()` instead.**
//!
//! ```ignore
//! use patina::component::service::Service;
//! use patina_smbios::Smbios;
//!
//! fn add_custom_record(
//!     smbios: Service<Smbios>
//! ) -> Result<()> {
//!     // Build a custom record as bytes (for types not yet implemented)
//!     let mut record = Vec::new();
//!     record.extend_from_slice(&[
//!         1,    // Type 1 (System Information)
//!         8,    // Length (header + 4 bytes data)
//!         0xFE, 0xFF, // Handle (will be auto-assigned)
//!         1,    // Manufacturer (string index 1)
//!         2,    // Product name (string index 2)
//!         3,    // Version (string index 3)
//!         4,    // Serial number (string index 4)
//!     ]);
//!     // Add strings (null-terminated, double-null at end)
//!     record.extend_from_slice(b"ACME Corp\0Product X\0v1.0\0SN12345\0\0");
//!
//!     // Add to SMBIOS table
//!     let handle = smbios.add_from_bytes(None, &record)?;
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
//! use patina::boot_services::StandardBootServices;
//! use patina::component::service::Service;
//! use patina_smbios::Smbios;
//!
//! fn publish_smbios(
//!     smbios: Service<Smbios>,
//!     boot_services: StandardBootServices
//! ) -> Result<()> {
//!     // Publish SMBIOS table to UEFI Configuration Table
//!     let (table_addr, entry_point_addr) = smbios.publish_table(&boot_services)?;
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
//! use patina_smbios::Smbios;
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
//! patina_smbios = "13.0"
//! ```
//!
//! ## Step 2: Register Provider Component
//!
//! In your platform initialization code, register the SMBIOS provider:
//!
//! ```ignore
//! commands.add_config(SmbiosConfiguration {
//!     major_version: 3,
//!     minor_version: 9,
//! });
//! commands.add_component(SmbiosProvider::new());
//! ```
//!
//! ## Step 3: Consume Service
//!
//! Request the SMBIOS service in your components:
//!
//! ```ignore
//! #[derive(IntoComponent)]
//! struct MyPlatformComponent;
//!
//! impl MyPlatformComponent {
//!     fn entry_point(
//!         self,
//!         smbios: Service<Smbios>,
//!     ) -> Result<()> {
//!         // Add records using type-safe or byte-level API
//!         // smbios.add_record(None, &my_record)?;
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
//! smbios.publish_table(&boot_services)?;
//! ```
//!
//! # Safety and Architecture Guarantees
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
//! - [`config`]: SMBIOS configuration types (version settings)
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
//!

#![cfg_attr(not(test), no_std)]
#![feature(coverage_attribute)]

pub mod component;
pub mod config;
pub mod error;
pub mod service;
pub mod smbios_record;

mod manager;

pub use component::{Smbios, SmbiosProvider};
pub use config::SmbiosConfiguration;
pub use error::SmbiosError;

// Re-export commonly used types and constants for convenience
pub use service::{SMBIOS_HANDLE_PI_RESERVED, SMBIOS_STRING_MAX_LENGTH, SmbiosHandle, SmbiosTableHeader, SmbiosType};
