//! Performance-related GUID constants.
//!
//! GUIDs used by the EDK II Firmware Performance Data Table (FPDT) infrastructure and by the
//! Patina performance measurement interface.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;

/// EDKII FPDT (Firmware Performance Data Table) extended firmware performance GUID.
///
/// Used in the HOB list to mark a HOB as containing performance reports, in the
/// report-status-code guide for the FBPT address, and as the configuration-table GUID for the FBPT
/// address.
///
/// (`3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25`)
/// ```
/// # use patina::performance::guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID;
/// # assert_eq!(
/// #     "3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25",
/// #     format!("{}", EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID)
/// # );
/// ```
pub const EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID: BinaryGuid =
    BinaryGuid::from_string("3B387BFD-7ABC-4CF2-A0CA-B6A16C1B1B25");

/// Performance Protocol GUID.
///
/// This protocol provides a means of adding performance records to the Firmware Basic Boot
/// Performance Table (FBPT).
///
/// (`76B6BDFA-2ACD-4462-9E3F-CB58C969D937`)
/// ```
/// # use patina::performance::guid::PERFORMANCE_PROTOCOL_GUID;
/// # assert_eq!("76B6BDFA-2ACD-4462-9E3F-CB58C969D937", format!("{}", PERFORMANCE_PROTOCOL_GUID));
/// ```
pub const PERFORMANCE_PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("76B6BDFA-2ACD-4462-9E3F-CB58C969D937");
