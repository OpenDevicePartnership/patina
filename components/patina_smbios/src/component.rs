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
    SmbiosError, SmbiosHandle, SmbiosManager, SmbiosRecords, SmbiosTableHeader, SmbiosType, install_smbios_protocol,
};
use core::cell::RefCell;
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
/// The provider wraps an internal [`SmbiosManager`] in a `RefCell` for interior mutability,
/// allowing the service to be used through immutable references while still supporting
/// mutable operations like adding and updating records.
///
/// It also attempts to install a C/EDKII compatible protocol (best‑effort; non‑fatal on failure)
/// for mixed Rust/C driver ecosystems.
#[derive(IntoComponent, IntoService)]
#[service(dyn SmbiosRecords<'static>)]
pub struct SmbiosProviderManager {
    manager: RefCell<SmbiosManager>,
}

impl Default for SmbiosProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SmbiosProviderManager {
    /// Create a new SMBIOS provider manager with default SMBIOS 3.9 version
    pub fn new() -> Self {
        Self { manager: RefCell::new(SmbiosManager::new(3, 9)) }
    }

    /// Initialize the SMBIOS provider and register it as a service
    fn entry_point(
        self,
        config: Option<Config<SmbiosConfiguration>>,
        mut commands: Commands,
        boot_services: StandardBootServices,
    ) -> Result<()> {
        let cfg = config.map(|c| (*c).clone()).unwrap_or_default();

        // Update manager with configured version
        *self.manager.borrow_mut() = SmbiosManager::new(cfg.major_version, cfg.minor_version);

        // Install the C protocol for EDKII compatibility
        match install_smbios_protocol(&self.manager.borrow(), &boot_services) {
            Ok(handle) => {
                log::info!("SMBIOS C protocol installed successfully at handle {:?}", handle);
            }
            Err(e) => {
                log::warn!("Failed to install SMBIOS C protocol: {:?}", e);
                // Continue anyway - the Rust service will still work
            }
        }

        // Register the service so other components can consume it
        commands.add_service(self);

        Ok(())
    }
}

// Delegate the SmbiosRecords trait implementation to the inner manager using interior mutability
impl SmbiosRecords<'static> for SmbiosProviderManager {
    fn add_from_bytes(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record_data: &[u8],
    ) -> core::result::Result<SmbiosHandle, SmbiosError> {
        self.manager.borrow_mut().add_from_bytes(producer_handle, record_data)
    }

    fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> core::result::Result<(), SmbiosError> {
        self.manager.borrow_mut().update_string(smbios_handle, string_number, string)
    }

    fn remove(&self, smbios_handle: SmbiosHandle) -> core::result::Result<(), SmbiosError> {
        self.manager.borrow_mut().remove(smbios_handle)
    }

    fn get_next(
        &self,
        smbios_handle: &mut SmbiosHandle,
        record_type: Option<SmbiosType>,
    ) -> core::result::Result<(SmbiosTableHeader, Option<r_efi::efi::Handle>), SmbiosError> {
        self.manager.borrow().get_next(smbios_handle, record_type)
    }

    fn version(&self) -> (u8, u8) {
        self.manager.borrow().version()
    }

    fn publish_table(
        &self,
        boot_services: &patina::boot_services::StandardBootServices,
    ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError> {
        self.manager.borrow().publish_table(boot_services)
    }
}
