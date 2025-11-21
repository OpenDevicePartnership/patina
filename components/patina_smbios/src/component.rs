//! SMBIOS Service Implementation
//!
//! Defines the SMBIOS provider for use as a service
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;
use crate::{
    error::SmbiosError,
    manager::SmbiosManager,
    service::{Smbios, SmbiosImpl},
};
use alloc::boxed::Box;
use patina::{
    boot_services::tpl::Tpl,
    component::{Storage, component, service::memory::MemoryManager},
    error::Result,
    tpl_mutex::TplMutex,
};

/// Internal configuration for SMBIOS service
#[derive(Debug, Clone, PartialEq, Eq)]
struct SmbiosConfiguration {
    /// SMBIOS major version (e.g., 3 for SMBIOS 3.x)
    major_version: u8,
    /// SMBIOS minor version (e.g., 0 for SMBIOS 3.0)
    minor_version: u8,
}

impl SmbiosConfiguration {
    /// Create a new SMBIOS configuration with the specified version
    ///
    /// # Errors
    ///
    /// Returns `SmbiosError::UnsupportedVersion` if major_version != 3
    fn new(major_version: u8, minor_version: u8) -> core::result::Result<Self, SmbiosError> {
        // Only SMBIOS 3.x is supported
        if major_version != 3 {
            return Err(SmbiosError::UnsupportedVersion);
        }

        // Accept any minor version for 3.x (forward compatible)
        Ok(Self { major_version, minor_version })
    }
}

/// Initializes and exposes SMBIOS provider service.
///
/// This component provides the `Service<Smbios>` which includes:
/// - Type-safe record operations: `add_record<T>()`
/// - Record management: `update_string()`, `remove()`
/// - Table management: `version()`, `publish_table()`
///
/// The provider creates an SMBIOS manager instance protected by a TplMutex
/// and installs the SMBIOS protocol for C/EDKII driver compatibility.
///
/// # Example
///
/// ```ignore
/// commands.add_component(SmbiosProvider::new(3, 9));
/// ```
pub struct SmbiosProvider {
    config: SmbiosConfiguration,
}

#[component]
impl SmbiosProvider {
    /// Create a new SMBIOS provider with the specified SMBIOS version.
    ///
    /// # Arguments
    ///
    /// * `major_version` - SMBIOS major version (must be 3)
    /// * `minor_version` - SMBIOS minor version (any value for version 3.x)
    ///
    /// # Panics
    ///
    /// Panics if the version is invalid (major version != 3).
    /// This is intentional to enforce correct version at compile/initialization time.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // For SMBIOS 3.9 specification
    /// commands.add_component(SmbiosProvider::new(3, 9));
    /// ```
    pub fn new(major_version: u8, minor_version: u8) -> Self {
        let config = SmbiosConfiguration::new(major_version, minor_version)
            .expect("Invalid SMBIOS version: only SMBIOS 3.x is supported");
        Self { config }
    }

    /// Initialize the SMBIOS provider and register it as a service
    #[coverage(off)] // Component integration - tested via integration tests
    fn entry_point(self, storage: &mut Storage) -> Result<()> {
        let cfg = self.config;

        let manager = SmbiosManager::new(cfg.major_version, cfg.minor_version)?;

        // SAFETY: Storage guarantees that boot_services has a 'static lifetime.
        // We use an unsafe cast because storage.boot_services() returns a reference with
        // a shorter lifetime bound, but Storage guarantees it lives for 'static.
        let boot_services_static: &'static patina::boot_services::StandardBootServices =
            unsafe { &*(storage.boot_services() as *const _) };

        // Get the MemoryManager service for memory allocations
        let memory_manager = storage.get_service::<dyn MemoryManager>().ok_or(patina::error::EfiError::Unsupported)?;

        // Allocate buffers and add Type 127 End-of-Table marker
        // This must be done before protocol installation to avoid allocate_pages during Add()
        manager.allocate_buffers(*memory_manager)?;

        // Create TplMutex at TPL_NOTIFY for thread safety against timer interrupts
        let manager_mutex = TplMutex::new(boot_services_static, Tpl::NOTIFY, manager);
        let smbios_service = SmbiosImpl {
            manager: manager_mutex,
            boot_services: boot_services_static,
            major_version: cfg.major_version,
            minor_version: cfg.minor_version,
        };

        // Leak the service to get a 'static reference for both Rust service and C protocol
        let smbios_static: &'static SmbiosImpl = Box::leak(Box::new(smbios_service));

        storage.add_service(smbios_static);

        // Install SMBIOS protocol for C/EDKII driver compatibility
        crate::manager::install_smbios_protocol(
            cfg.major_version,
            cfg.minor_version,
            &smbios_static.manager,
            boot_services_static,
        )?;

        // Publish initial table (Type 127 only) to register buffer with UEFI configuration table
        // Subsequent Add() calls will update this buffer in-place
        smbios_static.publish_table()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_smbios_provider_new() {
        let provider = SmbiosProvider::new(3, 9);
        assert_eq!(provider.config.major_version, 3);
        assert_eq!(provider.config.minor_version, 9);
    }

    #[test]
    fn test_smbios_configuration_custom() {
        let provider = SmbiosProvider::new(3, 7);
        assert_eq!(provider.config.major_version, 3);
        assert_eq!(provider.config.minor_version, 7);
    }

    #[test]
    #[should_panic(expected = "Invalid SMBIOS version")]
    fn test_smbios_provider_invalid_version() {
        // Should panic with invalid major version
        let _provider = SmbiosProvider::new(2, 0);
    }

    // Test that we can create the component - this tests the primary constructor path
    #[test]
    fn test_component_creation() {
        let provider = SmbiosProvider::new(3, 9);
        assert_eq!(provider.config.major_version, 3);
        assert_eq!(provider.config.minor_version, 9);
    }
}
