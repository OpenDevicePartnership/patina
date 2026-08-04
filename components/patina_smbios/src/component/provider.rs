//! SMBIOS Provider Component
//!
//! Defines the component that creates the SMBIOS manager and registers the `Smbios` service.
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
    component::{
        component,
        params::Commands,
        service::{
            Service,
            memory::MemoryManager,
            uefi_services::{config_table::ConfigurationTableServices, tpl::TplServices},
        },
    },
    error::Result,
    uefi::boot_services::tpl::Tpl,
    uefi::tpl_mutex::TplMutex,
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

/// Creates the SMBIOS manager and registers the `Service<dyn Smbios>`.
///
/// This component provides the `Service<Smbios>` which includes:
/// - Type-safe record operations: `add_record<T>()`
/// - Record management: `update_string()`, `remove()`
/// - Table management: `version()`, `publish_table()`
///
/// It publishes the initial (Type 127 only) table so the UEFI Configuration Table entry exists
/// before any platform component adds its own records. C/EDK II driver compatibility is provided
/// separately by [`SmbiosProtocolPublisher`](crate::component::protocol_publisher::SmbiosProtocolPublisher),
/// which depends on the `Service<dyn Smbios>` this component registers.
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
    fn entry_point(
        self,
        memory: Service<dyn MemoryManager>,
        config_table: Service<dyn ConfigurationTableServices>,
        tpl: Service<dyn TplServices>,
        mut commands: Commands,
    ) -> Result<()> {
        let cfg = self.config;

        let manager = SmbiosManager::new(cfg.major_version, cfg.minor_version)?;

        // Allocate buffers and add Type 127 End-of-Table marker.
        // This must be done before the table is first published to avoid allocating during Add().
        manager.allocate_buffers(*memory)?;

        // Create TplMutex at TPL_NOTIFY for thread safety against timer interrupts
        let manager_mutex = TplMutex::new(tpl, Tpl::NOTIFY, manager);
        let smbios_service = SmbiosImpl {
            manager: manager_mutex,
            config_table,
            major_version: cfg.major_version,
            minor_version: cfg.minor_version,
        };

        // Leak the service to get a 'static reference usable both as the Rust service and, later, by the
        // C protocol shim.
        let smbios_static: &'static SmbiosImpl = Box::leak(Box::new(smbios_service));

        commands.add_service(smbios_static);

        // Publish initial table (Type 127 only) to register the buffer with the UEFI Configuration Table.
        // Subsequent Add() calls update this buffer in place.
        smbios_static.publish_table()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use patina::component::{
        params::Commands,
        service::{
            memory::StdMemoryManager,
            uefi_services::{config_table::MockConfigurationTableServices, tpl::MockTplServices},
        },
    };

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

    fn mock_tpl_services() -> MockTplServices {
        let mut tpl = MockTplServices::new();
        tpl.expect_raise_tpl().returning(|_| patina::component::service::uefi_services::tpl::PreviousTpl::from_raw(4));
        tpl.expect_restore_tpl().returning(|_| ());
        tpl
    }

    fn mock_config_table_services() -> MockConfigurationTableServices {
        let mut config_table = MockConfigurationTableServices::new();
        config_table.expect_replace_typed_table().returning(|_, _, _| Ok(()));
        config_table
    }

    #[test]
    fn test_smbios_provider_entry_point_registers_service_and_publishes_table() {
        let memory: Service<dyn MemoryManager> = Service::mock(Box::new(StdMemoryManager::new()));
        let config_table: Service<dyn ConfigurationTableServices> =
            Service::mock(Box::new(mock_config_table_services()));
        let tpl: Service<dyn TplServices> = Service::mock(Box::new(mock_tpl_services()));
        let commands = Commands::mock();

        let result = SmbiosProvider::new(3, 9).entry_point(memory, config_table, tpl, commands);

        assert!(result.is_ok());
    }
}
