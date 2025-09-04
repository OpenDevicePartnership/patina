//! This module provides type definitions for Protocol Handles
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{ffi::c_void, ptr::NonNull};

use r_efi::efi;

/// Represents a registration handle for protocol notifications in the UEFI system.
pub type Registration = NonNull<c_void>;

/// Represents the type of handle search to perform in the UEFI system.
#[derive(Debug, Clone, Copy)]
pub enum HandleSearchType {
    /// Search for all handles in the system.
    AllHandle,
    /// Search for handles registered with a specific notification function.
    ByRegisterNotify(Registration),
    /// Search for handles that support a specific protocol.
    ByProtocol(&'static efi::Guid),
}

impl From<HandleSearchType> for efi::LocateSearchType {
    fn from(val: HandleSearchType) -> Self {
        match val {
            HandleSearchType::AllHandle => efi::ALL_HANDLES,
            HandleSearchType::ByRegisterNotify(_) => efi::BY_REGISTER_NOTIFY,
            HandleSearchType::ByProtocol(_) => efi::BY_PROTOCOL,
        }
    }
}
