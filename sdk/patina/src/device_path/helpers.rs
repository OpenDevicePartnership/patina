//! Helper functions for device path operations.
//!
//! Provides utilities for detecting and expanding partial (short-form) device paths.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{
    boot_services::BootServices,
    device_path::{
        node_defs::DevicePathType,
        paths::{DevicePath, DevicePathBuf},
    },
    error::{EfiError, Result},
};
use r_efi::efi;

/// Returns true if the device path is a partial (short-form) device path.
///
/// Full device paths start with Hardware (type 1) or ACPI (type 2) root nodes,
/// representing the complete path from system root to device.
///
/// Partial device paths start with other node types (e.g., Media type 4 for HD nodes,
/// Messaging type 3 for NVMe without root) and must be expanded by matching against
/// the current device topology before they can be used for many scenarios, including booting.
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

    // Full paths start with Hardware (1) or ACPI (2) nodes
    // Partial paths start with Media (4), Messaging (3), or other nodes
    let node_type = first_node.header.r#type;
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
pub fn expand_device_path<B: BootServices>(boot_services: &B, partial_path: &mut DevicePath) -> Result<DevicePathBuf> {
    // Return unchanged if already a full path
    if !is_partial_device_path(partial_path) {
        return Ok((&*partial_path).into());
    }

    // Use LocateDevicePath to find the handle with the best matching device path.
    // This is more efficient than enumerating all handles manually.
    let mut device_path_ptr = partial_path as *mut DevicePath as *mut u8 as *mut efi::protocols::device_path::Protocol;
    // SAFETY: device_path_ptr points to a valid device path from partial_path.
    let handle =
        unsafe { boot_services.locate_device_path(&efi::protocols::device_path::PROTOCOL_GUID, &mut device_path_ptr) }
            .map_err(EfiError::from)?;

    // Get the full device path from the matched handle
    // SAFETY: handle_protocol is safe when the handle is valid (from locate_device_path)
    // and we're requesting the device path protocol.
    let full_dp_ptr = unsafe { boot_services.handle_protocol::<efi::protocols::device_path::Protocol>(handle) }
        .map_err(EfiError::from)?;

    // SAFETY: The device path pointer comes from a valid protocol interface.
    let full_path =
        unsafe { DevicePath::try_from_ptr(full_dp_ptr as *const _ as *const u8) }.map_err(|_| EfiError::DeviceError)?;

    // Combine the full path prefix with the remaining partial path.
    // The remaining path (after the matched portion) needs to be appended.
    let mut result = DevicePathBuf::from(full_path);

    // SAFETY: device_path_ptr was updated by locate_device_path to point to the remaining path.
    let remaining_path = unsafe { DevicePath::try_from_ptr(device_path_ptr as *const u8) };
    if let Ok(remaining) = remaining_path {
        // Only append if there's a meaningful remaining path (not just EndEntire)
        if remaining.iter().any(|node| node.header.r#type != DevicePathType::End as u8) {
            result.append_device_path(&DevicePathBuf::from(remaining));
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use super::*;
    use crate::{
        boot_services::MockBootServices,
        device_path::node_defs::{Acpi, EndEntire, FilePath, HardDrive, Pci},
    };
    use alloc::boxed::Box;

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
        let mut full = build_full_path_with_hd([0xAA; 16]);
        let expected = full.clone();

        let mock = MockBootServices::new();
        // No mock setup needed since full paths return early

        let result = expand_device_path(&mock, &mut full);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_expand_partial_path_success() {
        let guid = [0xAA; 16];

        // Create the partial path: HD(1,GPT,<guid>)/File(\EFI\BOOT\BOOTX64.EFI)
        let mut partial = build_partial_hd_path(guid);
        let file_path =
            DevicePathBuf::from_device_path_node_iter([FilePath::new("\\EFI\\BOOT\\BOOTX64.EFI")].into_iter());
        partial.append_device_path(&file_path);

        // Create the full path that the handle will have (ACPI/PCI/HD)
        let full_handle_path = build_full_path_with_hd(guid);

        // Expected result: ACPI/PCI/HD/File (full path + remaining file path)
        let mut expected = full_handle_path.clone();
        expected.append_device_path(&file_path);

        // Clone the device path bytes into a Vec and leak it so we can return a pointer
        let path_ref: &DevicePath = full_handle_path.as_ref();
        // SAFETY: path_ref is a valid DevicePath reference and size() returns its exact byte length.
        let bytes: alloc::vec::Vec<u8> = unsafe {
            alloc::vec::Vec::from(core::slice::from_raw_parts(path_ref as *const _ as *const u8, path_ref.size()))
        };
        let leaked_bytes = Box::leak(bytes.into_boxed_slice());
        let full_path_ptr: usize = leaked_bytes.as_ptr() as usize;

        // Create a fake handle as usize for Send
        let fake_handle_addr: usize = 0x12345678;

        let mut mock = MockBootServices::new();

        // Mock locate_device_path to return the fake handle and update the device path pointer
        // to point to the remaining path (the FilePath node)
        mock.expect_locate_device_path().returning(move |_protocol, device_path_ptr| {
            // The device_path_ptr points to the partial path (HD/File)
            // After matching, it should point to the remaining path (File)
            // For this test, we'll advance it past the HD node to point at FilePath

            // SAFETY: Test code - we're simulating what locate_device_path does
            unsafe {
                // Read the current device path to find the HD node size
                let current_ptr = *device_path_ptr as *const u8;
                let header = current_ptr as *const efi::protocols::device_path::Protocol;
                let hd_node_size = u16::from_le_bytes([(*header).length[0], (*header).length[1]]) as usize;

                // Advance past the HD node to point to FilePath
                *device_path_ptr = current_ptr.add(hd_node_size) as *mut efi::protocols::device_path::Protocol;
            }
            Ok(fake_handle_addr as *mut core::ffi::c_void)
        });

        // Mock handle_protocol to return the full device path
        mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>().returning(move |_handle| {
            // SAFETY: Test code - returning reference to leaked bytes
            Ok(unsafe { &mut *(full_path_ptr as *mut efi::protocols::device_path::Protocol) })
        });

        let result = expand_device_path(&mock, &mut partial);
        assert!(result.is_ok(), "expand_device_path should succeed");

        let expanded = result.unwrap();
        assert_eq!(expanded, expected, "Expanded path should match expected full path with file");

        // Note: leaked_bytes is intentionally leaked for the test - in tests this is acceptable
    }

    #[test]
    fn test_expand_partial_path_not_found() {
        let mut partial = build_partial_hd_path([0xBB; 16]);

        let mut mock = MockBootServices::new();

        // Mock locate_device_path to return NOT_FOUND
        mock.expect_locate_device_path().returning(|_protocol, _device_path_ptr| Err(efi::Status::NOT_FOUND));

        let result = expand_device_path(&mock, &mut partial);
        assert!(result.is_err(), "expand_device_path should fail when device not found");
    }

    #[test]
    fn test_expand_partial_path_handle_protocol_fails() {
        let mut partial = build_partial_hd_path([0xCC; 16]);
        let fake_handle_addr: usize = 0x87654321;

        let mut mock = MockBootServices::new();

        // Mock locate_device_path to succeed
        mock.expect_locate_device_path()
            .returning(move |_protocol, _device_path_ptr| Ok(fake_handle_addr as *mut core::ffi::c_void));

        // Mock handle_protocol to fail
        mock.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
            .returning(|_handle| Err(efi::Status::UNSUPPORTED));

        let result = expand_device_path(&mock, &mut partial);
        assert!(result.is_err(), "expand_device_path should fail when handle_protocol fails");
    }
}
