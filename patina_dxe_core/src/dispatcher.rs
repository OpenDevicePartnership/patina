//! DXE Core Dispatcher
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use core::{cmp::Ordering, ffi::c_void};
use mu_rust_helpers::{function, guid::guid_fmt};
use patina::{
    component::service::Service,
    error::EfiError,
    performance::{
        logging::{perf_function_begin, perf_function_end},
        measurement::create_performance_measurement,
    },
    pi::{fw_fs::ffs, protocols::firmware_volume_block},
};
use patina_ffs::{
    section::{Section, SectionExtractor},
    volume::VolumeRef,
};
use patina_internal_depex::{AssociatedDependency, Depex, Opcode};
use patina_internal_device_path::concat_device_path_to_boxed_slice;
use r_efi::efi;

use mu_rust_helpers::guid::CALLER_ID;

use crate::{
    decompress::CoreExtractor,
    events::EVENT_DB,
    fv::{core_install_firmware_volume, device_path_bytes_for_fv_file},
    image::{core_load_image, core_start_image},
    protocol_db::DXE_CORE_HANDLE,
    protocols::PROTOCOL_DB,
    tpl_lock::TplMutex,
};

// Default Dependency expression per PI spec v1.2 Vol 2 section 10.9.
const ALL_ARCH_DEPEX: &[Opcode] = &[
    Opcode::Push(uuid::Uuid::from_u128(0x665e3ff6_46cc_11d4_9a38_0090273fc14d), false), //BDS Arch
    Opcode::Push(uuid::Uuid::from_u128(0x26baccb1_6f42_11d4_bce7_0080c73c8881), false), //Cpu Arch
    Opcode::Push(uuid::Uuid::from_u128(0x26baccb2_6f42_11d4_bce7_0080c73c8881), false), //Metronome Arch
    Opcode::Push(uuid::Uuid::from_u128(0x1da97072_bddc_4b30_99f1_72a0b56fff2a), false), //Monotonic Counter Arch
    Opcode::Push(uuid::Uuid::from_u128(0x27cfac87_46cc_11d4_9a38_0090273fc14d), false), //Real Time Clock Arch
    Opcode::Push(uuid::Uuid::from_u128(0x27cfac88_46cc_11d4_9a38_0090273fc14d), false), //Reset Arch
    Opcode::Push(uuid::Uuid::from_u128(0xb7dfb4e1_052f_449f_87be_9818fc91b733), false), //Runtime Arch
    Opcode::Push(uuid::Uuid::from_u128(0xa46423e3_4617_49f1_b9ff_d1bfa9115839), false), //Security Arch
    Opcode::Push(uuid::Uuid::from_u128(0x26baccb3_6f42_11d4_bce7_0080c73c8881), false), //Timer Arch
    Opcode::Push(uuid::Uuid::from_u128(0x6441f818_6362_4e44_b570_7dba31dd2453), false), //Variable Write Arch
    Opcode::Push(uuid::Uuid::from_u128(0x1e5668e2_8481_11d4_bcf1_0080c73c8881), false), //Variable Arch
    Opcode::Push(uuid::Uuid::from_u128(0x665e3ff5_46cc_11d4_9a38_0090273fc14d), false), //Watchdog Arch
    Opcode::And,                                                                        //Variable + Watchdog
    Opcode::And,                                                                        //+Variable Write
    Opcode::And,                                                                        //+Timer
    Opcode::And,                                                                        //+Security
    Opcode::And,                                                                        //+Runtime
    Opcode::And,                                                                        //+Reset
    Opcode::And,                                                                        //+Real Time Clock
    Opcode::And,                                                                        //+Monotonic Counter
    Opcode::And,                                                                        //+Metronome
    Opcode::And,                                                                        //+Cpu
    Opcode::And,                                                                        //+Bds
    Opcode::End,
];

struct PendingDriver {
    firmware_volume_handle: efi::Handle,
    device_path: *mut efi::protocols::device_path::Protocol,
    file_name: efi::Guid,
    depex: Option<Depex>,
    pe32: Section,
    image_handle: Option<efi::Handle>,
    security_status: efi::Status,
}

struct PendingFirmwareVolumeImage {
    parent_fv_handle: efi::Handle,
    file_name: efi::Guid,
    depex: Option<Depex>,
    fv_sections: Vec<Section>,
}

impl PendingFirmwareVolumeImage {
    // authenticate the pending firmware volume via the Security Architectural Protocol
    fn evaluate_auth(&self) -> Result<(), EfiError> {
        let security_protocol = unsafe {
            match PROTOCOL_DB.locate_protocol(patina::pi::protocols::security::PROTOCOL_GUID) {
                Ok(protocol) => (protocol as *mut patina::pi::protocols::security::Protocol)
                    .as_ref()
                    .expect("Security Protocol should not be null"),
                //If security protocol is not located, then assume it has not yet been produced and implicitly trust the
                //Firmware Volume.
                Err(_) => return Ok(()),
            }
        };
        let file_path = device_path_bytes_for_fv_file(self.parent_fv_handle, self.file_name)
            .map_err(|status| EfiError::status_to_result(status).unwrap_err())?;

        //Important Note: the present section extraction implementation does not support section extraction-based
        //authentication status, so it is hard-coded to zero here. The primary security handlers for the main usage
        //scenarios (TPM measurement and UEFI Secure Boot) do not use it.
        let status = (security_protocol.file_authentication_state)(
            security_protocol as *const _ as *mut patina::pi::protocols::security::Protocol,
            0,
            file_path.as_ptr() as *const _ as *mut efi::protocols::device_path::Protocol,
        );
        EfiError::status_to_result(status)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct OrdGuid(efi::Guid);

impl PartialOrd for OrdGuid {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdGuid {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

#[derive(Default)]
struct DispatcherContext {
    executing: bool,
    arch_protocols_available: bool,
    pending_drivers: Vec<PendingDriver>,
    fv_section_data: Vec<Box<[u8]>>,
    pending_firmware_volume_images: Vec<PendingFirmwareVolumeImage>,
    associated_before: BTreeMap<OrdGuid, Vec<PendingDriver>>,
    associated_after: BTreeMap<OrdGuid, Vec<PendingDriver>>,
    processed_fvs: BTreeSet<efi::Handle>,
    section_extractor: CoreExtractor,
}

impl DispatcherContext {
    const fn new() -> Self {
        Self {
            executing: false,
            arch_protocols_available: false,
            pending_drivers: Vec::new(),
            fv_section_data: Vec::new(),
            pending_firmware_volume_images: Vec::new(),
            associated_before: BTreeMap::new(),
            associated_after: BTreeMap::new(),
            processed_fvs: BTreeSet::new(),
            section_extractor: CoreExtractor::new(),
        }
    }
}

unsafe impl Send for DispatcherContext {}

static DISPATCHER_CONTEXT: TplMutex<DispatcherContext> =
    TplMutex::new(efi::TPL_NOTIFY, DispatcherContext::new(), "Dispatcher Context");

pub fn dispatch() -> Result<bool, EfiError> {
    if DISPATCHER_CONTEXT.lock().executing {
        return Err(EfiError::AlreadyStarted);
    }

    let scheduled: Vec<PendingDriver>;
    {
        let mut dispatcher = DISPATCHER_CONTEXT.lock();
        if !dispatcher.arch_protocols_available {
            dispatcher.arch_protocols_available = Depex::from(ALL_ARCH_DEPEX).eval(&PROTOCOL_DB.registered_protocols());
        }
        let driver_candidates: Vec<_> = dispatcher.pending_drivers.drain(..).collect();
        let mut scheduled_driver_candidates = Vec::new();
        for mut candidate in driver_candidates {
            log::debug!(target: "patina_internal_depex", "Evaluating depex for candidate: {:?}", guid_fmt!(candidate.file_name));
            let depex_satisfied = match candidate.depex {
                Some(ref mut depex) => depex.eval(&PROTOCOL_DB.registered_protocols()),
                None => dispatcher.arch_protocols_available,
            };

            if depex_satisfied {
                scheduled_driver_candidates.push(candidate)
            } else {
                match candidate.depex.as_ref().map(|x| x.is_associated()) {
                    Some(Some(AssociatedDependency::Before(guid))) => {
                        dispatcher.associated_before.entry(OrdGuid(guid)).or_default().push(candidate)
                    }
                    Some(Some(AssociatedDependency::After(guid))) => {
                        dispatcher.associated_after.entry(OrdGuid(guid)).or_default().push(candidate)
                    }
                    _ => dispatcher.pending_drivers.push(candidate),
                }
            }
        }

        // insert contents of associated_before/after at the appropriate point in the schedule if the associated driver is present.
        scheduled = scheduled_driver_candidates
            .into_iter()
            .flat_map(|scheduled_driver| {
                let filename = OrdGuid(scheduled_driver.file_name);
                let mut list = dispatcher.associated_before.remove(&filename).unwrap_or_default();
                let mut after_list = dispatcher.associated_after.remove(&filename).unwrap_or_default();
                list.push(scheduled_driver);
                list.append(&mut after_list);
                list
            })
            .collect();
    }
    log::info!("Depex evaluation complete, scheduled {:} drivers", scheduled.len());

    let mut dispatch_attempted = false;
    for mut driver in scheduled {
        if driver.image_handle.is_none() {
            log::info!("Loading file: {:?}", guid_fmt!(driver.file_name));
            let data = driver.pe32.try_content_as_slice()?;
            match core_load_image(false, DXE_CORE_HANDLE, driver.device_path, Some(data)) {
                Ok((image_handle, security_status)) => {
                    driver.image_handle = Some(image_handle);
                    driver.security_status = match security_status {
                        Ok(_) => efi::Status::SUCCESS,
                        Err(err) => err.into(),
                    };
                }
                Err(err) => log::error!("Failed to load: load_image returned {err:x?}"),
            }
        }

        if let Some(image_handle) = driver.image_handle {
            match driver.security_status {
                efi::Status::SUCCESS => {
                    dispatch_attempted = true;
                    // Note: ignore error result of core_start_image here - an image returning an error code is expected in some
                    // cases, and a debug output for that is already implemented in core_start_image.
                    let _status = core_start_image(image_handle);
                }
                efi::Status::SECURITY_VIOLATION => {
                    log::info!(
                        "Deferring driver: {:?} due to security status: {:x?}",
                        guid_fmt!(driver.file_name),
                        efi::Status::SECURITY_VIOLATION
                    );
                    DISPATCHER_CONTEXT.lock().pending_drivers.push(driver);
                }
                unexpected_status => {
                    log::info!(
                        "Dropping driver: {:?} due to security status: {:x?}",
                        guid_fmt!(driver.file_name),
                        unexpected_status
                    );
                }
            }
        }
    }

    {
        let mut dispatcher = DISPATCHER_CONTEXT.lock();
        let fv_image_candidates: Vec<_> = dispatcher.pending_firmware_volume_images.drain(..).collect();

        for mut candidate in fv_image_candidates {
            let depex_satisfied = match candidate.depex {
                Some(ref mut depex) => depex.eval(&PROTOCOL_DB.registered_protocols()),
                None => true,
            };

            if depex_satisfied && candidate.evaluate_auth().is_ok() {
                for section in candidate.fv_sections {
                    let fv_data = Box::from(section.try_content_as_slice()?);
                    dispatcher.fv_section_data.push(fv_data);
                    let data_ptr =
                        dispatcher.fv_section_data.last().expect("freshly pushed fv section data must be valid");

                    let volume_address: u64 = data_ptr.as_ptr() as u64;
                    // Safety: FV section data is stored in the dispatcher and is valid until end of UEFI (nothing drops it).
                    let res = unsafe { core_install_firmware_volume(volume_address, Some(candidate.parent_fv_handle)) };

                    if res.is_ok() {
                        dispatch_attempted = true;
                    } else {
                        log::warn!(
                            "couldn't install firmware volume image {:?}: {:?}",
                            guid_fmt!(candidate.file_name),
                            res
                        );
                    }
                }
            } else {
                dispatcher.pending_firmware_volume_images.push(candidate)
            }
        }
    }

    Ok(dispatch_attempted)
}

fn add_fv_handles(new_handles: Vec<efi::Handle>) -> Result<(), EfiError> {
    let mut dispatcher = DISPATCHER_CONTEXT.lock();
    for handle in new_handles {
        if dispatcher.processed_fvs.insert(handle) {
            //process freshly discovered FV
            let fvb_ptr = match PROTOCOL_DB.get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID) {
                Err(_) => {
                    panic!(
                        "get_interface_for_handle failed to return an interface on a handle where it should have existed"
                    )
                }
                Ok(protocol) => protocol as *mut firmware_volume_block::Protocol,
            };

            let fvb = unsafe {
                fvb_ptr.as_ref().expect("get_interface_for_handle returned NULL ptr for FirmwareVolumeBlock")
            };

            let mut fv_address: u64 = 0;
            let status = (fvb.get_physical_address)(fvb_ptr, core::ptr::addr_of_mut!(fv_address));
            if status.is_error() {
                log::error!("Failed to get physical address for fvb handle {handle:#x?}. Error: {status:#x?}");
                continue;
            }

            // Some FVB implementations return a zero physical address - assume that is invalid.
            if fv_address == 0 {
                log::error!("Physical address for fvb handle {handle:#x?} is zero - skipping.");
                continue;
            }

            let fv_device_path =
                PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::device_path::PROTOCOL_GUID);
            let fv_device_path =
                fv_device_path.unwrap_or(core::ptr::null_mut()) as *mut efi::protocols::device_path::Protocol;

            // Safety: this code assumes that the fv_address from FVB protocol yields a pointer to a real FV,
            // and that the memory backing the FVB is essentially permanent while the dispatcher is running (i.e.
            // that no one uninstalls the FVB protocol and frees the memory).
            let fv = match unsafe { VolumeRef::new_from_address(fv_address) } {
                Ok(fv) => fv,
                Err(err) => {
                    log::error!("Failed to instantiate memory mapped FV for fvb handle {handle:#x?}. Error: {err:#x?}");
                    continue;
                }
            };

            for file in fv.files() {
                let file = file?;
                if file.file_type_raw() == ffs::file::raw::r#type::DRIVER {
                    let file = file.clone();
                    let file_name = file.name();
                    let sections = file.sections_with_extractor(&dispatcher.section_extractor)?;

                    let depex = sections
                        .iter()
                        .find_map(|x| match x.section_type() {
                            Some(ffs::section::Type::DxeDepex) => Some(x.try_content_as_slice()),
                            _ => None,
                        })
                        .transpose()?
                        .map(Depex::from);

                    if let Some(pe32_section) =
                        sections.into_iter().find(|x| x.section_type() == Some(ffs::section::Type::Pe32))
                    {
                        // In this case, this is sizeof(guid) + sizeof(protocol) = 20, so it should always fit an u8
                        const FILENAME_NODE_SIZE: usize = core::mem::size_of::<efi::protocols::device_path::Protocol>()
                            + core::mem::size_of::<r_efi::efi::Guid>();
                        // In this case, this is sizeof(protocol) = 4, so it should always fit an u8
                        const END_NODE_SIZE: usize = core::mem::size_of::<efi::protocols::device_path::Protocol>();

                        let filename_node = efi::protocols::device_path::Protocol {
                            r#type: r_efi::protocols::device_path::TYPE_MEDIA,
                            sub_type: r_efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE,
                            length: [FILENAME_NODE_SIZE as u8, 0x00],
                        };
                        let filename_end_node = efi::protocols::device_path::Protocol {
                            r#type: r_efi::protocols::device_path::TYPE_END,
                            sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                            length: [END_NODE_SIZE as u8, 0x00],
                        };

                        let mut filename_nodes_buf = Vec::<u8>::with_capacity(FILENAME_NODE_SIZE + END_NODE_SIZE); // 20 bytes (filename_node + GUID) + 4 bytes (end node)
                        filename_nodes_buf.extend_from_slice(unsafe {
                            core::slice::from_raw_parts(
                                &filename_node as *const _ as *const u8,
                                core::mem::size_of::<efi::protocols::device_path::Protocol>(),
                            )
                        });
                        // Copy the GUID into the buffer
                        filename_nodes_buf.extend_from_slice(file_name.as_bytes());

                        // Copy filename_end_node into the buffer
                        filename_nodes_buf.extend_from_slice(unsafe {
                            core::slice::from_raw_parts(
                                &filename_end_node as *const _ as *const u8,
                                core::mem::size_of::<efi::protocols::device_path::Protocol>(),
                            )
                        });

                        let boxed_device_path = filename_nodes_buf.into_boxed_slice();
                        let filename_device_path =
                            boxed_device_path.as_ptr() as *const efi::protocols::device_path::Protocol;

                        let full_path_bytes = concat_device_path_to_boxed_slice(fv_device_path, filename_device_path);
                        let full_device_path_for_file = full_path_bytes
                            .map(|full_path| Box::into_raw(full_path) as *mut efi::protocols::device_path::Protocol)
                            .unwrap_or(fv_device_path);

                        dispatcher.pending_drivers.push(PendingDriver {
                            file_name,
                            firmware_volume_handle: handle,
                            pe32: pe32_section,
                            device_path: full_device_path_for_file,
                            depex,
                            image_handle: None,
                            security_status: efi::Status::NOT_READY,
                        });
                    } else {
                        log::warn!("driver {:?} does not contain a PE32 section.", guid_fmt!(file_name));
                    }
                }
                if file.file_type_raw() == ffs::file::raw::r#type::FIRMWARE_VOLUME_IMAGE {
                    let file = file.clone();
                    let file_name = file.name();

                    let sections = file.sections_with_extractor(&dispatcher.section_extractor)?;

                    let depex = sections
                        .iter()
                        .find_map(|x| match x.section_type() {
                            Some(ffs::section::Type::DxeDepex) => Some(x.try_content_as_slice()),
                            _ => None,
                        })
                        .transpose()?
                        .map(Depex::from);

                    let fv_sections = sections
                        .into_iter()
                        .filter(|s| s.section_type() == Some(ffs::section::Type::FirmwareVolumeImage))
                        .collect::<Vec<_>>();

                    if !fv_sections.is_empty() {
                        dispatcher.pending_firmware_volume_images.push(PendingFirmwareVolumeImage {
                            parent_fv_handle: handle,
                            file_name,
                            depex,
                            fv_sections,
                        });
                    } else {
                        log::warn!(
                            "firmware volume image {:?} does not contain a firmware volume image section.",
                            guid_fmt!(file_name)
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn core_schedule(handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
    let mut dispatcher = DISPATCHER_CONTEXT.lock();
    for driver in dispatcher.pending_drivers.iter_mut() {
        if driver.firmware_volume_handle == handle
            && OrdGuid(driver.file_name) == OrdGuid(*file)
            && let Some(depex) = &mut driver.depex
            && depex.is_sor()
        {
            depex.schedule();
            return Ok(());
        }
    }
    Err(EfiError::NotFound)
}

pub fn core_trust(handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
    let mut dispatcher = DISPATCHER_CONTEXT.lock();
    for driver in dispatcher.pending_drivers.iter_mut() {
        if driver.firmware_volume_handle == handle && OrdGuid(driver.file_name) == OrdGuid(*file) {
            driver.security_status = efi::Status::SUCCESS;
            return Ok(());
        }
    }
    Err(EfiError::NotFound)
}

pub fn core_dispatcher() -> Result<(), EfiError> {
    if DISPATCHER_CONTEXT.lock().executing {
        return Err(EfiError::AlreadyStarted);
    }

    perf_function_begin(function!(), &CALLER_ID, create_performance_measurement);

    let mut something_dispatched = false;
    while dispatch()? {
        something_dispatched = true;
    }

    perf_function_end(function!(), &CALLER_ID, create_performance_measurement);

    if something_dispatched { Ok(()) } else { Err(EfiError::NotFound) }
}

pub fn init_dispatcher() {
    //set up call back for FV protocol installation.
    let event = EVENT_DB
        .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(core_fw_vol_event_protocol_notify), None, None)
        .expect("Failed to create fv protocol installation callback.");

    PROTOCOL_DB
        .register_protocol_notify(firmware_volume_block::PROTOCOL_GUID, event)
        .expect("Failed to register protocol notify on fv protocol.");
}

pub fn register_section_extractor(extractor: Service<dyn SectionExtractor>) {
    DISPATCHER_CONTEXT.lock().section_extractor.set_extractor(extractor);
}

pub fn display_discovered_not_dispatched() {
    for driver in &DISPATCHER_CONTEXT.lock().pending_drivers {
        log::warn!("Driver {:?} found but not dispatched.", guid_fmt!(driver.file_name));
    }
}

/// Reset the dispatcher context to a clean default state for testing.
/// Note: This function should only be called from tests to ensure clean state between test runs.
#[cfg(test)]
pub(crate) fn reset_dispatcher_context_for_tests() {
    *DISPATCHER_CONTEXT.lock() = DispatcherContext::new();
}

extern "efiapi" fn core_fw_vol_event_protocol_notify(_event: efi::Event, _context: *mut c_void) {
    //Note: runs at TPL_CALLBACK
    match PROTOCOL_DB.locate_handles(Some(firmware_volume_block::PROTOCOL_GUID)) {
        Ok(fv_handles) => add_fv_handles(fv_handles).expect("Error adding FV handles"),
        Err(_) => panic!("could not locate handles in protocol call back"),
    };
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::sync::atomic::AtomicBool;
    use std::{fs::File, io::Read, vec};

    use log::{Level, LevelFilter, Metadata, Record};
    use patina_internal_device_path::DevicePathWalker;
    use uuid::uuid;

    use super::*;
    use crate::{test_collateral, test_support};

    // Simple logger for log crate to dump stuff in tests
    struct SimpleLogger;
    impl log::Log for SimpleLogger {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= Level::Info
        }

        fn log(&self, record: &Record) {
            if self.enabled(record.metadata()) {
                println!("{}", record.args());
            }
        }

        fn flush(&self) {}
    }
    static LOGGER: SimpleLogger = SimpleLogger;

    fn set_logger() {
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info));
    }

    // Monkey patch value for get_physical_address3
    static mut GET_PHYSICAL_ADDRESS3_VALUE: u64 = 0;

    // Locks and resets the dispatcher context before running the provided closure.
    fn with_locked_state<F>(f: F)
    where
        F: Fn() + std::panic::RefUnwindSafe,
    {
        test_support::with_global_lock(|| {
            unsafe { test_support::init_test_protocol_db() };
            test_support::reset_dispatcher_context();
            f();
        })
        .unwrap();
    }

    // Monkey patch for get_physical_address that always returns NOT_FOUND.
    extern "efiapi" fn get_physical_address1(
        _: *mut crate::dispatcher::firmware_volume_block::Protocol,
        _: *mut u64,
    ) -> efi::Status {
        efi::Status::NOT_FOUND
    }

    // Monkey patch for get_physical_address that always returns 0.
    extern "efiapi" fn get_physical_address2(
        _: *mut crate::dispatcher::firmware_volume_block::Protocol,
        addr: *mut u64,
    ) -> efi::Status {
        unsafe { addr.write(0) };
        efi::Status::SUCCESS
    }

    // Monkey patch for get_physical_address that returns a physical address as determined by `GET_PHYSICAL_ADDRESS3_VALUE`
    extern "efiapi" fn get_physical_address3(
        _: *mut crate::dispatcher::firmware_volume_block::Protocol,
        addr: *mut u64,
    ) -> efi::Status {
        unsafe { addr.write(GET_PHYSICAL_ADDRESS3_VALUE) };
        efi::Status::SUCCESS
    }

    #[test]
    fn test_guid_ordering() {
        let g1 = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
        let g2 = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);
        let g3 = efi::Guid::from_fields(0, 0, 0, 0, 1, &[0, 0, 0, 0, 0, 0]);
        let g4 = efi::Guid::from_fields(0, 0, 0, 1, 0, &[0, 0, 0, 0, 0, 0]);
        let g5 = efi::Guid::from_fields(0, 0, 1, 0, 0, &[0, 0, 0, 0, 0, 0]);
        let g6 = efi::Guid::from_fields(0, 1, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);
        let g7 = efi::Guid::from_fields(1, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]);

        // Test Partial Ord
        assert!(
            OrdGuid(g7) > OrdGuid(g6)
                && OrdGuid(g6) > OrdGuid(g5)
                && OrdGuid(g5) > OrdGuid(g4)
                && OrdGuid(g4) > OrdGuid(g3)
                && OrdGuid(g3) > OrdGuid(g2)
                && OrdGuid(g2) > OrdGuid(g1)
        );
        assert!(OrdGuid(g7) >= OrdGuid(g7));
        assert!(OrdGuid(g7) <= OrdGuid(g7));
        assert!(OrdGuid(g7) != OrdGuid(g6));
        assert!(OrdGuid(g7) == OrdGuid(g7));
        assert_eq!(g1.partial_cmp(&g2), Some(Ordering::Less));
        assert_eq!(g2.partial_cmp(&g1), Some(Ordering::Greater));
        assert_eq!(g1.partial_cmp(&g1), Some(Ordering::Equal));

        // Test Ord
        assert_eq!(OrdGuid(g4).max(OrdGuid(g5)), OrdGuid(g5));
        assert_eq!(OrdGuid(g4).max(OrdGuid(g3)), OrdGuid(g4));
        assert_eq!(OrdGuid(g4).min(OrdGuid(g5)), OrdGuid(g4));
        assert_eq!(OrdGuid(g4).min(OrdGuid(g3)), OrdGuid(g3));
        assert_eq!(OrdGuid(g4).clamp(OrdGuid(g3), OrdGuid(g5)), OrdGuid(g4));
        assert_eq!(OrdGuid(g1).clamp(OrdGuid(g3), OrdGuid(g5)), OrdGuid(g3));
        assert_eq!(OrdGuid(g7).clamp(OrdGuid(g3), OrdGuid(g5)), OrdGuid(g5));
        assert_eq!(OrdGuid(g1).cmp(&OrdGuid(g2)), Ordering::Less);
        assert_eq!(OrdGuid(g2).cmp(&OrdGuid(g1)), Ordering::Greater);
        assert_eq!(OrdGuid(g1).cmp(&OrdGuid(g1)), Ordering::Equal);
    }

    #[test]
    fn test_init_dispatcher() {
        set_logger();
        with_locked_state(|| {
            init_dispatcher();
            register_section_extractor(Service::mock(Box::new(patina_ffs_extractors::BrotliSectionExtractor)));
        });
    }

    #[test]
    fn test_add_fv_handle_with_valid_fv() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            const DRIVERS_IN_DXEFV: usize = 130;
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), DRIVERS_IN_DXEFV);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_add_fv_handle_with_invalid_handle() {
        set_logger();
        with_locked_state(|| {
            let result = std::panic::catch_unwind(|| {
                add_fv_handles(vec![std::ptr::null_mut::<c_void>()]).expect("Failed to add FV handle");
            });
            assert!(result.is_err());
        })
    }

    #[test]
    fn test_add_fv_handle_with_failing_get_physical_address() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            // Monkey Patch get_physical_address to one that returns an error.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address1;

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), 0);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_add_fv_handle_with_get_physical_address_of_0() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            // Monkey Patch get_physical_address to set address to 0.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address2;

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), 0);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_add_fv_handle_with_wrong_address() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let fv_phys_addr = fv_raw.expose_provenance() as u64;
            let handle = unsafe { crate::fv::core_install_firmware_volume(fv_phys_addr, None).unwrap() };

            // Monkey Patch get_physical_address to set to a slightly invalid address.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address3;

            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = fv_phys_addr + 0x1000 };
            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = 0 };

            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), 0);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_add_fv_handle_with_child_fv() {
        set_logger();
        let mut file = File::open(test_collateral!("NESTEDFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };
            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // 1 child FV should be pending contained in NESTEDFV.Fv
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_firmware_volume_images.len(), 1);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_display_discovered_not_dispatched_does_not_fail() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            display_discovered_not_dispatched();
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_core_fw_col_event_protocol_notify() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let _ =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };
            core_fw_vol_event_protocol_notify(std::ptr::null_mut::<c_void>(), std::ptr::null_mut::<c_void>());

            const DRIVERS_IN_DXEFV: usize = 130;
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), DRIVERS_IN_DXEFV);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_dispatch_when_already_dispatching() {
        set_logger();
        with_locked_state(|| {
            DISPATCHER_CONTEXT.lock().executing = true;
            let result = core_dispatcher();
            assert_eq!(result, Err(EfiError::AlreadyStarted));
        })
    }

    #[test]
    fn test_dispatch_with_nothing_to_dispatch() {
        set_logger();
        with_locked_state(|| {
            let result = core_dispatcher();
            assert_eq!(result, Err(EfiError::NotFound));
        })
    }

    #[test]
    fn test_dispatch() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // Cannot actually dispatch
            let result = core_dispatcher();
            assert_eq!(result, Err(EfiError::NotFound));
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_core_schedule() {
        set_logger();
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");
        let fv = fv.into_boxed_slice();
        let fv_raw = Box::into_raw(fv);

        with_locked_state(|| {
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle =
                unsafe { crate::fv::core_install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // No SOR drivers to schedule in DXEFV, but we can test all the way to detecting that it does not have a SOR depex.
            let result = core_schedule(
                handle,
                &efi::Guid::from_bytes(uuid::Uuid::from_u128(0x1fa1f39e_feff_4aae_bd7b_38a070a3b609).as_bytes()),
            );
            assert_eq!(result, Err(EfiError::NotFound));
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_fv_authentication() {
        set_logger();

        let mut file = File::open(test_collateral!("NESTEDFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            static SECURITY_CALL_EXECUTED: AtomicBool = AtomicBool::new(false);
            extern "efiapi" fn mock_file_authentication_state(
                this: *mut patina::pi::protocols::security::Protocol,
                authentication_status: u32,
                file: *mut efi::protocols::device_path::Protocol,
            ) -> efi::Status {
                assert!(!this.is_null());
                assert_eq!(authentication_status, 0);

                unsafe {
                    let mut node_walker = DevicePathWalker::new(file);
                    //outer FV of NESTEDFV.Fv does not have an extended header so expect MMAP device path.
                    let fv_node = node_walker.next().unwrap();
                    assert_eq!(fv_node.header().r#type, efi::protocols::device_path::TYPE_HARDWARE);
                    assert_eq!(fv_node.header().sub_type, efi::protocols::device_path::Hardware::SUBTYPE_MMAP);

                    //Internal nested FV file name is 2DFBCBC7-14D6-4C70-A9C5-AD0AD03F4D75
                    let file_node = node_walker.next().unwrap();
                    assert_eq!(file_node.header().r#type, efi::protocols::device_path::TYPE_MEDIA);
                    assert_eq!(
                        file_node.header().sub_type,
                        efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE
                    );
                    assert_eq!(file_node.data(), uuid!("2DFBCBC7-14D6-4C70-A9C5-AD0AD03F4D75").to_bytes_le());

                    //device path end node
                    let end_node = node_walker.next().unwrap();
                    assert_eq!(end_node.header().r#type, efi::protocols::device_path::TYPE_END);
                    assert_eq!(end_node.header().sub_type, efi::protocols::device_path::End::SUBTYPE_ENTIRE);
                }

                SECURITY_CALL_EXECUTED.store(true, core::sync::atomic::Ordering::SeqCst);

                efi::Status::SUCCESS
            }

            let security_protocol =
                patina::pi::protocols::security::Protocol { file_authentication_state: mock_file_authentication_state };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    patina::pi::protocols::security::PROTOCOL_GUID,
                    &security_protocol as *const _ as *mut _,
                )
                .unwrap();
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe { crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap() };

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            core_dispatcher().unwrap();

            assert!(SECURITY_CALL_EXECUTED.load(core::sync::atomic::Ordering::SeqCst));
        })
    }
}
