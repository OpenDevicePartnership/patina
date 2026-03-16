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

/// Status Code Runtime Protocol GUID.
pub const PROTOCOL_GUID: crate::BinaryGuid = crate::BinaryGuid::from_string("D2B2B828-0826-48A7-B3DF-983C006024F0");

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
    /// Size of the status code data header.
    pub header_size: u16,
    /// Size of the status code data.
    pub size: u16,
    /// GUID identifying the type of status code data.
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
    /// Function to report status codes.
    pub report_status_code: ReportStatusCode,
}
