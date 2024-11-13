use alloc::{boxed::Box, vec};
use core::{
    ffi::c_void,
    slice::{from_raw_parts, from_raw_parts_mut},
    sync::atomic::Ordering,
};
use mu_pi::protocols;
use mu_pi::status_code;
use r_efi::efi;
use super::{SystemState, ProtocolDb, EventDb};

use crate::{
    allocator::{terminate_memory_map, EFI_RUNTIME_SERVICES_DATA_ALLOCATOR}, systemtables::{EfiSystemTable, SYSTEM_TABLE}
};

// TODO [BEGIN]: LOCAL (TEMP) GUID DEFINITIONS (MOVE LATER)

// These will likely get moved to different places. Pre-exit boot services is defined in. DXE Core GUID is
// the GUID of this DXE Core instance. Exit Boot Services Failed is an edk2 customization.

// { 0x5f1d7e16, 0x784a, 0x4da2, { 0xb0, 0x84, 0xf8, 0x12, 0xf2, 0x3a, 0x8d, 0xce }}
pub const PRE_EBS_GUID: efi::Guid =
    efi::Guid::from_fields(0x5f1d7e16, 0x784a, 0x4da2, 0xb0, 0x84, &[0xf8, 0x12, 0xf2, 0x3a, 0x8d, 0xce]);

// { 0x4f6c5507, 0x232f, 0x4787, { 0xb9, 0x5e, 0x72, 0xf8, 0x62, 0x49, 0xc, 0xb1 } }
pub const EBS_FAILED_GUID: efi::Guid =
    efi::Guid::from_fields(0x4f6c5507, 0x232f, 0x4787, 0xb9, 0x5e, &[0x72, 0xf8, 0x62, 0x49, 0x0c, 0xb1]);

// DxeCore module GUID (23C9322F-2AF2-476A-BC4C-26BC88266C71)
pub const DXE_CORE_GUID: efi::Guid =
    efi::Guid::from_fields(0x23C9322F, 0x2AF2, 0x476A, 0xBC, 0x4C, &[0x26, 0xBC, 0x88, 0x26, 0x6C, 0x71]);

// TODO [END]: LOCAL (TEMP) GUID DEFINITIONS (MOVE LATER)

#[inline(always)]
pub fn calculate_crc32(data: *mut c_void, data_size: usize, crc_32: *mut u32) -> efi::Status {
    if data.is_null() || data_size == 0 || crc_32.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    unsafe {
        let buffer = from_raw_parts(data as *mut u8, data_size);
        crc_32.write(crc32fast::hash(buffer));
    }

    efi::Status::SUCCESS
}

#[inline(always)]
pub fn core_install_configuration_table(
    event_db: &mut EventDb,
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
    event_db.signal_group(vendor_guid);

    Ok(())
}

pub fn install_configuration_table(event_db: &mut EventDb, table_guid: *mut efi::Guid, table: *mut c_void) -> efi::Status {
    if table_guid.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let table_guid = unsafe { *table_guid };
    let table = unsafe { table.as_mut() };

    let mut st_guard = SYSTEM_TABLE.lock();
    let st = st_guard.as_mut().expect("System table support not initialized");

    match core_install_configuration_table(event_db, table_guid, table, st) {
        Err(err) => err,
        Ok(()) => efi::Status::SUCCESS,
    }
}

// Induces a fine-grained stall. Stalls execution on the processor for at least the requested number of microseconds.
// Execution of the processor is not yielded for the duration of the stall.
pub fn stall(st: &SystemState, microseconds: usize) -> efi::Status {
    let metronome_ptr = st.metronome_arch_ptr.load(Ordering::SeqCst);
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
pub fn set_watchdog_timer(
    st: &SystemState,
    timeout: usize,
    _watchdog_code: u64,
    _data_size: usize,
    _data: *mut efi::Char16,
) -> efi::Status {
    const WATCHDOG_TIMER_CALIBRATE_PER_SECOND: u64 = 10000000;
    let watchdog_ptr = st.watchdog_arch_ptr.load(Ordering::SeqCst);
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
pub fn metronome_arch_available(protocol_db: &mut ProtocolDb, event_db: &mut EventDb, st: &SystemState, event: efi::Event, _context: *mut c_void) {
    match protocol_db.locate_protocol(protocols::metronome::PROTOCOL_GUID) {
        Ok(metronome_arch_ptr) => {
            st.metronome_arch_ptr.store(metronome_arch_ptr as *mut protocols::metronome::Protocol, Ordering::SeqCst);
            if let Err(status_err) = event_db.close_event(event) {
                log::warn!("Could not close event for metronome_arch_available due to error {:?}", status_err);
            }
        }
        Err(err) => panic!("Unable to retrieve metronome arch: {:?}", err),
    }
}

// This callback is invoked when the Watchdog Timer Architectural protocol is installed. It initializes the
// WATCHDOG_ARCH_PTR to point to the Watchdog Timer Architectural protocol interface.
pub fn watchdog_arch_available(protocol_db: &mut ProtocolDb, event_db: &mut EventDb, st: &SystemState, event: efi::Event, _context: *mut c_void) {
    match protocol_db.locate_protocol(protocols::watchdog::PROTOCOL_GUID) {
        Ok(watchdog_arch_ptr) => {
            st.watchdog_arch_ptr.store(watchdog_arch_ptr as *mut protocols::watchdog::Protocol, Ordering::SeqCst);
            if let Err(status_err) = event_db.close_event(event) {
                log::warn!("Could not close event for watchdog_arch_available due to error {:?}", status_err);
            }
        }
        Err(err) => panic!("Unable to retrieve watchdog arch: {:?}", err),
    }
}

pub extern "efiapi" fn exit_boot_services(protocol_db: &mut ProtocolDb, event_db: &mut EventDb, st: &SystemState, _handle: efi::Handle, map_key: usize) -> efi::Status {
    // Pre-exit boot services is only signaled once
    if !st.pre_exit_boot_services_signal.load(Ordering::SeqCst) {
        event_db.signal_group(PRE_EBS_GUID);
        st.pre_exit_boot_services_signal.store(true, Ordering::SeqCst);
    }

    // Signal the event group before exit boot services
    event_db.signal_group(efi::EVENT_GROUP_BEFORE_EXIT_BOOT_SERVICES);

    // Disable the timer
    match protocol_db.locate_protocol(protocols::timer::PROTOCOL_GUID) {
        Ok(timer_arch_ptr) => {
            let timer_arch_ptr = timer_arch_ptr as *mut protocols::timer::Protocol;
            let timer_arch = unsafe { &*(timer_arch_ptr) };
            (timer_arch.set_timer_period)(timer_arch_ptr, 0);
        }
        Err(err) => log::error!("Unable to locate timer arch: {:?}", err),
    };

    // Terminate the memory map
    let status = terminate_memory_map(map_key);
    if status.is_error() {
        event_db.signal_group(EBS_FAILED_GUID);
        return status;
    }

    // Signal Exit Boot Services
    event_db.signal_group(efi::EVENT_GROUP_EXIT_BOOT_SERVICES);

    // Initialize StatusCode and send EFI_SW_BS_PC_EXIT_BOOT_SERVICES
    match protocol_db.locate_protocol(protocols::status_code::PROTOCOL_GUID) {
        Ok(status_code_ptr) => {
            let status_code_ptr = status_code_ptr as *mut protocols::status_code::Protocol;
            let status_code_protocol = unsafe { &*(status_code_ptr) };
            (status_code_protocol.report_status_code)(
                status_code::EFI_PROGRESS_CODE,
                status_code::EFI_SOFTWARE_EFI_BOOT_SERVICE | status_code::EFI_SW_BS_PC_EXIT_BOOT_SERVICES,
                0,
                &DXE_CORE_GUID,
                core::ptr::null(),
            );
        }
        Err(err) => log::error!("Unable to locate status code runtime protocol: {:?}", err),
    };

    // Disable CPU interrupts
    match protocol_db.locate_protocol(protocols::cpu_arch::PROTOCOL_GUID) {
        Ok(cpu_arch_ptr) => {
            let cpu_arch_ptr = cpu_arch_ptr as *mut protocols::cpu_arch::Protocol;
            let cpu_arch_protocol = unsafe { &*(cpu_arch_ptr) };
            (cpu_arch_protocol.disable_interrupt)(cpu_arch_ptr);
        }
        Err(err) => log::error!("Unable to locate CPU arch protocol: {:?}", err),
    };

    // Clear non-runtime services from the EFI System Table
    SYSTEM_TABLE
        .lock()
        .as_mut()
        .expect("The System Table pointer is null. This is invalid.")
        .clear_boot_time_services();

    match protocol_db.locate_protocol(protocols::runtime::PROTOCOL_GUID) {
        Ok(rt_arch_ptr) => {
            let rt_arch_ptr = rt_arch_ptr as *mut protocols::runtime::Protocol;
            let rt_arch_protocol = unsafe { &mut *(rt_arch_ptr) };
            rt_arch_protocol.at_runtime.store(true, Ordering::SeqCst);
        }
        Err(err) => log::error!("Unable to locate runtime architectural protocol: {:?}", err),
    };

    efi::Status::SUCCESS
}
