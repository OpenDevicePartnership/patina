//! Metronome Architectural Protocol
//!
//! Used to wait for ticks from a known time source in a platform. This protocol may be used to implement a simple
//! version of the Stall() Boot Service.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#metronome-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

/// Metronome Architectural Protocol GUID
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.4.1
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x26baccb2, 0x6f42, 0x11d4, 0xbc, 0xe7, &[0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81]);

/// Waits for a specified number of ticks from a known time source in a platform.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.4.2
pub type WaitForTick = extern "efiapi" fn(*const Protocol, tick_number: u32) -> efi::Status;

/// Used to wait for ticks from a known time source in a platform.
///
/// This protocol may be used to implement a simple version of the Stall() Boot Service. This protocol must be produced
/// by a boot service or runtime DXE driver and may only be consumed by the DXE Foundation and DXE drivers that produce
/// DXE Architectural Protocols.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.4.1
#[repr(C)]
pub struct Protocol {
    pub wait_for_tick: WaitForTick,
    /// The period of platform’s known time source in 100 ns units. This value on any platform must not exceed 200
    /// microseconds. The value in this field is a constant that must not be modified after the Metronome architectural
    /// protocol is installed. All consumers must treat this as a read-only field.
    pub tick_period: u32,
}
