//! SMBIOS Manager Module
//!
//! This module provides the core SMBIOS manager implementation organized into focused submodules:
//! - `core`: SmbiosManager struct and SmbiosRecords trait implementation
//! - `record`: Internal record structures (SmbiosRecord)
//! - `global`: Global state management (TplMutex-protected manager, boot services)
//! - `protocol`: C/EDKII protocol compatibility layer (SmbiosProtocol)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;

use patina::uefi_protocol::ProtocolInterface;

mod core;
mod global;
mod protocol;
mod record;

// Re-export main types and functions
pub use core::SmbiosManager;
pub(crate) use record::SmbiosRecord;

use alloc::boxed::Box;

use patina::boot_services::BootServices;
use patina::boot_services::StandardBootServices;
use patina::tpl_mutex::TplMutex;
use r_efi::efi;

use crate::error::SmbiosError;

use self::global::{BOOT_SERVICES, SMBIOS_MANAGER, SMBIOS_PROTOCOL_HANDLE, SMBIOS_PROTOCOL_INTERFACE};
use self::protocol::SmbiosProtocol;

/// Installs the SMBIOS C/EDKII protocol for legacy driver compatibility.
///
/// This function registers the SMBIOS protocol with UEFI so that C/EDK drivers can access
/// SMBIOS functionality. The protocol functions access the global manager reference.
///
/// The manager is protected by TplMutex at NOTIFY level. When protocol functions
/// are called, the TplMutex.lock() automatically raises TPL to NOTIFY, preventing
/// timer interrupt reentrancy. TPL is automatically restored when the lock guard drops.
#[coverage(off)] // Protocol installation - tested via integration tests
/// Installs the SMBIOS protocol into the system for external access
pub fn install_smbios_protocol(
    manager_mutex: &'static TplMutex<'static, SmbiosManager, StandardBootServices>,
    boot_services: &'static StandardBootServices,
) -> Result<efi::Handle, SmbiosError> {
    // Get the version from the manager
    let (major, minor) = {
        let manager = manager_mutex.lock();
        manager.version()
    };

    // Store boot_services reference globally for future use
    // SAFETY: This function should only be called once during system initialization
    unsafe {
        BOOT_SERVICES.initialize(boot_services);
    }

    // Initialize the global manager with the provided TplMutex reference
    // SAFETY: This function should only be called once during system initialization
    unsafe {
        SMBIOS_MANAGER.initialize(manager_mutex)?;
    }

    // Create the protocol instance
    let protocol = SmbiosProtocol::new(major, minor);
    let interface = Box::into_raw(Box::new(protocol));
    let interface_void = interface as *mut ();

    // Store the interface pointer for lifetime management
    // SAFETY: We just created this pointer and it's valid
    unsafe {
        SMBIOS_PROTOCOL_INTERFACE.set(interface_void as *mut _);
    }

    // Install the protocol using the unchecked interface since we have a raw pointer
    let handle = unsafe {
        boot_services.install_protocol_interface_unchecked(
            None, // Let UEFI create a new handle
            &SmbiosProtocol::PROTOCOL_GUID,
            interface_void as *mut _,
        )
    };

    match handle {
        Ok(h) => {
            // Store the handle
            // SAFETY: We just received this valid handle from boot_services
            unsafe {
                SMBIOS_PROTOCOL_HANDLE.set(h);
            }
            Ok(h)
        }
        Err(status) => {
            // Clean up on failure
            unsafe {
                drop(Box::from_raw(interface));
                SMBIOS_MANAGER.clear();
                SMBIOS_PROTOCOL_INTERFACE.clear();
            }
            log::error!("Failed to install SMBIOS protocol: {:?}", status);
            Err(SmbiosError::AllocationFailed)
        }
    }
}
