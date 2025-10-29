//! Protocol definitions for the Advanced Logger.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::uefi_protocol::ProtocolInterface;
use r_efi::efi;

/// C struct for the Advanced Logger protocol version 2.
#[repr(C)]
pub struct AdvancedLoggerProtocol {
    /// Signature for the Advanced Logger protocol.
    pub signature: u32,
    /// Version of the Advanced Logger protocol.
    pub version: u32,
    /// Function to write a log message to the Advanced Logger.
    pub write_log: AdvancedLoggerWrite,
    // Physical address of the Advanced Logger memory buffer. This is not a public
    // field so should should only be accessed from within the crate.
    pub(crate) log_info: efi::PhysicalAddress,
}

/// Function definition for writing a log message to the Advanced Logger through
/// the protocol.
type AdvancedLoggerWrite = extern "efiapi" fn(*const AdvancedLoggerProtocol, usize, *const u8, usize) -> efi::Status;

// SAFETY: The AdvancedLoggerProtocol struct layout matches the protocol definition.
unsafe impl ProtocolInterface for AdvancedLoggerProtocol {
    const PROTOCOL_GUID: efi::Guid = AdvancedLoggerProtocol::GUID;
}

impl AdvancedLoggerProtocol {
    /// Protocol GUID for the Advanced Logger protocol.
    pub const GUID: efi::Guid =
        efi::Guid::from_fields(0x434f695c, 0xef26, 0x4a12, 0x9e, 0xba, &[0xdd, 0xef, 0x00, 0x97, 0x49, 0x7c]);

    /// Signature used for the Advanced Logger protocol.
    pub const SIGNATURE: u32 = 0x50474F4C; // "LOGP"

    /// Current version of the Advanced Logger protocol.
    pub const VERSION: u32 = 2;

    /// Creates a new instance of the Advanced Logger protocol.
    pub(crate) const fn new(write_log: AdvancedLoggerWrite, log_info: efi::PhysicalAddress) -> Self {
        AdvancedLoggerProtocol { signature: Self::SIGNATURE, version: Self::VERSION, write_log, log_info }
    }
}
