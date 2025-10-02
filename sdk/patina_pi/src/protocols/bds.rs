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

use r_efi::efi;

/// BDS Architectural Protocol GUID
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.2.1
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x665E3FF6, 0x46CC, 0x11d4, 0x9A, 0x38, &[0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);

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
    pub entry: BdsEntry,
}
