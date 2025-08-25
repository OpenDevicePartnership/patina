//! Functionality for managing performance measurements.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;

use alloc::{boxed::Box, string::ToString, vec::Vec};
use core::{
    clone::Clone,
    convert::AsRef,
    ffi::{CStr, c_char, c_void},
    ops::BitOr,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    boot_services::BootServices,
    error::EfiError,
    guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE,
    performance::{
        self,
        _smm::{CommunicateProtocol, MmCommRegion, SmmGetRecordDataByOffset, SmmGetRecordSize},
        error::Error,
        globals::{get_load_image_count, get_static_state, increment_load_image_count},
        record::{
            extended::{
                DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
                GuidQwordStringEventRecord,
            },
            known::{KnownPerfId, KnownPerfToken},
        },
        table::FirmwareBasicBootPerfTable,
    },
    runtime_services::RuntimeServices,
    tpl_mutex::TplMutex,
    uefi_protocol::{performance_measurement::PerfAttribute, status_code::StatusCodeRuntimeProtocol},
};

use mu_pi::status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_BS_DRIVER};
use mu_rust_helpers::perf_timer::{Arch, ArchFunctionality};

use r_efi::{
    efi::{self, Guid},
    protocols::device_path::{Media, TYPE_MEDIA},
};

/// Functions intended to be registered as event callbacks for reporting performance measurements.
pub mod event_callback {

    use super::*;

    /// Reports the Firmware Basic Boot Performance Table (FBPT) record buffer.
    pub extern "efiapi" fn report_fbpt_record_buffer<BB, B, RR, R, F>(
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
            performance::table::find_previous_table_address(runtime_services.as_ref()),
            boot_services.as_ref(),
        ) else {
            log::error!("Performance: Fail to report FBPT.");
            return;
        };

        let Ok(p) = (unsafe { boot_services.as_ref().locate_protocol::<StatusCodeRuntimeProtocol>(None) }) else {
            log::error!("Performance: Fail to find status code protocol.");
            return;
        };

        let status = p.report_status_code_with_data(
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

    /// Adds SMM performance records to the Firmware Basic Boot Performance Table (FBPT).
    pub extern "efiapi" fn fetch_and_add_mm_performance_records<BB, B, F>(
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
                    "Performance: Asking for the smm perf records size result in an error with return status of: {return_status:?}",
                );
                return;
            }
            Err(status) => {
                log::error!(
                    "Performance: Error while trying to communicate with communicate protocol with error code: {status:?}",
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
                communication.communicate(
                    SmmGetRecordDataByOffset::<BUFFER_SIZE>::new(smm_boot_records_data.len()),
                    mm_comm_region,
                )
            } {
                Ok(record_data) if record_data.return_status == efi::Status::SUCCESS => {
                    // Append the byte to the total smm performance record data.
                    smm_boot_records_data.extend_from_slice(record_data.boot_record_data());
                }
                Ok(SmmGetRecordDataByOffset { return_status, .. }) => {
                    log::error!(
                        "Performance: Asking for smm perf records data result in an error with return status of: {return_status:?}",
                    );
                    return;
                }
                Err(status) => {
                    log::error!(
                        "Performance: Error while trying to communicate with communicate protocol with error status code: {status:?}",
                    );
                    return;
                }
            };
        }

        // Write found perf records in the fbpt table.
        let mut fbpt = fbpt.lock();
        let mut n = 0;
        for r in performance::record::Iter::new(&smm_boot_records_data) {
            _ = fbpt.add_record(r);
            n += 1;
        }

        log::info!("Performance: {n} smm performance records found.");
    }
}

#[coverage(off)]
// Tested via the generic version, see _create_performance_measurement. This one is using the static state which makes
// it not mockable.
///
/// # Safety
/// String must be a valid C string pointer.
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

    // NOTE: If the Perf is not the known Token used in the core but have same ID with the core Token, this case will
    //       not be supported.
    // And in current usage mode, for the unknown ID, there is a general rule:
    //   - If it is start pref: the lower 4 bits of the ID should be 0.
    //   - If it is end pref: the lower 4 bits of the ID should not be 0.
    //   - If input ID doesn't follow the rule, we will adjust it.
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

/// Create a performance measurement and add it to the FBPT.
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
                log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                return Err(EfiError::InvalidParameter.into());
            };
            let record = GuidEventRecord::new(perf_id, 0, timestamp, guid);
            fbpt.lock().add_record(record)?;
        }
        id @ KnownPerfId::ModuleLoadImageStart | id @ KnownPerfId::ModuleLoadImageEnd => {
            if id == KnownPerfId::ModuleLoadImageStart {
                increment_load_image_count();
            }
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                return Err(EfiError::InvalidParameter.into());
            };
            let record = GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, get_load_image_count() as u64);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::ModuleDbStart
        | KnownPerfId::ModuleDbEnd
        | KnownPerfId::ModuleDbSupportStart
        | KnownPerfId::ModuleDbSupportEnd
        | KnownPerfId::ModuleDbStopStart => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                return Err(EfiError::InvalidParameter.into());
            };
            let record = GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, address as u64);
            fbpt.lock().add_record(record)?;
        }
        KnownPerfId::ModuleDbStopEnd => {
            let module_handle = caller_identifier as efi::Handle;
            let Ok(guid) = get_module_guid_from_handle(boot_services, module_handle) else {
                log::error!("Performance Lib: Could not find the guid for module handle: {module_handle:?}");
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

/// Measurement enum that represents the different performance measurements that can be enabled.
#[derive(Debug, PartialEq)]
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

/// Implement bitwise OR for measurements (`Measurement | Measurement`).
impl BitOr for Measurement {
    type Output = u32;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.as_u32() | rhs.as_u32()
    }
}

/// Implement bitwise OR for measurements and u32 (`Measurement | u32`).
impl BitOr<u32> for Measurement {
    type Output = u32;

    fn bitor(self, rhs: u32) -> Self::Output {
        self.as_u32() | rhs
    }
}

/// Implement bitwise OR for u32 and measurements (`u32 | Measurement`).
impl BitOr<Measurement> for u32 {
    type Output = u32;

    fn bitor(self, rhs: Measurement) -> Self::Output {
        self | rhs.as_u32()
    }
}

/// Performance property structure used to store performance related properties.
#[repr(C)]
pub struct PerformanceProperty {
    revision: u32,
    reserved: u32,
    frequency: u64,
    timer_start_value: u64,
    timer_end_value: u64,
}

impl PerformanceProperty {
    /// Creates a new `PerformanceProperty` with the specified frequency, timer start value, and timer end value.
    ///
    /// # Arguments
    /// - `frequency`: The frequency of the performance measurement.
    /// - `timer_start_value`: The start value of the timer.
    /// - `timer_end_value`: The end value of the timer.
    ///
    /// # Returns
    /// A new instance of `PerformanceProperty`.
    ///
    /// # Example
    /// ```rust
    /// use patina_sdk::performance::measurement::PerformanceProperty;
    /// let performance_property = PerformanceProperty::new(1000, 0, 100);
    /// ```
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
#[coverage(off)]
mod tests {
    use super::*;

    use alloc::rc::Rc;
    use core::{mem::MaybeUninit, ptr};

    use mockall::predicate;

    use crate::{
        boot_services::{MockBootServices, c_ptr::CMutPtr, tpl::Tpl},
        performance::{
            globals::set_perf_measurement_mask,
            logging::*,
            table::{FirmwarePerformanceVariable, MockFirmwareBasicBootPerfTable},
        },
        runtime_services::MockRuntimeServices,
    };

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

        event_callback::report_fbpt_record_buffer(
            1_usize as efi::Event,
            Box::new((Rc::new(boot_services), Rc::new(runtime_services), fbpt)),
        );

        assert!(REPORT_STATUS_CODE_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_create_performance_measurement() {
        set_perf_measurement_mask(u32::MAX);
        let mut boot_services = MockBootServices::new();

        let mut loaded_image_protocol = MaybeUninit::<efi::protocols::loaded_image::Protocol>::zeroed();
        let mut media_fw_vol_file_path_device_path = MaybeUninit::<MediaFwVolFilepathDevicePath>::zeroed();
        unsafe {
            media_fw_vol_file_path_device_path.assume_init_mut().header.r#type = TYPE_MEDIA;
            media_fw_vol_file_path_device_path.assume_init_mut().header.sub_type = Media::SUBTYPE_PIWG_FIRMWARE_FILE;
            media_fw_vol_file_path_device_path.assume_init_mut().fv_file_name = efi::Guid::from_bytes(&[3; 16]);

            loaded_image_protocol.assume_init_mut().file_path =
                media_fw_vol_file_path_device_path.as_mut_ptr() as *mut efi::protocols::device_path::Protocol;
        }
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
