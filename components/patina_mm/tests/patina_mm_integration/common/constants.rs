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
    use patina::BinaryGuid;

    /// Echo handler GUID for testing
    pub const ECHO_HANDLER: BinaryGuid = BinaryGuid::from_string("12345678-1234-5678-1234-567890ABCDEF");

    /// Version handler GUID for testing
    /// Note: Not used now but the GUID is reserved for future usage
    #[allow(dead_code)]
    pub const VERSION_HANDLER: BinaryGuid = BinaryGuid::from_string("87654321-4321-8765-4321-FEDCBA987654");

    /// MM Supervisor GUID for supervisor protocol testing
    pub const MM_SUPERVISOR: BinaryGuid = BinaryGuid::from_string("8C633B23-1260-4EA6-830F-7DDC97382111");
}

// Convenience re-exports for common usage
pub use test_guids::ECHO_HANDLER as TEST_COMMUNICATION_GUID;
