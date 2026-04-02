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

/// MM Supervisor constants and definitions for testing
///
/// Note: These values are only used for testing. They're not meant to be
/// accurate or used in production code.
pub mod mm_supv {
    /// Mock supervisor version for testing
    pub const VERSION: u32 = 0x00130008;

    /// Mock supervisor patch level for testing
    pub const PATCH_LEVEL: u32 = 0x00010001;
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
