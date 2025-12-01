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
    manager::{SmbiosManager, install_smbios_protocol},
    service::SmbiosImpl,
};
use alloc::boxed::Box;
use patina::{
    boot_services::tpl::Tpl,
    component::{IntoComponent, Storage},
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
/// The provider creates an SMBIOS manager instance protected by a TplMutex.
/// A global reference is maintained for C/EDKII protocol compatibility.
///
/// # Example
///
/// ```ignore
/// commands.add_component(SmbiosProvider::new(3, 9));
/// ```
#[derive(IntoComponent)]
pub struct SmbiosProvider {
    config: SmbiosConfiguration,
}

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

        // Create the manager with configured version
        let manager = SmbiosManager::new(cfg.major_version, cfg.minor_version).map_err(|e| {
            log::error!("Failed to create SMBIOS manager: {:?}", e);
            patina::error::EfiError::Unsupported
        })?;

        // Get boot_services from storage - it already has 'static lifetime
        // We use an unsafe cast because storage.boot_services() returns a reference with
        // a shorter lifetime, but Storage guarantees boot_services lives for 'static
        let boot_services_static: &'static patina::boot_services::StandardBootServices =
            // SAFETY: Storage guarantees that boot_services has a 'static lifetime.
            unsafe { &*(storage.boot_services() as *const _) };

        // Create the SMBIOS service struct with owned TplMutex
        let manager_mutex = TplMutex::new(boot_services_static, Tpl::NOTIFY, manager);
        let smbios_service = SmbiosImpl {
            manager: manager_mutex,
            boot_services: boot_services_static,
            major_version: cfg.major_version,
            minor_version: cfg.minor_version,
        };

        // Leak the service to get a 'static reference
        // This single leak gives us a reference that can be used for both protocol and service
        let smbios_static: &'static SmbiosImpl = Box::leak(Box::new(smbios_service));

        // Install the C/EDKII protocol using the leaked service's manager
        if let Err(e) =
            install_smbios_protocol(cfg.major_version, cfg.minor_version, smbios_static.manager(), boot_services_static)
        {
            log::error!("Failed to install SMBIOS protocol: {:?}", e);
        }

        // Register the leaked service (the IntoService impl for &'static T handles this)
        storage.add_service(smbios_static);

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
