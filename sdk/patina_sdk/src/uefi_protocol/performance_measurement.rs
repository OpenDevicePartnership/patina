//! Definition of [`EdkiiPerformanceMeasurement`].
//!
//! This Protocol is use to log performance measurement records.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ffi::{c_char, c_void},
    fmt::Debug,
    option::Option,
};

use r_efi::efi;

use crate::uefi_protocol::ProtocolInterface;

/// GUID for the EDKII Performance Measurement Protocol.
pub const EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xc85d06be, 0x5f75, 0x48ce, 0xa8, 0x0f, &[0x12, 0x36, 0xba, 0x3b, 0x87, 0xb1]);

/// GUID for the EDKII SMM Performance Measurement Protocol.
pub const EDKII_SMM_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xd56b6d73, 0x1a7b, 0x4015, 0x9b, 0xb4, &[0x7b, 0x07, 0x17, 0x29, 0xed, 0x24]);

/// The attribute of the measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub enum PerfAttribute {
    /// A PERF_START/PERF_START_EX record.
    PerfStartEntry,
    /// A PERF_END/PERF_END_EX record.
    PerfEndEntry,
    /// A general performance record.
    PerfEntry,
}

/// Function to create performance record with event description and a timestamp.
pub type CreateMeasurement = unsafe extern "efiapi" fn(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status;

/// EDKII defined Performance Measurement Protocol structure.
pub struct EdkiiPerformanceMeasurement {
    /// Function to create performance record with event description and a timestamp.
    pub create_performance_measurement: CreateMeasurement,
}

unsafe impl ProtocolInterface for EdkiiPerformanceMeasurement {
    const PROTOCOL_GUID: efi::Guid = EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID;
}
