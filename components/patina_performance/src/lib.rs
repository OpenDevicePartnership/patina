//! A library that enables performance analysis of every step of the UEFI boot process.
//! The Performance component installs a protocol that can be used by other libraries or drivers to publish performance reports.
//! These reports are saved in the Firmware Basic Boot Performance Table (FBPT), so they can be extracted later from the operating system.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#![cfg_attr(not(test), no_std)]
#![allow(unexpected_cfgs)]

extern crate alloc;

mod _smm;
pub mod error;
pub mod log_perf_measurement;
pub mod performance_measurement_protocol;
pub mod performance_record;
pub mod performance_table;

use alloc::{boxed::Box, string::ToString, vec::Vec};
use core::{
    clone::Clone,
    convert::{AsRef, TryFrom},
    ffi::{c_char, c_void, CStr},
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use mu_pi::status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_BS_DRIVER};

use r_efi::{
    efi::{self, Guid},
    protocols::device_path::{Media, TYPE_MEDIA},
    system::EVENT_GROUP_READY_TO_BOOT,
};

pub use mu_rust_helpers::function;
use mu_rust_helpers::perf_timer::{Arch, ArchFunctionality};

use _smm::{CommunicateProtocol, MmCommRegion, SmmGetRecordDataByOffset, SmmGetRecordSize};

use patina_sdk::{
    boot_services::{event::EventType, tpl::Tpl, BootServices, StandardBootServices},
    component::{hob::Hob, params::Config, IntoComponent},
    error::EfiError,
    guid::{EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE, EVENT_GROUP_END_OF_DXE, PERFORMANCE_PROTOCOL},
    runtime_services::{RuntimeServices, StandardRuntimeServices},
    tpl_mutex::TplMutex,
    uefi_protocol::status_code::StatusCodeRuntimeProtocol,
};

use crate::{
    error::Error,
    performance_measurement_protocol::{EdkiiPerformanceMeasurement, PerfAttribute},
    performance_record::{
        extended::{
            DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
            GuidQwordStringEventRecord,
        },
        hob_records::{HobPerformanceData, HobPerformanceDataExtractor},
        known_records::{KnownPerfId, KnownPerfToken},
        Iter,
    },
    performance_table::{FirmwareBasicBootPerfTable, FBPT},
};

pub use log_perf_measurement::*;

static PERF_MEASUREMENT_MASK: AtomicU32 = AtomicU32::new(0);
static LOAD_IMAGE_COUNT: AtomicU32 = AtomicU32::new(0);

static STATIC_STATE_IS_INIT: AtomicBool = AtomicBool::new(false);
static mut BOOT_SERVICES: MaybeUninit<StandardBootServices> = MaybeUninit::uninit();
static mut FBPT: MaybeUninit<TplMutex<FBPT>> = MaybeUninit::uninit();

/// Set performance component static state.
#[allow(static_mut_refs)]
pub fn set_static_state(boot_services: StandardBootServices) -> Option<&'static TplMutex<'static, FBPT>> {
    // Return Ok if STATIC_STATE_INIT is false and set it to true. Make this run only once.
    if STATIC_STATE_IS_INIT.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        // SAFETY: This is safe because it is the entry point and no one is reading these value yet.
        unsafe {
            let boot_services_ref = BOOT_SERVICES.write(boot_services);
            Some(FBPT.write(TplMutex::new(boot_services_ref, Tpl::NOTIFY, FBPT::new())))
        }
    } else {
        None
    }
}

/// Get performance component static state.
#[allow(static_mut_refs)]
pub fn get_static_state() -> Option<(&'static StandardBootServices, &'static TplMutex<'static, FBPT>)> {
    if STATIC_STATE_IS_INIT.load(Ordering::Relaxed) {
        // SAFETY: This is safe because the state has been init.
        unsafe { Some((BOOT_SERVICES.assume_init_ref(), FBPT.assume_init_ref())) }
    } else {
        None
    }
}

/// A wrapper to generate a mask of all enabled measurements.
#[derive(Debug, Default)]
pub struct EnabledMeasurement(pub &'static [Measurement]);

impl EnabledMeasurement {
    /// Returns a mask of all enabled measurements.
    pub fn mask(&self) -> u32 {
        self.0.iter().fold(0, |mask, m| mask | m.as_u32())
    }
}

/// Measurement enum that represents the different performance measurements that can be enabled.
#[derive(Debug)]
#[repr(u32)]
pub enum Measurement {
    /// Dispatch modules entry point execution
    StartImage = 1,
    /// Load a dispatched module.
    LoadImage = 1 << 1,
    /// Diver binding support function call.
    DriverBindingSupport = 1 << 2,
    /// Diver binding start function call.
    DriverBindingStart = 1 << 3,
    /// Diver binding stop function call.
    DriverBindingStop = 1 << 4,
}

impl Measurement {
    /// [u32] representation of the measurement.
    pub fn as_u32(&self) -> u32 {
        match self {
            Measurement::StartImage => Measurement::StartImage as u32,
            Measurement::LoadImage => Measurement::LoadImage as u32,
            Measurement::DriverBindingSupport => Measurement::DriverBindingSupport as u32,
            Measurement::DriverBindingStart => Measurement::DriverBindingStart as u32,
            Measurement::DriverBindingStop => Measurement::DriverBindingStop as u32,
        }
    }
}

/// Performance Component.
#[derive(IntoComponent)]
pub struct Performance;

impl Performance {
    /// Entry point of [`Performance`]
    #[cfg(not(tarpaulin_include))] // This is tested via the generic version, see _entry_point.
    pub fn entry_point(
        self,
        enabled_measurements: Config<EnabledMeasurement>,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        records_buffers_hobs: Hob<HobPerformanceData>,
        mm_comm_region_hobs: Hob<MmCommRegion>,
    ) -> Result<(), EfiError> {
        PERF_MEASUREMENT_MASK.store(enabled_measurements.mask(), Ordering::Relaxed);

        let fbpt = set_static_state(StandardBootServices::clone(&boot_services))
            .expect("Static state should only be initialized here!");

        let Some(mm_comm_region) = mm_comm_region_hobs.iter().find(|r| r.is_user_type()) else {
            return Ok(());
        };

        self._entry_point(boot_services, runtime_services, records_buffers_hobs, *mm_comm_region, fbpt)
    }

    /// Entry point that have generic parameter.
    fn _entry_point<BB, B, RR, R, P, F>(
        self,
        boot_services: BB,
        runtime_services: RR,
        records_buffers_hobs: P,
        mm_comm_region: MmCommRegion,
        fbpt: &'static TplMutex<'static, F, B>,
    ) -> Result<(), EfiError>
    where
        BB: AsRef<B> + Clone + 'static,
        B: BootServices + 'static,
        RR: AsRef<R> + Clone + 'static,
        R: RuntimeServices + 'static,
        P: HobPerformanceDataExtractor,
        F: FirmwareBasicBootPerfTable,
    {
        // Register EndOfDxe event to allocate the boot performance table and report the table address through status code.
        boot_services.as_ref().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(report_fbpt_record_buffer),
            Box::new((BB::clone(&boot_services), RR::clone(&runtime_services), fbpt)),
            &EVENT_GROUP_END_OF_DXE,
        )?;

        let (hob_load_image_count, hob_perf_records) = records_buffers_hobs
            .extract_hob_perf_data()
            .inspect(|(_, perf_buf)| {
                log::info!("Performance: {} Hob performance records found.", perf_buf.iter().count());
            })
            .inspect_err(|_| {
                log::error!("Performance: Error while trying to insert hob performance records, using default values")
            })
            .unwrap_or_default();

        // Initialize perf data form hob values.
        LOAD_IMAGE_COUNT.store(hob_load_image_count, Ordering::Relaxed);
        fbpt.lock().set_perf_records(hob_perf_records);

        // Install the protocol interfaces for DXE performance.
        boot_services.as_ref().install_protocol_interface(
            None,
            Box::new(EdkiiPerformanceMeasurement { create_performance_measurement }),
        )?;

        // Register ReadyToBoot event to update the boot performance table for SMM performance data.
        boot_services.as_ref().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(fetch_and_add_mm_performance_records),
            Box::new((BB::clone(&boot_services), mm_comm_region, fbpt)),
            &EVENT_GROUP_READY_TO_BOOT,
        )?;

        // Install configuration table for performance property.
        unsafe {
            boot_services.as_ref().install_configuration_table(
                &PERFORMANCE_PROTOCOL,
                Box::new(PerformanceProperty::new(
                    Arch::perf_frequency(),
                    Arch::cpu_count_start(),
                    Arch::cpu_count_end(),
                )),
            )?
        };

        Ok(())
    }
}

/// Event callback that report the fbpt.
extern "efiapi" fn report_fbpt_record_buffer<BB, B, RR, R, F>(
    event: efi::Event,
    ctx: Box<(BB, RR, &TplMutex<'static, F, B>)>,
) where
    BB: AsRef<B> + Clone,
    B: BootServices + 'static,
    RR: AsRef<R> + Clone + 'static,
    R: RuntimeServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    let (boot_services, runtime_services, fbpt) = *ctx;
    let _ = boot_services.as_ref().close_event(event);

    let Ok(fbpt_address) = fbpt.lock().report_table(
        performance_table::find_previous_table_address(runtime_services.as_ref()),
        boot_services.as_ref(),
    ) else {
        log::error!("Performance: Fail to report FBPT.");
        return;
    };

    let Ok(p) = (unsafe { boot_services.as_ref().locate_protocol::<StatusCodeRuntimeProtocol>(None) }) else {
        log::error!("Performance: Fail to find status code protocol.");
        return;
    };

    let status = p.report_status_code(
        EFI_PROGRESS_CODE,
        EFI_SOFTWARE_DXE_BS_DRIVER,
        0,
        &mu_rust_helpers::guid::CALLER_ID,
        efi::Guid::clone(&EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE),
        fbpt_address,
    );

    if status.is_err() {
        log::error!("Performance: Fail to report FBPT status code.");
    }

    // SAFETY: This operation is valid because the expected configuration type of a entry with guid `EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE`
    // is a usize and the memory address is a valid and point to an FBPT.
    let status = unsafe {
        boot_services.as_ref().install_configuration_table_unchecked(
            &EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
            fbpt_address as *mut c_void,
        )
    };
    if status.is_err() {
        log::error!("Performance: Fail to install configuration table for FBPT firmware performance.");
    }
}

/// Event callback that add the SMM performance record to the FBPT.
extern "efiapi" fn fetch_and_add_mm_performance_records<BB, B, F>(
    event: efi::Event,
    ctx: Box<(BB, MmCommRegion, &TplMutex<'static, F, B>)>,
) where
    BB: AsRef<B> + Clone,
    B: BootServices + 'static,
    F: FirmwareBasicBootPerfTable,
{
    let (boot_services, mm_comm_region, fbpt) = *ctx;
    let _ = boot_services.as_ref().close_event(event);

    // SAFETY: This is safe because the reference returned by locate_protocol is never mutated after installation.
    let Ok(communication) = (unsafe { boot_services.as_ref().locate_protocol::<CommunicateProtocol>(None) }) else {
        log::error!("Performance: Could not locate communicate protocol interface.");
        return;
    };

    // SAFETY: Is safe to use because the memory region comes from a trusted source and can be considered valid.
    let boot_record_size = match unsafe {
        // Ask smm for the total size of the perf records.
        communication.communicate(SmmGetRecordSize::new(), mm_comm_region)
    } {
        Ok(SmmGetRecordSize { return_status, boot_record_size }) if return_status == efi::Status::SUCCESS => {
            boot_record_size
        }
        Ok(SmmGetRecordSize { return_status, .. }) => {
            log::error!(
                "Performance: Asking for the smm perf records size result in an error with return status of: {:?}",
                return_status
            );
            return;
        }
        Err(status) => {
            log::error!(
                "Performance: Error while trying to communicate with communicate protocol with error code: {:?}",
                status
            );
            return;
        }
    };

    let mut smm_boot_records_data = Vec::with_capacity(boot_record_size);

    while smm_boot_records_data.len() < boot_record_size {
        // SAFETY: Is safe to use because the memory region commes from a thrusted source and can be considered valid.
        match unsafe {
            // Ask smm to return us the next bytes in its buffer.
            const BUFFER_SIZE: usize = 1024;
            communication
                .communicate(SmmGetRecordDataByOffset::<BUFFER_SIZE>::new(smm_boot_records_data.len()), mm_comm_region)
        } {
            Ok(record_data) if record_data.return_status == efi::Status::SUCCESS => {
                // Append the byte to the total smm performance record data.
                smm_boot_records_data.extend_from_slice(record_data.boot_record_data());
            }
            Ok(SmmGetRecordDataByOffset { return_status, .. }) => {
                log::error!(
                    "Performance: Asking for smm perf records data result in an error with return status of: {:?}",
                    return_status
                );
                return;
            }
            Err(status) => {
                log::error!(
                    "Performance: Error while trying to communicate with communicate protocol with error status code: {:?}",
                    status
                );
                return;
            }
        };
    }

    // Write found perf records in the fbpt table.
    let mut fbpt = fbpt.lock();
    let mut n = 0;
    for r in Iter::new(&smm_boot_records_data) {
        _ = fbpt.add_record(r);
        n += 1;
    }

    log::info!("Performance: {} smm performance records found.", n);
}

#[cfg(not(tarpaulin_include))]
// Tested via the generic version, see _create_performance_measurement. This one is using the static state which makes it not mockable.
/// # Safety
/// string must be a valid C string pointer.
pub unsafe extern "efiapi" fn create_performance_measurement(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: PerfAttribute,
) -> efi::Status {
    let Some((boot_services, fbpt)) = get_static_state() else {
        // If the state is not initialized, it is because perf in not enabled.
        return efi::Status::SUCCESS;
    };

    let string = unsafe { string.as_ref().map(|s| CStr::from_ptr(s).to_string_lossy().to_string()) };

    // NOTE: If the Perf is not the known Token used in the core but have same ID with the core Token, this case will not be supported.
    // And in current usage mode, for the unknown ID, there is a general rule:
    // - If it is start pref: the lower 4 bits of the ID should be 0.
    // - If it is end pref: the lower 4 bits of the ID should not be 0.
    // - If input ID doesn't follow the rule, we will adjust it.
    let mut perf_id = identifier as u16;
    let is_known_id = KnownPerfId::try_from(perf_id).is_ok();
    let is_known_token = string.as_ref().is_some_and(|s| KnownPerfToken::try_from(s.as_str()).is_ok());
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
            match KnownPerfId::try_from_perf_info(caller_identifier as efi::Handle, string.as_ref(), attribute) {
                Ok(known_perf_id) => perf_id = known_perf_id.as_u16(),
                Err(status) => return status,
            }
        }
    }

    match _create_performance_measurement(
        caller_identifier,
        guid,
        string.as_deref(),
        ticker,
        address,
        perf_id,
        attribute,
        boot_services,
        fbpt,
    ) {
        Ok(_) => efi::Status::SUCCESS,
        Err(Error::OutOfResources) => {
            static HAS_BEEN_LOGGED: AtomicBool = AtomicBool::new(false);
            if HAS_BEEN_LOGGED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                log::info!("Performance: FBPT is full, can't add more performance records !");
            };
            efi::Status::OUT_OF_RESOURCES
        }
        Err(Error::Efi(status_code)) => {
            log::error!(
                "Performance: Something went wrong in create_performance_measurement. status_code: {status_code:?}"
            );
            status_code.into()
        }
        Err(error) => {
            log::error!("Performance: Something went wrong in create_performance_measurement. Error: {error}",);
            efi::Status::ABORTED
        }
    }
}

/// Crate a performance measurement and add it the the FBPT.
#[allow(clippy::too_many_arguments)]
fn _create_performance_measurement<B, F>(
    caller_identifier: *const c_void,
    guid: Option<&efi::Guid>,
    string: Option<&str>,
    ticker: u64,
    address: usize,
    perf_id: u16,
    attribute: PerfAttribute,
    boot_services: &B,
    fbpt: &TplMutex<'static, F, B>,
) -> Result<(), Error>
where
    B: BootServices,
    F: FirmwareBasicBootPerfTable,
{
    let cpu_count = Arch::cpu_count();
    let timestamp = match ticker {
        0 => (cpu_count as f64 / Arch::perf_frequency() as f64 * 1_000_000_000_f64) as u64,
        1 => 0,
        ticker => (ticker as f64 / Arch::perf_frequency() as f64 * 1_000_000_000_f64) as u64,
    };

    let Ok(known_perf_id) = KnownPerfId::try_from(perf_id) else {
        if attribute == PerfAttribute::PerfEntry {
            return Err(EfiError::InvalidParameter.into());
        }
        let guid = get_module_guid_from_handle(boot_services, caller_identifier as efi::Handle)
            .unwrap_or_else(|_| unsafe { *(caller_identifier as *const Guid) });
        let module_name = string.unwrap_or("unknown name");
        fbpt.lock().add_record(DynamicStringEventRecord::new(perf_id, 0, timestamp, guid, module_name))?;
        return Ok(());
    };

    match known_perf_id {
        KnownPerfId::ModuleStart | KnownPerfId::ModuleEnd => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance: Could not find the guid for module handle: {:?}", module_handle);
                return Err(EfiError::InvalidParameter.into());
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
                log::error!("Performance: Could not find the guid for module handle: {:?}", module_handle);
                return Err(EfiError::InvalidParameter.into());
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
                log::error!("Performance: Could not find the guid for module handle: {:?}", module_handle);
                return Err(EfiError::InvalidParameter.into());
            };
            let record = GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, address as u64);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::ModuleDbStopEnd => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance Lib: Could not find the guid for module handle: {:?}", module_handle);
                return Err(EfiError::InvalidParameter.into());
            };
            let module_name = "";
            let record = GuidQwordStringEventRecord::new(perf_id, 0, timestamp, guid, address as u64, module_name);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::PerfEventSignalStart
        | KnownPerfId::PerfEventSignalEnd
        | KnownPerfId::PerfCallbackStart
        | KnownPerfId::PerfCallbackEnd => {
            let (Some(function_string), Some(guid)) = (string.as_ref(), guid) else {
                return Err(EfiError::InvalidParameter.into());
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
            let string = string.unwrap_or("unknown name");
            let record = DynamicStringEventRecord::new(perf_id, 0, timestamp, module_guid, string);
            fbpt.lock().add_record(record)?;
        }
    }
    Ok(())
}

#[repr(C)]
struct PerformanceProperty {
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
                // Guid is stored after the device path in memory.
                guid = unsafe { ptr::read(loaded_image.file_path.add(1) as *const efi::Guid) }
            }
        };
    }

    Ok(guid)
}

/// This device path is used by systems implementing the UEFI PI Specification 1.0 to describe a firmware file.
#[repr(C)]
pub struct MediaFwVolFilepathDevicePath {
    header: efi::protocols::device_path::Protocol,
    /// Firmware file name
    fv_file_name: efi::Guid,
}

#[cfg(test)]
mod test {
    use super::*;

    use alloc::rc::Rc;
    use core::{assert_eq, ptr};

    use mockall::predicate;

    use patina_sdk::{
        boot_services::{
            c_ptr::{CMutPtr, CPtr},
            MockBootServices,
        },
        runtime_services::MockRuntimeServices,
        uefi_protocol::ProtocolInterface,
    };

    use crate::{
        performance_measurement_protocol::EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID,
        performance_record::{hob_records::MockHobPerformanceDataExtractor, PerformanceRecordBuffer},
        performance_table::{FirmwarePerformanceVariable, MockFirmwareBasicBootPerfTable},
    };

    #[test]
    fn test_get_set_static_state() {
        STATIC_STATE_IS_INIT.store(false, Ordering::Relaxed);
        unsafe {
            BOOT_SERVICES = MaybeUninit::zeroed();
            FBPT = MaybeUninit::zeroed();
        }

        assert!(get_static_state().is_none());
        assert!(set_static_state(StandardBootServices::new_uninit()).is_some());
        assert!(get_static_state().is_some());
        assert!(set_static_state(StandardBootServices::new_uninit()).is_none());
    }

    #[test]
    fn test_entry_point() {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().return_const(());

        // Test that the protocol in installed.
        boot_services
            .expect_install_protocol_interface::<EdkiiPerformanceMeasurement, Box<_>>()
            .once()
            .withf_st(|handle, _protocol_interface| {
                assert_eq!(&None, handle);
                assert_eq!(EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID, EdkiiPerformanceMeasurement::PROTOCOL_GUID);
                true
            })
            .returning(|_, protocol_interface| Ok((1 as efi::Handle, protocol_interface.metadata())));

        // Test that an event to report the fbpt at the end of dxe is created.
        boot_services
            .expect_create_event_ex::<Box<(
                Rc<MockBootServices>,
                Rc<MockRuntimeServices>,
                &TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>,
            )>>()
            .once()
            .withf_st(|event_type, notify_tpl, notify_function, _notify_context, event_group| {
                assert_eq!(&EventType::NOTIFY_SIGNAL, event_type);
                assert_eq!(&Tpl::CALLBACK, notify_tpl);
                assert_eq!(
                    report_fbpt_record_buffer::<
                        Rc<_>,
                        MockBootServices,
                        Rc<_>,
                        MockRuntimeServices,
                        MockFirmwareBasicBootPerfTable,
                    > as usize,
                    notify_function.unwrap() as usize
                );
                assert_eq!(&EVENT_GROUP_END_OF_DXE, event_group);
                true
            })
            .return_const_st(Ok(1_usize as efi::Event));

        // Test that an event to update the fbpt with smm data when ready to boot is created.
        boot_services
            .expect_create_event_ex::<Box<(
                Rc<MockBootServices>,
                MmCommRegion,
                &TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>,
            )>>()
            .once()
            .withf_st(|event_type, notify_tpl, notify_function, _notify_context, event_group| {
                assert_eq!(&EventType::NOTIFY_SIGNAL, event_type);
                assert_eq!(&Tpl::CALLBACK, notify_tpl);
                assert_eq!(
                    fetch_and_add_mm_performance_records::<Rc<_>, MockBootServices, MockFirmwareBasicBootPerfTable>
                        as usize,
                    notify_function.unwrap() as usize
                );
                assert_eq!(&EVENT_GROUP_READY_TO_BOOT, event_group);
                true
            })
            .return_const_st(Ok(1_usize as efi::Event));

        // Test that the address of the fbpt is installed to the configuration table.
        boot_services
            .expect_install_configuration_table::<Box<PerformanceProperty>>()
            .once()
            .withf(|guid, _data| {
                assert_eq!(&PERFORMANCE_PROTOCOL, guid);
                true
            })
            .return_const(Ok(()));

        let runtime_services = MockRuntimeServices::new();

        let mut hob_perf_data_extractor = MockHobPerformanceDataExtractor::new();
        hob_perf_data_extractor
            .expect_extract_hob_perf_data()
            .once()
            .returning(|| Ok((10, PerformanceRecordBuffer::new())));

        let mm_comm_region = MmCommRegion { region_type: 1, region_address: 10, region_nb_pages: 1 };

        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_set_perf_records().once().return_const(());

        let fbpt = TplMutex::new(unsafe { &*ptr::addr_of!(boot_services) }, Tpl::NOTIFY, fbpt);
        let fbpt = unsafe { &*ptr::addr_of!(fbpt) };

        let _ = Performance._entry_point(
            Rc::new(boot_services),
            Rc::new(runtime_services),
            hob_perf_data_extractor,
            mm_comm_region,
            fbpt,
        );
    }

    #[test]
    fn test_report_fbpt_record_buffer() {
        static REPORT_STATUS_CODE_CALLED: AtomicBool = AtomicBool::new(false);

        extern "efiapi" fn report_status_code(
            _a: u32,
            _b: u32,
            _c: u32,
            _d: *const efi::Guid,
            _e: *const mu_pi::protocols::status_code::EfiStatusCodeData,
        ) -> efi::Status {
            REPORT_STATUS_CODE_CALLED.store(true, Ordering::Relaxed);
            efi::Status::SUCCESS
        }
        let mut status_code_runtime_protocol = Box::new(StatusCodeRuntimeProtocol::new(report_status_code));
        let status_code_runtime_protocol_ptr = status_code_runtime_protocol.as_mut_ptr();

        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().returning(|tpl| tpl);
        boot_services.expect_restore_tpl().return_const(());

        // Test that the event is close so it run only one time.
        boot_services.expect_close_event().once().return_const(Ok(()));

        boot_services
            .expect_install_configuration_table_unchecked()
            .once()
            .with(predicate::eq(&EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE), predicate::always())
            .return_const(Ok(()));

        boot_services
            .expect_locate_protocol()
            .once()
            .returning_st(move |_| Ok(unsafe { &mut *status_code_runtime_protocol_ptr }));

        let mut runtime_services = MockRuntimeServices::new();
        runtime_services
            .expect_get_variable::<FirmwarePerformanceVariable>()
            .once()
            .returning(|_, _, _| Err(efi::Status::NOT_FOUND));

        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_report_table::<MockBootServices>().once().returning(|_, _| Ok(1));

        let fbpt = TplMutex::new(unsafe { &*ptr::addr_of!(boot_services) }, Tpl::NOTIFY, fbpt);
        let fbpt = unsafe { &*ptr::addr_of!(fbpt) };

        report_fbpt_record_buffer(
            1_usize as efi::Event,
            Box::new((Rc::new(boot_services), Rc::new(runtime_services), fbpt)),
        );

        assert!(REPORT_STATUS_CODE_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_create_performance_measurement() {
        PERF_MEASUREMENT_MASK.store(u32::MAX, Ordering::Relaxed);
        let mut boot_services = MockBootServices::new();

        let mut loaded_image_protocol = MaybeUninit::<efi::protocols::loaded_image::Protocol>::zeroed();
        let mut media_fw_vol_file_path_device_path = MaybeUninit::<MediaFwVolFilepathDevicePath>::zeroed();
        unsafe {
            media_fw_vol_file_path_device_path.assume_init_mut().header.r#type = TYPE_MEDIA;
            media_fw_vol_file_path_device_path.assume_init_mut().header.sub_type = Media::SUBTYPE_PIWG_FIRMWARE_FILE;
            media_fw_vol_file_path_device_path.assume_init_mut().fv_file_name = efi::Guid::from_bytes(&[3; 16]);

            loaded_image_protocol.assume_init_mut().file_path =
                media_fw_vol_file_path_device_path.as_mut_ptr() as *mut efi::protocols::device_path::Protocol;
        };
        let loaded_image_protocol_address = loaded_image_protocol.as_mut_ptr() as usize;

        boot_services.expect_handle_protocol::<efi::protocols::loaded_image::Protocol>().returning(move |_| unsafe {
            Ok((loaded_image_protocol_address as *mut efi::protocols::loaded_image::Protocol).as_mut().unwrap())
        });
        boot_services.expect_raise_tpl().returning(|tpl| tpl);
        boot_services.expect_restore_tpl().return_const(());

        let mut fbpt = MockFirmwareBasicBootPerfTable::new();
        fbpt.expect_add_record().times(EXPECTED_NUMBER_OF_RECORD).returning(|_| Ok(()));
        let fbpt = TplMutex::new(unsafe { &*ptr::addr_of!(boot_services) }, Tpl::NOTIFY, fbpt);

        // These functions call create_performance_measurement with the right arguments.
        let module_handle = 1_usize as efi::Handle;
        let controller_handle = 2_usize as efi::Handle;
        let caller_id = efi::Guid::from_bytes(&[1; 16]);
        let trigger_guid = efi::Guid::from_bytes(&[2; 16]);
        let event_guid = efi::Guid::from_bytes(&[3; 16]);

        static mut BOOT_SERVICES: Option<&MockBootServices> = None;
        static mut FBPT: Option<&TplMutex<'static, MockFirmwareBasicBootPerfTable, MockBootServices>> = None;

        unsafe {
            BOOT_SERVICES = Some(&*ptr::addr_of!(boot_services));
            FBPT = Some(&*ptr::addr_of!(fbpt));
        }

        extern "efiapi" fn test_create_performance_measurement(
            caller_identifier: *const c_void,
            guid: Option<&efi::Guid>,
            string: *const c_char,
            ticker: u64,
            address: usize,
            identifier: u32,
            attribute: PerfAttribute,
        ) -> efi::Status {
            let string = unsafe { string.as_ref().map(|s| CStr::from_ptr(s).to_str().unwrap().to_string()) };
            let perf_id = identifier as u16;
            _create_performance_measurement::<MockBootServices, MockFirmwareBasicBootPerfTable>(
                caller_identifier,
                guid,
                string.as_deref(),
                ticker,
                address,
                perf_id,
                attribute,
                unsafe { BOOT_SERVICES.unwrap() },
                unsafe { FBPT.unwrap() },
            )
            .unwrap();
            efi::Status::SUCCESS
        }

        const EXPECTED_NUMBER_OF_RECORD: usize = 21;

        perf_image_start_begin(module_handle, test_create_performance_measurement);
        perf_image_start_end(module_handle, test_create_performance_measurement);

        perf_load_image_begin(module_handle, test_create_performance_measurement);
        perf_load_image_end(module_handle, test_create_performance_measurement);

        perf_driver_binding_support_begin(module_handle, controller_handle, test_create_performance_measurement);
        perf_driver_binding_support_end(module_handle, controller_handle, test_create_performance_measurement);

        perf_driver_binding_start_begin(module_handle, controller_handle, test_create_performance_measurement);
        perf_driver_binding_start_end(module_handle, controller_handle, test_create_performance_measurement);

        perf_driver_binding_stop_begin(module_handle, controller_handle, test_create_performance_measurement);
        perf_driver_binding_stop_end(module_handle, controller_handle, test_create_performance_measurement);

        perf_event("event_string", &caller_id, test_create_performance_measurement);

        perf_event_signal_begin(&event_guid, "fun_name", &caller_id, test_create_performance_measurement);
        perf_event_signal_end(&event_guid, "fun_name", &caller_id, test_create_performance_measurement);

        perf_callback_begin(&trigger_guid, "fun_name", &caller_id, test_create_performance_measurement);
        perf_callback_end(&trigger_guid, "fun_name", &caller_id, test_create_performance_measurement);

        perf_function_begin("fun_name", &caller_id, test_create_performance_measurement);
        perf_function_end("fun_name", &caller_id, test_create_performance_measurement);

        perf_in_module_begin("measurement_str", &caller_id, test_create_performance_measurement);
        perf_in_module_end("measurement_str", &caller_id, test_create_performance_measurement);

        perf_cross_module_begin("measurement_str", &caller_id, test_create_performance_measurement);
        perf_cross_module_end("measurement_str", &caller_id, test_create_performance_measurement);
    }
}
