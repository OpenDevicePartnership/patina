//! Patina GUID constants.
//!
//! Firmware identity GUIDs used by the Patina DXE Core and its components.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// Rust equivalent to `gEfiCallerIdGuid` from AutoGen.c in edk2.
///
/// The edk2 build system will populate the `FILE_GUID` environment variable with the module INF GUID.
/// A zero-GUID is generated as a backup to support test case usage.
///
/// This should only be used by Rust code using the Patina SDK in code built in the edk2 build system.
pub const CALLER_ID: BinaryGuid = BinaryGuid::from_string(match option_env!("FILE_GUID") {
    Some(guid_str) => guid_str,
    None => "00000000-0000-0000-0000-000000000000",
});

/// DXE Core Module GUID.
///
/// The FFS file GUID for the DXE Core module. Interfaces that depend upon a module GUID such as the
/// Memory Allocation Module HOB and status codes that are produced by the DXE Core module will use
/// this GUID.
///
/// Platforms that integrate the DXE Core module into their firmware volumes should use this GUID to
/// identify the DXE Core FFS file.
///
/// (`23C9322F-2AF2-476A-BC4C-26BC88266C71`)
/// ```
/// # use patina::guid::DXE_CORE_ID;
/// # assert_eq!("23C9322F-2AF2-476A-BC4C-26BC88266C71", format!("{}", DXE_CORE_ID));
/// ```
pub const DXE_CORE_ID: BinaryGuid = BinaryGuid::from_string("23C9322F-2AF2-476A-BC4C-26BC88266C71");
