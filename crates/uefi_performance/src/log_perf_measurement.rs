//! Module that define every performance macro used to log performance records.
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

use r_efi::efi;

use uefi_sdk::{boot_services::BootServices, tpl_mutex::TplMutex};

use crate::{
    performance_table::FirmwareBasicBootPerfTable, KnownPerfId, _create_performance_measurement,
    create_performance_measurement, performance_measurement_protocol::PerfAttribute,
};

fn log_perf_measurement<B, F>(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: Option<&str>,
    address: usize,
    identifier: u16,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    _ = _create_performance_measurement(
        caller_identifier,
        guid,
        string,
        0,
        address,
        identifier,
        PerfAttribute::PerfEntry,
        boot_services,
        fbpt,
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
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_image_start_begin($caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_image_start_begin<B, F>(module_handle: efi::Handle, boot_services: &B, fbpt: &TplMutex<'static, F, B>)
where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleStart.as_u16(), boot_services, fbpt);
}

#[macro_export]
macro_rules! perf_image_start_end {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_image_start_end($caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_image_start_end<F, B>(module_handle: efi::Handle, boot_services: &B, fbpt: &TplMutex<'static, F, B>)
where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleEnd.as_u16(), boot_services, fbpt);
}

#[macro_export]
macro_rules! perf_load_image_begin {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_load_image_begin($caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_load_image_begin<F, B>(module_handle: efi::Handle, boot_services: &B, fbpt: &TplMutex<'static, F, B>)
where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleLoadImageStart.as_u16(), boot_services, fbpt);
}

#[macro_export]
macro_rules! perf_load_image_end {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_load_image_end($caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_load_image_end<B, F>(module_handle: efi::Handle, boot_services: &B, fbpt: &TplMutex<'static, F, B>)
where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(module_handle, None, None, 0, KnownPerfId::ModuleLoadImageEnd.as_u16(), boot_services, fbpt);
}

#[macro_export]
macro_rules! perf_driver_binding_support_begin {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_driver_binding_support_begin($caller_id, $address, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_driver_binding_support_begin<B, F>(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_driver_binding_support_end {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_driver_binding_support_end($caller_id, $address, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_driver_binding_support_end<B, F>(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbSupportEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_driver_binding_start_begin {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_driver_binding_start_begin($caller_id, $address, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_driver_binding_start_begin<B, F>(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_driver_binding_start_end {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_driver_binding_start_end($caller_id, $address, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_driver_binding_start_end<B, F>(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_driver_binding_stop_begin {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_driver_binding_stop_begin($caller_id, $address, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_driver_binding_stop_begin<B, F>(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_driver_binding_stop_end {
    ($caller_id:expr, $address:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_driver_binding_stop_end($caller_id, $address, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_driver_binding_stop_end<B, F>(
    module_handle: efi::Handle,
    controller_handle: efi::Handle,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        module_handle,
        None,
        None,
        controller_handle as usize,
        KnownPerfId::ModuleDbStopEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_event {
    ($event_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_event($event_guid, $crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_event<B, F>(event_string: &str, caller_id: &efi::Guid, boot_services: &B, fbpt: &TplMutex<'static, F, B>)
where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(event_string),
        0,
        KnownPerfId::PerfEvent.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_event_signal_begin {
    ($event_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_event_signal_begin($event_guid, $crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_event_signal_begin<B, F>(
    event_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_event_signal_end {
    ($event_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_event_signal_end($event_guid, $crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_event_signal_end<B, F>(
    event_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(event_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfEventSignalEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_callback_begin {
    ($trigger_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_callback_begin($trigger_guid, $crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_callback_begin<B, F>(
    trigger_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_callback_end {
    ($trigger_guid:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_callback_end($trigger_guid, $crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_callback_end<B, F>(
    trigger_guid: &efi::Guid,
    fun_name: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        Some(trigger_guid),
        Some(fun_name),
        0,
        KnownPerfId::PerfCallbackEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_function_begin {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_function_begin($crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_function_begin<B, F>(
    fun_name: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_function_end {
    ($caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_function_end($crate::function!(), $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_function_end<B, F>(
    fun_name: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(fun_name),
        0,
        KnownPerfId::PerfFunctionEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_in_module_begin {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_in_module_begin($measurement_str, $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_in_module_begin<B, F>(
    measurement_str: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_in_module_end {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_in_module_end($measurement_str, $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_in_module_end<B, F>(
    measurement_str: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfInModuleEnd.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_in_cross_module_begin {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_in_cross_module_begin($measurement_str, $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_in_cross_module_begin<B, F>(
    measurement_str: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleStart.as_u16(),
        boot_services,
        fbpt,
    );
}

#[macro_export]
macro_rules! perf_cross_module_end {
    ($measurement_str:expr, $caller_id:expr) => {
        if $crate::PERF_ENABLED {
            if let Some((boot_services, fbpt)) = $crate::get_static_state() {
                $crate::_perf_cross_module_end($measurement_str, $caller_id, boot_services, fbpt);
            }
        }
    };
}

pub fn _perf_cross_module_end<B, F>(
    measurement_str: &str,
    caller_id: &efi::Guid,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) where
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    log_perf_measurement(
        caller_id as *const efi::Guid as *mut c_void,
        None,
        Some(measurement_str),
        0,
        KnownPerfId::PerfCrossModuleEnd.as_u16(),
        boot_services,
        fbpt,
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
