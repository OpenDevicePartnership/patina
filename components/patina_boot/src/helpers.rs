//! Helper functions for boot orchestration.
//!
//! This module provides helper functions for platforms implementing custom boot flows.
//! The [`SimpleBootManager`](crate::SimpleBootManager) uses these internally, and
//! platforms can use them directly for custom orchestration.
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
    device_path::{
        node_defs::{DevicePathType, HardDrive, MediaSubType},
        paths::{DevicePath, DevicePathBuf},
    },
    error::{EfiError, Result},
    guids::{EFI_GLOBAL_VARIABLE, EVENT_GROUP_END_OF_DXE},
    runtime_services::RuntimeServices,
};
use r_efi::{
    efi,
    protocols::simple_text_input,
    system::{EVENT_GROUP_READY_TO_BOOT, VARIABLE_BOOTSERVICE_ACCESS, VARIABLE_NON_VOLATILE, VARIABLE_RUNTIME_ACCESS},
};

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
    // Expand partial device paths to full paths
    let full_path = if is_partial_device_path(device_path.as_ref()) {
        expand_device_path(boot_services, device_path.as_ref())?
    } else {
        device_path.clone()
    };

    // Load the image
    let device_path_ptr = full_path.as_ref() as *const _ as *mut efi::protocols::device_path::Protocol;
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

/// Discover console devices and write ConIn, ConOut, and ErrOut UEFI variables.
///
/// Enumerates all handles supporting console-related protocols (SimpleTextInput,
/// SimpleTextOutput, GraphicsOutput) and writes multi-instance device paths to the
/// corresponding UEFI global variables. These variables allow UEFI applications and
/// OS loaders to discover available console devices.
///
/// Individual variable failures are non-fatal — the function logs a warning and
/// continues with the remaining variables.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface for handle enumeration
/// * `runtime_services` - Runtime services interface for writing UEFI variables
pub fn discover_console_devices<B: BootServices, R: RuntimeServices>(
    boot_services: &B,
    runtime_services: &R,
) -> Result<()> {
    let attrs = VARIABLE_NON_VOLATILE | VARIABLE_BOOTSERVICE_ACCESS | VARIABLE_RUNTIME_ACCESS;

    // UTF-16 null-terminated variable names
    let con_in_name: &[u16] = &[b'C' as u16, b'o' as u16, b'n' as u16, b'I' as u16, b'n' as u16, 0];
    let con_out_name: &[u16] = &[b'C' as u16, b'o' as u16, b'n' as u16, b'O' as u16, b'u' as u16, b't' as u16, 0];
    let err_out_name: &[u16] = &[b'E' as u16, b'r' as u16, b'r' as u16, b'O' as u16, b'u' as u16, b't' as u16, 0];

    let console_vars: &[(&str, &[u16], &[&'static efi::Guid])] = &[
        ("ConIn", con_in_name, &[&simple_text_input::PROTOCOL_GUID]),
        (
            "ConOut",
            con_out_name,
            &[&efi::protocols::simple_text_output::PROTOCOL_GUID, &efi::protocols::graphics_output::PROTOCOL_GUID],
        ),
        ("ErrOut", err_out_name, &[&efi::protocols::simple_text_output::PROTOCOL_GUID]),
    ];

    for &(label, name, guids) in console_vars {
        let device_path = build_multi_instance_device_path(boot_services, guids);

        if let Some(dp) = device_path {
            let bytes = dp.as_ref().as_bytes().to_vec();

            if let Err(e) = runtime_services.set_variable(name, &EFI_GLOBAL_VARIABLE, attrs, &bytes) {
                log::error!("{label}: failed to set variable: {e:?}");
            }
        }
    }

    Ok(())
}

/// Build a multi-instance device path from all handles supporting the given protocols.
///
/// For each protocol GUID, locates all handles via `locate_handle_buffer` and extracts
/// their device paths, combining them into a single multi-instance device path separated
/// by `EndInstance` nodes.
///
fn build_multi_instance_device_path<B: BootServices>(
    boot_services: &B,
    protocol_guids: &[&'static efi::Guid],
) -> Option<DevicePathBuf> {
    let mut result: Option<DevicePathBuf> = None;
    let mut seen_handles: Vec<efi::Handle> = Vec::new();

    for &guid in protocol_guids {
        let handles = match boot_services.locate_handle_buffer(HandleSearchType::ByProtocol(guid)) {
            Ok(handles) => handles,
            Err(_) => continue,
        };

        for &handle in handles.iter() {
            if seen_handles.contains(&handle) {
                continue;
            }
            seen_handles.push(handle);
            // SAFETY: handle is valid from locate_handle_buffer, requesting device path protocol.
            let dp_ptr = match unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) }
            {
                Ok(ptr) => ptr,
                Err(_) => continue,
            };

            // SAFETY: The device path pointer comes from a valid protocol interface.
            let device_path = match unsafe { DevicePath::try_from_ptr(dp_ptr as *const _ as *const u8) } {
                Ok(dp) => dp,
                Err(_) => continue,
            };

            match &mut result {
                Some(multi) => multi.append_device_path_instances(device_path),
                None => result = Some(DevicePathBuf::from(device_path)),
            }
        }
    }

    result
}

/// No-op event callback for signal-only events.
#[coverage(off)] // Extern callback - tested via integration tests
extern "efiapi" fn signal_event_noop(_event: *mut core::ffi::c_void, _context: *mut ()) {}

/// Returns true if the device path is a partial (short-form) device path.
///
/// Full device paths start with Hardware (type 1) or ACPI (type 2) root nodes,
/// representing the complete path from system root to device.
///
/// Partial device paths start with other node types (e.g., Media type 4 for HD nodes,
/// Messaging type 3 for NVMe without root) and must be expanded by matching against
/// the current device topology before they can be used for booting.
///
/// # Arguments
///
/// * `device_path` - The device path to check
///
/// # Returns
///
/// `true` if the device path is partial (does not start with Hardware or ACPI node),
/// `false` if it's a full device path or empty.
pub fn is_partial_device_path(device_path: &DevicePath) -> bool {
    let Some(first_node) = device_path.iter().next() else {
        return false;
    };

    // Full paths start with Hardware (1) or ACPI (2) nodes.
    // Media FV/FvFile paths are also complete — LoadImage resolves them directly.
    // Partial paths start with Media HardDrive (4/1), Messaging (3), or other nodes.
    let node_type = first_node.header.r#type;
    let node_subtype = first_node.header.sub_type;

    if node_type == DevicePathType::Media as u8
        && (node_subtype == MediaSubType::PiwgFirmwareFile as u8
            || node_subtype == MediaSubType::PiwgFirmwareVolume as u8)
    {
        return false;
    }

    node_type != DevicePathType::Hardware as u8
        && node_type != DevicePathType::Acpi as u8
        && node_type != DevicePathType::End as u8
}

/// Expands a partial device path to a full device path by matching against device topology.
///
/// This function takes a partial (short-form) device path and finds the corresponding
/// full device path by enumerating all device handles and matching against the partial
/// path's identifying characteristics (e.g., partition GUID for HardDrive nodes).
///
/// If the input is already a full device path (starts with Hardware or ACPI node),
/// it is returned unchanged.
///
/// # Arguments
///
/// * `boot_services` - Boot services for handle enumeration
/// * `partial_path` - The device path to expand (may be full or partial)
///
/// # Returns
///
/// * `Ok(DevicePathBuf)` - The expanded full device path, or the original if already full
/// * `Err(EfiError::InvalidParameter)` - If the partial path is empty
/// * `Err(EfiError::NotFound)` - If no matching device was found in the topology
///
/// # Supported Partial Path Types
///
/// Currently supports:
/// - **HardDrive (Media type 4, subtype 1)**: Matches by partition signature and signature type
///
/// Future enhancements may add support for:
/// - FilePath-only paths (require filesystem enumeration)
/// - Messaging node paths without root
pub fn expand_device_path<B: BootServices>(boot_services: &B, partial_path: &DevicePath) -> Result<DevicePathBuf> {
    // Return unchanged if already a full path
    if !is_partial_device_path(partial_path) {
        return Ok(partial_path.into());
    }

    // Parse the HardDrive node from the partial path to extract the partition signature.
    let target_sig = partial_path.iter().find_map(|node| {
        let hd = HardDrive::try_from_node(&node)?;
        Some((hd.partition_signature.to_vec(), hd.signature_type))
    });

    let (target_sig, target_sig_type) = match target_sig {
        Some(s) => s,
        None => {
            log::error!("expand_device_path: no HardDrive node found in partial path");
            return Err(EfiError::InvalidParameter);
        }
    };

    // Collect remaining nodes after the HardDrive node in the partial path.
    // Typically this is the FilePath node (e.g., \EFI\Boot\BOOTX64.efi).
    let remaining_nodes: Vec<_> = {
        let mut past_hd = false;
        partial_path
            .iter()
            .filter(move |node| {
                if past_hd && node.header.r#type != DevicePathType::End as u8 {
                    return true;
                }
                if HardDrive::try_from_node(node).is_some() {
                    past_hd = true;
                }
                false
            })
            .collect()
    };

    // Enumerate all handles with DevicePath protocol
    let handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&efi::protocols::device_path::PROTOCOL_GUID))
        .map_err(EfiError::from)?;

    // Search for a handle whose device path contains a matching HardDrive node
    for &handle in handles.iter() {
        // SAFETY: handle is valid from locate_handle_buffer, requesting device path protocol.
        let dp_ptr = match unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) } {
            Ok(ptr) => ptr,
            Err(_) => continue,
        };

        // SAFETY: The device path pointer comes from a valid protocol interface.
        let handle_path = match unsafe { DevicePath::try_from_ptr(dp_ptr as *const _ as *const u8) } {
            Ok(path) => path,
            Err(_) => continue,
        };

        // Walk the handle's device path looking for a matching HardDrive node.
        // Collect nodes up to and including the HD node so we truncate any nodes
        // beyond HD (e.g., filesystem handles may extend past HD with FilePath nodes
        // that would conflict with remaining_nodes from the partial path).
        let mut prefix_nodes = Vec::new();
        let mut found = false;
        for node in handle_path.iter() {
            if node.header.r#type == DevicePathType::End as u8 {
                break;
            }
            let is_match = HardDrive::try_from_node(&node).is_some_and(|hd| {
                hd.partition_signature == target_sig.as_slice() && hd.signature_type == target_sig_type
            });
            prefix_nodes.push(node);
            if is_match {
                found = true;
                break;
            }
        }

        if found {
            let mut result = DevicePathBuf::from_device_path_node_iter(prefix_nodes.into_iter());
            let remaining_dp = DevicePathBuf::from_device_path_node_iter(remaining_nodes.into_iter());
            result.append_device_path(&remaining_dp);
            return Ok(result);
        }
    }

    log::error!("expand_device_path: no matching partition found in device topology");
    Err(EfiError::NotFound)
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use alloc::boxed::Box;

    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use patina::{
        boot_services::{MockBootServices, boxed::BootServicesBox},
        device_path::node_defs::{Acpi, EndEntire, HardDrive},
    };

    fn create_test_device_path() -> DevicePathBuf {
        // Create a full device path (starts with ACPI node) so it won't trigger partial path expansion
        DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter())
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

    // Tests for partial device path expansion

    use patina::device_path::node_defs::Pci;

    /// Helper to build a partial device path starting with HD node.
    fn build_partial_hd_path(guid: [u8; 16]) -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter([HardDrive::new_gpt(1, 2048, 1000000, guid)].into_iter())
    }

    /// Helper to build a full device path starting with ACPI root.
    fn build_full_path_with_hd(guid: [u8; 16]) -> DevicePathBuf {
        let mut path = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
        let pci_path = DevicePathBuf::from_device_path_node_iter([Pci { function: 0, device: 0x1D }].into_iter());
        path.append_device_path(&pci_path);
        let hd_path =
            DevicePathBuf::from_device_path_node_iter([HardDrive::new_gpt(1, 2048, 1000000, guid)].into_iter());
        path.append_device_path(&hd_path);
        path
    }

    #[test]
    fn test_is_partial_with_hd_node() {
        let partial = build_partial_hd_path([0xAA; 16]);
        assert!(is_partial_device_path(&partial));
    }

    #[test]
    fn test_is_partial_with_full_path_acpi() {
        let full = build_full_path_with_hd([0xAA; 16]);
        assert!(!is_partial_device_path(&full));
    }

    #[test]
    fn test_is_partial_empty_path() {
        let empty = DevicePathBuf::from_device_path_node_iter([EndEntire].into_iter());
        // EndEntire is type 0x7F (End) - an end-only path is not a meaningful partial path
        assert!(!is_partial_device_path(&empty));
    }

    #[test]
    fn test_expand_already_full_returns_unchanged() {
        let full = build_full_path_with_hd([0xAA; 16]);

        let mock = MockBootServices::new();
        // No mock setup needed since full paths return early

        let result = expand_device_path(&mock, &full);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), full);
    }

    #[test]
    fn test_expand_partial_path_locate_fails() {
        let partial = build_partial_hd_path([0xBB; 16]);

        let mut mock = MockBootServices::new();

        // locate_handle_buffer fails — no device path handles available
        mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));

        let result = expand_device_path(&mock, &partial);
        assert!(result.is_err(), "expand_device_path should fail when no handles found");
    }

    /// Helper: leaked MockBootServices whose only job is to accept free_pool calls
    /// when a BootServicesBox is dropped.
    fn leaked_boot_services_for_box() -> &'static MockBootServices {
        Box::leak(Box::new({
            let mut m = MockBootServices::new();
            m.expect_free_pool().returning(|_| Ok(()));
            m
        }))
    }

    /// Helper: build a BootServicesBox<[Handle]> for use in test mock returns.
    ///
    /// Handle storage is intentionally leaked; the mock's free_pool is a no-op.
    fn mock_handle_buffer(
        handle_addrs: &[usize],
        boot_services: &'static MockBootServices,
    ) -> BootServicesBox<'static, [efi::Handle], MockBootServices> {
        let handles: Vec<efi::Handle> = handle_addrs.iter().map(|&a| a as efi::Handle).collect();
        let leaked = handles.leak();
        // SAFETY: leaked is a valid pointer+length from Vec::leak.
        unsafe { BootServicesBox::from_raw_parts_mut(leaked.as_mut_ptr(), leaked.len(), boot_services) }
    }

    #[test]
    fn test_build_multi_instance_device_path_single_protocol() {
        let dp = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
        let dp_addr = dp.as_ref() as *const DevicePath as *const u8 as usize;
        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning a pointer to a valid DevicePathBuf kept alive by the test.
        unsafe {
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
                .returning(move |_| Ok((dp_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap()));
        }

        let result = build_multi_instance_device_path(&mock, &[&simple_text_input::PROTOCOL_GUID]);
        assert!(result.is_some());

        let multi = result.unwrap();
        assert_eq!(multi.as_ref().as_bytes(), dp.as_ref().as_bytes());
    }

    #[test]
    fn test_build_multi_instance_device_path_deduplicates_handles() {
        let dp = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
        let dp_addr = dp.as_ref() as *const DevicePath as *const u8 as usize;

        // Same handle for both protocols — should appear only once
        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning pointer to valid DevicePathBuf.
        unsafe {
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
                .times(1) // Called only once despite two GUIDs — handle is deduplicated
                .returning(move |_| Ok((dp_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap()));
        }

        let result = build_multi_instance_device_path(
            &mock,
            &[&efi::protocols::simple_text_output::PROTOCOL_GUID, &efi::protocols::graphics_output::PROTOCOL_GUID],
        );
        assert!(result.is_some());
    }

    #[test]
    fn test_build_multi_instance_device_path_handle_protocol_failure() {
        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // handle_protocol fails — handle has no device path
        mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
            .returning(|_| Err(efi::Status::UNSUPPORTED));

        let result = build_multi_instance_device_path(&mock, &[&simple_text_input::PROTOCOL_GUID]);
        assert!(result.is_none());
    }

    #[test]
    fn test_discover_console_devices_sets_variables() {
        use patina::runtime_services::MockRuntimeServices;

        let dp = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
        let dp_addr = dp.as_ref() as *const DevicePath as *const u8 as usize;
        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut boot_mock = MockBootServices::new();
        let mut runtime_mock = MockRuntimeServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning pointer to valid DevicePathBuf.
        unsafe {
            boot_mock
                .expect_handle_protocol::<efi::protocols::device_path::Protocol>()
                .returning(move |_| Ok((dp_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap()));
        }

        runtime_mock
            .expect_set_variable::<Vec<u8>>()
            .times(3) // ConIn, ConOut, ErrOut
            .returning(|_, _, _, _| Ok(()));

        let result = discover_console_devices(&boot_mock, &runtime_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_discover_console_devices_set_variable_failure_is_non_fatal() {
        use patina::runtime_services::MockRuntimeServices;

        let dp = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
        let dp_addr = dp.as_ref() as *const DevicePath as *const u8 as usize;
        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut boot_mock = MockBootServices::new();
        let mut runtime_mock = MockRuntimeServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning pointer to valid DevicePathBuf.
        unsafe {
            boot_mock
                .expect_handle_protocol::<efi::protocols::device_path::Protocol>()
                .returning(move |_| Ok((dp_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap()));
        }

        // set_variable fails — should still return Ok
        runtime_mock.expect_set_variable::<Vec<u8>>().returning(|_, _, _, _| Err(efi::Status::OUT_OF_RESOURCES));

        let result = discover_console_devices(&boot_mock, &runtime_mock);
        assert!(result.is_ok(), "set_variable failure should be non-fatal");
    }

    // Tests for connect_all

    #[test]
    fn test_connect_all_stabilizes_after_two_iterations() {
        let box_mock = leaked_boot_services_for_box();
        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let mut mock = MockBootServices::new();

        // First call returns 1 handle, second returns 2 (new device discovered),
        // third returns 2 again (stabilized).
        mock.expect_locate_handle_buffer().returning(move |_| {
            let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
            let count = if n == 0 { 1 } else { 2 };
            let addrs: Vec<usize> = (1..=count).collect();
            Ok(mock_handle_buffer(&addrs, box_mock))
        });

        mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let result = connect_all(&mock);
        assert!(result.is_ok());
        // 3 iterations: 1 handle, 2 handles, 2 handles (stabilized)
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    // Tests for detect_hotkey_from_handles

    /// Test extern "efiapi" read_key_stroke that returns F12 then NOT_READY.
    extern "efiapi" fn mock_read_key_stroke_f12(
        _this: *mut simple_text_input::Protocol,
        key: *mut simple_text_input::InputKey,
    ) -> efi::Status {
        static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            // SAFETY: key is a valid pointer provided by the caller.
            unsafe {
                (*key).scan_code = 0x16; // F12
                (*key).unicode_char = 0;
            }
            efi::Status::SUCCESS
        } else {
            efi::Status::NOT_READY
        }
    }

    /// Test extern "efiapi" read_key_stroke that always returns NOT_READY.
    extern "efiapi" fn mock_read_key_stroke_empty(
        _this: *mut simple_text_input::Protocol,
        _key: *mut simple_text_input::InputKey,
    ) -> efi::Status {
        efi::Status::NOT_READY
    }

    /// Test extern "efiapi" reset (unused, required for struct completeness).
    extern "efiapi" fn mock_reset(_this: *mut simple_text_input::Protocol, _extended: efi::Boolean) -> efi::Status {
        efi::Status::SUCCESS
    }

    #[test]
    fn test_detect_hotkey_from_handles_finds_matching_key() {
        let mut protocol = simple_text_input::Protocol {
            reset: mock_reset,
            read_key_stroke: mock_read_key_stroke_f12,
            wait_for_key: ptr::null_mut(),
        };
        let protocol_addr = &mut protocol as *mut _ as usize;

        let mut mock = MockBootServices::new();

        // SAFETY: Test code — returning pointer to a valid Protocol kept alive by the test.
        unsafe {
            mock.expect_handle_protocol::<simple_text_input::Protocol>()
                .returning(move |_| Ok((protocol_addr as *mut simple_text_input::Protocol).as_mut().unwrap()));
        }

        let handle: efi::Handle = 1usize as efi::Handle;
        // SAFETY: handle is a test value, mock returns a valid protocol.
        let result = unsafe { detect_hotkey_from_handles(&mock, &[handle], 0x16) };
        assert!(result, "F12 hotkey should be detected");
    }

    #[test]
    fn test_detect_hotkey_from_handles_no_keys_buffered() {
        let mut protocol = simple_text_input::Protocol {
            reset: mock_reset,
            read_key_stroke: mock_read_key_stroke_empty,
            wait_for_key: ptr::null_mut(),
        };
        let protocol_addr = &mut protocol as *mut _ as usize;

        let mut mock = MockBootServices::new();

        // SAFETY: Test code — returning pointer to a valid Protocol kept alive by the test.
        unsafe {
            mock.expect_handle_protocol::<simple_text_input::Protocol>()
                .returning(move |_| Ok((protocol_addr as *mut simple_text_input::Protocol).as_mut().unwrap()));
        }

        let handle: efi::Handle = 1usize as efi::Handle;
        // SAFETY: handle is a test value, mock returns a valid protocol.
        let result = unsafe { detect_hotkey_from_handles(&mock, &[handle], 0x16) };
        assert!(!result, "No keys in buffer should return false");
    }

    #[test]
    fn test_detect_hotkey_from_handles_protocol_failure() {
        let mut mock = MockBootServices::new();

        // handle_protocol fails — no SimpleTextInput on this handle
        mock.expect_handle_protocol::<simple_text_input::Protocol>().returning(|_| Err(efi::Status::UNSUPPORTED));

        let handle: efi::Handle = 1usize as efi::Handle;
        // SAFETY: handle is a test value, mock returns Err.
        let result = unsafe { detect_hotkey_from_handles(&mock, &[handle], 0x16) };
        assert!(!result, "Protocol failure should return false");
    }

    // Tests for expand_device_path success path

    #[test]
    fn test_expand_partial_path_matches_partition() {
        let guid = [0xAA; 16];
        let partial = build_partial_hd_path(guid);
        let full = build_full_path_with_hd(guid);
        let full_addr = full.as_ref() as *const DevicePath as *const u8 as usize;

        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning pointer to valid DevicePathBuf.
        unsafe {
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
                .returning(move |_| Ok((full_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap()));
        }

        let result = expand_device_path(&mock, &partial);
        assert!(result.is_ok(), "Should find matching partition");

        // The expanded path should start with ACPI root (from the full path)
        let expanded = result.unwrap();
        assert!(!is_partial_device_path(&expanded), "Expanded path should be a full path");
    }

    #[test]
    fn test_expand_partial_path_no_matching_partition() {
        let partial = build_partial_hd_path([0xAA; 16]);
        // Full path has a different GUID — no match
        let full = build_full_path_with_hd([0xBB; 16]);
        let full_addr = full.as_ref() as *const DevicePath as *const u8 as usize;

        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning pointer to valid DevicePathBuf.
        unsafe {
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
                .returning(move |_| Ok((full_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap()));
        }

        let result = expand_device_path(&mock, &partial);
        assert!(result.is_err(), "Should fail when no partition matches");
    }

    #[test]
    /// Verify that expansion truncates the handle's device path at the matched node,
    /// discarding any trailing nodes. This prevents duplication when a handle's path
    /// extends past the node we match on (e.g., filesystem handles that include
    /// FilePath nodes after HD). The same principle applies to any future partial
    /// path types — we must only use the prefix up to the matched node.
    fn test_expand_partial_path_truncates_at_matched_node() {
        use patina::device_path::node_defs::FilePath;

        let guid = [0xAA; 16];
        // Partial path: HD()/FilePath(\EFI\Boot\BOOTX64.efi)
        let mut partial =
            DevicePathBuf::from_device_path_node_iter([HardDrive::new_gpt(1, 2048, 1000000, guid)].into_iter());
        let fp = DevicePathBuf::from_device_path_node_iter([FilePath::new("\\EFI\\Boot\\BOOTX64.efi")].into_iter());
        partial.append_device_path(&fp);

        // Handle has nodes beyond the matched node — simulates a handle whose device
        // path extends past the point we match on (e.g., a filesystem handle).
        // ACPI/PCI/HD()/FilePath(\some\other\path)
        let mut handle_dp = build_full_path_with_hd(guid);
        let extra_fp = DevicePathBuf::from_device_path_node_iter([FilePath::new("\\some\\other\\path")].into_iter());
        handle_dp.append_device_path(&extra_fp);

        let handle_dp_addr = handle_dp.as_ref() as *const DevicePath as *const u8 as usize;
        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // SAFETY: Test code — returning pointer to valid DevicePathBuf.
        unsafe {
            mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>().returning(move |_| {
                Ok((handle_dp_addr as *mut efi::protocols::device_path::Protocol).as_mut().unwrap())
            });
        }

        let result = expand_device_path(&mock, &partial);
        assert!(result.is_ok());

        let expanded = result.unwrap();
        let node_count = expanded.as_ref().node_count();
        assert_eq!(node_count, 5, "Should have prefix(3) + remaining(1) + End, got {node_count} nodes");
    }

    #[test]
    fn test_expand_partial_path_handle_protocol_failure() {
        let partial = build_partial_hd_path([0xAA; 16]);

        let handle_addr: usize = 1;
        let box_mock = leaked_boot_services_for_box();

        let mut mock = MockBootServices::new();

        mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[handle_addr], box_mock)));

        // handle_protocol fails
        mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
            .returning(|_| Err(efi::Status::UNSUPPORTED));

        let result = expand_device_path(&mock, &partial);
        assert!(result.is_err(), "Should fail when handle_protocol fails");
    }
}
