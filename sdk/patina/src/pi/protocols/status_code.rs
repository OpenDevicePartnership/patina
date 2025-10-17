//! Status Code Protocol
//!
//! Provides the service required to report a status code to the platform firmware.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Runtime_Protocols.html#efi-status-code-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xD2B2B828, 0x0826, 0x48A7, 0xB3, 0xDF, &[0x98, 0x3C, 0x00, 0x60, 0x24, 0xF0]);

/// Status Code Type Definition.
///
pub type EfiStatusCodeType = u32;

/// Status Code Value Definition.
///
pub type EfiStatusCodeValue = u32;

/// The definition of the status code extended data header. The data will follow HeaderSize bytes from the
/// beginning of the structure and is Size bytes long.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section III-6.6.2.1
#[repr(C)]
pub struct EfiStatusCodeData {
    pub header_size: u16,
    pub size: u16,
    pub r#type: efi::Guid,
}

/// Provides an interface that a software module can call to report a status code.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-14.2.1
pub type ReportStatusCode =
    extern "efiapi" fn(u32, u32, u32, *const efi::Guid, *const EfiStatusCodeData) -> efi::Status;

/// Provides the service required to report a status code to the platform firmware.
/// This protocol must be produced by a runtime DXE driver.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-14.2.1
#[repr(C)]
pub struct Protocol {
    pub report_status_code: ReportStatusCode,
}
