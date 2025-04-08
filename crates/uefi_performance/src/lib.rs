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
mod _smm;
pub mod _status_code_runtime_protocol;
pub mod _utils;
pub mod performance_measurement_protocol;
pub mod performance_record;
pub mod performance_table;

use _status_code_runtime_protocol::{ReportStatusCode, StatusCodeRuntimeProtocol};
use alloc::vec::Vec;
use core::{
    convert::TryFrom,
    ffi::{c_char, c_void},
    fmt::Debug,
    mem::{self, MaybeUninit},
    option::Option::{self, None},
    ptr,
    result::Result::{self, Err, Ok},
    slice,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use _utils::c_char_ptr_from_str;
use alloc::{boxed::Box, string::String};

use r_efi::{
    efi::{self, Guid},
    protocols::device_path::{Media, TYPE_MEDIA},
};

use performance_record::{
    extended::{
        DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
        GuidQwordStringEventRecord,
    },
    known_records::{KnownPerfId, KnownPerfToken},
    Iter, PerformanceRecordBuffer,
};

use mu_pi::hob::{Hob, HobList};

use performance_measurement_protocol::{
    EdkiiPerformanceMeasurement, EdkiiPerformanceMeasurementInterface, PerfAttribute,
};
use performance_table::FBPT;

use r_efi::system::EVENT_GROUP_READY_TO_BOOT;

pub use mu_rust_helpers::function;
use mu_rust_helpers::perf_timer::{Arch, ArchFunctionality};

use _smm::{
    CommunicateProtocol, SmmCommunicationRegionTable, SmmFpdtGetRecordDataByOffset, SmmFpdtGetRecordSize,
    EDKII_PI_SMM_COMMUNICATION_REGION_TABLE_GUID,
};
use scroll::Pread;

use uefi_sdk::{
    boot_services::{event::EventType, tpl::Tpl, BootServices, StandardBootServices},
    component::IntoComponent,
    error::EfiError,
    guid,
    protocol::{DriverBinding, LoadedImage},
    runtime_services::{RuntimeServices, StandardRuntimeServices},
    tpl_mutex::TplMutex,
};

#[doc(hidden)]
pub const PERF_ENABLED: bool = cfg!(feature = "instrument_performance");

static IS_PERF_READY: AtomicBool = AtomicBool::new(false);

static LOAD_IMAGE_COUNT: AtomicU32 = AtomicU32::new(0);

static BOOT_SERVICES: MaybeUninit<&'static StandardBootServices> = MaybeUninit::zeroed();
static FBPT: MaybeUninit<TplMutex<FBPT>> = MaybeUninit::zeroed();

#[derive(IntoComponent)]
pub struct PerformanceLibComponent;

impl PerformanceLibComponent {
    pub fn new() -> Self {
        Self
    }

    pub fn entry_point(
        self,
        boot_services: &'static StandardBootServices,
        runtime_services: &'static StandardRuntimeServices,
        system_table: &'static efi::SystemTable,
        hob_list: &'static HobList,
    ) -> Result<(), EfiError> {
        let (pei_perf_records, pei_load_image_count) = extract_pei_performance_records(hob_list).unwrap_or_else(|_| {
            log::error!("Performance Lib: Error while trying to extract pei performance records");
            (PerformanceRecordBuffer::new(), 0)
        });

        LOAD_IMAGE_COUNT.store(pei_load_image_count, Ordering::Relaxed);
        log::info!("Performance Lib: {} PEI performance records found.", pei_perf_records.iter().count());

        let mut fbpt = FBPT::new();
        fbpt.set_records(pei_perf_records);

        // SAFETY: This is safe because it is the entry point and no one is reading these value yet.
        unsafe {
            ptr::write(BOOT_SERVICES.as_ptr() as *mut &'static StandardBootServices, boot_services);
            ptr::write(FBPT.as_ptr() as *mut TplMutex<FBPT>, TplMutex::new(boot_services, Tpl::NOTIFY, fbpt));
        }
        IS_PERF_READY.store(true, Ordering::Relaxed);

        // Install the protocol interfaces for DXE performance library instance.
        boot_services
            .install_protocol_interface(
                None,
                &EdkiiPerformanceMeasurement,
                Box::new(EdkiiPerformanceMeasurementInterface { create_performance_measurement }),
            )
            .map_err(|(_, err)| err)?;

        // Register EndOfDxe event to allocate the boot performance table and report the table address through status code.
        boot_services.create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(report_fpdt_record_buffer),
            Box::new((boot_services, runtime_services)),
            &guid::EVENT_GROUP_END_OF_DXE,
        )?;

        // Register ReadyToBoot event to update the boot performance table for SMM performance data.
        boot_services.create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(fetch_and_add_smm_performance_records),
            Box::new((boot_services, system_table)),
            &EVENT_GROUP_READY_TO_BOOT,
        )?;

        // Install configuration table for performance property.
        boot_services.install_configuration_table(
            &guid::PERFORMANCE_PROTOCOL,
            Box::new(PerformanceProperty::new(Arch::perf_frequency(), Arch::cpu_count_start(), Arch::cpu_count_end())),
        )?;

        Ok(())
    }
}

fn extract_pei_performance_records(hob_list: &HobList) -> Result<(PerformanceRecordBuffer, u32), efi::Status> {
    let mut pei_records = PerformanceRecordBuffer::new();
    let mut pei_load_image_count = 0;

    for hob in hob_list.iter() {
        match hob {
            Hob::GuidHob(hob, data) if hob.name == guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE => {
                let mut offset = 0;
                let [size_of_all_entries, load_image_count, _hob_is_full] =
                    data.gread::<[u32; 3]>(&mut offset).unwrap();
                let records_data_buffer = &data[offset..offset + size_of_all_entries as usize];
                pei_load_image_count += load_image_count;
                for r in Iter::new(records_data_buffer) {
                    pei_records.push_record(r)?;
                }
            }
            _ => continue,
        };
    }
    Ok((pei_records, pei_load_image_count))
}

extern "efiapi" fn report_fpdt_record_buffer<B, R>(_event: efi::Event, ctx: Box<(&B, &R)>)
where
    B: BootServices + Debug,
    R: RuntimeServices + Debug,
{
    let (boot_services, runtime_services) = *ctx;
    // SAFETY: This is safe because FBPT is initialize before the creation of this event.
    let mut fbpt = unsafe { FBPT.assume_init_ref() }.lock();
    if fbpt.report_table(boot_services, runtime_services).is_err() {
        log::error!("Performance Lib: Fail to report FPDT.");
        return;
    }

    const EFI_SOFTWARE: u32 = 0x03000000;
    const EFI_PROGRESS_CODE: u32 = 0x00000001;
    const EFI_SOFTWARE_DXE_BS_DRIVER: u32 = EFI_SOFTWARE | 0x00050000;

    let status = StatusCodeRuntimeProtocol::report_status_code(
        boot_services,
        EFI_PROGRESS_CODE,
        EFI_SOFTWARE_DXE_BS_DRIVER,
        0,
        None,
        efi::Guid::clone(&guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE),
        FBPT.lock().fbpt_address(),
    );
    if status.is_err() {
        log::error!("Fail to report FBPT status code.");
    }

    // SAFETY: This operation is safe because the expected configuration type of a entry with guid `EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE`
    // is a usize and the memory address is a valid and point to an FBPT.
    let status = unsafe {
        boot_services.install_configuration_table_unchecked(
            &guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
            FBPT.lock().fbpt_address() as *mut c_void,
        )
    };
    if status.is_err() {
        log::error!("Fail to install configuration table for FPDT firmware performance.");
    }
}

extern "efiapi" fn fetch_and_add_smm_performance_records(
    _event: efi::Event,
    ctx: Box<(&StandardBootServices, &efi::SystemTable)>,
) {
    // Make sure that this event only run once.
    static HAS_RUN_ONCE: AtomicBool = AtomicBool::new(false);
    let Ok(false) = HAS_RUN_ONCE.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed) else {
        mem::forget(ctx); // ctx has already been dropped.
        return;
    };

    let (boot_services, system_table) = *ctx;

    let configuration_tables =
        unsafe { slice::from_raw_parts(system_table.configuration_table, system_table.number_of_table_entries) };

    let Some(smm_comm_region_table) = configuration_tables
        .iter()
        .find(|config_table| config_table.vendor_guid == EDKII_PI_SMM_COMMUNICATION_REGION_TABLE_GUID)
        .and_then(|config_table| {
            // SAFETY: The cast of vendor_table to `SmmCommunicationRegionTable` is valid
            // because the configuration table vendor guid is `EDKII_PI_SMM_COMMUNICATION_REGION_TABLE_GUID`
            // and the expected value of this configuration is a `SmmCommunicationRegionTable`.
            unsafe { (config_table.vendor_table as *const SmmCommunicationRegionTable).as_ref() }
        })
    else {
        log::error!("Performance Lib: Could not find any smm communication region table.");
        return;
    };

    let Some(smm_communication_memory_region) =
        smm_comm_region_table.iter().find(|r| r.r#type == efi::CONVENTIONAL_MEMORY)
    else {
        log::error!("Performance Lib: Could not find an available memory region to communication with smm.");
        return;
    };
    if smm_communication_memory_region.physical_start == 0 || smm_communication_memory_region.number_of_pages == 0 {
        log::error!("Performance Lib: Something is wrong with the smm communication memory region.");
        return;
    }

    // SAFETY: This is safe because the reference returned by locate_protocol is never mutated after installation.
    let Ok(communication) = (unsafe { boot_services.locate_protocol(&CommunicateProtocol, None) }) else {
        log::error!("Performance Lib: Could not locate communicate protocol interface.");
        return;
    };

    // SAFETY: Is safe to use because the memory region comes for a trusted source and can be considered valid.
    let boot_record_size = match unsafe {
        // Ask smm for the total size of the perf records.
        communication.communicate(SmmFpdtGetRecordSize::new(), smm_communication_memory_region)
    } {
        Ok(SmmFpdtGetRecordSize { return_status, boot_record_size }) if return_status == efi::Status::SUCCESS => {
            boot_record_size
        }
        Ok(SmmFpdtGetRecordSize { return_status, .. }) => {
            log::error!(
                "Performance Lib: Asking for the smm perf records size result in an error with return status of: {:?}",
                return_status
            );
            return;
        }
        Err(status) => {
            log::error!(
                "Performance Lib: Error while trying to communicate with communicate protocol with error code: {:?}",
                status
            );
            return;
        }
    };

    let mut smm_boot_records_data = Vec::with_capacity(boot_record_size);

    while smm_boot_records_data.len() < boot_record_size {
        // SAFETY: Is safe to use because the memroy region commes for a thrusted source and can be considered valid.
        match unsafe {
            // Ask smm to return us the next bytes in its buffer.
            communication.communicate(
                SmmFpdtGetRecordDataByOffset::<1024>::new(smm_boot_records_data.len()),
                smm_communication_memory_region,
            )
        } {
            Ok(record_data) if record_data.return_status == efi::Status::SUCCESS => {
                // Append the byte to the total smm performance record data.
                smm_boot_records_data.extend_from_slice(record_data.boot_record_data());
            }
            Ok(SmmFpdtGetRecordDataByOffset { return_status, .. }) => {
                log::error!(
                    "Performance Lib: Asking for smm perf records data result in an error with return status of: {:?}",
                    return_status
                );
                return;
            }
            Err(status) => {
                log::error!(
                    "Performance Lib: Error while trying to communicate with communicate protocol with error status code: {:?}",
                    status
                );
                return;
            }
        };
    }

    // Write found perf records in the fbpt table.
    let mut fbpt = unsafe { FBPT.assume_init_ref() }.lock();
    let mut n = 0;
    for r in Iter::new(&smm_boot_records_data) {
        fbpt.add_record(r).unwrap();
        n += 1;
    }

    log::info!("Performance Lib: {} smm performance records found.", n);
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
    if !IS_PERF_READY.load(Ordering::Relaxed) || !PERF_ENABLED {
        return efi::Status::SUCCESS;
    }
    let fbpt;
    let boot_services;
    // SAFETY: This is safe because FBPT and BOOTSERVCES are initialize if IS_PERF_READY is true.
    unsafe {
        fbpt = FBPT.assume_init_ref();
        boot_services = BOOT_SERVICES.assume_init();
    }

    let string = unsafe { _utils::string_from_c_char_ptr(string) };

    // NOTE: If the Perf is not the known Token used in the core but have same ID with the core Token, this case will not be supported.
    // And in current usage mode, for the unkown ID, there is a general rule:
    // - If it is start pref: the lower 4 bits of the ID should be 0.
    // - If it is end pref: the lower 4 bits of the ID should not be 0.
    // - If input ID doesn't follow the rule, we will adjust it.
    let mut perf_id = identifier as u16;
    let is_known_id = KnownPerfId::try_from(perf_id).is_ok();
    let is_known_token = string.as_ref().map_or(false, |s| KnownPerfToken::try_from(s.as_str()).is_ok());
    if attribute != PerfAttribute::PerfEntry {
        if perf_id != 0 && is_known_id && is_known_token {
            return efi::Status::INVALID_PARAMETER;
        } else if perf_id != 0 && !is_known_id && !is_known_token {
            if attribute == PerfAttribute::PerfStartEntry && ((perf_id & 0x000F) != 0) {
                perf_id &= 0xFFF0;
            } else if attribute == PerfAttribute::PerfEndEntry && ((perf_id & 0x000F) == 0) {
                perf_id += 1;
            }
        } else if perf_id == 0 {
            match get_fpdt_record_id(attribute, caller_identifier, string.as_ref()) {
                Ok(known_perf_id) => perf_id = known_perf_id.as_u16(),
                Err(status) => return status,
            }
        }
    }

    let cpu_count = Arch::cpu_count();
    let timestamp = match ticker {
        0 => (cpu_count as f64 / Arch::perf_frequency() as f64 * 1_000_000_000_f64) as u64,
        1 => 0,
        ticker => (ticker as f64 / Arch::perf_frequency() as f64 * 1_000_000_000_f64) as u64,
    };

    match _create_performance_measurement(
        caller_identifier,
        guid,
        string,
        timestamp,
        address,
        perf_id,
        attribute,
        fbpt,
        boot_services,
    ) {
        Ok(_) => efi::Status::SUCCESS,
        Err(status) => {
            log::error!(
                "Performance Lib: Something went wrong in create_performance_measurement. Status code: {:?}",
                status
            );
            status
        }
    }
}

fn _create_performance_measurement(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: Option<String>,
    timestamp: u64,
    address: usize,
    perf_id: u16,
    attribute: PerfAttribute,
    fbpt: &TplMutex<'static, FBPT, StandardBootServices>,
    boot_services: &StandardBootServices,
) -> Result<(), efi::Status> {
    let Ok(known_perf_id) = KnownPerfId::try_from(perf_id) else {
        if attribute == PerfAttribute::PerfEntry {
            return Err(efi::Status::INVALID_PARAMETER);
        }
        let guid = get_module_guid_from_handle(boot_services, caller_identifier as efi::Handle)
            .unwrap_or_else(|_| unsafe { *(caller_identifier as *const Guid) });
        let module_name = string.as_ref().map(String::as_str).unwrap_or("unkown name");
        fbpt.lock().add_record(DynamicStringEventRecord::new(perf_id, 0, timestamp, guid, &module_name))?;
        return Ok(());
    };

    match known_perf_id {
        KnownPerfId::ModuleStart | KnownPerfId::ModuleEnd => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance Lib: Could not find the guid for module handle: {:?}", module_handle);
                return Err(efi::Status::INVALID_PARAMETER);
            };
            let record = GuidEventRecord::new(perf_id, 0, timestamp, guid);
            fbpt.lock().add_record(record)?;
        }
        id @ KnownPerfId::ModuleLoadImageStart | id @ KnownPerfId::ModuleLoadImageEnd => {
            if id == KnownPerfId::ModuleLoadImageStart {
                LOAD_IMAGE_COUNT.fetch_add(1, Ordering::Relaxed);
            }
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance Lib: Could not find the guid for module handle: {:?}", module_handle);
                return Err(efi::Status::INVALID_PARAMETER);
            };
            let record =
                GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, LOAD_IMAGE_COUNT.load(Ordering::Relaxed) as u64);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::ModuleDbStart
        | KnownPerfId::ModuleDbEnd
        | KnownPerfId::ModuleDbSupportStart
        | KnownPerfId::ModuleDbSupportEnd
        | KnownPerfId::ModuleDbStopStart => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance Lib: Could not find the guid for module handle: {:?}", module_handle);
                return Err(efi::Status::INVALID_PARAMETER);
            };
            let record = GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, address as u64);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::ModuleDbStopEnd => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance Lib: Could not find the guid for module handle: {:?}", module_handle);
                return Err(efi::Status::INVALID_PARAMETER);
            };
            // TODO: use of commponent 2 protocol, need usecase to test further.
            let module_name = "";
            let record = GuidQwordStringEventRecord::new(perf_id, 0, timestamp, guid, address as u64, module_name);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::PerfEventSignalStart
        | KnownPerfId::PerfEventSignalEnd
        | KnownPerfId::PerfCallbackStart
        | KnownPerfId::PerfCallbackEnd => {
            let (Some(function_string), Some(guid)) = (string.as_ref(), guid) else {
                return Err(efi::Status::INVALID_PARAMETER);
            };
            // SAFETY: On these usecases, caller identifier is actually a guid. See macro for more detailed.
            // This strange behavior need to be kept for backward compatibility.
            let module_guid = unsafe { *(caller_identifier as *const efi::Guid) };
            let record = DualGuidStringEventRecord::new(perf_id, 0, timestamp, module_guid, *guid, function_string);
            fbpt.lock().add_record(record)?;
        }

        KnownPerfId::PerfFunctionStart
        | KnownPerfId::PerfFunctionEnd
        | KnownPerfId::PerfInModuleStart
        | KnownPerfId::PerfInModuleEnd
        | KnownPerfId::PerfCrossModuleStart
        | KnownPerfId::PerfCrossModuleEnd
        | KnownPerfId::PerfEvent => {
            // SAFETY: On these usecases, caller identifier is actually a guid. See macro for more detailed.
            // This strange behavior need to be kept for backward compatibility.
            let module_guid = unsafe { *(caller_identifier as *const efi::Guid) };
            let string = string.as_ref().map(String::as_str).unwrap_or("unkown name");
            let record = DynamicStringEventRecord::new(perf_id, 0, timestamp, module_guid, string);
            fbpt.lock().add_record(record)?;
        }
    }

    Ok(())
}

fn get_fpdt_record_id(
    attribute: PerfAttribute,
    handle: *const c_void,
    string: Option<&String>,
) -> Result<KnownPerfId, efi::Status> {
    if let Some(string) = string {
        let perf_id = match string.as_str() {
            "StartImage:" if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::ModuleStart,
            "StartImage:" => KnownPerfId::ModuleEnd,
            "LoadImage:" if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::ModuleLoadImageStart,
            "LoadImage:" => KnownPerfId::ModuleLoadImageEnd,
            "DB:Start:" if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::ModuleDbStart,
            "DB:Start:" => KnownPerfId::ModuleDbEnd,
            "DB:Support:" if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::ModuleDbSupportStart,
            "DB:Support:" => KnownPerfId::ModuleDbSupportEnd,
            "DB:Stop:" if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::ModuleDbStopStart,
            "DB:Stop:" => KnownPerfId::ModuleDbStopEnd,
            "PEI" | "DXE" | "BDS" if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::PerfCrossModuleStart,
            "PEI" | "DXE" | "BDS" => KnownPerfId::PerfCrossModuleEnd,
            _ if attribute == PerfAttribute::PerfStartEntry => KnownPerfId::PerfInModuleStart,
            _ => KnownPerfId::PerfInModuleEnd,
        };
        Ok(perf_id)
    } else if !handle.is_null() {
        if attribute == PerfAttribute::PerfStartEntry {
            Ok(KnownPerfId::PerfInModuleStart)
        } else {
            Ok(KnownPerfId::PerfInModuleEnd)
        }
    } else {
        Err(efi::Status::INVALID_PARAMETER)
    }
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

fn get_module_guid_from_handle(
    boot_services: &impl BootServices,
    handle: efi::Handle,
) -> Result<efi::Guid, efi::Status> {
    let mut guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);

    let loaded_image_protocol = 'find_loaded_image_protocol: {
        if let Ok(loaded_image_protocol) =
            unsafe { boot_services.handle_protocol::<efi::protocols::loaded_image::Protocol>(handle) }
        {
            break 'find_loaded_image_protocol Some(loaded_image_protocol);
        }

        // SAFETY: This is safe because the protocol is not mutated.
        if let Ok(driver_binding_protocol) = unsafe {
            boot_services.open_protocol::<efi::protocols::driver_binding::Protocol>(
                handle,
                ptr::null_mut(),
                ptr::null_mut(),
                efi::OPEN_PROTOCOL_GET_PROTOCOL,
            )
        } {
            if let Ok(loaded_image_protocol) = unsafe {
                boot_services
                    .handle_protocol::<efi::protocols::loaded_image::Protocol>(driver_binding_protocol.image_handle)
            } {
                break 'find_loaded_image_protocol Some(loaded_image_protocol);
            }
        }
        None
    };

    if let Some(loaded_image) = loaded_image_protocol {
        // SAFETY: File path is a pointer from C that is valid and of type Device Path (efi).
        if let Some(file_path) = unsafe { loaded_image.file_path.as_ref() } {
            if file_path.r#type == TYPE_MEDIA && file_path.sub_type == Media::SUBTYPE_PIWG_FIRMWARE_FILE {
                guid = unsafe { ptr::read(loaded_image.file_path.add(1) as *const efi::Guid) }
            }
        };
    }

    Ok(guid)
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, KnownPerfId::ModuleStart.as_u16());
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, KnownPerfId::ModuleEnd.as_u16());
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, KnownPerfId::ModuleLoadImageStart.as_u16());
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
    log_perf_measurement(module_handle, None, ptr::null(), 0, KnownPerfId::ModuleLoadImageEnd.as_u16());
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
        ptr::null(),
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
        ptr::null(),
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
    log_perf_measurement(
        module_handle,
        None,
        ptr::null(),
        controller_handle as usize,
        KnownPerfId::ModuleDbStart.as_u16(),
    );
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
    log_perf_measurement(
        module_handle,
        None,
        ptr::null(),
        controller_handle as usize,
        KnownPerfId::ModuleDbEnd.as_u16(),
    );
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
        ptr::null(),
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
    log_perf_measurement(
        module_handle,
        None,
        ptr::null(),
        controller_handle as usize,
        KnownPerfId::ModuleDbStopEnd.as_u16(),
    );
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
        c_char_ptr_from_str(fun_name),
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
        c_char_ptr_from_str(fun_name),
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
        c_char_ptr_from_str(fun_name),
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
        c_char_ptr_from_str(fun_name),
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
        c_char_ptr_from_str(fun_name),
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
        c_char_ptr_from_str(fun_name),
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
        c_char_ptr_from_str(measurement_str),
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
        c_char_ptr_from_str(measurement_str),
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
        c_char_ptr_from_str(measurement_str),
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
        c_char_ptr_from_str(measurement_str),
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
