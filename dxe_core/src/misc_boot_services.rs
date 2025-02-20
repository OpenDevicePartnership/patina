//! DXE Core Miscellaneous Boot Services
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{boxed::Box, vec};
use core::{
    ffi::c_void,
    slice::{from_raw_parts, from_raw_parts_mut},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};
use mu_pi::{protocols, status_code};
use r_efi::efi;
use uefi_cpu::interrupts;
use uefi_sdk::guid;

use crate::{
    allocator::{terminate_memory_map, EFI_RUNTIME_SERVICES_DATA_ALLOCATOR},
    events::EVENT_DB,
    protocols::PROTOCOL_DB,
    systemtables::{EfiSystemTable, SYSTEM_TABLE},
    GCD,
};

static METRONOME_ARCH_PTR: AtomicPtr<protocols::metronome::Protocol> = AtomicPtr::new(core::ptr::null_mut());
static WATCHDOG_ARCH_PTR: AtomicPtr<protocols::watchdog::Protocol> = AtomicPtr::new(core::ptr::null_mut());

// TODO [BEGIN]: LOCAL (TEMP) GUID DEFINITIONS (MOVE LATER)

// These will likely get moved to different places. DXE Core GUID is the GUID of this DXE Core instance.
// Exit Boot Services Failed is an edk2 customization.

// Pre-EBS GUID is a Project Mu defined GUID. It should be removed in favor of the UEFI Spec defined
// Before Exit Boot Services event group when all platform usage is confirmed to be transitioned to that.
// { 0x5f1d7e16, 0x784a, 0x4da2, { 0xb0, 0x84, 0xf8, 0x12, 0xf2, 0x3a, 0x8d, 0xce }}
pub const PRE_EBS_GUID: efi::Guid =
    efi::Guid::from_fields(0x5f1d7e16, 0x784a, 0x4da2, 0xb0, 0x84, &[0xf8, 0x12, 0xf2, 0x3a, 0x8d, 0xce]);

// TODO [END]: LOCAL (TEMP) GUID DEFINITIONS (MOVE LATER)

extern "efiapi" fn calculate_crc32(data: *mut c_void, data_size: usize, crc_32: *mut u32) -> efi::Status {
    if data.is_null() || data_size == 0 || crc_32.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe {
        let buffer = from_raw_parts(data as *mut u8, data_size);
        crc_32.write(crc32fast::hash(buffer));
    }

    efi::Status::SUCCESS
}

pub fn core_install_configuration_table(
    vendor_guid: efi::Guid,
    vendor_table: Option<&mut c_void>,
    efi_system_table: &mut EfiSystemTable,
) -> Result<(), efi::Status> {
    let system_table = efi_system_table.as_mut();
    //if a table is already present, reconstruct it from the pointer and length in the st.
    let old_cfg_table = if system_table.configuration_table.is_null() {
        assert_eq!(system_table.number_of_table_entries, 0);
        None
    } else {
        let ct_slice_box = unsafe {
            Box::from_raw_in(
                from_raw_parts_mut(system_table.configuration_table, system_table.number_of_table_entries),
                &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
            )
        };
        Some(ct_slice_box)
    };

    // construct the new table contents as a vector.
    let new_table = match old_cfg_table {
        Some(cfg_table) => {
            // a configuration table list is already present.
            let mut current_table = cfg_table.to_vec();
            let existing_entry = current_table.iter_mut().find(|x| x.vendor_guid == vendor_guid);
            if let Some(vendor_table) = vendor_table {
                //vendor_table is some; we are adding or modifying an entry.
                if let Some(entry) = existing_entry {
                    //entry exists, modify it.
                    entry.vendor_table = vendor_table;
                } else {
                    //entry doesn't exist, add it.
                    current_table.push(efi::ConfigurationTable { vendor_guid, vendor_table });
                }
            } else {
                //vendor_table is none; we are deleting an entry.
                if let Some(_entry) = existing_entry {
                    //entry exists, we can delete it
                    current_table.retain(|x| x.vendor_guid != vendor_guid);
                } else {
                    //entry does not exist, we can't delete it. We have to put the original box back
                    //in the config table so it doesn't get dropped though. Pointer should be the same
                    //so we should not need to recompute CRC.
                    system_table.configuration_table = Box::into_raw(cfg_table) as *mut efi::ConfigurationTable;
                    return Err(efi::Status::NOT_FOUND);
                }
            }
            current_table
        }
        None => {
            //config table list doesn't exist.
            if let Some(table) = vendor_table {
                // table is some, meaning we should create the list and add this as the new entry.
                vec![efi::ConfigurationTable { vendor_guid, vendor_table: table }]
            } else {
                //table is none, but can't delete a table entry in a list that doesn't exist.
                //since the list doesn't exist, we can leave the (null) pointer in the st alone.
                return Err(efi::Status::NOT_FOUND);
            }
        }
    };

    if new_table.is_empty() {
        // if empty, just set config table ptr to null
        system_table.number_of_table_entries = 0;
        system_table.configuration_table = core::ptr::null_mut();
    } else {
        //Box up the new table and put it in the system table. The old table (if any) will be dropped
        //when old_cfg_table goes out of scope at the end of the function.
        system_table.number_of_table_entries = new_table.len();
        let new_table = new_table.to_vec_in(&EFI_RUNTIME_SERVICES_DATA_ALLOCATOR).into_boxed_slice();
        system_table.configuration_table = Box::into_raw(new_table) as *mut efi::ConfigurationTable;
    }
    //since we modified the system table, re-calculate CRC.
    efi_system_table.checksum();

    //signal the table guid as an event group
    EVENT_DB.signal_group(vendor_guid);

    Ok(())
}

extern "efiapi" fn install_configuration_table(table_guid: *mut efi::Guid, table: *mut c_void) -> efi::Status {
    if table_guid.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let table_guid = unsafe { *table_guid };
    let table = unsafe { table.as_mut() };

    let mut st_guard = SYSTEM_TABLE.lock();
    let st = st_guard.as_mut().expect("System table support not initialized");

    match core_install_configuration_table(table_guid, table, st) {
        Err(err) => err,
        Ok(()) => efi::Status::SUCCESS,
    }
}

// Induces a fine-grained stall. Stalls execution on the processor for at least the requested number of microseconds.
// Execution of the processor is not yielded for the duration of the stall.
extern "efiapi" fn stall(microseconds: usize) -> efi::Status {
    let metronome_ptr = METRONOME_ARCH_PTR.load(Ordering::SeqCst);
    if let Some(metronome) = unsafe { metronome_ptr.as_mut() } {
        let ticks_100ns: u128 = (microseconds as u128) * 10;
        let mut ticks = ticks_100ns / metronome.tick_period as u128;
        while ticks > u32::MAX as u128 {
            let status = (metronome.wait_for_tick)(metronome_ptr, u32::MAX);
            if status.is_error() {
                log::warn!("metronome.wait_for_tick returned unexpected error {:#x?}", status);
            }
            ticks -= u32::MAX as u128;
        }
        if ticks != 0 {
            let status = (metronome.wait_for_tick)(metronome_ptr, ticks as u32);
            if status.is_error() {
                log::warn!("metronome.wait_for_tick returned unexpected error {:#x?}", status);
            }
        }
        efi::Status::SUCCESS
    } else {
        efi::Status::NOT_READY //technically this should be NOT_AVAILABLE_YET.
    }
}

// The SetWatchdogTimer() function sets the system's watchdog timer.
// If the watchdog timer expires, the event is logged by the firmware. The system may then either reset with the Runtime
// Service ResetSystem() or perform a platform specific action that must eventually cause the platform to be reset. The
// watchdog timer is armed before the firmware's boot manager invokes an EFI boot option. The watchdog must be set to a
// period of 5 minutes. The EFI Image may reset or disable the watchdog timer as needed. If control is returned to the
// firmware's boot manager, the watchdog timer must be disabled.
//
// The watchdog timer is only used during boot services. On successful completion of
// EFI_BOOT_SERVICES.ExitBootServices() the watchdog timer is disabled.
extern "efiapi" fn set_watchdog_timer(
    timeout: usize,
    _watchdog_code: u64,
    _data_size: usize,
    _data: *mut efi::Char16,
) -> efi::Status {
    const WATCHDOG_TIMER_CALIBRATE_PER_SECOND: u64 = 10000000;
    let watchdog_ptr = WATCHDOG_ARCH_PTR.load(Ordering::SeqCst);
    if let Some(watchdog) = unsafe { watchdog_ptr.as_mut() } {
        let timeout = (timeout as u64).saturating_mul(WATCHDOG_TIMER_CALIBRATE_PER_SECOND);
        let status = (watchdog.set_timer_period)(watchdog_ptr, timeout);
        if status.is_error() {
            return efi::Status::DEVICE_ERROR;
        }
        efi::Status::SUCCESS
    } else {
        efi::Status::NOT_READY
    }
}

// This callback is invoked when the Metronome Architectural protocol is installed. It initializes the
// METRONOME_ARCH_PTR to point to the Metronome Architectural protocol interface.
extern "efiapi" fn metronome_arch_available(event: efi::Event, _context: *mut c_void) {
    match PROTOCOL_DB.locate_protocol(protocols::metronome::PROTOCOL_GUID) {
        Ok(metronome_arch_ptr) => {
            METRONOME_ARCH_PTR.store(metronome_arch_ptr as *mut protocols::metronome::Protocol, Ordering::SeqCst);
            if let Err(status_err) = EVENT_DB.close_event(event) {
                log::warn!("Could not close event for metronome_arch_available due to error {:?}", status_err);
            }
        }
        Err(err) => panic!("Unable to retrieve metronome arch: {:?}", err),
    }
}

// This callback is invoked when the Watchdog Timer Architectural protocol is installed. It initializes the
// WATCHDOG_ARCH_PTR to point to the Watchdog Timer Architectural protocol interface.
extern "efiapi" fn watchdog_arch_available(event: efi::Event, _context: *mut c_void) {
    match PROTOCOL_DB.locate_protocol(protocols::watchdog::PROTOCOL_GUID) {
        Ok(watchdog_arch_ptr) => {
            WATCHDOG_ARCH_PTR.store(watchdog_arch_ptr as *mut protocols::watchdog::Protocol, Ordering::SeqCst);
            if let Err(status_err) = EVENT_DB.close_event(event) {
                log::warn!("Could not close event for watchdog_arch_available due to error {:?}", status_err);
            }
        }
        Err(err) => panic!("Unable to retrieve watchdog arch: {:?}", err),
    }
}

pub extern "efiapi" fn exit_boot_services(_handle: efi::Handle, map_key: usize) -> efi::Status {
    static EXIT_BOOT_SERVICES_CALLED: AtomicBool = AtomicBool::new(false);

    log::info!("EBS initiated.");
    // Pre-exit boot services and before exit boot services are only signaled once
    if !EXIT_BOOT_SERVICES_CALLED.load(Ordering::SeqCst) {
        EVENT_DB.signal_group(PRE_EBS_GUID);

        // Signal the event group before exit boot services
        EVENT_DB.signal_group(efi::EVENT_GROUP_BEFORE_EXIT_BOOT_SERVICES);

        EXIT_BOOT_SERVICES_CALLED.store(true, Ordering::SeqCst);
    }

    // Disable the timer
    match PROTOCOL_DB.locate_protocol(protocols::timer::PROTOCOL_GUID) {
        Ok(timer_arch_ptr) => {
            let timer_arch_ptr = timer_arch_ptr as *mut protocols::timer::Protocol;
            let timer_arch = unsafe { &*(timer_arch_ptr) };
            (timer_arch.set_timer_period)(timer_arch_ptr, 0);
        }
        Err(err) => log::error!("Unable to locate timer arch: {:?}", err),
    };

    // Lock the memory space to prevent edits to the memory map after this point.
    GCD.lock_memory_space();

    // Terminate the memory map
    // According to UEFI spec, in case of an incomplete or failed EBS call we must restore boot services memory allocation functionality
    let status = terminate_memory_map(map_key);
    if status.is_error() {
        GCD.unlock_memory_space();
        EVENT_DB.signal_group(guid::EBS_FAILED);
        return status;
    }

    // Signal Exit Boot Services
    EVENT_DB.signal_group(efi::EVENT_GROUP_EXIT_BOOT_SERVICES);

    // Initialize StatusCode and send EFI_SW_BS_PC_EXIT_BOOT_SERVICES
    match PROTOCOL_DB.locate_protocol(protocols::status_code::PROTOCOL_GUID) {
        Ok(status_code_ptr) => {
            let status_code_ptr = status_code_ptr as *mut protocols::status_code::Protocol;
            let status_code_protocol = unsafe { &*(status_code_ptr) };
            (status_code_protocol.report_status_code)(
                status_code::EFI_PROGRESS_CODE,
                status_code::EFI_SOFTWARE_EFI_BOOT_SERVICE | status_code::EFI_SW_BS_PC_EXIT_BOOT_SERVICES,
                0,
                &guid::DXE_CORE,
                core::ptr::null(),
            );
        }
        Err(err) => log::error!("Unable to locate status code runtime protocol: {:?}", err),
    };

    // Disable CPU interrupts
    interrupts::disable_interrupts();

    // Clear non-runtime services from the EFI System Table
    SYSTEM_TABLE
        .lock()
        .as_mut()
        .expect("The System Table pointer is null. This is invalid.")
        .clear_boot_time_services();

    match PROTOCOL_DB.locate_protocol(protocols::runtime::PROTOCOL_GUID) {
        Ok(rt_arch_ptr) => {
            let rt_arch_ptr = rt_arch_ptr as *mut protocols::runtime::Protocol;
            let rt_arch_protocol = unsafe { &mut *(rt_arch_ptr) };
            rt_arch_protocol.at_runtime.store(true, Ordering::SeqCst);
        }
        Err(err) => log::error!("Unable to locate runtime architectural protocol: {:?}", err),
    };

    efi::Status::SUCCESS
}

pub fn init_misc_boot_services_support(bs: &mut efi::BootServices) {
    bs.calculate_crc32 = calculate_crc32;
    bs.exit_boot_services = exit_boot_services;
    bs.install_configuration_table = install_configuration_table;
    bs.stall = stall;
    bs.set_watchdog_timer = set_watchdog_timer;

    //set up call back for metronome arch protocol installation.
    let event = EVENT_DB
        .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(metronome_arch_available), None, None)
        .expect("Failed to create metronome available callback.");

    PROTOCOL_DB
        .register_protocol_notify(protocols::metronome::PROTOCOL_GUID, event)
        .expect("Failed to register protocol notify on metronome available.");

    //set up call back for watchdog arch protocol installation.
    let event = EVENT_DB
        .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(watchdog_arch_available), None, None)
        .expect("Failed to create watchdog available callback.");

    PROTOCOL_DB
        .register_protocol_notify(protocols::watchdog::PROTOCOL_GUID, event)
        .expect("Failed to register protocol notify on metronome available.");
}
