//! Protocol definitions for the Advanced Logger.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use r_efi::efi;

/// C struct for the Advanced Logger protocol.
pub struct AdvancedLoggerProtocol {
    pub signature: u32,
    pub version: u32,
    pub write_log: AdvancedLoggerWrite,
    pub log_info: efi::PhysicalAddress, // Internal field for access lib.
}

pub struct AdvancedLoggerProtocolRegister {}

type AdvancedLoggerWrite = extern "efiapi" fn(*const AdvancedLoggerProtocol, usize, *const u8, usize) -> efi::Status;

unsafe impl uefi_sdk::protocol::Protocol for AdvancedLoggerProtocolRegister {
    type Interface = AdvancedLoggerProtocol;

    fn protocol_guid(&self) -> &'static efi::Guid {
        &AdvancedLoggerProtocol::GUID
    }
}

impl core::ops::Deref for AdvancedLoggerProtocolRegister {
    type Target = r_efi::efi::Guid;

    fn deref(&self) -> &Self::Target {
        &AdvancedLoggerProtocol::GUID
    }
}

impl AdvancedLoggerProtocol {
    /// Protocol GUID for the Advanced Logger protocol.
    pub const GUID: efi::Guid =
        efi::Guid::from_fields(0x434f695c, 0xef26, 0x4a12, 0x9e, 0xba, &[0xdd, 0xef, 0x00, 0x97, 0x49, 0x7c]);

    /// Signature used for the Advanced Logger protocol.
    pub const SIGNATURE: u32 = 0x50474F4C; // "LOGP"

    /// Current version of the Advanced Logger protocol.
    pub const VERSION: u32 = 2;

    pub const fn new(write_log: AdvancedLoggerWrite, log_info: efi::PhysicalAddress) -> Self {
        AdvancedLoggerProtocol { signature: Self::SIGNATURE, version: Self::VERSION, write_log, log_info }
    }
}
