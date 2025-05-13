//! Definition of [`EdkiiPerformanceMeasurement`].
//!
//! This Protocol is use to log performance measurement records.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::{
    ffi::{c_char, c_void},
    fmt::Debug,
    option::Option,
};

use r_efi::efi;

use patina_sdk::protocol::ProtocolInterface;

pub const EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xc85d06be, 0x5f75, 0x48ce, 0xa8, 0x0f, &[0x12, 0x36, 0xba, 0x3b, 0x87, 0xb1]);
pub const EDKII_SMM_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xd56b6d73, 0x1a7b, 0x4015, 0x9b, 0xb4, &[0x7b, 0x07, 0x17, 0x29, 0xed, 0x24]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub enum PerfAttribute {
    PerfStartEntry,
    PerfEndEntry,
    PerfEntry,
}

pub type CreateMeasurement = unsafe extern "efiapi" fn(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status;

pub struct EdkiiPerformanceMeasurement {
    pub create_performance_measurement: CreateMeasurement,
}

unsafe impl ProtocolInterface for EdkiiPerformanceMeasurement {
    const PROTOCOL_GUID: efi::Guid = EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID;
}
