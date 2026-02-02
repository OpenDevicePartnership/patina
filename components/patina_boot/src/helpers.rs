//! Helper functions for boot orchestration.
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
use r_efi::{efi, protocols::simple_text_input, system::EVENT_GROUP_READY_TO_BOOT};

/// Watchdog timeout in seconds per UEFI Specification Section 3.1.2.
const WATCHDOG_TIMEOUT_SECONDS: usize = 300; // 5 minutes

/// Check if a hotkey was pressed during boot.
///
/// Reads any pending keystrokes from all SimpleTextInput protocol instances
/// and returns `true` if any key matches the specified scancode.
///
/// This is a non-blocking check that consumes any buffered keystrokes.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `hotkey_scancode` - The scancode to check for (e.g., 0x16 for F12)
///
/// # Returns
///
/// Returns `true` if the hotkey was detected, `false` otherwise.
pub fn detect_hotkey<B: BootServices>(boot_services: &B, hotkey_scancode: u16) -> bool {
    // Locate all SimpleTextInput handles
    let handles =
        match boot_services.locate_handle_buffer(HandleSearchType::ByProtocol(&simple_text_input::PROTOCOL_GUID)) {
            Ok(handles) => handles,
            Err(_) => return false,
        };

    // SAFETY: Handles are valid from locate_handle_buffer, protocol_ptr is valid from handle_protocol
    unsafe { detect_hotkey_from_handles(boot_services, &handles, hotkey_scancode) }
}

/// Inner hotkey detection loop over handles.
///
/// This function is separated from `detect_hotkey` because it uses raw protocol
/// function pointers that cannot be unit tested with mocks. Integration tests
/// verify this code path on real hardware/emulators.
///
/// # Safety
///
/// - `handles` must contain valid handles obtained from `locate_handle_buffer`
/// - Each handle must support the `SimpleTextInput` protocol for `handle_protocol` to succeed
#[coverage(off)] // Uses raw protocol function pointers - tested via integration tests
unsafe fn detect_hotkey_from_handles<B: BootServices>(
    boot_services: &B,
    handles: &[efi::Handle],
    hotkey_scancode: u16,
) -> bool {
    for &handle in handles.iter() {
        // Get the protocol interface for this handle
        // SAFETY: handle is valid per function contract (from locate_handle_buffer)
        let protocol_ptr = match unsafe { boot_services.handle_protocol::<simple_text_input::Protocol>(handle) } {
            Ok(ptr) => ptr,
            Err(_) => continue,
        };

        // Read any pending keystrokes (non-blocking)
        // The protocol will return NOT_READY if no key is available
        loop {
            let mut key = simple_text_input::InputKey::default();
            let status = (protocol_ptr.read_key_stroke)(protocol_ptr, &mut key);

            if status == efi::Status::SUCCESS {
                if key.scan_code == hotkey_scancode {
                    return true;
                }
                // Key didn't match, continue reading to drain buffer
            } else {
                // NOT_READY or error - no more keys in buffer
                break;
            }
        }
    }

    false
}

/// Load and start a boot image with UEFI spec compliance.
///
/// Enables a 5-minute watchdog timer before `StartImage()` per UEFI Specification
/// Section 3.1.2. Disables watchdog when boot returns control.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `parent_handle` - Parent image handle for the loaded image (typically the calling image's handle)
/// * `device_path` - Device path to the boot image
///
/// # Returns
///
/// Returns `Ok(())` if the boot image was successfully started (which typically
/// means it returned control). Returns an error if loading or starting fails.
pub fn boot_from_device_path<B: BootServices>(
    boot_services: &B,
    parent_handle: efi::Handle,
    device_path: &DevicePathBuf,
) -> Result<()> {
    // Load the image
    let device_path_ptr = device_path.as_ref() as *const _ as *mut efi::protocols::device_path::Protocol;
    let image_handle = match boot_services.load_image(true, parent_handle, device_path_ptr, None) {
        Ok(handle) => handle,
        Err(status) => {
            log::error!("LoadImage failed with status: {:?}", status);
            return Err(EfiError::from(status));
        }
    };

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

/// Connect all controllers recursively for device enumeration.
///
/// Connects all handles in the system recursively until the device topology
/// stabilizes (no new handles are created).
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
///
/// # Returns
///
/// Returns `Ok(())` when device topology enumeration is complete.
///
/// # Coverage
///
/// This function is marked `#[coverage(off)]` because `BootServicesBox` return
/// values from `locate_handle_buffer` cannot be created in unit tests. The
/// function is tested via integration tests on real hardware/emulators.
#[coverage(off)] // BootServicesBox return type cannot be mocked - tested via integration tests
pub fn connect_all<B: BootServices>(boot_services: &B) -> Result<()> {
    // Loop until the number of handles stabilizes, indicating device topology is complete.
    // This is needed because connecting a PCI bus creates new handles for PCI devices,
    // which then need to be connected to bind drivers like NVMe, which creates namespace
    // handles, etc.
    const MAX_ITERATIONS: usize = 10;
    let mut prev_handle_count = 0;

    for _iteration in 0..MAX_ITERATIONS {
        // Get all handles in the system
        let handles = boot_services.locate_handle_buffer(HandleSearchType::AllHandle).map_err(EfiError::from)?;
        let current_handle_count = handles.len();

        // Connect each handle recursively
        for &handle in handles.iter() {
            // SAFETY: Empty driver handle list and null device path are valid per UEFI spec
            let _ = unsafe { boot_services.connect_controller(handle, Vec::new(), ptr::null_mut(), true) };
        }

        // Check if handle count has stabilized
        if current_handle_count == prev_handle_count {
            break;
        }

        prev_handle_count = current_handle_count;
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

    let signal_result = boot_services.signal_event(event);
    // Always close the event, even if signal failed
    let close_result = boot_services.close_event(event);

    signal_result.map_err(EfiError::from)?;
    close_result.map_err(EfiError::from)?;

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

    let signal_result = boot_services.signal_event(event);
    // Always close the event, even if signal failed
    let close_result = boot_services.close_event(event);

    signal_result.map_err(EfiError::from)?;
    close_result.map_err(EfiError::from)?;

    Ok(())
}

/// Discover console devices (stub implementation).
///
/// This is a placeholder that locates GOP and SimpleTextInput handles but does
/// not yet write the `ConIn`, `ConOut`, and `ErrOut` UEFI variables.
///
/// Platforms requiring full console variable support should implement this
/// functionality or use platform-specific console initialization.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `runtime_services` - Runtime services interface (unused in stub)
#[allow(unused_variables)]
pub fn discover_console_devices<B: BootServices, R: RuntimeServices>(
    boot_services: &B,
    runtime_services: &R,
) -> Result<()> {
    // Stub: Locate handles to verify protocols exist, but don't write variables.
    // Full implementation would create multi-instance device paths and write
    // ConIn/ConOut/ErrOut variables via runtime_services.set_variable().
    let _gop_handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&efi::protocols::graphics_output::PROTOCOL_GUID))
        .ok();

    let _input_handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&efi::protocols::simple_text_input::PROTOCOL_GUID))
        .ok();

    Ok(())
}

/// No-op event callback for signal-only events.
#[coverage(off)] // Extern callback - tested via integration tests
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

    fn dummy_parent_handle() -> efi::Handle {
        std::ptr::dangling_mut::<core::ffi::c_void>()
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

        let result = boot_from_device_path(&mock, dummy_parent_handle(), &device_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_boot_from_device_path_load_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        // Expect load_image to fail
        mock.expect_load_image().returning(|_, _, _, _| Err(efi::Status::NOT_FOUND));

        let result = boot_from_device_path(&mock, dummy_parent_handle(), &device_path);
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

        let result = boot_from_device_path(&mock, dummy_parent_handle(), &device_path);
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

        let _ = boot_from_device_path(&mock, dummy_parent_handle(), &device_path);

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

    #[test]
    fn test_connect_all_locate_failure() {
        let mut mock = MockBootServices::new();

        // locate_handle_buffer fails on first call
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));

        let result = connect_all(&mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_discover_console_devices_handles_missing_protocols() {
        use patina::runtime_services::MockRuntimeServices;

        let mut boot_mock = MockBootServices::new();
        let runtime_mock = MockRuntimeServices::new();

        // Protocols not found - returns error but function should still succeed
        boot_mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));

        // Function should still succeed even with no console devices
        let result = discover_console_devices(&boot_mock, &runtime_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_signal_bds_phase_entry_create_event_failure() {
        let mut mock = MockBootServices::new();

        // Event creation fails
        mock.expect_create_event_ex_unchecked::<()>().returning(|_, _, _, _, _| Err(efi::Status::OUT_OF_RESOURCES));

        let result = signal_bds_phase_entry(&mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_signal_bds_phase_entry_signal_failure() {
        let mut mock = MockBootServices::new();

        mock.expect_create_event_ex_unchecked::<()>().returning(|_, _, _, _, _| Ok(core::ptr::null_mut()));

        // Signal fails
        mock.expect_signal_event().returning(|_| Err(efi::Status::INVALID_PARAMETER));

        // close_event is always called, even on signal failure
        mock.expect_close_event().returning(|_| Ok(()));

        let result = signal_bds_phase_entry(&mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_signal_bds_phase_entry_close_event_failure() {
        let mut mock = MockBootServices::new();

        mock.expect_create_event_ex_unchecked::<()>().returning(|_, _, _, _, _| Ok(core::ptr::null_mut()));
        mock.expect_signal_event().returning(|_| Ok(()));

        // Close fails
        mock.expect_close_event().returning(|_| Err(efi::Status::INVALID_PARAMETER));

        let result = signal_bds_phase_entry(&mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_signal_ready_to_boot_create_event_failure() {
        let mut mock = MockBootServices::new();

        mock.expect_create_event_ex_unchecked::<()>().returning(|_, _, _, _, _| Err(efi::Status::OUT_OF_RESOURCES));

        let result = signal_ready_to_boot(&mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_boot_from_device_path_watchdog_set_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        mock.expect_load_image().returning(|_, _, _, _| Ok(core::ptr::null_mut()));

        // Watchdog set fails
        mock.expect_set_watchdog_timer().returning(|_| Err(efi::Status::DEVICE_ERROR));

        let result = boot_from_device_path(&mock, dummy_parent_handle(), &device_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_hotkey_no_input_handles() {
        let mut mock = MockBootServices::new();

        // No SimpleTextInput handles found
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));

        let result = detect_hotkey(&mock, 0x16); // F12
        assert!(!result);
    }
}
