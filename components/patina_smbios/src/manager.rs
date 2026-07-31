//! SMBIOS Manager Module
//!
//! This module provides the core SMBIOS manager implementation organized into focused submodules:
//! - `core`: `SmbiosManager` struct and `SmbiosRecords` trait implementation
//! - `record`: Internal record structures (`SmbiosRecord`)
//! - `protocol`: C/EDKII protocol compatibility layer (`SmbiosProtocol`)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

mod core;
mod protocol;
mod record;

// Re-export main types and functions
pub use core::{Smbios30EntryPoint, SmbiosManager};
pub(crate) use record::SmbiosRecord;

use alloc::boxed::Box;

use patina::component::service::{
    Service,
    uefi_services::protocol::{ProtocolServices, ProtocolServicesExt},
};

use crate::{error::SmbiosError, service::Smbios};

use self::protocol::SmbiosProtocolInternal;

/// Installs the SMBIOS C/EDKII protocol for legacy driver compatibility.
///
/// This function registers the SMBIOS protocol with UEFI so that C/EDK drivers can access
/// SMBIOS functionality. The protocol functions access the manager through the `Smbios` service.
///
/// # Errors
///
/// Returns `SmbiosError::AllocationFailed` if the protocol could not be installed.
#[cfg_attr(coverage, coverage(off))] // Protocol installation - tested via integration tests
pub fn install_smbios_protocol(
    major_version: u8,
    minor_version: u8,
    service: Service<dyn Smbios>,
    protocols: &Service<dyn ProtocolServices>,
) -> Result<(), SmbiosError> {
    let internal = SmbiosProtocolInternal::new(major_version, minor_version, service);

    protocols.install_protocol(None, Box::new(internal)).map_err(|e| {
        log::error!("Failed to install SMBIOS protocol: {e:?}");
        SmbiosError::AllocationFailed
    })?;

    Ok(())
}
