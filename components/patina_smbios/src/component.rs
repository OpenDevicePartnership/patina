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
use crate::config::SmbiosConfiguration;
use crate::error::SmbiosError;
use crate::manager::{SmbiosManager, install_smbios_protocol};
use crate::service::SmbiosHandle;
use alloc::boxed::Box;
use patina::{
    boot_services::{StandardBootServices, tpl::Tpl},
    component::{
        IntoComponent,
        params::{Commands, Config},
        service::IntoService,
    },
    error::Result,
    tpl_mutex::TplMutex,
};
use r_efi::efi::Handle;

/// SMBIOS Service Trait (Stable Surface)
///
/// This trait defines the stable service interface for SMBIOS record operations.
/// It provides byte-level record management (CRUD operations).
///
/// For type-safe record operations using generics, use the inherent
/// `add_record<T>()` method on the concrete `Smbios` type.
///
/// For table-level operations (version, publishing), use the inherent methods
/// on the `Smbios` type: `version()` and `publish_table()`.
pub trait SmbiosRecords {
    /// Add a pre-serialized SMBIOS record from raw bytes
    ///
    /// # Parameters
    /// - `producer`: Optional producer handle for tracking record ownership
    /// - `record`: Byte slice containing complete SMBIOS record (header + data + strings)
    ///
    /// # Returns
    /// - `Ok(SmbiosHandle)`: Handle to the newly added record
    /// - `Err(SmbiosError)`: If validation fails or table is full
    fn add_from_bytes(
        &self,
        producer: Option<Handle>,
        record: &[u8],
    ) -> core::result::Result<SmbiosHandle, SmbiosError>;

    /// Update a string in an existing SMBIOS record
    ///
    /// # Parameters
    /// - `handle`: Handle of the record to update
    /// - `string_number`: 1-based string index (0 means no string)
    /// - `new_value`: New string value
    ///
    /// # Returns
    /// - `Ok(())`: String updated successfully
    /// - `Err(SmbiosError)`: If handle invalid, string_number out of range, or string too long
    fn update_string(
        &self,
        handle: SmbiosHandle,
        string_number: usize,
        new_value: &str,
    ) -> core::result::Result<(), SmbiosError>;

    /// Remove an SMBIOS record by handle
    ///
    /// # Parameters
    /// - `handle`: Handle of the record to remove
    ///
    /// # Returns
    /// - `Ok(())`: Record removed successfully
    /// - `Err(SmbiosError)`: If handle not found
    fn remove(&self, handle: SmbiosHandle) -> core::result::Result<(), SmbiosError>;
}

/// Initializes and exposes SMBIOS provider service.
///
/// This component provides the `Service<Smbios>` which includes:
/// - Type-safe record operations: `add_record<T>()`
/// - Byte-level CRUD operations: `add_from_bytes()`, `update_string()`, `remove()`
/// - Table management: `version()`, `publish_table()`
///
/// The provider creates an SMBIOS manager instance protected by a TplMutex.
/// A global reference is maintained for C/EDKII protocol compatibility.
#[derive(IntoComponent)]
pub struct SmbiosProvider;

impl Default for SmbiosProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SmbiosProvider {
    /// Create a new SMBIOS provider
    pub fn new() -> Self {
        Self {}
    }

    /// Initialize the SMBIOS provider and register it as a service
    #[coverage(off)] // Component integration - tested via integration tests
    fn entry_point(
        self,
        config: Config<SmbiosConfiguration>,
        mut commands: Commands,
        boot_services: StandardBootServices,
    ) -> Result<()> {
        let cfg = (*config).clone();

        // Create the manager with configured version
        let manager = SmbiosManager::new(cfg.major_version, cfg.minor_version).map_err(|e| {
            log::error!("Failed to create SMBIOS manager: {:?}", e);
            patina::error::EfiError::Unsupported
        })?;

        // Convert boot_services to 'static by leaking it
        // SAFETY: This is safe because boot_services remains valid for the entire lifetime
        // of the system. We leak it intentionally to get a 'static reference.
        let boot_services_static: &'static StandardBootServices = Box::leak(Box::new(boot_services));

        // Wrap the manager in TplMutex for TPL-aware reentrancy protection at TPL_NOTIFY level
        let manager_mutex = Box::leak(Box::new(TplMutex::new(boot_services_static, Tpl::NOTIFY, manager)));

        // Install the C/EDKII protocol - this installs a global reference for C compatibility
        if let Err(e) = install_smbios_protocol(manager_mutex, boot_services_static) {
            log::error!("Failed to install SMBIOS protocol: {:?}", e);
        }

        // Register unified SMBIOS service with all operations
        commands.add_service(Smbios { manager: manager_mutex });

        Ok(())
    }
}

/// SMBIOS service providing all SMBIOS operations
///
/// This service provides a complete interface for SMBIOS table management:
/// - **Type-safe operations**: `add_record<T>()` for structured record types
/// - **Byte-level operations**: `add_from_bytes()`, `update_string()`, `remove()`
/// - **Table management**: `version()`, `publish_table()`
///
/// All operations are protected by a TplMutex at TPL_NOTIFY level for thread safety.
///
/// # Example
///
/// ```ignore
/// fn entry_point(
///     smbios: Service<Smbios>,
///     boot_services: StandardBootServices,
/// ) -> Result<()> {
///     // Add structured records
///     smbios.add_record(None, &bios_info)?;
///     
///     // Publish to configuration table
///     smbios.publish_table(&boot_services)?;
///     Ok(())
/// }
/// ```
#[derive(IntoService, Clone, Copy)]
#[service(Smbios)]
pub struct Smbios {
    manager: &'static TplMutex<'static, SmbiosManager, StandardBootServices>,
}

impl Smbios {
    /// Gets the SMBIOS version information.
    ///
    /// # Returns
    ///
    /// A tuple of (major_version, minor_version).
    pub fn version(&self) -> (u8, u8) {
        let manager = self.manager.lock();
        manager.version()
    }

    /// Publishes the SMBIOS table to the UEFI Configuration Table
    ///
    /// This should be called after all records have been added and the table
    /// is ready to be consumed by the operating system or other firmware components.
    ///
    /// # Arguments
    ///
    /// * `boot_services` - Reference to UEFI Boot Services for installing
    ///   the configuration table
    ///
    /// # Returns
    ///
    /// Returns a tuple of (table_address, entry_point_address) on success:
    /// - `table_address`: Physical address of the SMBIOS table data
    /// - `entry_point_address`: Physical address of the SMBIOS 3.x entry point structure
    ///
    /// # Errors
    ///
    /// Returns `SmbiosError` if:
    /// - No records have been added
    /// - Memory allocation fails
    /// - Configuration table installation fails
    pub fn publish_table(
        &self,
        boot_services: &StandardBootServices,
    ) -> core::result::Result<(r_efi::efi::PhysicalAddress, r_efi::efi::PhysicalAddress), SmbiosError> {
        let manager = self.manager.lock();
        manager.publish_table(boot_services)
    }

    /// Adds an SMBIOS record to the SMBIOS table from a complete byte representation.
    ///
    /// **This is the recommended method for adding SMBIOS records.** It provides memory safety
    /// and specification compliance by taking the complete record data as a validated byte slice,
    /// avoiding unsafe pointer arithmetic and potential security vulnerabilities.
    ///
    /// # Arguments
    ///
    /// * `producer_handle` - Optional handle of the producer creating this record
    /// * `record_data` - Complete SMBIOS record as a byte slice, including:
    ///   - Header (4 bytes: type, length, handle)
    ///   - Structured data (length - 4 bytes)
    ///   - String pool (null-terminated strings ending with double null)
    ///
    /// # Returns
    ///
    /// Returns the assigned SMBIOS handle for the newly added record.
    pub fn add_from_bytes(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record_data: &[u8],
    ) -> core::result::Result<SmbiosHandle, SmbiosError> {
        let manager = self.manager.lock(); // TPL raised to NOTIFY
        manager.add_from_bytes(producer_handle, record_data)
        // TPL automatically restored when guard drops
    }

    /// Updates a string in an existing SMBIOS record.
    ///
    /// # Arguments
    ///
    /// * `smbios_handle` - Handle of the record to update
    /// * `string_number` - 1-based index of the string to update
    /// * `string` - New string value
    pub fn update_string(
        &self,
        smbios_handle: SmbiosHandle,
        string_number: usize,
        string: &str,
    ) -> core::result::Result<(), SmbiosError> {
        let manager = self.manager.lock();
        manager.update_string(smbios_handle, string_number, string)
    }

    /// Removes an SMBIOS record from the SMBIOS table.
    ///
    /// # Arguments
    ///
    /// * `smbios_handle` - Handle of the record to remove
    pub fn remove(&self, smbios_handle: SmbiosHandle) -> core::result::Result<(), SmbiosError> {
        let manager = self.manager.lock();
        manager.remove(smbios_handle)
    }

    /// Add an SMBIOS record from a structured type.
    ///
    /// This is a type-safe convenience method that automatically serializes
    /// a structured record and adds it to the SMBIOS table.
    ///
    /// # Arguments
    ///
    /// * `producer_handle` - Optional handle of the producer creating this record
    /// * `record` - A reference to any type implementing `SmbiosRecordStructure`
    ///
    /// # Returns
    ///
    /// Returns the assigned SMBIOS handle for the newly added record.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let bios_info = Type0PlatformFirmwareInformation { ... };
    /// let handle = smbios.add_record(None, &bios_info)?;
    /// ```
    pub fn add_record<T>(
        &self,
        producer_handle: Option<r_efi::efi::Handle>,
        record: &T,
    ) -> core::result::Result<SmbiosHandle, SmbiosError>
    where
        T: crate::smbios_record::SmbiosRecordStructure,
    {
        let bytes = record.to_bytes();
        // Delegate to the manager
        let manager = self.manager.lock(); // TPL raised to NOTIFY
        manager.add_from_bytes(producer_handle, &bytes)
    }
}

/// Implement the SmbiosRecords trait for the concrete Smbios type
///
/// This implementation delegates all trait methods to the inherent implementations
/// above. The trait provides a formal contract for record CRUD operations only.
///
/// Note: The generic `add_record<T>()` method is NOT part of the trait (would break
/// object safety) and remains as an inherent method on the concrete type.
///
/// Table-level operations (`version()`, `publish_table()`) are also inherent methods,
/// not part of this trait, as they operate on the table as a whole rather than
/// individual records.
impl SmbiosRecords for Smbios {
    fn add_from_bytes(
        &self,
        producer: Option<Handle>,
        record: &[u8],
    ) -> core::result::Result<SmbiosHandle, SmbiosError> {
        // Delegate to inherent method
        Self::add_from_bytes(self, producer, record)
    }

    fn update_string(
        &self,
        handle: SmbiosHandle,
        string_number: usize,
        new_value: &str,
    ) -> core::result::Result<(), SmbiosError> {
        // Delegate to inherent method
        Self::update_string(self, handle, string_number, new_value)
    }

    fn remove(&self, handle: SmbiosHandle) -> core::result::Result<(), SmbiosError> {
        // Delegate to inherent method
        Self::remove(self, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_smbios_provider_new() {
        let provider = SmbiosProvider::new();
        // Basic test to ensure the provider can be created
        // The provider is a simple struct, so just check it exists
        let _ = provider;
    }

    #[test]
    fn test_smbios_configuration_default() {
        let config = SmbiosConfiguration::default();
        assert_eq!(config.major_version, 3);
        assert_eq!(config.minor_version, 9);
    }

    #[test]
    fn test_smbios_configuration_custom() {
        let config = SmbiosConfiguration { major_version: 2, minor_version: 4 };
        assert_eq!(config.major_version, 2);
        assert_eq!(config.minor_version, 4);
    }

    #[test]
    fn test_smbios_configuration_clone() {
        let config1 = SmbiosConfiguration { major_version: 3, minor_version: 0 };
        let config2 = config1.clone();
        assert_eq!(config1.major_version, config2.major_version);
        assert_eq!(config1.minor_version, config2.minor_version);
    }

    // Test that we can create the component - this tests the primary constructor path
    #[test]
    fn test_component_creation() {
        let provider = SmbiosProvider::new();
        // Since the struct is just a marker type, there's not much to test
        // but this ensures the new() method is covered
        let _ = provider;
    }
}
