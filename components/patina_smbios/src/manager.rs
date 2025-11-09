//! SMBIOS Manager Module
//!
//! This module provides the core SMBIOS manager implementation organized into focused submodules:
//! - `core`: SmbiosManager struct and SmbiosRecords trait implementation
//! - `record`: Internal record structures (SmbiosRecord)
//! - `protocol`: C/EDKII protocol compatibility layer (SmbiosProtocol)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use patina::uefi_protocol::ProtocolInterface;

mod core;
mod protocol;
mod record;

// Re-export main types and functions
pub use core::{SMBIOS_3_X_TABLE_GUID, SmbiosManager};
pub(crate) use record::SmbiosRecord;

use alloc::boxed::Box;

use patina::boot_services::{BootServices, StandardBootServices};
use r_efi::efi;

use crate::error::SmbiosError;

use self::protocol::{SmbiosProtocol, SmbiosProtocolInternal};

/// Installs the SMBIOS C/EDKII protocol for legacy driver compatibility.
///
/// This function registers the SMBIOS protocol with UEFI so that C/EDK drivers can access
/// SMBIOS functionality. The protocol functions access the manager through the protocol struct.
///
/// Returns the protocol handle on success. The caller is responsible for managing the
/// protocol lifetime (though in practice, UEFI protocols persist for the system lifetime).
#[coverage(off)] // Protocol installation - tested via integration tests
pub fn install_smbios_protocol(
    major_version: u8,
    minor_version: u8,
    manager_cell: &'static ::core::cell::RefCell<SmbiosManager>,
    boot_services: &'static StandardBootServices,
) -> Result<efi::Handle, SmbiosError> {
    // Create the protocol instance with internal struct
    let internal = SmbiosProtocolInternal::new(major_version, minor_version, manager_cell, boot_services);
    let interface = Box::into_raw(Box::new(internal));

    // Install the protocol using the unchecked interface since we have a raw pointer
    // We install the protocol field of the internal struct
    // SAFETY: With #[repr(C)], the protocol field is at offset 0 of SmbiosProtocolInternal,
    // so the address of the internal struct is the same as the address of the protocol field
    let handle = unsafe {
        boot_services.install_protocol_interface_unchecked(
            None, // Let UEFI create a new handle
            &SmbiosProtocol::PROTOCOL_GUID,
            interface as *mut _,
        )
    };

    match handle {
        Ok(h) => Ok(h),
        Err(_status) => {
            // Clean up on failure
            unsafe {
                drop(Box::from_raw(interface));
            }
            Err(SmbiosError::AllocationFailed)
        }
    }
}
