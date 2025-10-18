//! EsrtRecords Service Trait
//!
//! Service trait for managing ESRT entries. This trait is implemented by the
//! ESRT component and can be used by other components or drivers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::{error::EsrtError, types::SystemResourceEntry};
use r_efi::base::Guid;

/// Service trait for ESRT record management.
///
/// This service allows platform code and other components to register,
/// update, and query ESRT entries for firmware update support.
pub trait EsrtRecords {
    /// Register a new ESRT entry.
    ///
    /// # Errors
    /// - `EsrtError::AlreadyExists` if an entry with the same fw_class exists
    /// - `EsrtError::OutOfResources` if the repository is full
    /// - `EsrtError::WriteProtected` if the repository is locked
    fn register_entry(&mut self, entry: SystemResourceEntry) -> Result<(), EsrtError>;

    /// Replace an existing ESRT entry with a new entry.
    ///
    /// This function completely replaces the existing entry with the provided entry,
    /// which will cause the old entry to be dropped. If `SystemResourceEntry` contains
    /// resources that require special drop handling, consider using field-by-field
    /// update methods instead.
    ///
    /// # Parameters
    /// - `entry`: The new entry that will replace the existing one
    ///
    /// # Returns
    /// - `Ok(())` if the entry was successfully replaced
    ///
    /// # Errors
    /// - `EsrtError::NotFound` if no entry with matching fw_class exists
    /// - `EsrtError::WriteProtected` if the repository is locked
    ///
    fn update_entry(&mut self, entry: SystemResourceEntry) -> Result<(), EsrtError>;

    /// Unregister an ESRT entry by firmware class GUID.
    ///
    /// # Errors
    /// - `EsrtError::NotFound` if no entry with matching fw_class exists
    /// - `EsrtError::WriteProtected` if the repository is locked
    fn unregister_entry(&mut self, fw_class: &Guid) -> Result<(), EsrtError>;

    /// Get an ESRT entry by firmware class GUID.
    ///
    /// Returns `None` if no entry with matching fw_class exists.
    fn get_entry(&self, fw_class: &Guid) -> Option<&SystemResourceEntry>;

    /// Synchronize FMP (Firmware Management Protocol) entries.
    ///
    /// Enumerates all FMP protocol instances and updates the FMP repository.
    ///
    /// # Errors
    /// - `EsrtError::FmpEnumerationError` if FMP enumeration fails
    fn sync_fmp(&mut self) -> Result<(), EsrtError>;

    /// Lock the repository to prevent further modifications.
    ///
    /// This should be called before OS handoff to ensure ESRT table stability.
    fn lock_repository(&mut self);
}
