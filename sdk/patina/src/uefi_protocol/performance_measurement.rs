//! Definition of [`EdkiiPerformanceMeasurement`].
//!
//! This Protocol is use to log performance measurement records.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ffi::{c_char, c_void},
    fmt::Debug,
    option::Option,
};

use r_efi::efi;

use crate::{performance::measurement::CallerIdentifier, uefi_protocol::ProtocolInterface};

/// GUID for the EDKII Performance Measurement Protocol.
pub const EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("C85D06BE-5F75-48CE-A80F-1236BA3B87B1");

/// GUID for the EDKII SMM Performance Measurement Protocol.
pub const EDKII_SMM_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("D56B6D73-1A7B-4015-9BB4-7B071729ED24");

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
pub type CreateMeasurementUefi = unsafe extern "efiapi" fn(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status;

/// Function to log performance record with event description and a timestamp.
pub type CreateMeasurement = fn(
    caller_identifier: CallerIdentifier,
    guid: Option<&efi::Guid>,
    string: Option<&str>,
    ticker: u64,
    address: usize,
    identifier: u16,
    attribute: PerfAttribute,
) -> Result<(), crate::performance::error::Error>;

/// EDKII defined Performance Measurement Protocol structure.
pub struct EdkiiPerformanceMeasurement {
    /// Function to create performance record with event description and a timestamp.
    pub create_performance_measurement: CreateMeasurementUefi,
}

// SAFETY: EdkiiPerformanceMeasurement implements the EDK II Performance Measurement protocol interface.
// The PROTOCOL_GUID matches the EDK II defined value. The protocol structure layout matches the protocol
// interface requirements.
unsafe impl ProtocolInterface for EdkiiPerformanceMeasurement {
    const PROTOCOL_GUID: crate::BinaryGuid = EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID;
}
