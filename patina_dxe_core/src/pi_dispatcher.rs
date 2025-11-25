//! DXE Core subsystem for the PI Dispatcher.
//!
//! This subsystem is responsible for managing the lifecycle of PI spec compliant drivers and the firmware volumes that
//! contain them.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
mod fv;
mod section_decompress;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use core::{cmp::Ordering, ffi::c_void};
use mu_rust_helpers::{function, guid::guid_fmt};
use patina::{
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

use fv::device_path_bytes_for_fv_file;
use section_decompress::CoreExtractor;

use crate::{
    PlatformInfo,
    events::EVENT_DB,
    image::{core_load_image, core_start_image},
    protocol_db::DXE_CORE_HANDLE,
    protocols::PROTOCOL_DB,
    tpl_mutex::TplMutex,
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

/// The internal state of the PI Dispatcher.
pub(crate) struct PiDispatcher<P: PlatformInfo> {
    /// State for the dispatcher itself.
    dispatcher_context: TplMutex<DispatcherContext>,
    /// State tracking firmware volumes installed by the Patina DXE Core.
    fv_data: TplMutex<fv::FvProtocolData<P>>,
    /// Section extractor used when working with firmware volumes.
    section_extractor: CoreExtractor<P::Extractor>,
}

impl<P: PlatformInfo> PiDispatcher<P> {
    /// Creates a new `PiDispatcher` instance.
    pub const fn new(section_extractor: P::Extractor) -> Self {
        Self {
            dispatcher_context: DispatcherContext::new_locked(),
            fv_data: fv::FvProtocolData::new_locked(),
            section_extractor: CoreExtractor::new(section_extractor),
        }
    }

    /// Displays drivers that were discovered but not dispatched.
    pub fn display_discovered_not_dispatched(&self) {
        for driver in &self.dispatcher_context.lock().pending_drivers {
            log::warn!("Driver {:?} found but not dispatched.", guid_fmt!(driver.file_name));
        }
    }

    /// Initializes the dispatcher by registering for FV protocol installation events.
    pub fn init(&self) {
        //set up call back for FV protocol installation.
        let event = EVENT_DB
            .create_event(
                efi::EVT_NOTIFY_SIGNAL,
                efi::TPL_CALLBACK,
                Some(Self::fw_vol_event_protocol_notify_efiapi),
                None,
                None,
            )
            .expect("Failed to create fv protocol installation callback.");

        PROTOCOL_DB
            .register_protocol_notify(firmware_volume_block::PROTOCOL_GUID, event)
            .expect("Failed to register protocol notify on fv protocol.");
    }

    /// Installs any firmware volumes from FV HOBs in the hob list
    #[inline(always)]
    #[coverage(off)]
    pub fn install_firmware_volumes_from_hoblist(
        &self,
        hob_list: &patina::pi::hob::HobList,
    ) -> Result<(), efi::Status> {
        self.fv_data.lock().install_firmware_volumes_from_hoblist(hob_list)
    }

    /// Performs a single dispatch iteration.
    pub fn dispatch(&self) -> Result<bool, EfiError> {
        if self.dispatcher_context.lock().executing {
            return Err(EfiError::AlreadyStarted);
        }

        let scheduled: Vec<PendingDriver>;
        {
            let mut dispatcher = self.dispatcher_context.lock();
            if !dispatcher.arch_protocols_available {
                dispatcher.arch_protocols_available =
                    Depex::from(ALL_ARCH_DEPEX).eval(&PROTOCOL_DB.registered_protocols());
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
                        self.dispatcher_context.lock().pending_drivers.push(driver);
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
            let mut dispatcher = self.dispatcher_context.lock();
            let fv_image_candidates: Vec<_> = dispatcher.pending_firmware_volume_images.drain(..).collect();

            for mut candidate in fv_image_candidates {
                let depex_satisfied = match candidate.depex {
                    Some(ref mut depex) => depex.eval(&PROTOCOL_DB.registered_protocols()),
                    None => true,
                };

                if depex_satisfied && candidate.evaluate_auth().is_ok() {
                    for section in candidate.fv_sections {
                        let fv_data: Box<[u8]> = Box::from(section.try_content_as_slice()?);

                        // Check if this FV is already installed (using the FV name GUID)
                        let fv_name_guid = {
                            // Safety: fv_data is a valid FV section allocated above
                            let volume = match unsafe { VolumeRef::new_from_address(fv_data.as_ptr() as u64) } {
                                Ok(vol) => vol,
                                Err(e) => {
                                    log::warn!(
                                        "Failed to parse FV from file {:?}: {:?}",
                                        guid_fmt!(candidate.file_name),
                                        e
                                    );
                                    continue;
                                }
                            };
                            volume.fv_name()
                        };

                        if let Some(fv_name_guid) = fv_name_guid
                            && self.is_fv_already_installed(fv_name_guid)
                        {
                            log::debug!(
                                "Skipping FV file {:?} - FV with name GUID {:?} is already installed",
                                guid_fmt!(candidate.file_name),
                                guid_fmt!(fv_name_guid)
                            );
                            continue;
                        }

                        dispatcher.fv_section_data.push(fv_data);
                        let data_ptr =
                            dispatcher.fv_section_data.last().expect("freshly pushed fv section data must be valid");

                        let volume_address: u64 = data_ptr.as_ptr() as u64;
                        // Safety: FV section data is stored in the dispatcher and is valid until end of UEFI (nothing drops it).
                        let res = unsafe {
                            self.fv_data
                                .lock()
                                .install_firmware_volume(volume_address, Some(candidate.parent_fv_handle))
                        };

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

    /// Performs a full dispatch until no more drivers can be dispatched.
    pub fn dispatcher(&self) -> Result<(), EfiError> {
        if self.dispatcher_context.lock().executing {
            return Err(EfiError::AlreadyStarted);
        }

        perf_function_begin(function!(), &CALLER_ID, create_performance_measurement);

        let mut something_dispatched = false;
        while self.dispatch()? {
            something_dispatched = true;
        }

        perf_function_end(function!(), &CALLER_ID, create_performance_measurement);

        if something_dispatched { Ok(()) } else { Err(EfiError::NotFound) }
    }

    /// Checks if a firmware volume with the given name GUID is already installed.
    ///
    /// Searches all installed firmware volume block (FVB) protocol handles to determine if
    /// any have a matching FV name GUID.
    ///
    /// # Arguments
    /// * `fv_name_guid` - The firmware volume name GUID to check for
    ///
    /// # Returns
    /// `true` if a firmware volume with the given name GUID is already installed
    /// `false` otherwise
    fn is_fv_already_installed(&self, fv_name_guid: efi::Guid) -> bool {
        // Get all handles with a FVB protocol
        let fvb_handles = match PROTOCOL_DB.locate_handles(Some(firmware_volume_block::PROTOCOL_GUID)) {
            Ok(handles) => handles,
            Err(_) => return false,
        };

        // Check each FVB handle to see if it has the same FV name GUID
        for handle in fvb_handles {
            let Ok(ptr) = PROTOCOL_DB.get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID) else {
                continue;
            };
            let fvb_ptr = ptr as *mut firmware_volume_block::Protocol;

            // Safety: fvb_ptr is obtained from a valid handle that has a FVB protocol instance
            // and the as_ref() call checks for null
            let Some(fvb) = (unsafe { fvb_ptr.as_ref() }) else {
                continue;
            };

            let mut fv_address: u64 = 0;
            let status = (fvb.get_physical_address)(fvb_ptr, core::ptr::addr_of_mut!(fv_address));
            if status.is_error() || fv_address == 0 {
                continue;
            }

            // Safety: fv_address is checked for being non-zero above
            let Ok(volume) = (unsafe { VolumeRef::new_from_address(fv_address) }) else {
                continue;
            };

            if let Some(name) = volume.fv_name()
                && name == fv_name_guid
            {
                return true;
            }
        }

        false
    }

    /// Schedules a driver for execution.
    #[inline(always)]
    #[coverage(off)]
    pub fn schedule(&self, handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
        self.dispatcher_context.lock().schedule(handle, file)
    }

    /// Marks a driver as trusted for execution.
    #[inline(always)]
    #[coverage(off)]
    pub fn trust(&self, handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
        self.dispatcher_context.lock().trust(handle, file)
    }

    #[inline(always)]
    #[coverage(off)]
    fn add_fv_handles(&self, new_handles: Vec<efi::Handle>) -> Result<(), EfiError> {
        self.dispatcher_context.lock().add_fv_handles(new_handles, &self.section_extractor)
    }

    #[inline(always)]
    #[coverage(off)]
    /// Caller must ensure that the base address is a valid firmware volume.
    pub unsafe fn install_firmware_volume(
        &self,
        base_address: u64,
        parent_handle: Option<efi::Handle>,
    ) -> Result<efi::Handle, EfiError> {
        // SAFETY: Caller must uphold the safety contract of this function.
        unsafe { self.fv_data.lock().install_firmware_volume(base_address, parent_handle) }
    }

    /// EFIAPI event callback to add the FV handles when FVB protocol is installed.
    extern "efiapi" fn fw_vol_event_protocol_notify_efiapi(_event: efi::Event, _context: *mut c_void) {
        let pd = &crate::Core::<P>::instance().pi_dispatcher;
        //Note: runs at TPL_CALLBACK
        match PROTOCOL_DB.locate_handles(Some(firmware_volume_block::PROTOCOL_GUID)) {
            Ok(fv_handles) => pd.add_fv_handles(fv_handles).expect("Error adding FV handles"),
            Err(_) => panic!("could not locate handles in protocol call back"),
        };
    }
}

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
        }
    }

    const fn new_locked() -> TplMutex<Self> {
        TplMutex::new(efi::TPL_NOTIFY, Self::new(), "Dispatcher Context")
    }

    fn add_fv_handles(
        &mut self,
        new_handles: Vec<efi::Handle>,
        extractor: &impl SectionExtractor,
    ) -> Result<(), EfiError> {
        for handle in new_handles {
            if self.processed_fvs.insert(handle) {
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
                        log::error!(
                            "Failed to instantiate memory mapped FV for fvb handle {handle:#x?}. Error: {err:#x?}"
                        );
                        continue;
                    }
                };

                for file in fv.files() {
                    let file = file?;
                    if file.file_type_raw() == ffs::file::raw::r#type::DRIVER {
                        let file = file.clone();
                        let file_name = file.name();
                        let sections = file.sections_with_extractor(extractor)?;

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
                            const FILENAME_NODE_SIZE: usize =
                                core::mem::size_of::<efi::protocols::device_path::Protocol>()
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

                            let full_path_bytes =
                                concat_device_path_to_boxed_slice(fv_device_path, filename_device_path);
                            let full_device_path_for_file = full_path_bytes
                                .map(|full_path| Box::into_raw(full_path) as *mut efi::protocols::device_path::Protocol)
                                .unwrap_or(fv_device_path);

                            self.pending_drivers.push(PendingDriver {
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

                        let sections = file.sections_with_extractor(extractor)?;

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
                            self.pending_firmware_volume_images.push(PendingFirmwareVolumeImage {
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

    fn schedule(&mut self, handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
        for driver in self.pending_drivers.iter_mut() {
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

    fn trust(&mut self, handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
        for driver in self.pending_drivers.iter_mut() {
            if driver.firmware_volume_handle == handle && OrdGuid(driver.file_name) == OrdGuid(*file) {
                driver.security_status = efi::Status::SUCCESS;
                return Ok(());
            }
        }
        Err(EfiError::NotFound)
    }
}

unsafe impl Send for DispatcherContext {}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::sync::atomic::AtomicBool;
    use std::{fs::File, io::Read, vec};

    use log::{Level, LevelFilter, Metadata, Record};
    use patina::pi;
    use patina_ffs_extractors::NullSectionExtractor;
    use patina_internal_device_path::DevicePathWalker;
    use uuid::uuid;

    use super::*;
    use crate::{MockCore, MockPlatformInfo, test_collateral, test_support};

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
            f();
        })
        .unwrap();
    }

    // Monkey patch for get_physical_address that always returns NOT_FOUND.
    extern "efiapi" fn get_physical_address1(
        _: *mut pi::protocols::firmware_volume_block::Protocol,
        _: *mut u64,
    ) -> efi::Status {
        efi::Status::NOT_FOUND
    }

    // Monkey patch for get_physical_address that always returns 0.
    extern "efiapi" fn get_physical_address2(
        _: *mut pi::protocols::firmware_volume_block::Protocol,
        addr: *mut u64,
    ) -> efi::Status {
        unsafe { addr.write(0) };
        efi::Status::SUCCESS
    }

    // Monkey patch for get_physical_address that returns a physical address as determined by `GET_PHYSICAL_ADDRESS3_VALUE`
    extern "efiapi" fn get_physical_address3(
        _: *mut pi::protocols::firmware_volume_block::Protocol,
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();
            CORE.pi_dispatcher.init();
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            const DRIVERS_IN_DXEFV: usize = 130;
            assert_eq!(CORE.pi_dispatcher.dispatcher_context.lock().pending_drivers.len(), DRIVERS_IN_DXEFV);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_add_fv_handle_with_invalid_handle() {
        set_logger();
        with_locked_state(|| {
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let result = std::panic::catch_unwind(|| {
                CORE.pi_dispatcher
                    .add_fv_handles(vec![std::ptr::null_mut::<c_void>()])
                    .expect("Failed to add FV handle");
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };

            // Monkey Patch get_physical_address to one that returns an error.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address1;

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            assert_eq!(CORE.pi_dispatcher.dispatcher_context.lock().pending_drivers.len(), 0);
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };

            // Monkey Patch get_physical_address to set address to 0.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address2;

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            assert_eq!(CORE.pi_dispatcher.dispatcher_context.lock().pending_drivers.len(), 0);
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let fv_phys_addr = fv_raw.expose_provenance() as u64;
            let handle =
                unsafe { CORE.pi_dispatcher.fv_data.lock().install_firmware_volume(fv_phys_addr, None).unwrap() };

            // Monkey Patch get_physical_address to set to a slightly invalid address.
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address3;

            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = fv_phys_addr + 0x1000 };
            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = 0 };

            assert_eq!(CORE.pi_dispatcher.dispatcher_context.lock().pending_drivers.len(), 0);
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };
            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            // 1 child FV should be pending contained in NESTEDFV.Fv
            assert_eq!(CORE.pi_dispatcher.dispatcher_context.lock().pending_firmware_volume_images.len(), 1);
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            CORE.pi_dispatcher.display_discovered_not_dispatched();
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let _ = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };
            PiDispatcher::<MockPlatformInfo>::fw_vol_event_protocol_notify_efiapi(
                std::ptr::null_mut::<c_void>(),
                std::ptr::null_mut::<c_void>(),
            );

            const DRIVERS_IN_DXEFV: usize = 130;
            assert_eq!(CORE.pi_dispatcher.dispatcher_context.lock().pending_drivers.len(), DRIVERS_IN_DXEFV);
        });

        let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
    }

    #[test]
    fn test_dispatch_when_already_dispatching() {
        set_logger();
        with_locked_state(|| {
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            CORE.pi_dispatcher.dispatcher_context.lock().executing = true;
            let result = CORE.pi_dispatcher.dispatcher();
            assert_eq!(result, Err(EfiError::AlreadyStarted));
        })
    }

    #[test]
    fn test_dispatch_with_nothing_to_dispatch() {
        set_logger();
        with_locked_state(|| {
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let result = CORE.pi_dispatcher.dispatcher();
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // Cannot actually dispatch
            let result = CORE.pi_dispatcher.dispatcher();
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();
            // Safety: fv is leaked to ensure it is not freed and remains valid for the duration of the program.
            let handle = unsafe {
                CORE.pi_dispatcher
                    .fv_data
                    .lock()
                    .install_firmware_volume(fv_raw.expose_provenance() as u64, None)
                    .unwrap()
            };

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");

            // No SOR drivers to schedule in DXEFV, but we can test all the way to detecting that it does not have a SOR depex.
            let result = CORE.pi_dispatcher.dispatcher_context.lock().schedule(
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
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

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
            let handle =
                unsafe { CORE.pi_dispatcher.fv_data.lock().install_firmware_volume(fv.as_ptr() as u64, None).unwrap() };

            CORE.pi_dispatcher.add_fv_handles(vec![handle]).expect("Failed to add FV handle");
            CORE.pi_dispatcher.dispatcher().unwrap();

            assert!(SECURITY_CALL_EXECUTED.load(core::sync::atomic::Ordering::SeqCst));
        })
    }

    #[test]
    fn test_fv_already_installed() {
        set_logger();

        with_locked_state(|| {
            // Load a test FV and install it
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");

            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let _handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            // Get the actual FV name GUID from the installed FV
            let actual_fv_guid = {
                let volume = unsafe { VolumeRef::new_from_address(fv_raw.expose_provenance() as u64).unwrap() };
                volume.fv_name().expect("Test FV should have a name GUID")
            };

            // Check that the installed FV is detected
            assert!(
                CORE.pi_dispatcher.is_fv_already_installed(actual_fv_guid),
                "Should return true when FV is installed"
            );

            // Check that a non-existent FV GUID is not detected
            let non_existent_guid = r_efi::efi::Guid::from_fields(
                0x11111111,
                0x2222,
                0x3333,
                0x44,
                0x55,
                &[0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB],
            );
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(non_existent_guid),
                "Should return false for non-existent FV GUID"
            );

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
        });
    }

    #[test]
    fn test_is_fv_already_installed_error_paths() {
        set_logger();

        with_locked_state(|| {
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();
            // Test that no FVB handles installed returns false
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(r_efi::efi::Guid::from_fields(
                    0xAAAAAAAA,
                    0xBBBB,
                    0xCCCC,
                    0xDD,
                    0xEE,
                    &[0xFF, 0x00, 0x11, 0x22, 0x33, 0x44],
                )),
                "Should return false when no FVB handles exist"
            );

            // Test that installing an FV and checking a non-matching GUID returns false
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            let _handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            // Test that a non-matching GUID returns false
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(r_efi::efi::Guid::from_fields(
                    0x11111111,
                    0x2222,
                    0x3333,
                    0x44,
                    0x55,
                    &[0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB],
                )),
                "Should return false when GUID doesn't match any installed FV"
            );

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
        });
    }

    #[test]
    fn test_dispatch_with_duplicate_fv_prevention() {
        set_logger();

        with_locked_state(|| {
            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Load the parent FV that contains a compressed child FV
            let mut file = File::open(test_collateral!("NESTEDFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            // Install the parent FV
            let parent_handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            CORE.pi_dispatcher.add_fv_handles(vec![parent_handle]).expect("Failed to add parent FV handle");

            // Verify that there is a pending FV image file (the compressed child)
            assert_eq!(
                CORE.pi_dispatcher.dispatcher_context.lock().pending_firmware_volume_images.len(),
                1,
                "Should have one pending FV image file"
            );

            // Get the child FV GUID from the pending image
            let child_fv_sections = CORE
                .pi_dispatcher
                .dispatcher_context
                .lock()
                .pending_firmware_volume_images
                .first()
                .map(|img| img.fv_sections.clone())
                .expect("There should be a pending FV image"); // Extract and install the child FV separately to simulate it being already installed
            if let Some(section) = child_fv_sections.first() {
                let child_fv_data = section.try_content_as_slice().expect("Should be able to get child FV data");
                let child_volume = unsafe { VolumeRef::new_from_address(child_fv_data.as_ptr() as u64) }
                    .expect("Should be able to parse the child FV");

                if let Some(child_fv_guid) = child_volume.fv_name() {
                    // Install the child FV directly
                    let child_fv_box: Box<[u8]> = Box::from(child_fv_data);
                    let child_fv_raw = Box::into_raw(child_fv_box);
                    let _child_handle = unsafe {
                        CORE.pi_dispatcher
                            .install_firmware_volume(child_fv_raw.expose_provenance() as u64, Some(parent_handle))
                            .expect("Should be able to install the child FV")
                    };

                    assert!(
                        CORE.pi_dispatcher.is_fv_already_installed(child_fv_guid),
                        "Child FV should be detected as already installed"
                    );

                    // Dispatch should now skip the child FV since it's already installed
                    let dispatch_result = CORE.pi_dispatcher.dispatch();

                    assert!(
                        dispatch_result.is_ok() || dispatch_result == Err(EfiError::NotFound),
                        "Dispatch should complete without error or return NotFound"
                    );

                    // The pending child FV should be removed now
                    assert_eq!(
                        CORE.pi_dispatcher.dispatcher_context.lock().pending_firmware_volume_images.len(),
                        0,
                        "Pending FV images should be empty after dispatch skipped duplicate"
                    );

                    let _dropped_child = unsafe { Box::from_raw(child_fv_raw) };
                }
            }

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
        });
    }

    #[test]
    fn test_is_fv_already_installed_with_null_fvb() {
        set_logger();

        with_locked_state(|| {
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");

            PROTOCOL_DB
                .uninstall_protocol_interface(handle, firmware_volume_block::PROTOCOL_GUID, protocol)
                .expect("Failed to uninstall protocol");

            PROTOCOL_DB
                .install_protocol_interface(
                    Some(handle),
                    firmware_volume_block::PROTOCOL_GUID,
                    core::ptr::null_mut::<c_void>(),
                )
                .expect("Failed to install null protocol");

            // Should return false since the FVB protocol is null
            let test_guid = r_efi::efi::Guid::from_fields(
                0xAAAAAAAA,
                0xBBBB,
                0xCCCC,
                0xDD,
                0xEE,
                &[0xFF, 0x00, 0x11, 0x22, 0x33, 0x44],
            );
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(test_guid),
                "Should return false when the FVB protocol is null"
            );

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
        });
    }

    #[test]
    fn test_is_fv_already_installed_with_get_physical_address_error() {
        set_logger();

        with_locked_state(|| {
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            // Patch get_physical_address to return an error
            unsafe { &mut *protocol }.get_physical_address = get_physical_address1;

            let test_guid = r_efi::efi::Guid::from_fields(
                0xAAAAAAAA,
                0xBBBB,
                0xCCCC,
                0xDD,
                0xEE,
                &[0xFF, 0x00, 0x11, 0x22, 0x33, 0x44],
            );
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(test_guid),
                "Should return false when get_physical_address fails"
            );

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
        });
    }

    #[test]
    fn test_is_fv_already_installed_with_zero_address() {
        set_logger();

        with_locked_state(|| {
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            // Patch get_physical_address to return zero
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address2;

            let test_guid = r_efi::efi::Guid::from_fields(
                0xAAAAAAAA,
                0xBBBB,
                0xCCCC,
                0xDD,
                0xEE,
                &[0xFF, 0x00, 0x11, 0x22, 0x33, 0x44],
            );
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(test_guid),
                "Should return false when the address is zero"
            );

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
        });
    }

    #[test]
    fn test_is_fv_already_installed_with_invalid_volume() {
        set_logger();

        with_locked_state(|| {
            let mut file = File::open(test_collateral!("DXEFV.Fv")).unwrap();
            let mut fv: Vec<u8> = Vec::new();
            file.read_to_end(&mut fv).expect("failed to read test file");
            let fv = fv.into_boxed_slice();
            let fv_raw = Box::into_raw(fv);

            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            let handle =
                unsafe { CORE.pi_dispatcher.install_firmware_volume(fv_raw.expose_provenance() as u64, None).unwrap() };

            // Create some invalid FV data in memory
            let invalid_fv_data = vec![0xFFu8; 1024];
            let invalid_fv_box = invalid_fv_data.into_boxed_slice();
            let invalid_fv_raw = Box::into_raw(invalid_fv_box);

            // Patch get_physical_address to return the invalid FV address
            let protocol = PROTOCOL_DB
                .get_interface_for_handle(handle, firmware_volume_block::PROTOCOL_GUID)
                .expect("Failed to get FVB protocol");
            let protocol = protocol as *mut firmware_volume_block::Protocol;
            unsafe { &mut *protocol }.get_physical_address = get_physical_address3;

            // Set to the address of the invalid FV data
            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = invalid_fv_raw.expose_provenance() as u64 };

            let test_guid = r_efi::efi::Guid::from_fields(
                0xAAAAAAAA,
                0xBBBB,
                0xCCCC,
                0xDD,
                0xEE,
                &[0xFF, 0x00, 0x11, 0x22, 0x33, 0x44],
            );
            assert!(
                !CORE.pi_dispatcher.is_fv_already_installed(test_guid),
                "Should return false when volume parsing fails"
            );

            unsafe { GET_PHYSICAL_ADDRESS3_VALUE = 0 };

            let _dropped_fv = unsafe { Box::from_raw(fv_raw) };
            let _dropped_invalid_fv = unsafe { Box::from_raw(invalid_fv_raw) };
        });
    }

    #[test]
    fn test_dispatch_with_corrupted_fv_section() {
        set_logger();

        with_locked_state(|| {
            // Create corrupted FV data (an invalid FV signature/header)
            let corrupted_fv_data = vec![0xFFu8; 256];

            // Create a valid FirmwareVolumeImage section header with the corrupted data
            use patina_ffs::section::SectionHeader;
            let section_header = SectionHeader::Standard(ffs::section::raw_type::FIRMWARE_VOLUME_IMAGE, 256);

            // Create a section with a valid header but corrupted FV data
            let corrupted_section = Section::new_from_header_with_data(section_header, corrupted_fv_data)
                .expect("Should create section from header and data");

            // Create a pending FV image with the corrupted section
            let pending_fv = PendingFirmwareVolumeImage {
                parent_fv_handle: std::ptr::null_mut(),
                file_name: r_efi::efi::Guid::from_fields(
                    0x11111111,
                    0x2222,
                    0x3333,
                    0x44,
                    0x55,
                    &[0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB],
                ),
                depex: None,
                fv_sections: vec![corrupted_section],
            };

            static CORE: MockCore = MockCore::new(NullSectionExtractor::new());
            CORE.override_instance();

            // Add the pending FV to the dispatcher
            CORE.pi_dispatcher.dispatcher_context.lock().pending_firmware_volume_images.push(pending_fv);

            // Dispatch should log a warning and continue
            let result = CORE.pi_dispatcher.dispatch();

            assert!(
                result.is_ok() || result == Err(EfiError::NotFound),
                "Dispatch should handle corrupted FV section gracefully"
            );

            // The corrupted FV should have been removed from pending FV images
            assert_eq!(
                CORE.pi_dispatcher.dispatcher_context.lock().pending_firmware_volume_images.len(),
                0,
                "A corrupted FV should be removed after a dispatch attempt"
            );
        });
    }
}
