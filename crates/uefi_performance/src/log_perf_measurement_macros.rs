// #![cfg(not(tarpaulin_include))]

use alloc::ffi::CString;
use core::{ptr, ffi::{c_void, c_char}};

use r_efi::efi;

use crate::KnownPerfId;

use crate::{create_performance_measurement, performance_measurement_protocol::PerfAttribute};


macro_rules! __log_perf_measurement {
    (
        $caller_identifier:expr,
        $guid:expr,
        $string:literal,
        $ticker:expr,
        $identifier:expr,
        $perf_id:expr
    ) => {{
        if $crate::PERF_ENABLED {
            let string = concat!($string, "\0").as_ptr() as *const c_char;
            create_performance_measurement(caller_identifier, guid, string, ticker, 0, identifier, perf_id);
        }
    }};
}

fn log_perf_measurement(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: Option<&str>,
    address: usize,
    identifier: u16,
) {
    create_performance_measurement(
        caller_identifier,
        guid,
        string
            .map(|s| CString::new(s).map_or(core::ptr::null(), |c_string| c_string.into_raw()))
            .unwrap_or(ptr::null()),
        0,
        address,
        identifier as u32,
        PerfAttribute::PerfEntry,
    );
}

fn start_perf_measurement(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
) {
    let string = if !token.is_null() {
        token
    } else if !module.is_null() {
        module
    } else {
        ptr::null()
    };
    create_performance_measurement(handle, None, string, timestamp, 0, identifier, PerfAttribute::PerfStartEntry);
}

fn end_perf_measurement(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
) {
    let string = if !token.is_null() {
        token
    } else if !module.is_null() {
        module
    } else {
        ptr::null()
    };
    create_performance_measurement(handle, None, string, timestamp, 0, identifier, PerfAttribute::PerfEndEntry);
}

#[macro_export]
macro_rules! perf_image_start_begin {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_image_start_begin($caller_id);
        }
    };
}

pub fn _perf_image_start_begin(module_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleStart.as_u16());
}

#[macro_export]
macro_rules! perf_image_start_end {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_image_start_end($caller_id);
        }
    };
}

pub fn _perf_image_start_end(module_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleEnd.as_u16());
}

#[macro_export]
macro_rules! perf_load_image_begin {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_load_image_begin($caller_id);
        }
    };
}

pub fn _perf_load_image_begin(module_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleLoadImageStart.as_u16());
}

#[macro_export]
macro_rules! perf_load_image_end {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_load_image_end($caller_id);
        }
    };
}

pub fn _perf_load_image_end(module_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleLoadImageEnd.as_u16());
}

#[macro_export]
macro_rules! perf_driver_binding_support_begin {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_driver_binding_support_begin($caller_id, $address);
        }
    };
}

pub fn _perf_driver_binding_support_begin(module_handle: efi::Handle, controller_handle: efi::Handle) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_driver_binding_support_end {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_driver_binding_support_end($caller_id, $address);
        }
    };
}

pub fn _perf_driver_binding_support_end(module_handle: efi::Handle, controller_handle: efi::Handle) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportEnd.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_driver_binding_start_begin {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_driver_binding_start_begin($caller_id, $address);
        }
    };
}

pub fn _perf_driver_binding_start_begin(module_handle: efi::Handle, controller_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, controller_handle as usize, KnownPerfId::ModuleDbStart.as_u16());
}

#[macro_export]
macro_rules! perf_driver_binding_start_end {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_driver_binding_start_end($caller_id, $address);
        }
    };
}

pub fn _perf_driver_binding_start_end(module_handle: efi::Handle, controller_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, controller_handle as usize, KnownPerfId::ModuleDbEnd.as_u16());
}

#[macro_export]
macro_rules! perf_driver_binding_stop_begin {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_driver_binding_stop_begin($caller_id, $address);
        }
    };
}

pub fn _perf_driver_binding_stop_begin(module_handle: efi::Handle, controller_handle: efi::Handle) {
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_driver_binding_stop_end {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_driver_binding_stop_end($caller_id, $address);
        }
    };
}

pub fn _perf_driver_binding_stop_end(module_handle: efi::Handle, controller_handle: efi::Handle) {
    log_perf_measurement(module_handle, None, None, controller_handle as usize, KnownPerfId::ModuleDbStopEnd.as_u16());
}

#[macro_export]
macro_rules! perf_event {
    ($event_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_event($event_guid, $crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_event(event_string: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(event_string),
        0,
        KnownPerfId::PerfEvent.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_event_signal_begin {
    ($event_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_event_signal_begin($event_guid, $crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_event_signal_begin(event_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_event_signal_end {
    ($event_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_event_signal_end($event_guid, $crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_event_signal_end(event_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalEnd.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_callback_begin {
    ($trigger_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_callback_begin($trigger_guid, $crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_callback_begin(trigger_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_callback_end {
    ($trigger_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_callback_end($trigger_guid, $crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_callback_end(trigger_guid: &efi::Guid, fun_name: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackEnd.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_function_begin {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_function_begin($crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_function_begin(fun_name: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_function_end {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_function_end($crate::function!(), $caller_id)
        }
    };
}

pub fn _perf_function_end(fun_name: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionEnd.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_in_module_begin {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_in_module_begin($measurement_str, $caller_id)
        }
    };
}

pub fn _perf_in_module_begin(measurement_str: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_in_module_end {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_in_module_end($measurement_str, $caller_id)
        }
    };
}

pub fn _perf_in_module_end(measurement_str: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleEnd.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_in_cross_module_begin {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_in_cross_module_begin($measurement_str, $caller_id)
        }
    };
}

pub fn _perf_in_cross_module_begin(measurement_str: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleStart.as_u16(),
    );
}

#[macro_export]
macro_rules! perf_cross_module_end {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            $crate::_perf_cross_module_end($measurement_str, $caller_id)
        }
    };
}

pub fn _perf_cross_module_end(measurement_str: &str, caller_id: &efi::Guid) {
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleEnd.as_u16(),
    );
}

pub fn perf_start(handle: efi::Handle, token: *const c_char, module: *const c_char, timestamp: u64) {
    start_perf_measurement(handle, token, module, timestamp, 0);
}

pub fn perf_end(handle: efi::Handle, token: *const c_char, module: *const c_char, timestamp: u64) {
    end_perf_measurement(handle, token, module, timestamp, 0);
}

pub fn perf_start_ex(
    handle: efi::Handle,
    token: *const c_char,
    module: *const c_char,
    timestamp: u64,
    identifier: u32,
) {
    start_perf_measurement(handle, token, module, timestamp, identifier);
}

pub fn perf_end_ex(handle: efi::Handle, token: *const c_char, module: *const c_char, timestamp: u64, identifier: u32) {
    end_perf_measurement(handle, token, module, timestamp, identifier);
}