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
use alloc::boxed::Box;
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

        // Convert boot_services to 'static by leaking it
        // SAFETY: This is safe because boot_services remains valid for the entire lifetime
        // of the system. We leak it intentionally to get a 'static reference.
        let boot_services_static: &'static StandardBootServices = Box::leak(Box::new(boot_services));

        // Install the protocol - this transfers ownership to the global singleton
        // and wraps the manager in TplMutex for TPL-aware reentrancy protection
        if let Err(e) = install_smbios_protocol(manager, boot_services_static) {
            log::error!("Failed to install SMBIOS protocol: {:?}", e);
        }

        // Register the service so other components can consume it
        commands.add_service(self);

        Ok(())
    }
}

// Delegate the SmbiosRecords trait implementation to the global manager
// All methods acquire TplMutex.lock() which automatically raises TPL to CALLBACK
impl SmbiosRecords<'static> for SmbiosProviderManager {
    fn add_from_bytes(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record_data: &[u8],
    ) -> core::result::Result<SmbiosHandle, SmbiosError> {
        let tpl_mutex = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
        manager.add_from_bytes(producer_handle, record_data)
        // TPL automatically restored when manager guard drops
    }

    fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> core::result::Result<(), SmbiosError> {
        let tpl_mutex = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
        manager.update_string(smbios_handle, string_number, string)
        // TPL automatically restored when manager guard drops
    }

    fn remove(&self, smbios_handle: SmbiosHandle) -> core::result::Result<(), SmbiosError> {
        let tpl_mutex = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
        manager.remove(smbios_handle)
        // TPL automatically restored when manager guard drops
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> core::result::Result<(SmbiosTableHeader, Option<r_efi::efi::Handle>), SmbiosError> {
        let tpl_mutex = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
        manager.get_next(smbios_handle, record_type)
        // TPL automatically restored when manager guard drops
    }

    fn version(&self) -> (u8, u8) {
        match get_global_smbios_manager() {
            Some(tpl_mutex) => {
                let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
                manager.version()
            }
            None => {
                log::error!("SMBIOS manager not installed; returning version (0,0)");
                (0, 0)
            }
        }
    }

    fn publish_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError> {
        let tpl_mutex = get_global_smbios_manager().ok_or(SmbiosError::OutOfResources)?;
        let manager = tpl_mutex.lock(); // TPL raised to CALLBACK
        manager.publish_table(boot_services)
        // TPL automatically restored when manager guard drops
    }
}
