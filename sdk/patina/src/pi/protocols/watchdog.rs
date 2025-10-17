//! Watchdog Architectural Protocol
//!
//! Used to implement the Boot Service SetWatchdogTimer() . The watchdog timer may be implemented in
//! software using Boot Services, or it may be implemented with specialized hardware. The protocol provides a service
//! to register a handler when the watchdog timer fires and a service to set the amount of time to wait before the
//! watchdog timer is fired.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#watchdog-timer-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

/// Watchdog Architectrural Protocol GUID
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.14.1
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x665E3FF5, 0x46CC, 0x11d4, 0x9A, 0x38, &[0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);

/// Function type definition for watchdog timer notify.
pub type WatchdogTimerNotify = extern "efiapi" fn(u64);

/// Registers a handler that is to be invoked when the watchdog timer fires.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.14.2
pub type RegisterHandler = extern "efiapi" fn(*const Protocol, WatchdogTimerNotify) -> efi::Status;

/// Sets the amount of time in the future to fire the watchdog timer.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.14.3
pub type SetTimerPeriod = extern "efiapi" fn(*const Protocol, u64) -> efi::Status;

/// Retrieves the amount of time in 100 ns units that the system will wait before firing the watchdog timer.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.14.4
pub type GetTimerPeriod = extern "efiapi" fn(*const Protocol, *mut u64) -> efi::Status;

/// Used to program the watchdog timer and optionally register a handler when the watchdog timer fires.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.14.1
#[repr(C)]
pub struct Protocol {
    pub register_handler: RegisterHandler,
    pub set_timer_period: SetTimerPeriod,
    pub get_timer_period: GetTimerPeriod,
}
