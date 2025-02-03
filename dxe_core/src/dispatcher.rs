//! DXE Core Dispatcher
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use core::{cmp::Ordering, ffi::c_void};
use mu_pi::{
    fw_fs::{FfsFileRawType, FfsSectionType, FirmwareVolume, Section, SectionExtractor},
    protocols::firmware_volume_block,
};
use mu_rust_helpers::guid::guid_fmt;
use r_efi::efi;
use tpl_lock::TplMutex;
use uefi_depex::{AssociatedDependency, Depex, Opcode};
use uefi_device_path::concat_device_path_to_boxed_slice;

use crate::{
    events::EVENT_DB,
    fv::{core_install_firmware_volume, device_path_bytes_for_fv_file},
    image::{core_load_image, core_start_image},
    protocol_db::DXE_CORE_HANDLE,
    protocols::PROTOCOL_DB,
    tpl_lock,
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
    fn evaluate_auth(&self) -> Result<(), efi::Status> {
        let security_protocol = unsafe {
            match PROTOCOL_DB.locate_protocol(mu_pi::protocols::security::PROTOCOL_GUID) {
                Ok(protocol) => (protocol as *mut mu_pi::protocols::security::Protocol)
                    .as_ref()
                    .expect("Security Protocol should not be null"),
                //If security protocol is not located, then assume it has not yet been produced and implicitly trust the
                //Firmware Volume.
                Err(_) => return Ok(()),
            }
        };
        let file_path = device_path_bytes_for_fv_file(self.parent_fv_handle, self.file_name)?;

        //Important Note: the present section extraction implementation does not support section extraction-based
        //authentication status, so it is hard-coded to zero here. The primary security handlers for the main usage
        //scenarios (TPM measurement and UEFI Secure Boot) do not use it.
        let status = (security_protocol.file_authentication_state)(
            security_protocol as *const _ as *mut mu_pi::protocols::security::Protocol,
            0,
            file_path.as_ptr() as *const _ as *mut efi::protocols::device_path::Protocol,
        );
        if status != efi::Status::SUCCESS {
            return Err(status);
        }
        Ok(())
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
    pending_firmware_volume_images: Vec<PendingFirmwareVolumeImage>,
    loaded_firmware_volume_sections: Vec<Section>,
    associated_before: BTreeMap<OrdGuid, Vec<PendingDriver>>,
    associated_after: BTreeMap<OrdGuid, Vec<PendingDriver>>,
    processed_fvs: BTreeSet<efi::Handle>,
    section_extractor: Option<Box<dyn SectionExtractor>>,
}

impl DispatcherContext {
    const fn new() -> Self {
        Self {
            executing: false,
            arch_protocols_available: false,
            pending_drivers: Vec::new(),
            pending_firmware_volume_images: Vec::new(),
            loaded_firmware_volume_sections: Vec::new(),
            associated_before: BTreeMap::new(),
            associated_after: BTreeMap::new(),
            processed_fvs: BTreeSet::new(),
            section_extractor: None,
        }
    }
}

unsafe impl Send for DispatcherContext {}

static DISPATCHER_CONTEXT: TplMutex<DispatcherContext> =
    TplMutex::new(efi::TPL_NOTIFY, DispatcherContext::new(), "Dispatcher Context");

fn dispatch() -> Result<bool, efi::Status> {
    let scheduled: Vec<PendingDriver>;
    {
        let mut dispatcher = DISPATCHER_CONTEXT.lock();
        if !dispatcher.arch_protocols_available {
            dispatcher.arch_protocols_available = Depex::from(ALL_ARCH_DEPEX).eval(&PROTOCOL_DB.registered_protocols());
        }
        let driver_candidates: Vec<_> = dispatcher.pending_drivers.drain(..).collect();
        let mut scheduled_driver_candidates = Vec::new();
        for mut candidate in driver_candidates {
            log::info!("Evaluting depex for candidate: {:?}", guid_fmt!(candidate.file_name));
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
            match core_load_image(false, DXE_CORE_HANDLE, driver.device_path, Some(driver.pe32.section_data())) {
                Ok((image_handle, security_status)) => {
                    driver.image_handle = Some(image_handle);
                    driver.security_status = security_status;
                }
                Err(err) => log::error!("Failed to load: load_image returned {:x?}", err),
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
                    let volume_address: u64 = section.section_data().as_ptr() as u64;

                    if core_install_firmware_volume(volume_address, Some(candidate.parent_fv_handle)).is_ok() {
                        dispatch_attempted = true;
                        dispatcher.loaded_firmware_volume_sections.push(section);
                    } else {
                        log::warn!("couldn't install firmware volume image {:?}", guid_fmt!(candidate.file_name));
                    }
                }
            } else {
                dispatcher.pending_firmware_volume_images.push(candidate)
            }
        }
    }

    Ok(dispatch_attempted)
}

fn add_fv_handles(new_handles: Vec<efi::Handle>) -> Result<(), efi::Status> {
    let mut dispatcher = DISPATCHER_CONTEXT.lock();
    for handle in new_handles {
        if dispatcher.processed_fvs.insert(handle) {
            //process freshly discovered FV
            let fvb_ptr = match PROTOCOL_DB.get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID) {
                Err(_) => {
                    panic!("get_interface_for_handle failed to return an interface on a handle where it should have existed")
                }
                Ok(protocol) => protocol as *mut firmware_volume_block::Protocol,
            };

            let fvb = unsafe {
                fvb_ptr.as_ref().expect("get_interface_for_handle returned NULL ptr for FirmwareVolumeBlock")
            };

            let mut fv_address: u64 = 0;
            let status = (fvb.get_physical_address)(fvb_ptr, core::ptr::addr_of_mut!(fv_address));
            if status.is_error() {
                log::error!("Failed to get physical address for fvb handle {:#x?}. Error: {:#x?}", handle, status);
                continue;
            }

            // Some FVB implementations return a zero physical address - assume that is invalid.
            if fv_address == 0 {
                log::error!("Physical address for fvb handle {:#x?} is zero - skipping.", handle);
                continue;
            }

            let fv_device_path =
                PROTOCOL_DB.get_interface_for_handle(handle, efi::protocols::device_path::PROTOCOL_GUID);
            let fv_device_path =
                fv_device_path.unwrap_or(core::ptr::null_mut()) as *mut efi::protocols::device_path::Protocol;

            // Safety: this code assumes that the fv_address from FVB protocol yields a pointer to a real FV.
            let fv = match unsafe { FirmwareVolume::new_from_address(fv_address) } {
                Ok(fv) => fv,
                Err(err) => {
                    log::error!(
                        "Failed to instantiate memory mapped FV for fvb handle {:#x?}. Error: {:#x?}",
                        handle,
                        err
                    );
                    continue;
                }
            };

            for file in fv.file_iter() {
                let file = file?;
                if file.file_type_raw() == FfsFileRawType::DRIVER {
                    let file = file.clone();
                    let file_name = file.name();
                    let sections = {
                        if let Some(extractor) = &dispatcher.section_extractor {
                            file.section_iter_with_extractor(extractor.as_ref())
                                .collect::<Result<Vec<_>, efi::Status>>()?
                        } else {
                            file.section_iter().collect::<Result<Vec<_>, efi::Status>>()?
                        }
                    };

                    let depex = sections
                        .iter()
                        .find_map(|x| {
                            if x.section_type() == Some(FfsSectionType::DxeDepex) {
                                let data = x.section_data().to_vec();
                                Some(data)
                            } else {
                                None
                            }
                        })
                        .map(Depex::from);

                    if let Some(pe32_section) =
                        sections.into_iter().find(|x| x.section_type() == Some(FfsSectionType::Pe32))
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
                        log::warn!(
                            "driver {:?} does not contain a PE32 section.",
                            uuid::Uuid::from_bytes(*file_name.as_bytes())
                        );
                    }
                }
                if file.file_type_raw() == FfsFileRawType::FIRMWARE_VOLUME_IMAGE {
                    let file = file.clone();
                    let file_name = file.name();

                    let sections = {
                        if let Some(extractor) = &dispatcher.section_extractor {
                            file.section_iter_with_extractor(extractor.as_ref())
                                .collect::<Result<Vec<_>, efi::Status>>()?
                        } else {
                            file.section_iter().collect::<Result<Vec<_>, efi::Status>>()?
                        }
                    };

                    let depex = sections
                        .iter()
                        .find_map(|x| {
                            if x.section_type() == Some(FfsSectionType::DxeDepex) {
                                let data = x.section_data().to_vec();
                                Some(data)
                            } else {
                                None
                            }
                        })
                        .map(Depex::from);

                    let fv_sections = sections
                        .into_iter()
                        .filter(|s| s.section_type() == Some(FfsSectionType::FirmwareVolumeImage))
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
                            uuid::Uuid::from_bytes(*file_name.as_bytes())
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn core_schedule(handle: efi::Handle, file: &efi::Guid) -> Result<(), efi::Status> {
    let mut dispatcher = DISPATCHER_CONTEXT.lock();
    for driver in dispatcher.pending_drivers.iter_mut() {
        if driver.firmware_volume_handle == handle && OrdGuid(driver.file_name) == OrdGuid(*file) {
            if let Some(depex) = &mut driver.depex {
                if depex.is_sor() {
                    depex.schedule();
                    return Ok(());
                }
            }
        }
    }
    Err(efi::Status::NOT_FOUND)
}

pub fn core_trust(handle: efi::Handle, file: &efi::Guid) -> Result<(), efi::Status> {
    let mut dispatcher = DISPATCHER_CONTEXT.lock();
    for driver in dispatcher.pending_drivers.iter_mut() {
        if driver.firmware_volume_handle == handle && OrdGuid(driver.file_name) == OrdGuid(*file) {
            driver.security_status = efi::Status::SUCCESS;
            return Ok(());
        }
    }
    Err(efi::Status::NOT_FOUND)
}

pub fn core_dispatcher() -> Result<(), efi::Status> {
    if DISPATCHER_CONTEXT.lock().executing {
        return Err(efi::Status::ALREADY_STARTED);
    }
    let mut something_dispatched = false;
    while dispatch()? {
        something_dispatched = true;
    }
    if something_dispatched {
        Ok(())
    } else {
        Err(efi::Status::NOT_FOUND)
    }
}

pub fn init_dispatcher(extractor: Box<dyn SectionExtractor>) {
    //set up call back for FV protocol installation.
    let event = EVENT_DB
        .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(core_fw_vol_event_protocol_notify), None, None)
        .expect("Failed to create fv protocol installation callback.");

    PROTOCOL_DB
        .register_protocol_notify(firmware_volume_block::PROTOCOL_GUID, event)
        .expect("Failed to register protocol notify on fv protocol.");

    DISPATCHER_CONTEXT.lock().section_extractor = Some(extractor);
}

pub fn display_discovered_not_dispatched() {
    for driver in &DISPATCHER_CONTEXT.lock().pending_drivers {
        log::warn!("Driver {:?} found but not dispatched.", guid_fmt!(driver.file_name));
    }
}

extern "efiapi" fn core_fw_vol_event_protocol_notify(_event: efi::Event, _context: *mut c_void) {
    //Note: runs at TPL_CALLBACK
    match PROTOCOL_DB.locate_handles(Some(firmware_volume_block::PROTOCOL_GUID)) {
        Ok(fv_handles) => add_fv_handles(fv_handles).expect("Error adding FV handles"),
        Err(_) => panic!("could not locate handles in protocol call back"),
    };
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicBool;
    use std::{fs::File, io::Read, vec};

    use uefi_device_path::DevicePathWalker;
    use uuid::uuid;

    use super::*;
    use crate::test_collateral;

    // Monkey patch value for get_physical_address3
    static mut GET_PHYSICAL_ADDRESS3_VALUE: u64 = 0;

    // Locks and resets the dispatcher context before running the provided closure.
    fn with_locked_state<F>(f: F)
    where
        F: Fn() + std::panic::RefUnwindSafe,
    {
        crate::test_support::with_global_lock(|| {
            unsafe { crate::test_support::init_test_protocol_db() };
            *DISPATCHER_CONTEXT.lock() = DispatcherContext::new();
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
        with_locked_state(|| {
            init_dispatcher(Box::new(section_extractor::BrotliSectionExtractor));
        });
    }

    #[test]
    fn test_add_fv_handle_with_valid_fv() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            const DRIVERS_IN_DXEFV: usize = 130;
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), DRIVERS_IN_DXEFV);
        })
    }

    #[test]
    fn test_add_fv_handle_with_invalid_handle() {
        with_locked_state(|| {
            let result = std::panic::catch_unwind(|| {
                add_fv_handles(vec![std::ptr::null_mut::<c_void>()]).expect("Failed to add FV handle");
            });
            assert!(result.is_err());
        })
    }

    #[test]
    fn test_add_fv_handle_with_failing_get_physical_address() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            // Monkey Patch get_physical_address to one that returns an error.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address1;

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), 0);
        })
    }

    #[test]
    fn test_add_fv_handle_with_get_physical_address_of_0() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            // Monkey Patch get_physical_address to set address to 0.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address2;

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), 0);
        })
    }

    #[test]
    fn test_add_fv_handle_with_wrong_address() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            // Monkey Patch get_physical_address to set to a slightly invalid address.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address3;

            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = (fv.as_ptr() as u64) + 0x1000 };
            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = 0 };

            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), 0);
        })
    }

    #[test]
    fn test_add_fv_handle_with_child_fv() {
        let mut file = File::open(test_collateral!("NESTEDFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();
            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // 1 child FV should be pending contained in NESTEDFV.Fv
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_firmware_volume_images.len(), 1);
        })
    }

    #[test]
    fn test_display_discovered_not_dispatched_does_not_fail() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            display_discovered_not_dispatched();
        })
    }

    #[test]
    fn test_core_fw_col_event_protocol_notify() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let _ = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();
            core_fw_vol_event_protocol_notify(std::ptr::null_mut::<c_void>(), std::ptr::null_mut::<c_void>());

            const DRIVERS_IN_DXEFV: usize = 130;
            assert_eq!(DISPATCHER_CONTEXT.lock().pending_drivers.len(), DRIVERS_IN_DXEFV);
        })
    }

    #[test]
    fn test_dispatch_when_already_dispatching() {
        with_locked_state(|| {
            DISPATCHER_CONTEXT.lock().executing = true;
            let result = core_dispatcher();
            assert_eq!(result, Err(efi::Status::ALREADY_STARTED));
        })
    }

    #[test]
    fn test_dispatch_with_nothing_to_dispatch() {
        with_locked_state(|| {
            let result = core_dispatcher();
            assert_eq!(result, Err(efi::Status::NOT_FOUND));
        })
    }

    #[test]
    fn test_dispatch() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // Cannot actually dispatch
            let result = core_dispatcher();
            assert_eq!(result, Err(efi::Status::NOT_FOUND));
        })
    }

    #[test]
    fn test_core_schedule() {
        let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // No SOR drivers to schedule in DXEFV, but we can test all the way to detecting that it does not have a SOR depex.
            let result = core_schedule(
                handle,
                &efi::Guid::from_bytes(uuid::Uuid::from_u128(0x1fa1f39e_feff_4aae_bd7b_38a070a3b609).as_bytes()),
            );
            assert_eq!(result, Err(efi::Status::NOT_FOUND));
        })
    }

    #[test]
    fn test_fv_authentication() {
        let mut file = File::open(test_collateral!("NESTEDFV.Fv")).unwrap();
        let mut fv: Vec<u8> = Vec::new();
        file.read_to_end(&mut fv).expect("failed to read test file");

        with_locked_state(|| {
            static SECURITY_CALL_EXECUTED: AtomicBool = AtomicBool::new(false);
            extern "efiapi" fn mock_file_authentication_state(
                this: *mut mu_pi::protocols::security::Protocol,
                authentication_status: u32,
                file: *mut efi::protocols::device_path::Protocol,
            ) -> efi::Status {
                assert!(!this.is_null());
                assert_eq!(authentication_status, 0);

                unsafe {
                    let mut node_walker = DevicePathWalker::new(file);
                    //outer FV of NESTEDFV.Fv does not have an extended header so expect MMAP device path.
                    let fv_node = node_walker.next().unwrap();
                    assert_eq!(fv_node.header.r#type, efi::protocols::device_path::TYPE_HARDWARE);
                    assert_eq!(fv_node.header.sub_type, efi::protocols::device_path::Hardware::SUBTYPE_MMAP);

                    //Internal nested FV file name is 2DFBCBC7-14D6-4C70-A9C5-AD0AD03F4D75
                    let file_node = node_walker.next().unwrap();
                    assert_eq!(file_node.header.r#type, efi::protocols::device_path::TYPE_MEDIA);
                    assert_eq!(
                        file_node.header.sub_type,
                        efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE
                    );
                    assert_eq!(file_node.data, uuid!("2DFBCBC7-14D6-4C70-A9C5-AD0AD03F4D75").to_bytes_le());

                    //device path end node
                    let end_node = node_walker.next().unwrap();
                    assert_eq!(end_node.header.r#type, efi::protocols::device_path::TYPE_END);
                    assert_eq!(end_node.header.sub_type, efi::protocols::device_path::End::SUBTYPE_ENTIRE);
                }

                SECURITY_CALL_EXECUTED.store(true, core::sync::atomic::Ordering::SeqCst);

                efi::Status::SUCCESS
            }

            let security_protocol =
                mu_pi::protocols::security::Protocol { file_authentication_state: mock_file_authentication_state };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    mu_pi::protocols::security::PROTOCOL_GUID,
                    &security_protocol as *const _ as *mut _,
                )
                .unwrap();

            let handle = crate::fv::core_install_firmware_volume(fv.as_ptr() as u64, None).unwrap();

            add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            core_dispatcher().unwrap();

            assert!(SECURITY_CALL_EXECUTED.load(core::sync::atomic::Ordering::SeqCst));
        })
    }
}
