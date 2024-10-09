//! DXE Component Interface
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::ffi::c_void;
use r_efi::efi;
use uefi_component_interface::DxeComponentInterface;

use crate::protocols::core_install_protocol_interface;

pub struct ComponentInterface;

impl DxeComponentInterface for ComponentInterface {
    fn install_protocol_interface(
        &self,
        handle: Option<efi::Handle>,
        protocol: efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        core_install_protocol_interface(handle, protocol, interface)
    }
}
