//! A library that enables performance analysis of every step of the UEFI boot process.
//! The Performance library exports a protocol that can be used by other libraries or drivers to publish performance reports.
//! These reports are saved in the Firmware Basic Boot Performance Table (FBPT), so they can be extracted later from the operating system.
//!
//!  ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod _debug;
pub mod _status_code_runtime;
pub mod _utils;
pub mod performance_measurement_protocol;
pub mod performance_record;
pub mod performance_table;

use core::{
    ffi::{c_char, c_void},
    mem, ptr, slice,
    sync::atomic::{AtomicU32, Ordering},
};

use _status_code_runtime::{ReportStatusCode, StatusCodeRuntimeProtocol};
use _utils::c_char_ptr_from_str;
use alloc::{boxed::Box, string::String};

use r_efi::{
    efi,
    protocols::device_path::{Media, TYPE_MEDIA},
};

use performance_record::{
    extended::{
        DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
        GuidQwordStringEventRecord,
    },
    Iter, PerformanceRecordBuffer,
};

use mu_pi::hob::{GuidHob, Hob, HobList};

use performance_measurement_protocol::{
    EdkiiPerformanceMeasurement, EdkiiPerformanceMeasurementInterface, PerfAttribute, PerfId,
};
use performance_table::FBPT;

use r_efi::system::EVENT_GROUP_READY_TO_BOOT;

pub use mu_rust_helpers::function;
use mu_rust_helpers::perf_timer::{Arch, ArchFunctionality};

use uefi_device_path::DevicePathWalker;
use uefi_sdk::{
    boot_services::{event::EventType, tpl::Tpl, BootServices, StandardBootServices},
    guid,
    protocol::{DevicePath, DriverBinding, LoadedImage},
    runtime_services::StandardRuntimeServices,
    tpl_mutex::TplMutex,
};

static BOOT_SERVICES: StandardBootServices = StandardBootServices::new_uninit();
static RUNTIME_SERVICES: StandardRuntimeServices = StandardRuntimeServices::new_uninit();
static FBPT: TplMutex<FBPT> = TplMutex::new(&BOOT_SERVICES, Tpl::NOTIFY, FBPT::new());

static LOAD_IMAGE_COUNT: AtomicU32 = AtomicU32::new(0);

#[doc(hidden)]
pub const PERF_ENABLED: bool = cfg!(feature = "instrument_performance");

pub fn init_performance_lib(
    hob_list: &HobList,
    efi_boot_services: &efi::BootServices,
    efi_runtime_services: &efi::RuntimeServices,
) -> Result<(), efi::Status> {
    BOOT_SERVICES.initialize(efi_boot_services);
    RUNTIME_SERVICES.initialize(efi_runtime_services);

    let (pei_records, pei_load_image_count) = extract_pei_performance_records(hob_list)?;
    LOAD_IMAGE_COUNT.store(pei_load_image_count, Ordering::Relaxed);
    log::info!("{} PEI Records found.", pei_records.iter().count());
    FBPT.lock().set_records(pei_records);

    // Install the protocol interfaces for DXE performance library instance.
    BOOT_SERVICES
        .install_protocol_interface(
            None,
            &EdkiiPerformanceMeasurement,
            Box::new(EdkiiPerformanceMeasurementInterface { create_performance_measurement }),
        )
        .map_err(|(_, err)| err)?;

    // Register EndOfDxe event to allocate the boot performance table and report the table address through status code.
    BOOT_SERVICES.create_event_ex(
        EventType::NOTIFY_SIGNAL,
        Tpl::CALLBACK,
        Some(report_fpdt_record_buffer),
        &(),
        &guid::EVENT_GROUP_END_OF_DXE,
    )?;

    // Register ReadyToBoot event to update the boot performance table for SMM performance data.
    BOOT_SERVICES.create_event_ex(
        EventType::NOTIFY_SIGNAL,
        Tpl::CALLBACK,
        Some(update_boot_performance_table),
        &(),
        &EVENT_GROUP_READY_TO_BOOT,
    )?;

    // Install configuration table for performance property.
    BOOT_SERVICES.install_configuration_table(
        &guid::PERFORMANCE_PROTOCOL,
        Box::new(PerformanceProperty::new(Arch::cpu_count_frequency(), Arch::cpu_count_start(), Arch::cpu_count_end())),
    )?;
    Ok(())
}

fn extract_pei_performance_records(hob_list: &HobList) -> Result<(PerformanceRecordBuffer, u32), efi::Status> {
    let mut pei_records = PerformanceRecordBuffer::new();
    let mut pei_load_image_count = 0;

    for hob in hob_list.iter() {
        let guid_hob = match *hob {
            Hob::GuidHob(hob, _) if hob.name == guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE => hob,
            _ => continue,
        };
        let perf_header_ptr =
            unsafe { (guid_hob as *const _ as *const (u32, u32, u32)).byte_add(mem::size_of::<GuidHob>()) };
        let (size_of_all_entries, load_image_count, _hob_is_full) = unsafe { ptr::read_unaligned(perf_header_ptr) };
        let record_data_ptr = unsafe { perf_header_ptr.add(1) as *const u8 };
        let records_data_buffer = unsafe { slice::from_raw_parts(record_data_ptr, size_of_all_entries as usize) };

        pei_load_image_count += load_image_count;
        for r in Iter::new(records_data_buffer) {
            pei_records.push_record(r)?;
        }
    }
    Ok((pei_records, pei_load_image_count))
}

extern "efiapi" fn create_performance_measurement(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status {
    fn is_known_token(token: Option<&String>) -> bool {
        let Some(token) = token else {
            return false;
        };
        matches!(
            token.as_str(),
            "SEC"
                | "PEI"
                | "DXE"
                | "BDS"
                | "DB:Start:"
                | "DB:Support:"
                | "DB:Stop:"
                | "LoadImage:"
                | "StartImage:"
                | "PEIM"
        )
    }

    fn is_known_id(identifier: u16) -> bool {
        matches!(
            identifier,
            PerfId::MODULE_START
                | PerfId::MODULE_END
                | PerfId::MODULE_LOAD_IMAGE_START
                | PerfId::MODULE_LOAD_IMAGE_END
                | PerfId::MODULE_DB_START
                | PerfId::MODULE_DB_END
                | PerfId::MODULE_DB_SUPPORT_START
                | PerfId::MODULE_DB_SUPPORT_END
                | PerfId::MODULE_DB_STOP_START
                | PerfId::MODULE_DB_STOP_END
        )
    }

    fn get_fpdt_record_id(
        attribute: PerfAttribute,
        handle: *const c_void,
        string: Option<&String>,
    ) -> Result<u16, efi::Status> {
        if let Some(string) = string {
            let perf_id = match string.as_str() {
                "StartImage:" if attribute == PerfAttribute::PerfStartEntry => PerfId::MODULE_START,
                "StartImage:" => PerfId::MODULE_END,
                "LoadImage:" if attribute == PerfAttribute::PerfStartEntry => PerfId::MODULE_LOAD_IMAGE_START,
                "LoadImage:" => PerfId::MODULE_LOAD_IMAGE_END,
                "DB:Start:" if attribute == PerfAttribute::PerfStartEntry => PerfId::MODULE_DB_START,
                "DB:Start:" => PerfId::MODULE_DB_END,
                "DB:Support:" if attribute == PerfAttribute::PerfStartEntry => PerfId::MODULE_DB_SUPPORT_START,
                "DB:Support:" => PerfId::MODULE_DB_SUPPORT_END,
                "DB:Stop:" if attribute == PerfAttribute::PerfStartEntry => PerfId::MODULE_DB_STOP_START,
                "DB:Stop:" => PerfId::MODULE_DB_STOP_END,
                "PEI" | "DXE" | "BDS" if attribute == PerfAttribute::PerfStartEntry => PerfId::PERF_CROSS_MODULE_START,
                "PEI" | "DXE" | "BDS" => PerfId::PERF_CROSS_MODULE_END,
                _ if attribute == PerfAttribute::PerfStartEntry => PerfId::PERF_IN_MODULE_START,
                _ => PerfId::PERF_IN_MODULE_END,
            };
            Ok(perf_id)
        } else if !handle.is_null() {
            if attribute == PerfAttribute::PerfStartEntry {
                Ok(PerfId::PERF_IN_MODULE_START)
            } else {
                Ok(PerfId::PERF_IN_MODULE_END)
            }
        } else {
            Err(efi::Status::INVALID_PARAMETER)
        }
    }

    if !PERF_ENABLED {
        return efi::Status::SUCCESS;
    }

    let string = unsafe { _utils::string_from_c_char_ptr(string) };

    let mut perf_id = identifier as u16;
    if attribute != PerfAttribute::PerfEntry {
        if perf_id != 0 && is_known_id(perf_id) && !is_known_token(string.as_ref()) {
            return efi::Status::INVALID_PARAMETER;
        } else if perf_id != 0 && !is_known_id(perf_id) && !is_known_token(string.as_ref()) {
            if attribute == PerfAttribute::PerfStartEntry && ((perf_id & 0x000F) != 0) {
                perf_id &= 0xFFF0;
            } else if attribute == PerfAttribute::PerfEndEntry && ((perf_id & 0x000F) == 0) {
                perf_id += 1;
            }
        } else if perf_id == 0 {
            match get_fpdt_record_id(attribute, caller_identifier, string.as_ref()) {
                Ok(record_id) => perf_id = record_id,
                Err(status) => return status,
            }
        }
    }

    let cpu_count = Arch::cpu_count();
    let timestamp = match ticker {
        0 => (cpu_count as f64 / Arch::cpu_count_frequency() as f64 * 1_000_000_000_f64) as u64,
        1 => 0,
        ticker => (ticker as f64 / Arch::cpu_count_frequency() as f64 * 1_000_000_000_f64) as u64,
    };

    let controller_handle = address as efi::Handle;

    match perf_id {
        PerfId::MODULE_START | PerfId::MODULE_END => {
            if let Ok((_, guid)) = get_module_info_from_handle(
                &BOOT_SERVICES,
                caller_identifier as *mut c_void,
                controller_handle,
                perf_id,
            ) {
                let record = GuidEventRecord::new(perf_id, 0, timestamp, guid);
                _ = &FBPT.lock().add_record(record);
            }
        }
        PerfId::MODULE_LOAD_IMAGE_START | PerfId::MODULE_LOAD_IMAGE_END => {
            if perf_id == PerfId::MODULE_LOAD_IMAGE_START {
                LOAD_IMAGE_COUNT.fetch_add(1, Ordering::Relaxed);
            }
            if let Ok((_, guid)) = get_module_info_from_handle(
                &BOOT_SERVICES,
                caller_identifier as *mut c_void,
                controller_handle,
                perf_id,
            ) {
                let record = GuidQwordEventRecord::new(
                    perf_id,
                    timestamp,
                    guid,
                    LOAD_IMAGE_COUNT.load(Ordering::Relaxed) as u64,
                );
                _ = &FBPT.lock().add_record(record);
            }
        }
        PerfId::MODULE_DB_SUPPORT_START
        | PerfId::MODULE_DB_SUPPORT_END
        | PerfId::MODULE_DB_STOP_START
        | PerfId::MODULE_DB_STOP_END
        | PerfId::MODULE_DB_START => {
            if let Ok((_, guid)) = get_module_info_from_handle(
                &BOOT_SERVICES,
                caller_identifier as *mut c_void,
                controller_handle,
                perf_id,
            ) {
                let record = GuidQwordEventRecord::new(perf_id, timestamp, guid, address as u64);
                _ = &FBPT.lock().add_record(record);
            }
        }
        PerfId::MODULE_DB_END => {
            if let Ok((Some(module_name), guid)) = get_module_info_from_handle(
                &BOOT_SERVICES,
                caller_identifier as *mut c_void,
                controller_handle,
                perf_id,
            ) {
                let record = GuidQwordStringEventRecord::new(perf_id, 0, timestamp, guid, address as u64, &module_name);
                _ = &FBPT.lock().add_record(record);
            }
            // TODO something to do if address is not 0 need example to continue development. (https://github.com/OpenDevicePartnership/uefi-dxe-core/issues/194)
        }
        PerfId::PERF_EVENT_SIGNAL_START
        | PerfId::PERF_EVENT_SIGNAL_END
        | PerfId::PERF_CALLBACK_START
        | PerfId::PERF_CALLBACK_END => {
            let (Some(string), Some(guid_2)) = (string, guid) else {
                return efi::Status::INVALID_PARAMETER;
            };
            let guid_1 = *unsafe { (caller_identifier as *const efi::Guid).as_ref() }.unwrap();
            let record = DualGuidStringEventRecord::new(perf_id, 0, timestamp, guid_1, *guid_2, string.as_str());
            _ = &FBPT.lock().add_record(record);
        }
        PerfId::PERF_EVENT
        | PerfId::PERF_FUNCTION_START
        | PerfId::PERF_FUNCTION_END
        | PerfId::PERF_IN_MODULE_START
        | PerfId::PERF_IN_MODULE_END
        | PerfId::PERF_CROSS_MODULE_START
        | PerfId::PERF_CROSS_MODULE_END => {
            let guid = *unsafe { (caller_identifier as *const efi::Guid).as_ref() }.unwrap();
            let record =
                DynamicStringEventRecord::new(perf_id, 0, timestamp, guid, string.as_deref().unwrap_or("unknown name"));
            _ = &FBPT.lock().add_record(record);
        }
        _ if attribute != PerfAttribute::PerfEntry => {
            let (module_name, guid) = if let Ok((Some(module_name), guid)) = get_module_info_from_handle(
                &BOOT_SERVICES,
                caller_identifier as *mut c_void,
                controller_handle,
                perf_id,
            ) {
                (module_name, guid)
            } else if let Some(string) = string {
                let guid = *unsafe { (caller_identifier as *const efi::Guid).as_ref() }.unwrap();
                (string, guid)
            } else {
                let guid = *unsafe { (caller_identifier as *const efi::Guid).as_ref() }.unwrap();
                (String::from("unknown name"), guid)
            };
            let record = DynamicStringEventRecord::new(perf_id, 0, timestamp, guid, &module_name);
            _ = &FBPT.lock().add_record(record);
        }
        _ => {
            return efi::Status::INVALID_PARAMETER;
        }
    };

    efi::Status::SUCCESS
}

extern "efiapi" fn report_fpdt_record_buffer(_event: efi::Event, _ctx: &()) {
    let fbpt = &mut FBPT.lock();
    fbpt.report_table(&BOOT_SERVICES, &RUNTIME_SERVICES).expect("Failed to allocate table.");

    const EFI_SOFTWARE: u32 = 0x03000000;
    const EFI_PROGRESS_CODE: u32 = 0x00000001;
    const EFI_SOFTWARE_DXE_BS_DRIVER: u32 = EFI_SOFTWARE | 0x00050000;

    let status = StatusCodeRuntimeProtocol::report_status_code(
        &BOOT_SERVICES,
        EFI_PROGRESS_CODE,
        EFI_SOFTWARE_DXE_BS_DRIVER,
        0,
        None,
        efi::Guid::clone(&guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE),
        fbpt.fbpt_address(),
    );
    if status.is_err() {
        log::error!("Fail to report FBPT table.");
    }

    let status = unsafe {
        BOOT_SERVICES.install_configuration_table_unchecked(
            &guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
            fbpt.fbpt_address() as *mut c_void,
        )
    };
    if status.is_err() {
        log::error!("Fail to install configuration table for FPDT firmware performance.");
    }
}

extern "efiapi" fn update_boot_performance_table(_event: efi::Event, _: &()) {
    // TODO: There is a task in the backlog for this.
}

#[repr(C)]
pub struct PerformanceProperty {
    revision: u32,
    reserved: u32,
    frequency: u64,
    timer_start_value: u64,
    timer_end_value: u64,
}

impl PerformanceProperty {
    pub fn new(frequency: u64, timer_start_value: u64, timer_end_value: u64) -> Self {
        Self { revision: 0x1, reserved: 0, frequency, timer_start_value, timer_end_value }
    }
}

fn get_module_info_from_handle(
    boot_services: &impl BootServices,
    handle: efi::Handle,
    controller_handle: efi::Handle,
    perf_id: u16,
) -> Result<(Option<String>, efi::Guid), efi::Status> {
    let mut guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);

    let loaded_image_protocol = 'find_loaded_image_protocol: {
        if let Ok(loaded_image_protocol) = unsafe { boot_services.handle_protocol(handle, &LoadedImage) } {
            break 'find_loaded_image_protocol Some(loaded_image_protocol);
        }
        if let Ok(driver_binding_protocol) = unsafe {
            boot_services.open_protocol(
                handle,
                &DriverBinding,
                ptr::null_mut(),
                ptr::null_mut(),
                efi::OPEN_PROTOCOL_GET_PROTOCOL,
            )
        } {
            if let Ok(loaded_image_protocol) =
                unsafe { boot_services.handle_protocol(driver_binding_protocol.image_handle, &LoadedImage) }
            {
                break 'find_loaded_image_protocol Some(loaded_image_protocol);
            }
        }
        None
    };

    let mut _module_guid_is_ffs = false;
    if let Some(loaded_image) = loaded_image_protocol {
        if let Some(file_path) = unsafe { loaded_image.file_path.as_ref() } {
            if file_path.r#type == TYPE_MEDIA && file_path.sub_type == Media::SUBTYPE_PIWG_FIRMWARE_FILE {
                _module_guid_is_ffs = true;
                guid = unsafe { ptr::read(loaded_image.file_path.add(1) as *const efi::Guid) }
            }
        };

        if perf_id == PerfId::MODULE_DB_END
            || perf_id == PerfId::MODULE_DB_SUPPORT_END
            || perf_id == PerfId::MODULE_DB_STOP_END
        {
            let device_path_protocol = unsafe { boot_services.handle_protocol(controller_handle, &DevicePath) };
            if let Ok(device_path_protocol) = device_path_protocol {
                let device_path_string: String = unsafe { DevicePathWalker::new(device_path_protocol) }.into();
                return Ok((Some(device_path_string), guid));
            }
        }

        let _image_bytes = unsafe {
            slice::from_raw_parts(loaded_image.image_base as *const _ as *const u8, loaded_image.image_size as usize)
        };
        // TODO: Find Module name in handle (image_bytes) (https://github.com/OpenDevicePartnership/uefi-dxe-core/issues/187).

        return Ok((Some(String::from("TODO Get name from UefiPeInfo")), guid));
    }

    // Method 2 - Get the name string from ComponentName2
    // TODO: https://github.com/OpenDevicePartnership/uefi-dxe-core/issues/192

    // Method 3 - Get the name string from FFS UI Section.
    // TODO: https://github.com/OpenDevicePartnership/uefi-dxe-core/issues/193

    Ok((None, guid))
}

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
    string: *const c_char,
    address: usize,
    identifier: u16,
) {
    create_performance_measurement(
        caller_identifier,
        guid,
        string,
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, PerfId::MODULE_START);
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, PerfId::MODULE_END);
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, PerfId::MODULE_LOAD_IMAGE_START);
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, PerfId::MODULE_LOAD_IMAGE_END);
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
    log_perf_measurement(module_handle, None, ptr::null(), controller_handle as usize, PerfId::MODULE_DB_SUPPORT_START);
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
    log_perf_measurement(module_handle, None, ptr::null(), controller_handle as usize, PerfId::MODULE_DB_SUPPORT_END);
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
    log_perf_measurement(module_handle, None, ptr::null(), controller_handle as usize, PerfId::MODULE_DB_START);
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
    log_perf_measurement(module_handle, None, ptr::null(), controller_handle as usize, PerfId::MODULE_DB_END);
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
    log_perf_measurement(module_handle, None, ptr::null(), controller_handle as usize, PerfId::MODULE_DB_STOP_START);
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
    log_perf_measurement(module_handle, None, ptr::null(), controller_handle as usize, PerfId::MODULE_DB_STOP_END);
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
        c_char_ptr_from_str(event_string),
        0,
        PerfId::PERF_EVENT,
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
        c_char_ptr_from_str(fun_name),
        0,
        PerfId::PERF_EVENT_SIGNAL_START,
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
        c_char_ptr_from_str(fun_name),
        0,
        PerfId::PERF_EVENT_SIGNAL_END,
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
        c_char_ptr_from_str(fun_name),
        0,
        PerfId::PERF_CALLBACK_START,
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
        c_char_ptr_from_str(fun_name),
        0,
        PerfId::PERF_CALLBACK_END,
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
        c_char_ptr_from_str(fun_name),
        0,
        PerfId::PERF_FUNCTION_START,
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
        c_char_ptr_from_str(fun_name),
        0,
        PerfId::PERF_FUNCTION_END,
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
        c_char_ptr_from_str(measurement_str),
        0,
        PerfId::PERF_IN_MODULE_START,
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
        c_char_ptr_from_str(measurement_str),
        0,
        PerfId::PERF_IN_MODULE_END,
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
        c_char_ptr_from_str(measurement_str),
        0,
        PerfId::PERF_CROSS_MODULE_START,
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
        c_char_ptr_from_str(measurement_str),
        0,
        PerfId::PERF_CROSS_MODULE_END,
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
