//! Component Interface
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![no_std]

use core::{ffi::c_void, option::Option, result::Result};
use r_efi::efi;
use uefi_core::error;

/// A trait wrapper for the DXE component interfaces. This is an initial implementation
/// and will be replaced by a more robust component interface in the future.
pub trait DxeComponentInterface {
    fn install_protocol_interface(
        &self,
        handle: Option<efi::Handle>,
        protocol: efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status>;
}

/// The trait to be implemented by a DXE Component
///
/// This trait is used to ensure that the DXE component entry_point function interface
/// is satisfied for a component to be executed by the DXE Core.
pub trait DxeComponent {
    fn entry_point(&self, interface: &dyn DxeComponentInterface) -> error::Result<()>;
}
