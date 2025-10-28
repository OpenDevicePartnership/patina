//! Communication2 Protocol
//!
//! Provides a means of communicating between drivers outside of MM and MMI handlers inside of MM, in a way that hides
//! the implementation details regarding whether traditional or standalone MM is being used.
//!
//! See <https://uefi.org/specs/PI/1.9/V4_UEFI_Protocols.html#efi-mm-communication2-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use r_efi::efi;

/// MM Communication Protocol GUID.
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x378daedc, 0xf06b, 0x4446, 0x83, 0x14, &[0x40, 0xab, 0x93, 0x3c, 0x87, 0xa3]);

/// Sends/receives a message for a registered handler.
///
/// This protocol provides runtime services for communicating between DXE drivers and a registered MMI handler.
///
/// Usage is identical to EFI_MM_COMMUNICATION_PROTOCOL.Communicate() except for the notes below:
///
/// - Instead of passing just the physical address via the comm_buffer parameter, the caller must pass both the physical
///   and the virtual addresses of the communication buffer.
///
/// - If no virtual remapping has taken place, the physical address will be equal to the virtual address, and so the
///   caller is required to pass the same value for both parameters.
///
///  @param this                       The protocol instance.
///  @param comm_buffer_physical       Physical address of the buffer to convey into MMRAM.
///  @param comm_buffer_virtual        Virtual address of the buffer to convey into MMRAM.
///  @param comm_size                  The size of the data buffer being passed in. On exit, the size of data
///                                    being returned. Zero if the handler does not wish to reply with any data.
///                                    This parameter is optional and may be NULL.
///
///  @retval Status::SUCCESS           The message was successfully posted.
///  @retval Status::INVALID_PARAMETER The comm_buffer parameters do not refer to the same location in memory.
///  @retval Status::BAD_BUFFER_SIZE   The buffer is too large for the MM implementation.
///                                    If this error is returned, the message_length field
///                                    in the comm_buffer header or the integer pointed by
///                                    comm_size, are updated to reflect the maximum payload
///                                    size the implementation can accommodate.
///  @retval Status::ACCESS_DENIED     The communicate buffer parameters or comm_size parameter,
///                                    if not omitted, are in an address range that cannot be
///                                    accessed by the MM environment.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.9, Section IV-5.7.4
pub type Communicate2 = extern "efiapi" fn(
    this: *const Protocol,
    comm_buffer_physical: *mut c_void,
    comm_buffer_virtual: *mut c_void,
    comm_size: usize,
) -> efi::Status;

#[repr(C)]
/// MM Communication Protocol structure.
pub struct Protocol {
    /// Communicate with the MM environment (v2).
    /// See [`Communicate2`] for more details.
    pub communicate2: Communicate2,
}
