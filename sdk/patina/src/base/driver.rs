//! UEFI Driver Binding Definitions
//!
//! Common definitions related to UEFI drivers and the UEFI Driver Binding Model.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// Disable code coverage since this module only contains constant base definitions.
#![cfg_attr(coverage, coverage(off))]

use crate::standard::efi;

/// Performs the same functions as `handle_protocol` with additional functionality.
pub const OPEN_PROTOCOL_BY_HANDLE: u32 = efi::OPEN_PROTOCOL_BY_HANDLE_PROTOCOL;

/// Used to get a protocol interface from a handle. Care must be taken when using this
/// open mode because the driver that opens a protocol interface in this manner will
/// not be informed if the protocol interface is uninstalled or reinstalled. The caller
/// is also not responsible to close the protocol interface.
pub const OPEN_PROTOCOL_GET: u32 = efi::OPEN_PROTOCOL_GET_PROTOCOL;

/// Used by a driver to test for the existence of a protocol interface on a handle.
/// Interface is optional for this attribute value, so it is ignored, and the caller should
/// only use the return status code. The caller is also not required to close the protocol
/// interface.
pub const OPEN_PROTOCOL_TEST: u32 = efi::OPEN_PROTOCOL_TEST_PROTOCOL;

/// Used by bus drivers to show that a protocol interface is being used by one of the child
/// controllers of a bus. This information is used when connecting controllers to recursively
/// connect all child controllers and when disconnecting controllers to get the list of child
/// controllers that a bus driver created.
pub const OPEN_PROTOCOL_BY_CHILD_CONTROLLER: u32 = efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER;

/// Used by a driver to gain access to a protocol interface. When this mode is used, the driver’s
/// `stop` function will be called by `disconnect_controller` if the protocol interface is
/// reinstalled or uninstalled. Once a protocol interface is opened by a driver with this attribute,
/// no other drivers will be allowed to open the same protocol interface with the
/// [`OPEN_PROTOCOL_BY_DRIVER`] attribute.
pub const OPEN_PROTOCOL_BY_DRIVER: u32 = efi::OPEN_PROTOCOL_BY_DRIVER;

/// Used by applications to gain exclusive access to a protocol interface.
///
/// Any [`OPEN_PROTOCOL_BY_DRIVER`] holders owned by other agents are first removed with
/// `disconnect_controller`. The open will fail with `ACCESS_DENIED` if any `BY_DRIVER` holder
/// remains afterward. While the interface is held, every other `BY_DRIVER` or `EXCLUSIVE` open
/// request, including a repeat request from the same agent, fails with `ACCESS_DENIED`.
///
/// The caller is not treated as a driver managing a controller (unlike in [`OPEN_PROTOCOL_BY_DRIVER_EXCLUSIVE`]):
///
/// - `ControllerHandle` is not required.
/// - `disconnect_controller` does not stop the caller.
/// - The usage is not released automatically when the interface is uninstalled or reinstalled.
///   Those requests fail with `ACCESS_DENIED` until the caller closes the protocol.
pub const OPEN_PROTOCOL_EXCLUSIVE: u32 = efi::OPEN_PROTOCOL_EXCLUSIVE;

/// Open protocol attribute that combines `EFI_OPEN_PROTOCOL_BY_DRIVER` and `EFI_OPEN_PROTOCOL_EXCLUSIVE`.
///
/// Used by a driver to gain exclusive access to a protocol interface.
///
/// Any [`OPEN_PROTOCOL_BY_DRIVER`] holders owned by other agents are first removed with
/// `disconnect_controller`. The open will fail with `ACCESS_DENIED` if any `BY_DRIVER` holder
/// remains afterward. While the interface is held, every other `BY_DRIVER` or `EXCLUSIVE` open
/// request fails with `ACCESS_DENIED`. A repeat request from the same agent for the same controller
/// returns `ALREADY_STARTED`.
///
/// The caller is treated as a driver managing the controller as with [`OPEN_PROTOCOL_BY_DRIVER`]
/// (and unlike [`OPEN_PROTOCOL_EXCLUSIVE`]).
///
/// - `ControllerHandle` is required.
/// - `disconnect_controller` can stop the caller.
/// - When the interface is uninstalled or reinstalled, `disconnect_controller` calls the driver's
///   `stop` function to release the interface.
pub const OPEN_PROTOCOL_BY_DRIVER_EXCLUSIVE: u32 = OPEN_PROTOCOL_BY_DRIVER | OPEN_PROTOCOL_EXCLUSIVE;
