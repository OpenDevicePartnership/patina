//! Boot Device Selection (BDS) Architectural Protocol
//!
//! Transfers control from the DXE phase to an operating system or system utility.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#boot-device-selection-bds-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// BDS Architectural Protocol GUID
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.2.1
pub const PROTOCOL_GUID: crate::BinaryGuid = crate::BinaryGuid::from_string("665E3FF6-46CC-11D4-9A38-0090273FC14D");

/// Performs Boot Device Selection (BDS) and transfers control from the DXE Foundation to the selected boot device.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.2.2
pub type BdsEntry = extern "efiapi" fn(*mut Protocol);

/// Transfers control from the DXE phase to an operating system or system utility.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.2.1
#[repr(C)]
pub struct Protocol {
    /// BDS architectural protocol entry point.
    pub entry: BdsEntry,
}
