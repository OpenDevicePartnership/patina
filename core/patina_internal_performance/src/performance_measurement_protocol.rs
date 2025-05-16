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

pub struct PerfId;

impl PerfId {
    pub const PERF_EVENT: u16 = 0x00;
    pub const MODULE_START: u16 = 0x01;
    pub const MODULE_END: u16 = 0x02;
    pub const MODULE_LOAD_IMAGE_START: u16 = 0x03;
    pub const MODULE_LOAD_IMAGE_END: u16 = 0x04;
    pub const MODULE_DB_START: u16 = 0x05;
    pub const MODULE_DB_END: u16 = 0x06;
    pub const MODULE_DB_SUPPORT_START: u16 = 0x07;
    pub const MODULE_DB_SUPPORT_END: u16 = 0x08;
    pub const MODULE_DB_STOP_START: u16 = 0x09;
    pub const MODULE_DB_STOP_END: u16 = 0x0A;
    pub const PERF_EVENT_SIGNAL_START: u16 = 0x10;
    pub const PERF_EVENT_SIGNAL_END: u16 = 0x11;
    pub const PERF_CALLBACK_START: u16 = 0x20;
    pub const PERF_CALLBACK_END: u16 = 0x21;
    pub const PERF_FUNCTION_START: u16 = 0x30;
    pub const PERF_FUNCTION_END: u16 = 0x31;
    pub const PERF_IN_MODULE_START: u16 = 0x40;
    pub const PERF_IN_MODULE_END: u16 = 0x41;
    pub const PERF_CROSS_MODULE_START: u16 = 0x50;
    pub const PERF_CROSS_MODULE_END: u16 = 0x51;

    pub fn fmt(id: u16) -> &'static str {
        match id {
            PerfId::PERF_EVENT => "PERF_EVENT",
            PerfId::MODULE_START => "MODULE_START",
            PerfId::MODULE_END => "MODULE_END",
            PerfId::MODULE_LOAD_IMAGE_START => "MODULE_LOAD_IMAGE_START",
            PerfId::MODULE_LOAD_IMAGE_END => "MODULE_LOAD_IMAGE_END",
            PerfId::MODULE_DB_START => "MODULE_DB_START",
            PerfId::MODULE_DB_END => "MODULE_DB_END",
            PerfId::MODULE_DB_SUPPORT_START => "MODULE_DB_SUPPORT_START",
            PerfId::MODULE_DB_SUPPORT_END => "MODULE_DB_SUPPORT_END",
            PerfId::MODULE_DB_STOP_START => "MODULE_DB_STOP_START",
            PerfId::MODULE_DB_STOP_END => "MODULE_DB_STOP_END",
            PerfId::PERF_EVENT_SIGNAL_START => "PERF_EVENT_SIGNAL_START",
            PerfId::PERF_EVENT_SIGNAL_END => "PERF_EVENT_SIGNAL_END",
            PerfId::PERF_CALLBACK_START => "PERF_CALLBACK_START",
            PerfId::PERF_CALLBACK_END => "PERF_CALLBACK_END",
            PerfId::PERF_FUNCTION_START => "PERF_FUNCTION_START",
            PerfId::PERF_FUNCTION_END => "PERF_FUNCTION_END",
            PerfId::PERF_IN_MODULE_START => "PERF_IN_MODULE_START",
            PerfId::PERF_IN_MODULE_END => "PERF_IN_MODULE_END",
            PerfId::PERF_CROSS_MODULE_START => "PERF_CROSS_MODULE_START",
            PerfId::PERF_CROSS_MODULE_END => "PERF_CROSS_MODULE_END",
            _ => "Unknown",
        }
    }
}

pub type CreateMeasurementProtocol = extern "efiapi" fn(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status;

pub struct EdkiiPerformanceMeasurement {
    pub create_performance_measurement: CreateMeasurementProtocol,
}

unsafe impl ProtocolInterface for EdkiiPerformanceMeasurement {
    const PROTOCOL_GUID: efi::Guid = EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID;
}
