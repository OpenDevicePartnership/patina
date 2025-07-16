//! DXE Core Miscellaneous Boot Services
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use core::{
    ffi::c_void,
    slice::from_raw_parts,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};
use mu_pi::{protocols, status_code};
use patina_internal_cpu::interrupts;
use patina_sdk::guid;
use r_efi::efi;

use crate::{
    GCD, allocator::terminate_memory_map, events::EVENT_DB, protocols::PROTOCOL_DB, systemtables::SYSTEM_TABLE,
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
// Requires excessive Mocking for the OK case.
#[cfg(not(tarpaulin_include))]
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
// Requires excessive Mocking for the OK case.
#[cfg(not(tarpaulin_include))]
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
    match terminate_memory_map(map_key) {
        Ok(_) => (),
        Err(err) => {
            log::error!("Failed to terminate memory map: {:?}", err);
            GCD.unlock_memory_space();
            EVENT_DB.signal_group(guid::EBS_FAILED);
            return err.into();
        }
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

    log::info!("EBS completed successfully.");

    efi::Status::SUCCESS
}

pub fn init_misc_boot_services_support(bs: &mut efi::BootServices) {
    bs.calculate_crc32 = calculate_crc32;
    bs.exit_boot_services = exit_boot_services;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systemtables;
    use core::{ffi::c_void, ptr};
    use r_efi::efi;
    use std::cell::UnsafeCell;

    // Define a global static variable to store the Boot Services pointer
    struct BootServicesWrapper {
        boot_services: UnsafeCell<Option<&'static mut efi::BootServices>>,
    }

    unsafe impl Sync for BootServicesWrapper {}

    static BOOT_SERVICES: BootServicesWrapper = BootServicesWrapper { boot_services: UnsafeCell::new(None) };

    // Function to initialize the global Boot Services pointer
    pub fn initialize_boot_services(bs: &mut efi::BootServices) {
        unsafe {
            *BOOT_SERVICES.boot_services.get() = Some(&mut *(bs as *mut _));
        }
    }

    #[test]
    fn test_init_misc_boot_services_support() {
        let mut st = systemtables::SYSTEM_TABLE.lock();
        let st = st.as_mut().expect("System Table not initialized!");

        // Initialize BOOT_SERVICES using the BootServices instance from SYSTEM_TABLE
        initialize_boot_services(st.boot_services_mut());
        init_misc_boot_services_support(st.boot_services_mut());
        // Release the lock at the end
        std::mem::drop(st_guard);
    }

    #[test]
    fn test_misc_calc_crc32() {
        let mut st_guard = systemtables::SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().expect("System Table not initialized!");

        // Initialize BOOT_SERVICES using the BootServices instance from SYSTEM_TABLE
        initialize_boot_services(st.boot_services_mut());
        init_misc_boot_services_support(st.boot_services_mut());

        static BUFFER: [u8; 16] = [0; 16];
        let mut data_crc: u32 = 0;
        (st.boot_services_mut().calculate_crc32)(
            BUFFER.as_ptr() as *mut c_void,
            BUFFER.len(),
            &mut data_crc as *mut u32,
        );

        (st.boot_services_mut().calculate_crc32)(BUFFER.as_ptr() as *mut c_void, 0, &mut data_crc as *mut u32);
        // Release the lock at the end
        std::mem::drop(st_guard);
    }
    #[test]
    fn test_misc_watchdog_timer() {
        let mut st_guard = systemtables::SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().expect("System Table not initialized!");

        // Initialize BOOT_SERVICES using the BootServices instance from SYSTEM_TABLE
        initialize_boot_services(st.boot_services_mut());
        init_misc_boot_services_support(st.boot_services_mut());

        (st.boot_services_mut().set_watchdog_timer)(300, 0, 0, ptr::null_mut());
        (st.boot_services_mut().set_watchdog_timer)(0, 0, 0, ptr::null_mut()); //nothing changed.

        let data: [efi::Char16; 6] = [b'H' as u16, b'e' as u16, b'l' as u16, b'l' as u16, b'o' as u16, 0];
        let data_ptr = data.as_ptr() as *mut efi::Char16;
        // Case 1: Set the watchdog timer with non-null data
        let _status = (st.boot_services_mut().set_watchdog_timer)(300, 0, data.len(), data_ptr);

        // Case 2: Disable the watchdog timer with non-null data
        let _status = (st.boot_services_mut().set_watchdog_timer)(0, 0, data.len(), data_ptr);
        // Release the lock at the end
        std::mem::drop(st_guard);
    }
    #[test]
    fn test_misc_stall() {
        let mut st_guard = systemtables::SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().expect("System Table not initialized!");

        // Initialize BOOT_SERVICES using the BootServices instance from SYSTEM_TABLE
        initialize_boot_services(st.boot_services_mut());
        init_misc_boot_services_support(st.boot_services_mut());

        (st.boot_services_mut().stall)(10000);
        (st.boot_services_mut().stall)(0); // Changed
        (st.boot_services_mut().stall)(usize::MAX); // Changed
        // Release the lock at the end
        std::mem::drop(st_guard);
    }

    #[test]
    fn test_misc_exit_boot_services() {
        let valid_map_key: usize = 0x2000;
        // Acquire the lock on SYSTEM_TABLE
        let mut st_guard = systemtables::SYSTEM_TABLE.lock();
        let st = st_guard.as_mut().expect("System Table not initialized!");
        init_misc_boot_services_support(st.boot_services_mut());
        // Call exit_boot_services with a valid map_key
        let handle: efi::Handle = 0x1000 as efi::Handle; // Example handle
        let _status = (st.boot_services_mut().exit_boot_services)(handle, valid_map_key);
        // Release the lock at the end
        std::mem::drop(st_guard);
    }
}
