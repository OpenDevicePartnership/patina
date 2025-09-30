//! Test constants for Patina MM integration tests
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::base::SIZE_4KB;

/// Standard test buffer size
pub const TEST_BUFFER_SIZE: usize = SIZE_4KB;

/// MM Supervisor constants and definitions
///
/// Note: These values are only used for testing. They're not meant to be
/// accurate or used in production code.
pub mod mm_supv {
    /// Supervisor signature bytes
    pub const SIGNATURE: [u8; 4] = [b'M', b'S', b'U', b'P'];

    /// Communication protocol revision
    pub const REVISION: u32 = 1;

    /// Request signature as a DWORD
    pub const REQUEST_SIGNATURE: u32 = 0x5055534D; // 'MSUP'

    /// Supervisor version
    pub const VERSION: u32 = 0x00130008;

    /// Supervisor patch level
    pub const PATCH_LEVEL: u32 = 0x00010001;

    /// Maximum request level supported
    pub const MAX_REQUEST_LEVEL: u64 = 0x0000000000000004; // COMM_UPDATE

    /// Request type constants
    pub mod requests {
        /// Request for unblocking memory regions
        pub const UNBLOCK_MEM: u32 = 0x0001;

        /// Request to fetch security policy
        pub const FETCH_POLICY: u32 = 0x0002;

        /// Request version information
        pub const VERSION_INFO: u32 = 0x0003;

        /// Request to update the communication buffer address
        pub const COMM_UPDATE: u32 = 0x0004;
    }

    /// Response code constants
    pub mod responses {
        /// Operation completed successfully
        pub const SUCCESS: u64 = 0;

        /// Operation failed with error
        pub const ERROR: u64 = 0xFFFFFFFFFFFFFFFF;
    }
}

/// Test GUIDs for different handlers
///
/// Provides predefined GUIDs used throughout the patina_mm test framework for registering
/// and identifying different types of test handlers.
pub mod test_guids {
    use r_efi::efi;

    /// Echo handler GUID for testing
    pub const ECHO_HANDLER: efi::Guid =
        efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34, &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

    /// Version handler GUID for testing
    /// Note: Not used now but the GUID is reserved for future usage
    #[allow(dead_code)]
    pub const VERSION_HANDLER: efi::Guid =
        efi::Guid::from_fields(0x87654321, 0x4321, 0x8765, 0x43, 0x21, &[0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54]);

    /// MM Supervisor GUID for supervisor protocol testing
    pub const MM_SUPERVISOR: efi::Guid =
        efi::Guid::from_fields(0x8c633b23, 0x1260, 0x4ea6, 0x83, 0x0F, &[0x7d, 0xdc, 0x97, 0x38, 0x21, 0x11]);
}

// Convenience re-exports for common usage
pub use test_guids::ECHO_HANDLER as TEST_COMMUNICATION_GUID;
