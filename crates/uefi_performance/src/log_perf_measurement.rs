//! Module that defines all performance functions used to log performance records.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::{
    ffi::{c_char, c_void},
    ptr,
};

use alloc::ffi::CString;
use r_efi::efi;

use crate::{
    performance_measurement_protocol::{CreateMeasurement, PerfAttribute},
    KnownPerfId,
};

fn log_perf_measurement(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: Option<&str>,
    address: usize,
    identifier: u16,
    create_performance_measurement: CreateMeasurement,
) {
    let s = string
        .map(CString::new)
        .transpose()
        .expect("String should not contain 0 bytes.")
        .map_or(ptr::null(), |s| s.into_raw());

    // Safety: string parameter is expected to be a valid C string.
    _ = unsafe {
        (create_performance_measurement)(
            caller_identifier,
            guid,
            s,
            0,
            address,
            identifier as u32,
            PerfAttribute::PerfEntry,
        )
    };
}

fn start_perf_measurement(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    let string = if !token.is_null() {
        token
    } else if !module.is_null() {
        module
    } else {
        ptr::null()
    };
    // Safety: string parameter is expected to be a valid C string.
    unsafe {
        (create_performance_measurement)(handle, None, string, timestamp, 0, identifier, PerfAttribute::PerfStartEntry);
    }
}

fn end_perf_measurement(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    let string = if !token.is_null() {
        token
    } else if !module.is_null() {
        module
    } else {
        ptr::null()
    };
    // Safety: string parameter is expected to be a valid C string.
    unsafe {
        (create_performance_measurement)(handle, None, string, timestamp, 0, identifier, PerfAttribute::PerfEndEntry)
    };
}

pub fn perf_image_start_begin(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        0,
        KnownPerfId::ModuleStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_image_start_end(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleEnd.as_u16(), create_performance_measurement);
}

pub fn perf_load_image_begin(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        0,
        KnownPerfId::ModuleLoadImageStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_load_image_end(module_handle: efi::Handle, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        0,
        KnownPerfId::ModuleLoadImageEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_driver_binding_support_begin(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_driver_binding_support_end(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_driver_binding_start_begin(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_driver_binding_start_end(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_driver_binding_stop_begin(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_driver_binding_stop_end(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_event(event_string: &str, caller_id: &efi::Guid, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(event_string),
        0,
        KnownPerfId::PerfEvent.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_event_signal_begin(
    event_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_event_signal_end(
    event_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_callback_begin(
    trigger_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_callback_end(
    trigger_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_function_begin(fun_name: &str, caller_id: &efi::Guid, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_function_end(fun_name: &str, caller_id: &efi::Guid, create_performance_measurement: CreateMeasurement) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_in_module_begin(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_in_module_end(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_in_cross_module_begin(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_in_cross_module_end(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_cross_module_begin(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleStart.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_cross_module_end(
    measurement_str: &str,
    caller_id: &efi::Guid,
    create_performance_measurement: CreateMeasurement,
) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleEnd.as_u16(),
        create_performance_measurement,
    );
}

pub fn perf_start(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    create_performance_measurement: CreateMeasurement,
) {
    start_perf_measurement(handle, token, module, timestamp, 0, create_performance_measurement);
}

pub fn perf_end(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    create_performance_measurement: CreateMeasurement,
) {
    end_perf_measurement(handle, token, module, timestamp, 0, create_performance_measurement);
}

pub fn perf_start_ex(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    start_perf_measurement(handle, token, module, timestamp, identifier, create_performance_measurement);
}

pub fn perf_end_ex(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
    create_performance_measurement: CreateMeasurement,
) {
    end_perf_measurement(handle, token, module, timestamp, identifier, create_performance_measurement);
}
