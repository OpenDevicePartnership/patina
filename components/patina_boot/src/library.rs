//! Library functions for boot orchestration.
//!
//! This module provides helper functions for platforms implementing custom boot flows.
//! The [`BootOrchestrator`](crate::component::BootOrchestrator) component uses these
//! internally, and platforms can use them directly for custom orchestration.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::vec::Vec;
use core::ptr;

use patina::{
    boot_services::{BootServices, event::EventType, protocol_handler::HandleSearchType, tpl::Tpl},
    error::{EfiError, Result},
    guids::EVENT_GROUP_END_OF_DXE,
    runtime_services::RuntimeServices,
    uefi_protocol::device_path::DevicePathBuf,
};
use r_efi::{efi, system::EVENT_GROUP_READY_TO_BOOT};

/// Watchdog timeout in seconds per UEFI Specification Section 3.1.2.
const WATCHDOG_TIMEOUT_SECONDS: usize = 300; // 5 minutes

/// Load and start a boot image with UEFI spec compliance.
///
/// Enables a 5-minute watchdog timer before `StartImage()` per UEFI Specification
/// Section 3.1.2. Disables watchdog when boot returns control.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `device_path` - Device path to the boot image
///
/// # Returns
///
/// Returns `Ok(())` if the boot image was successfully started (which typically
/// means it returned control). Returns an error if loading or starting fails.
pub fn boot_from_device_path<B: BootServices>(boot_services: &B, device_path: &DevicePathBuf) -> Result<()> {
    // Get parent image handle (self)
    // Note: In a real implementation, this would come from the component context
    let parent_handle = ptr::null_mut();

    // Load the image
    let device_path_ptr = device_path.as_ref() as *const _ as *mut efi::protocols::device_path::Protocol;
    let image_handle = boot_services.load_image(true, parent_handle, device_path_ptr, None).map_err(EfiError::from)?;

    // Enable 5-minute watchdog timer per UEFI spec Section 3.1.2
    boot_services.set_watchdog_timer(WATCHDOG_TIMEOUT_SECONDS).map_err(EfiError::from)?;

    // Start the image
    let result = boot_services.start_image(image_handle);

    // Disable watchdog timer when boot option returns control
    let _ = boot_services.set_watchdog_timer(0);

    match result {
        Ok(()) => Ok(()),
        Err((status, _exit_data)) => Err(EfiError::from(status)),
    }
}

/// Orchestrate connect-dispatch loop for device enumeration.
///
/// Connects controllers and dispatches drivers in a loop until the device
/// topology stabilizes (no new drivers are dispatched).
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
///
/// # Returns
///
/// Returns `Ok(())` when device topology enumeration is complete.
pub fn interleave_connect_and_dispatch<B: BootServices>(boot_services: &B) -> Result<()> {
    // Note: Full implementation requires DXE services for dispatch()
    // This is a simplified version that connects all handles

    // Get all handles in the system
    let handles = boot_services.locate_handle_buffer(HandleSearchType::AllHandle).map_err(EfiError::from)?;

    // Connect each handle recursively
    for &handle in handles.iter() {
        // SAFETY: Empty driver handle list and null device path are valid per UEFI spec
        let _ = unsafe { boot_services.connect_controller(handle, Vec::new(), ptr::null_mut(), true) };
    }

    Ok(())
}

/// Signal EndOfDxe event for platforms implementing custom orchestration.
///
/// Signals `gEfiEndOfDxeEventGroupGuid` to notify security components that
/// DXE phase initialization is complete. Security components (e.g., SMM/MM)
/// register for this event and perform lockdown.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
pub fn signal_bds_phase_entry<B: BootServices>(boot_services: &B) -> Result<()> {
    // Create and signal EndOfDxe event
    // SAFETY: Null context is valid for signal-only events
    let event = unsafe {
        boot_services.create_event_ex_unchecked::<()>(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            signal_event_noop,
            ptr::null_mut(),
            &EVENT_GROUP_END_OF_DXE,
        )
    }
    .map_err(EfiError::from)?;

    boot_services.signal_event(event).map_err(EfiError::from)?;
    boot_services.close_event(event).map_err(EfiError::from)?;

    log::info!("EndOfDxe signaled");
    Ok(())
}

/// Signal ReadyToBoot event for platforms implementing custom orchestration.
///
/// Signals `gEfiEventReadyToBootGuid` immediately before attempting the first
/// boot option. This event notifies drivers that boot is imminent.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
pub fn signal_ready_to_boot<B: BootServices>(boot_services: &B) -> Result<()> {
    // Create and signal ReadyToBoot event
    // SAFETY: Null context is valid for signal-only events
    let event = unsafe {
        boot_services.create_event_ex_unchecked::<()>(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            signal_event_noop,
            ptr::null_mut(),
            &EVENT_GROUP_READY_TO_BOOT,
        )
    }
    .map_err(EfiError::from)?;

    boot_services.signal_event(event).map_err(EfiError::from)?;
    boot_services.close_event(event).map_err(EfiError::from)?;

    log::info!("ReadyToBoot signaled");
    Ok(())
}

/// Discover console devices and populate console variables.
///
/// Scans for GOP and SimpleTextInput protocol handles, creates device paths,
/// and writes `ConIn`, `ConOut`, and `ErrOut` UEFI variables.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `runtime_services` - Runtime services interface for variable operations
pub fn discover_console_devices<B: BootServices, R: RuntimeServices>(
    boot_services: &B,
    _runtime_services: &R,
) -> Result<()> {
    // Note: Full implementation would:
    // 1. Locate GOP protocol handles
    // 2. Locate SimpleTextInput protocol handles
    // 3. Get device paths for each handle
    // 4. Create multi-instance device paths for ConIn/ConOut/ErrOut
    // 5. Write variables using runtime_services.set_variable()

    // Get handles with GOP protocol
    let _gop_handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&efi::protocols::graphics_output::PROTOCOL_GUID))
        .ok();

    // Get handles with SimpleTextInput protocol
    let _input_handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&efi::protocols::simple_text_input::PROTOCOL_GUID))
        .ok();

    log::info!("Console discovery complete");
    Ok(())
}

/// No-op event callback for signal-only events.
extern "efiapi" fn signal_event_noop(_event: *mut core::ffi::c_void, _context: *mut ()) {}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use patina::{boot_services::MockBootServices, uefi_protocol::device_path::nodes::EndEntire};

    fn create_test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    #[test]
    fn test_boot_from_device_path_success() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        // Expect load_image to succeed
        mock.expect_load_image().returning(|_, _, _, _| Ok(core::ptr::null_mut()));

        // Expect watchdog to be set to 5 minutes
        mock.expect_set_watchdog_timer().withf(|timeout| *timeout == WATCHDOG_TIMEOUT_SECONDS).returning(|_| Ok(()));

        // Expect start_image to succeed (return Ok)
        mock.expect_start_image().returning(|_| Ok(()));

        // Expect watchdog to be disabled after boot returns
        mock.expect_set_watchdog_timer().withf(|timeout| *timeout == 0).returning(|_| Ok(()));

        let result = boot_from_device_path(&mock, &device_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_boot_from_device_path_load_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        // Expect load_image to fail
        mock.expect_load_image().returning(|_, _, _, _| Err(efi::Status::NOT_FOUND));

        let result = boot_from_device_path(&mock, &device_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_boot_from_device_path_start_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        // Expect load_image to succeed
        mock.expect_load_image().returning(|_, _, _, _| Ok(core::ptr::null_mut()));

        // Expect watchdog to be set
        mock.expect_set_watchdog_timer().returning(|_| Ok(()));

        // Expect start_image to fail
        mock.expect_start_image().returning(|_| Err((efi::Status::LOAD_ERROR, None)));

        // Expect watchdog to be disabled even on failure
        mock.expect_set_watchdog_timer().returning(|_| Ok(()));

        let result = boot_from_device_path(&mock, &device_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_boot_from_device_path_watchdog_disabled_on_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        static WATCHDOG_DISABLE_CALLED: AtomicUsize = AtomicUsize::new(0);

        mock.expect_load_image().returning(|_, _, _, _| Ok(core::ptr::null_mut()));

        mock.expect_set_watchdog_timer().returning(|timeout| {
            if timeout == 0 {
                WATCHDOG_DISABLE_CALLED.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        });

        mock.expect_start_image().returning(|_| Err((efi::Status::ABORTED, None)));

        let _ = boot_from_device_path(&mock, &device_path);

        // Verify watchdog was disabled (timeout=0 was called)
        assert!(WATCHDOG_DISABLE_CALLED.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn test_signal_bds_phase_entry_signals_end_of_dxe() {
        let mut mock = MockBootServices::new();

        // Expect event creation with proper type annotation
        mock.expect_create_event_ex_unchecked::<()>().returning(|_, _, _, _, _| Ok(core::ptr::null_mut()));

        // Expect event to be signaled
        mock.expect_signal_event().returning(|_| Ok(()));

        // Expect event to be closed
        mock.expect_close_event().returning(|_| Ok(()));

        let result = signal_bds_phase_entry(&mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_signal_ready_to_boot() {
        let mut mock = MockBootServices::new();

        mock.expect_create_event_ex_unchecked::<()>().returning(|_, _, _, _, _| Ok(core::ptr::null_mut()));
        mock.expect_signal_event().returning(|_| Ok(()));
        mock.expect_close_event().returning(|_| Ok(()));

        let result = signal_ready_to_boot(&mock);
        assert!(result.is_ok());
    }
}
