//! SMBIOS Service Implementation
//!
//! Defines the SMBIOS provider for use as a service
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;
use crate::manager::{
    SmbiosError, SmbiosHandle, SmbiosManager, SmbiosRecords, SmbiosTableHeader, SmbiosType, get_global_smbios_manager,
    install_smbios_protocol,
};
use patina::{
    boot_services::StandardBootServices,
    component::{
        IntoComponent,
        params::{Commands, Config},
        service::IntoService,
    },
    error::Result,
};

/// Configuration for SMBIOS service
#[derive(Debug, Clone)]
pub struct SmbiosConfiguration {
    /// SMBIOS major version (e.g., 3 for SMBIOS 3.x)
    pub major_version: u8,
    /// SMBIOS minor version (e.g., 0 for SMBIOS 3.0)
    pub minor_version: u8,
}

impl Default for SmbiosConfiguration {
    fn default() -> Self {
        Self { major_version: 3, minor_version: 9 }
    }
}

/// Initializes and exposes an SMBIOS provider service.
///
/// The provider installs a global SMBIOS manager instance that is accessible throughout
/// the boot process. This ensures a single source of truth for SMBIOS data and allows
/// both Rust services and C/EDKII drivers to access the same SMBIOS tables.
///
/// The global instance is thread-safe via an internal Mutex and has 'static lifetime.
#[derive(IntoComponent, IntoService)]
#[service(dyn SmbiosRecords<'static>)]
pub struct SmbiosProviderManager {
    // No internal state - uses global singleton
}

impl Default for SmbiosProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SmbiosProviderManager {
    /// Create a new SMBIOS provider manager
    pub fn new() -> Self {
        Self {}
    }

    /// Initialize the SMBIOS provider and register it as a service
    fn entry_point(
        self,
        config: Option<Config<SmbiosConfiguration>>,
        mut commands: Commands,
        boot_services: StandardBootServices,
    ) -> Result<()> {
        let cfg = config.map(|c| (*c).clone()).unwrap_or_default();

        // Create the manager with configured version
        let manager = SmbiosManager::new(cfg.major_version, cfg.minor_version);

        // Install the protocol - this transfers ownership to the global singleton
        match install_smbios_protocol(manager, &boot_services) {
            Ok(handle) => {
                log::info!("SMBIOS protocol installed successfully at handle {:?}", handle);
            }
            Err(e) => {
                log::error!("Failed to install SMBIOS protocol: {:?}", e);
                // Cannot proceed without the manager - this is a fatal error
                panic!("SMBIOS manager installation failed");
            }
        }

        // Register the service so other components can consume it
        commands.add_service(self);

        Ok(())
    }
}

// Delegate the SmbiosRecords trait implementation to the global manager
impl SmbiosRecords<'static> for SmbiosProviderManager {
    fn add_from_bytes(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record_data: &[u8],
    ) -> core::result::Result<SmbiosHandle, SmbiosError> {
        let manager = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        manager.lock().add_from_bytes(producer_handle, record_data)
    }

    fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> core::result::Result<(), SmbiosError> {
        let manager = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        manager.lock().update_string(smbios_handle, string_number, string)
    }

    fn remove(&self, smbios_handle: SmbiosHandle) -> core::result::Result<(), SmbiosError> {
        let manager = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        manager.lock().remove(smbios_handle)
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> core::result::Result<(SmbiosTableHeader, Option<r_efi::efi::Handle>), SmbiosError> {
        let manager = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        manager.lock().get_next(smbios_handle, record_type)
    }

    fn version(&self) -> (u8, u8) {
        let manager = get_global_smbios_manager().expect("SMBIOS manager not installed");
        manager.lock().version()
    }

    fn publish_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError> {
        let manager = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        manager.lock().publish_table(boot_services)
    }
}
