//! Core performance measurement service.
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    mem, ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use alloc::vec::Vec;

use patina::{
    BinaryGuid,
    component::{
        hob::FromHob,
        service::{IntoService, Service, perf_timer::ArchTimerFunctionality, performance::PerformanceManager},
    },
    error::EfiError,
    performance::{
        Measurement,
        config::PerformanceConfig,
        error::Error,
        measurement::{CallerIdentifier, PerfAttribute},
        record::{
            GenericPerformanceRecord, PerformanceRecord, PerformanceRecordBuffer,
            extended::{
                DualGuidStringEventRecord, DynamicStringEventRecord, GuidEventRecord, GuidQwordEventRecord,
                GuidQwordStringEventRecord,
            },
            known::KnownPerfId,
        },
    },
    pi::hob::{Hob as PiHob, HobList},
};

use crate::{cpu::PerfTimer, protocols::PROTOCOL_DB, tpl_mutex::TplMutex};

use patina::standard::efi::{
    self,
    protocols::device_path::{Media, TYPE_MEDIA},
};

mod hob;
mod table;

use hob::{HobPerformanceData, merge_hob_performance_buffer};
use table::Fbpt;

/// This is a temporary global reference for code that has not yet been converted to use the instanced
/// core mechanisms. This should be removed once `driver_services.rs` is converted.
pub(crate) static CORE_PERFORMANCE: Service<CorePerformance> = Service::new_uninit();

/// Performance measurement service owned by the DXE Core.
///
/// Owns all performance measurement state (the FBPT, measurement mask, load-image count, and arch timer). It is
/// registered as a [`Service<dyn PerformanceManager>`] for components and used directly by core internals.
#[derive(IntoService)]
#[service(dyn PerformanceManager, CorePerformance)]
pub(crate) struct CorePerformance {
    enabled: AtomicBool,
    loaded_image_count: AtomicU32,
    perf_measurement_mask: AtomicU32,
    performance_table: TplMutex<Fbpt>,
    timer: PerfTimer,
}

impl CorePerformance {
    pub(crate) const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            loaded_image_count: AtomicU32::new(0),
            perf_measurement_mask: AtomicU32::new(0),
            performance_table: TplMutex::new(efi::TPL_NOTIFY, Fbpt::new(), "PerformanceTableLock"),
            timer: PerfTimer::new(),
        }
    }

    fn measurement_enabled(&self, measurement: Measurement) -> bool {
        self.enabled.load(Ordering::Relaxed)
            && self.perf_measurement_mask.load(Ordering::Relaxed) & measurement as u32 != 0
    }

    /// Initializes the core performance service.
    pub(crate) fn init(
        &self,
        frequency: u64,
        config: PerformanceConfig,
        hob_records: Option<(u32, PerformanceRecordBuffer)>,
    ) {
        if config.enabled == PerformanceConfig::DISABLED {
            self.enabled.store(false, Ordering::Relaxed);
            log::info!("Performance measurement is disabled.");
            return;
        }

        let enabled_measurements = config.enabled_measurements;
        log::info!("Performance measurement is enabled. measurements: {enabled_measurements:#X}");
        self.enabled.store(true, Ordering::Relaxed);
        self.timer.set_frequency(frequency);

        self.perf_measurement_mask.store(config.enabled_measurements, Ordering::Relaxed);
        if let Some((load_image_count, perf_records)) = hob_records {
            self.loaded_image_count.store(load_image_count, Ordering::Relaxed);
            self.performance_table.lock().set_perf_records(perf_records);
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Begins performance measurement of start image in core.
    pub(crate) fn perf_image_start_begin(&self, module_handle: efi::Handle) {
        if self.measurement_enabled(Measurement::StartImage) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(module_handle),
                None,
                None,
                0,
                0,
                KnownPerfId::ModuleStart.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Ends performance measurement of start image in core.
    pub(crate) fn perf_image_start_end(&self, image_handle: efi::Handle) {
        if self.measurement_enabled(Measurement::StartImage) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(image_handle),
                None,
                None,
                0,
                0,
                KnownPerfId::ModuleEnd.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Begins performance measurement of load image in core.
    pub(crate) fn perf_load_image_begin(&self, module_handle: efi::Handle) {
        if self.measurement_enabled(Measurement::LoadImage) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(module_handle),
                None,
                None,
                0,
                0,
                KnownPerfId::ModuleLoadImageStart.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Ends performance measurement of load image in core.
    pub(crate) fn perf_load_image_end(&self, module_handle: efi::Handle) {
        if self.measurement_enabled(Measurement::LoadImage) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(module_handle),
                None,
                None,
                0,
                0,
                KnownPerfId::ModuleLoadImageEnd.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Begins performance measurement of driver binding support in the core.
    pub(crate) fn perf_driver_binding_support_begin(
        &self,
        driver_binding_handle: efi::Handle,
        controller_handle: efi::Handle,
    ) {
        if self.measurement_enabled(Measurement::DriverBindingSupport) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(driver_binding_handle),
                None,
                None,
                0,
                controller_handle as usize,
                KnownPerfId::ModuleDbSupportStart.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Ends performance measurement of driver binding support in the core.
    pub(crate) fn perf_driver_binding_support_end(
        &self,
        driver_binding_handle: efi::Handle,
        controller_handle: efi::Handle,
    ) {
        if self.measurement_enabled(Measurement::DriverBindingSupport) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(driver_binding_handle),
                None,
                None,
                0,
                controller_handle as usize,
                KnownPerfId::ModuleDbSupportEnd.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Begins performance measurement of driver binding start in the core.
    pub(crate) fn perf_driver_binding_start_begin(
        &self,
        driver_binding_handle: efi::Handle,
        controller_handle: efi::Handle,
    ) {
        if self.measurement_enabled(Measurement::DriverBindingStart) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(driver_binding_handle),
                None,
                None,
                0,
                controller_handle as usize,
                KnownPerfId::ModuleDbStart.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Ends performance measurement of driver binding start in the core.
    pub(crate) fn perf_driver_binding_start_end(
        &self,
        driver_binding_handle: efi::Handle,
        controller_handle: efi::Handle,
    ) {
        if self.measurement_enabled(Measurement::DriverBindingStart) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(driver_binding_handle),
                None,
                None,
                0,
                controller_handle as usize,
                KnownPerfId::ModuleDbEnd.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Begins performance measurement of driver binding stop in the core.
    pub(crate) fn perf_driver_binding_stop_begin(&self, module_handle: efi::Handle, controller_handle: efi::Handle) {
        if self.measurement_enabled(Measurement::DriverBindingStop) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(module_handle),
                None,
                None,
                0,
                controller_handle as usize,
                KnownPerfId::ModuleDbStopStart.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }

    /// Ends performance measurement of driver binding stop in the core.
    pub(crate) fn perf_driver_binding_stop_end(&self, module_handle: efi::Handle, controller_handle: efi::Handle) {
        if self.measurement_enabled(Measurement::DriverBindingStop) {
            let _ = self.create_measurement(
                CallerIdentifier::Handle(module_handle),
                None,
                None,
                0,
                controller_handle as usize,
                KnownPerfId::ModuleDbStopEnd.as_u16(),
                PerfAttribute::PerfEntry,
            );
        }
    }
}

/// Reads the [`PerformanceConfig`] from the HOB list, returning a disabled default when no such HOB is present.
pub(crate) fn read_performance_config(hob_list: &HobList) -> Option<PerformanceConfig> {
    for hob in hob_list.iter() {
        if let PiHob::GuidHob(guid, data) = hob
            && guid.name == PerformanceConfig::HOB_GUID
        {
            return Some(PerformanceConfig::parse(data));
        }
    }
    None
}

/// Reads and merges any performance records carried over from a previous phase via HOBs.
///
/// Returns `None` when no performance record HOBs are present.
pub(crate) fn read_hob_performance_records(hob_list: &HobList) -> Option<(u32, PerformanceRecordBuffer)> {
    let perf_hobs: Vec<HobPerformanceData> = hob_list
        .iter()
        .filter_map(|hob| match hob {
            PiHob::GuidHob(guid, data) if guid.name == HobPerformanceData::HOB_GUID => {
                Some(HobPerformanceData::parse(data))
            }
            _ => None,
        })
        .collect();

    if perf_hobs.is_empty() {
        return None;
    }

    merge_hob_performance_buffer(perf_hobs.iter())
        .inspect_err(|e| log::error!("Performance: failed to merge HOB performance records: {e:?}"))
        .ok()
}

impl PerformanceManager for CorePerformance {
    fn create_measurement(
        &self,
        caller_identifier: CallerIdentifier,
        guid: Option<&efi::Guid>,
        string: Option<&str>,
        ticker: u64,
        address: usize,
        perf_id: u16,
        attribute: PerfAttribute,
    ) -> Result<(), Error> {
        self.create_measurement_inner(caller_identifier, guid, string, ticker, address, perf_id, attribute)
    }

    fn add_generic_record(&self, record: &GenericPerformanceRecord) -> Result<(), Error> {
        self.add_fbpt_record(record)
    }

    fn published_table_size(&self) -> Result<usize, Error> {
        match self.performance_table.try_lock() {
            Some(table) => Ok(table.published_table_size()),
            None => Err(EfiError::AccessDenied.into()),
        }
    }

    fn publish_table(&self, buffer: &'static mut [u8]) -> Result<(), Error> {
        match self.performance_table.try_lock() {
            Some(mut table) => table.publish_table(buffer).map(|_| ()),
            None => Err(EfiError::AccessDenied.into()),
        }
    }
}

impl CorePerformance {
    /// Adds a record to the FBPT, returning an error if the table lock cannot be acquired.
    ///
    /// The FBPT [`TplMutex`] must never be acquired re-entrantly. A re-entrant performance measurement
    /// may occur when dropping a measurement's lock guard restores the TPL and dispatches a pending
    /// event notification whose callback creates another measurement before the guard finishes releasing
    /// the lock. To avoid panicking, this attempts lock acquisition and, on contention, drops the record and returns
    /// [`EfiError::AccessDenied`] so the caller can observe that the record was not added. This is similar
    /// in behavior and return status to the edk2 implementation of [`create_performance_measurement`].
    fn add_fbpt_record<T: PerformanceRecord>(&self, record: T) -> Result<(), Error> {
        match self.performance_table.try_lock() {
            Some(mut table) => table.add_record(record),
            // Re-entrant measurement: See function docs.
            None => Err(EfiError::AccessDenied.into()),
        }
    }

    /// Builds the appropriate performance record and adds it to the FBPT.
    #[allow(clippy::too_many_arguments)]
    fn create_measurement_inner(
        &self,
        caller_identifier: CallerIdentifier,
        guid: Option<&efi::Guid>,
        string: Option<&str>,
        ticker: u64,
        address: usize,
        perf_id: u16,
        attribute: PerfAttribute,
    ) -> Result<(), Error> {
        let cpu_count = self.timer.cpu_count();
        let timestamp = match ticker {
            0 => (cpu_count as f64 / self.timer.perf_frequency() as f64 * 1_000_000_000_f64) as u64,
            1 => 0,
            ticker => (ticker as f64 / self.timer.perf_frequency() as f64 * 1_000_000_000_f64) as u64,
        };

        // If the `perf_id` is not a known one, we create a DynamicStringEventRecord.
        // In this case, `caller_id` can be either an image handle or a guid pointer.
        let Ok(known_perf_id) = KnownPerfId::try_from(perf_id) else {
            // PERF_ENTRY must have a matching start and end.
            // Unknown IDs cannot be matched, so we reject PERF_ENTRY for unknown IDs.
            if attribute == PerfAttribute::PerfEntry {
                return Err(EfiError::InvalidParameter.into());
            }

            let handle = caller_identifier.as_handle().ok_or(EfiError::InvalidParameter)?;

            // Mirroring EDK2 behavior, when the ID is unknown, we treat `caller_identifier` as a handle.
            let Ok(guid) = get_module_guid_from_handle(handle) else {
                log::error!("Performance: Could not find the guid for module handle: {handle:?}");
                return Err(EfiError::InvalidParameter.into());
            };
            let module_name = string.unwrap_or("unknown name");
            return match self.add_fbpt_record(DynamicStringEventRecord::new(perf_id, 0, timestamp, guid, module_name)) {
                Ok(()) => Ok(()),
                Err(e) => Err(report_add_record_error(e)),
            };
        };

        let result = match known_perf_id {
            KnownPerfId::ModuleStart | KnownPerfId::ModuleEnd => {
                let module_handle = caller_identifier.as_handle().ok_or(EfiError::InvalidParameter)?;
                let Ok(guid) = get_module_guid_from_handle(module_handle) else {
                    log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                    return Err(EfiError::InvalidParameter.into());
                };
                self.add_fbpt_record(GuidEventRecord::new(perf_id, 0, timestamp, guid))
            }
            id @ (KnownPerfId::ModuleLoadImageStart | KnownPerfId::ModuleLoadImageEnd) => {
                if id == KnownPerfId::ModuleLoadImageStart {
                    self.loaded_image_count.fetch_add(1, Ordering::Relaxed);
                }
                let module_handle = caller_identifier.as_handle().ok_or(EfiError::InvalidParameter)?;
                let Ok(guid) = get_module_guid_from_handle(module_handle) else {
                    log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                    return Err(EfiError::InvalidParameter.into());
                };
                let load_image_count = u64::from(self.loaded_image_count.load(Ordering::Relaxed));
                self.add_fbpt_record(GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, load_image_count))
            }
            KnownPerfId::ModuleDbStart
            | KnownPerfId::ModuleDbSupportStart
            | KnownPerfId::ModuleDbSupportEnd
            | KnownPerfId::ModuleDbStopStart
            | KnownPerfId::ModuleDbStopEnd => {
                let module_handle = caller_identifier.as_handle().ok_or(EfiError::InvalidParameter)?;
                let Ok(guid) = get_module_guid_from_handle(module_handle) else {
                    log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                    return Err(EfiError::InvalidParameter.into());
                };
                self.add_fbpt_record(GuidQwordEventRecord::new(perf_id, 0, timestamp, guid, address as u64))
            }
            KnownPerfId::ModuleDbEnd => {
                let module_handle = caller_identifier.as_handle().ok_or(EfiError::InvalidParameter)?;
                let Ok(guid) = get_module_guid_from_handle(module_handle) else {
                    log::error!("Performance: Could not find the guid for module handle: {module_handle:?}");
                    return Err(EfiError::InvalidParameter.into());
                };
                let module_name = "";
                self.add_fbpt_record(GuidQwordStringEventRecord::new(
                    perf_id,
                    0,
                    timestamp,
                    guid,
                    address as u64,
                    module_name,
                ))
            }
            KnownPerfId::PerfEventSignalStart
            | KnownPerfId::PerfEventSignalEnd
            | KnownPerfId::PerfCallbackStart
            | KnownPerfId::PerfCallbackEnd => {
                let (Some(function_string), Some(guid)) = (string.as_ref(), guid) else {
                    return Err(EfiError::InvalidParameter.into());
                };
                let module_guid = caller_identifier.as_guid().ok_or(EfiError::InvalidParameter)?;
                self.add_fbpt_record(DualGuidStringEventRecord::new(
                    perf_id,
                    0,
                    timestamp,
                    (*module_guid).into(),
                    (*guid).into(),
                    function_string,
                ))
            }
            KnownPerfId::PerfFunctionStart
            | KnownPerfId::PerfFunctionEnd
            | KnownPerfId::PerfInModuleStart
            | KnownPerfId::PerfInModuleEnd
            | KnownPerfId::PerfCrossModuleStart
            | KnownPerfId::PerfCrossModuleEnd
            | KnownPerfId::PerfEvent => {
                let module_guid = caller_identifier.as_guid().ok_or(EfiError::InvalidParameter)?;
                let string = string.unwrap_or("unknown name");
                self.add_fbpt_record(DynamicStringEventRecord::new(
                    perf_id,
                    0,
                    timestamp,
                    (*module_guid).into(),
                    string,
                ))
            }
        };

        result.map_err(report_add_record_error)
    }
}

/// Logs and forwards an error encountered while adding a record to the FBPT.
fn report_add_record_error(error: Error) -> Error {
    match error {
        Error::OutOfResources => {
            static HAS_BEEN_LOGGED: AtomicBool = AtomicBool::new(false);
            if HAS_BEEN_LOGGED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                log::info!("Performance: FBPT is full, can't add more performance records!");
            }
            Error::OutOfResources
        }
        e => {
            log::error!("Performance: Something went wrong in create_measurement. status_code: {e:?}");
            e
        }
    }
}

/// Resolves the firmware file GUID for the module backing the given handle.
fn get_module_guid_from_handle(handle: efi::Handle) -> Result<BinaryGuid, efi::Status> {
    let mut guid = patina::BinaryGuid::ZERO;

    let loaded_image_ptr = 'find_loaded_image_protocol: {
        if let Ok(interface) = PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::loaded_image::PROTOCOL_GUID)
        {
            break 'find_loaded_image_protocol Some(interface as *const efi::protocols::loaded_image::Protocol);
        }

        // Fall back to a driver binding protocol on the handle and use its loaded image.
        if let Ok(driver_binding) =
            PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::driver_binding::PROTOCOL_GUID)
        {
            // SAFETY: `get_interface_for_handle` returned a valid driver binding protocol interface for this handle.
            let image_handle =
                unsafe { (*(driver_binding as *const efi::protocols::driver_binding::Protocol)).image_handle };
            if let Ok(interface) =
                PROTOCOL_DB.get_interface_for_handle(image_handle, efi::protocols::loaded_image::PROTOCOL_GUID)
            {
                break 'find_loaded_image_protocol Some(interface as *const efi::protocols::loaded_image::Protocol);
            }
        }
        None
    };

    if let Some(loaded_image_ptr) = loaded_image_ptr {
        // SAFETY: `loaded_image_ptr` is a valid pointer to a `loaded_image::Protocol` returned by the protocol database.
        let loaded_image = unsafe { &*loaded_image_ptr };
        // SAFETY: File path is a pointer from C that is valid and of type Device Path (efi).
        if let Some(file_path) = unsafe { loaded_image.file_path.as_ref() }
            && file_path.r#type == TYPE_MEDIA
            && file_path.sub_type == Media::SUBTYPE_PIWG_FIRMWARE_FILE
        {
            // The layout of MEDIA_FW_VOL_FILEPATH_DEVICE_PATH in memory is { Protocol (header) | Guid (file name) }.
            let node_len = u16::from_le_bytes(file_path.length);
            let expected_len =
                (mem::size_of::<efi::protocols::device_path::Protocol>() + mem::size_of::<efi::Guid>()) as u16;

            // Sanity check that the header matches the expected size.
            if node_len != expected_len {
                return Err(efi::Status::NOT_FOUND);
            }

            // SAFETY: To be honest there is no way to guarantee the memory read here is valid and owned by us,
            // but we have at least validated that the type gives a known layout and the layout of the device path
            // matches its claimed length.
            unsafe {
                let guid_ptr = (loaded_image.file_path as *const u8)
                    .add(mem::size_of::<efi::protocols::device_path::Protocol>())
                    as *const BinaryGuid;
                guid = ptr::read_unaligned(guid_ptr);
            }
        }
    }

    Ok(guid)
}

/// Test helper: builds a generic performance record from its parts and pushes it into `buffer`.
#[cfg(test)]
pub(crate) fn push_generic_record(buffer: &mut PerformanceRecordBuffer, record_type: u16, revision: u8, data: &[u8]) {
    use patina::performance::record::PERFORMANCE_RECORD_HEADER_SIZE;
    let length = (PERFORMANCE_RECORD_HEADER_SIZE + data.len()) as u8;
    let mut bytes = Vec::with_capacity(length as usize);
    bytes.extend_from_slice(&record_type.to_le_bytes());
    bytes.push(length);
    bytes.push(revision);
    bytes.extend_from_slice(data);
    buffer.push_record(GenericPerformanceRecord::ref_from_bytes(&bytes).unwrap()).unwrap();
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    //! Global-state isolation has to be considered for the tests in this module.
    //!
    //! Each test should pick whether to use a wrapper and exactly one wrapper based on the following
    //! criteria:
    //!
    //! - No wrapper: The test only touches locally-constructed resources (its own atomics and timer).
    //!   It (and helpers it might use) reaches no global state, so it needs neither serialization
    //!   nor cleanup.
    //! - `with_global_lock`: The test locks the FBPT `TplMutex`. Since raising and restoring the TPL
    //!   mutates the SDK's process-global interrupt state, the test must serialize against other
    //!   tests. It does not touch the GCD, protocol database, or allocators, so no reset is required.
    //! - `with_clean_global_lock`: The test reads or writes the global `PROTOCOL_DB` (directly or
    //!   indirectly). In addition to taking the global lock, it resets that shared state before and
    //!   after so the test starts and ends from a clean slate.
    use super::*;
    use crate::test_support::{with_clean_global_lock, with_global_lock};
    use patina::standard::efi;

    /// Builds a `CorePerformance` backed by a real FBPT and a fixed-frequency timer for host testing.
    fn test_core_performance() -> CorePerformance {
        CorePerformance {
            enabled: AtomicBool::new(true),
            loaded_image_count: AtomicU32::new(0),
            perf_measurement_mask: AtomicU32::new(0),
            performance_table: TplMutex::new(efi::TPL_NOTIFY, Fbpt::new(), "TestPerfTableLock"),
            timer: PerfTimer::with_frequency(100),
        }
    }

    /// Exercises every record-building arm of [`CorePerformance::create_measurement_inner`], verifying each
    /// known performance id produces exactly one record in the table.
    #[test]
    fn test_core_performance_create_measurement_all_records() {
        with_clean_global_lock(|| {
            const EXPECTED_NUMBER_OF_RECORD: usize = 21;
            let perf = test_core_performance();

            let module_handle = 1_usize as efi::Handle;
            let caller_guid = efi::Guid::from_bytes(&[1; 16]);
            let event_guid = efi::Guid::from_bytes(&[2; 16]);

            macro_rules! measure {
                ($caller:expr, $guid:expr, $string:expr, $perf_id:ident) => {
                    perf.create_measurement_inner(
                        $caller,
                        $guid,
                        $string,
                        0,
                        0,
                        KnownPerfId::$perf_id.as_u16(),
                        PerfAttribute::PerfEntry,
                    )
                    .unwrap();
                };
            }

            // Handle-based records (GUID resolved from the handle, which yields ZERO here).
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleStart);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleEnd);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleLoadImageStart);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleLoadImageEnd);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleDbStart);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleDbSupportStart);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleDbSupportEnd);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleDbStopStart);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleDbStopEnd);
            measure!(CallerIdentifier::Handle(module_handle), None, None, ModuleDbEnd);

            // Dual-guid + string records (require a caller GUID, a trigger GUID, and a string).
            measure!(CallerIdentifier::Guid(caller_guid), Some(&event_guid), Some("fun_name"), PerfEventSignalStart);
            measure!(CallerIdentifier::Guid(caller_guid), Some(&event_guid), Some("fun_name"), PerfEventSignalEnd);
            measure!(CallerIdentifier::Guid(caller_guid), Some(&event_guid), Some("fun_name"), PerfCallbackStart);
            measure!(CallerIdentifier::Guid(caller_guid), Some(&event_guid), Some("fun_name"), PerfCallbackEnd);

            // Dynamic-string records (require a caller GUID and a string).
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfFunctionStart);
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfFunctionEnd);
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfInModuleStart);
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfInModuleEnd);
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfCrossModuleStart);
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfCrossModuleEnd);
            measure!(CallerIdentifier::Guid(caller_guid), None, Some("measurement_str"), PerfEvent);

            assert_eq!(perf.performance_table.lock().perf_records().iter().count(), EXPECTED_NUMBER_OF_RECORD);
        })
        .unwrap();
    }

    /// Verifies the validation paths of [`CorePerformance::create_measurement_inner`] for unknown perf ids.
    #[test]
    fn test_core_performance_create_measurement_invalid_params() {
        with_clean_global_lock(|| {
            let perf = test_core_performance();

            // A PerfEntry must have a known perf id.
            let unknown_perf_id = 0xFFFF;
            let result = perf.create_measurement_inner(
                CallerIdentifier::Handle(0x1_usize as efi::Handle),
                None,
                Some("test"),
                0,
                0,
                unknown_perf_id,
                PerfAttribute::PerfEntry,
            );
            assert_eq!(result.unwrap_err(), Error::Efi(EfiError::InvalidParameter));

            // If the perf id is unknown, the caller identifier must be a handle.
            let result = perf.create_measurement_inner(
                CallerIdentifier::Guid(efi::Guid::from_bytes(&[1; 16])),
                None,
                Some("test"),
                0,
                0,
                unknown_perf_id,
                PerfAttribute::PerfStartEntry,
            );
            assert_eq!(result.unwrap_err(), Error::Efi(EfiError::InvalidParameter));
        })
        .unwrap();
    }

    /// Verifies that [`read_performance_config`] reads the config HOB when present and falls back to a disabled
    /// default when absent.
    #[test]
    fn test_core_performance_read_performance_config() {
        use patina::pi::hob::{GUID_EXTENSION, GuidHob, HobHeader};

        // Absent: returns a disabled default.
        let empty = HobList::new();
        let config = read_performance_config(&empty);
        assert!(config.is_none());

        // Present: enabled with a measurement mask of 0x9 (packed u8 + u32, little-endian).
        let bytes: [u8; 5] = [PerformanceConfig::ENABLED, 0x09, 0x00, 0x00, 0x00];
        let guid_hob = GuidHob {
            header: HobHeader { r#type: GUID_EXTENSION, length: 0, reserved: 0 },
            name: PerformanceConfig::HOB_GUID,
        };
        let mut hob_list = HobList::new();
        hob_list.push(PiHob::GuidHob(&guid_hob, &bytes));

        let config = read_performance_config(&hob_list).expect("config present");
        assert_eq!(config.enabled, PerformanceConfig::ENABLED);
        assert_eq!({ config.enabled_measurements }, 0x9);
    }

    /// Verifies that [`read_hob_performance_records`] merges carried-over performance HOBs and returns `None` when
    /// none are present.
    #[test]
    fn test_core_performance_read_hob_performance_records() {
        use patina::pi::hob::{GUID_EXTENSION, GuidHob, HobHeader};

        // Absent: returns None.
        let empty = HobList::new();
        assert!(read_hob_performance_records(&empty).is_none());

        // Present: a single HOB carrying a load-image count and no records.
        const LOAD_IMAGE_COUNT: u32 = 7;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes()); // size_of_all_entries
        bytes.extend_from_slice(&LOAD_IMAGE_COUNT.to_le_bytes()); // load_image_count
        bytes.extend_from_slice(&0u32.to_le_bytes()); // hob_is_full
        let guid_hob = GuidHob {
            header: HobHeader { r#type: GUID_EXTENSION, length: 0, reserved: 0 },
            name: HobPerformanceData::HOB_GUID,
        };
        let mut hob_list = HobList::new();
        hob_list.push(PiHob::GuidHob(&guid_hob, &bytes));

        let (load_image_count, records) = read_hob_performance_records(&hob_list).expect("records present");
        assert_eq!(load_image_count, LOAD_IMAGE_COUNT);
        assert_eq!(records.iter().count(), 0);
    }

    /// Verifies that [`CorePerformance::init`] honors the disable gate: a disabled config leaves the service
    /// disabled and records nothing.
    #[test]
    fn test_core_performance_init_disabled() {
        const FREQ: u64 = 987_654;
        let perf = test_core_performance();
        let config = PerformanceConfig::new();
        assert_eq!(config.enabled, PerformanceConfig::DISABLED);

        perf.init(FREQ, config, None);

        assert!(!perf.enabled());
    }

    /// Verifies that [`CorePerformance::init`] applies the enabled config: it sets the timer frequency and
    /// measurement mask and restores the carried-over load-image count and performance records.
    #[test]
    fn test_core_performance_init_enabled() {
        with_global_lock(|| {
            const FREQ: u64 = 987_654;
            const LOAD_IMAGE_COUNT: u32 = 7;
            let perf = test_core_performance();
            let config = PerformanceConfig { enabled: PerformanceConfig::ENABLED, enabled_measurements: 0xFF };

            perf.init(FREQ, config, Some((LOAD_IMAGE_COUNT, PerformanceRecordBuffer::new())));

            assert!(perf.enabled());
            assert_eq!(perf.timer.perf_frequency(), FREQ);
            assert_eq!(perf.loaded_image_count.load(Ordering::Relaxed), LOAD_IMAGE_COUNT);
            assert_eq!(perf.performance_table.lock().perf_records().iter().count(), 0);
        })
        .unwrap();
    }

    fn test_core_performance_with_mask(mask: u32) -> CorePerformance {
        let perf = test_core_performance();
        perf.perf_measurement_mask.store(mask, Ordering::Relaxed);
        perf
    }

    fn install_loaded_image_with_fw_path(node_length: u16, file_guid: efi::Guid) -> efi::Handle {
        #[repr(C)]
        struct FwFilePathNode {
            header: efi::protocols::device_path::Protocol,
            guid: efi::Guid,
        }

        let node = alloc::boxed::Box::new(FwFilePathNode {
            header: efi::protocols::device_path::Protocol {
                r#type: TYPE_MEDIA,
                sub_type: Media::SUBTYPE_PIWG_FIRMWARE_FILE,
                length: node_length.to_le_bytes(),
            },
            guid: file_guid,
        });
        let node_ptr = alloc::boxed::Box::into_raw(node) as *mut efi::protocols::device_path::Protocol;

        let loaded_image = alloc::boxed::Box::new(efi::protocols::loaded_image::Protocol {
            revision: efi::protocols::loaded_image::REVISION,
            parent_handle: ptr::null_mut(),
            system_table: ptr::null_mut(),
            device_handle: ptr::null_mut(),
            file_path: node_ptr,
            reserved: ptr::null_mut(),
            load_options_size: 0,
            load_options: ptr::null_mut(),
            image_base: ptr::null_mut(),
            image_size: 0,
            image_code_type: efi::BOOT_SERVICES_CODE,
            image_data_type: efi::BOOT_SERVICES_DATA,
            unload: None,
        });
        let loaded_image_ptr = alloc::boxed::Box::into_raw(loaded_image) as *mut core::ffi::c_void;

        let (handle, _) = PROTOCOL_DB
            .install_protocol_interface(None, efi::protocols::loaded_image::PROTOCOL_GUID, loaded_image_ptr)
            .unwrap();
        handle
    }

    #[test]
    fn test_core_performance_new_is_disabled() {
        let perf = CorePerformance::new();
        assert!(!perf.enabled());
        assert_eq!(perf.loaded_image_count.load(Ordering::Relaxed), 0);
        assert_eq!(perf.perf_measurement_mask.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_core_performance_perf_helpers_record_when_enabled() {
        with_clean_global_lock(|| {
            const ALL_MEASUREMENTS: u32 = 0x1F;
            let perf = test_core_performance_with_mask(ALL_MEASUREMENTS);

            let module = 1_usize as efi::Handle;
            let controller = 2_usize as efi::Handle;

            perf.perf_image_start_begin(module);
            perf.perf_image_start_end(module);
            perf.perf_load_image_begin(module);
            perf.perf_load_image_end(module);
            perf.perf_driver_binding_support_begin(module, controller);
            perf.perf_driver_binding_support_end(module, controller);
            perf.perf_driver_binding_start_begin(module, controller);
            perf.perf_driver_binding_start_end(module, controller);
            perf.perf_driver_binding_stop_begin(module, controller);
            perf.perf_driver_binding_stop_end(module, controller);

            assert_eq!(perf.performance_table.lock().perf_records().iter().count(), 10);
        })
        .unwrap();
    }

    #[test]
    fn test_core_performance_perf_helpers_noop_when_disabled() {
        with_global_lock(|| {
            let perf = test_core_performance_with_mask(0);

            let module = 1_usize as efi::Handle;
            let controller = 2_usize as efi::Handle;

            perf.perf_image_start_begin(module);
            perf.perf_image_start_end(module);
            perf.perf_load_image_begin(module);
            perf.perf_load_image_end(module);
            perf.perf_driver_binding_support_begin(module, controller);
            perf.perf_driver_binding_support_end(module, controller);
            perf.perf_driver_binding_start_begin(module, controller);
            perf.perf_driver_binding_start_end(module, controller);
            perf.perf_driver_binding_stop_begin(module, controller);
            perf.perf_driver_binding_stop_end(module, controller);

            assert_eq!(perf.performance_table.lock().perf_records().iter().count(), 0);
        })
        .unwrap();
    }

    #[test]
    fn test_core_performance_manager_trait_methods() {
        with_global_lock(|| {
            let perf = test_core_performance();

            // create_measurement forwards to the inner implementation.
            PerformanceManager::create_measurement(
                &perf,
                CallerIdentifier::Guid(efi::Guid::from_bytes(&[3; 16])),
                None,
                Some("measurement"),
                0,
                0,
                KnownPerfId::PerfEvent.as_u16(),
                PerfAttribute::PerfEntry,
            )
            .unwrap();

            // add_generic_record appends a manually-built generic record.
            let data = [0xAA_u8; 4];
            let record_type = 0x1010_u16;
            let length = (patina::performance::record::PERFORMANCE_RECORD_HEADER_SIZE + data.len()) as u8;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&record_type.to_le_bytes());
            bytes.push(length);
            bytes.push(0); // revision
            bytes.extend_from_slice(&data);
            let record = GenericPerformanceRecord::ref_from_bytes(&bytes).unwrap();
            perf.add_generic_record(record).unwrap();

            assert_eq!(perf.performance_table.lock().perf_records().iter().count(), 2);

            // published_table_size reports a non-zero size, and publish_table writes into a large-enough buffer.
            let size = perf.published_table_size().unwrap();
            assert!(size > 0);
            let buffer: &'static mut [u8] = alloc::boxed::Box::leak(alloc::vec![0u8; size].into_boxed_slice());
            perf.publish_table(buffer).unwrap();
        })
        .unwrap();
    }

    #[test]
    fn test_core_performance_manager_locked_table_returns_error() {
        with_global_lock(|| {
            let perf = test_core_performance();
            let _guard = perf.performance_table.lock();

            assert_eq!(perf.published_table_size().unwrap_err(), Error::Efi(EfiError::AccessDenied));

            let buffer: &'static mut [u8] = alloc::boxed::Box::leak(alloc::vec![0u8; 64].into_boxed_slice());
            assert_eq!(perf.publish_table(buffer).unwrap_err(), Error::Efi(EfiError::AccessDenied));

            // A re-entrant record add cannot acquire the held table lock and drops the record.
            let data = [0u8; 4];
            let length = (patina::performance::record::PERFORMANCE_RECORD_HEADER_SIZE + data.len()) as u8;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&0x1010_u16.to_le_bytes());
            bytes.push(length);
            bytes.push(0);
            bytes.extend_from_slice(&data);
            let record = GenericPerformanceRecord::ref_from_bytes(&bytes).unwrap();
            assert_eq!(perf.add_generic_record(record).unwrap_err(), Error::Efi(EfiError::AccessDenied));
        })
        .unwrap();
    }

    #[test]
    fn test_core_performance_report_add_record_error() {
        assert_eq!(report_add_record_error(Error::OutOfResources), Error::OutOfResources);
        assert_eq!(
            report_add_record_error(Error::Efi(EfiError::InvalidParameter)),
            Error::Efi(EfiError::InvalidParameter)
        );
    }

    #[test]
    fn test_get_module_guid_from_handle_resolves_fw_file_guid() {
        with_clean_global_lock(|| {
            let file_guid = efi::Guid::from_bytes(&[0xAB; 16]);
            let node_length =
                (mem::size_of::<efi::protocols::device_path::Protocol>() + mem::size_of::<efi::Guid>()) as u16;
            let handle = install_loaded_image_with_fw_path(node_length, file_guid);

            let resolved = get_module_guid_from_handle(handle).unwrap();
            assert_eq!(resolved, BinaryGuid::from(file_guid));
        })
        .unwrap();
    }

    #[test]
    fn test_get_module_guid_from_handle_rejects_bad_node_length() {
        with_clean_global_lock(|| {
            let file_guid = efi::Guid::from_bytes(&[0xCD; 16]);
            // A node length that does not match the expected header + GUID size.
            let handle = install_loaded_image_with_fw_path(4, file_guid);

            assert_eq!(get_module_guid_from_handle(handle), Err(efi::Status::NOT_FOUND));
        })
        .unwrap();
    }

    #[test]
    fn test_get_module_guid_from_handle_without_protocol_returns_zero() {
        with_clean_global_lock(|| {
            let resolved = get_module_guid_from_handle(0x1234_usize as efi::Handle).unwrap();
            assert_eq!(resolved, patina::BinaryGuid::ZERO);
        })
        .unwrap();
    }

    #[test]
    fn test_get_module_guid_from_handle_uses_driver_binding_fallback() {
        extern "efiapi" fn stub_supported(
            _: *mut efi::protocols::driver_binding::Protocol,
            _: efi::Handle,
            _: *mut efi::protocols::device_path::Protocol,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn stub_start(
            _: *mut efi::protocols::driver_binding::Protocol,
            _: efi::Handle,
            _: *mut efi::protocols::device_path::Protocol,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }
        extern "efiapi" fn stub_stop(
            _: *mut efi::protocols::driver_binding::Protocol,
            _: efi::Handle,
            _: usize,
            _: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::SUCCESS
        }

        with_clean_global_lock(|| {
            let file_guid = efi::Guid::from_bytes(&[0xEF; 16]);
            let node_length =
                (mem::size_of::<efi::protocols::device_path::Protocol>() + mem::size_of::<efi::Guid>()) as u16;
            let image_handle = install_loaded_image_with_fw_path(node_length, file_guid);

            // A separate handle carries only a driver-binding protocol referencing the image handle above.
            let driver_binding = alloc::boxed::Box::new(efi::protocols::driver_binding::Protocol {
                supported: stub_supported,
                start: stub_start,
                stop: stub_stop,
                version: 1,
                image_handle,
                driver_binding_handle: ptr::null_mut(),
            });
            let driver_binding_ptr = alloc::boxed::Box::into_raw(driver_binding) as *mut core::ffi::c_void;
            let (db_handle, _) = PROTOCOL_DB
                .install_protocol_interface(None, efi::protocols::driver_binding::PROTOCOL_GUID, driver_binding_ptr)
                .unwrap();

            let resolved = get_module_guid_from_handle(db_handle).unwrap();
            assert_eq!(resolved, BinaryGuid::from(file_guid));
        })
        .unwrap();
    }
}
