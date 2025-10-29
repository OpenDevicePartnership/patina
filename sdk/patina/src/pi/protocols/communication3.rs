//! Communication3 Protocol
//!
//! Provides a means of communicating between drivers outside of MM and MMI handlers inside of MM, for communication
//! buffer that is compliant with EFI_MM_COMMUNICATE_HEADER_V3.
//!
//! See <https://uefi.org/specs/PI/1.9/V4_UEFI_Protocols.html#efi-mm-communication3-protocol>
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
    efi::Guid::from_fields(0xf7234a14, 0x0df2, 0x46c0, 0xad, 0x28, &[0x90, 0xe6, 0xb8, 0x83, 0xa7, 0x2f]);

/// MM Communicate Header V3 GUID.
pub const COMMUNICATE_HEADER_V3_GUID: efi::Guid =
    efi::Guid::from_fields(0x68e8c853, 0x2ba9, 0x4dd7, 0x9a, 0xc0, &[0x91, 0xe1, 0x61, 0x55, 0xc9, 0x35]);

/// Sends/receives a message for a registered handler.
///
/// This protocol provides runtime services for communicating between DXE drivers and a registered MMI handler.
///
/// Usage is similar to EFI_MM_COMMUNICATION_PROTOCOL.Communicate() except for the notes below:
///
/// - Communication buffer transfer to MM core should start with self::EfiMmCommunicateHeader.
/// - With the updated header, the header_guid field is redefine as header GUID for MM core to differentiate the
///   header format.
/// - The message_guid field is moved to be after the header_guid field to allow for decent alignment and message
///   disambiguation.
/// - The message_data field is replaced with a flexible array to allow users not having to consume extra data
///   during communicate.
/// - Instead of passing just the physical address via the comm_buffer parameter, the caller must pass both the
///   physical and the virtual addresses of the communication buffer.
/// - If no virtual remapping has taken place, the physical address will be equal to the virtual address, and so the
///   caller is required to pass the same value for both parameters.
///
///  @param this                       The protocol instance.
///  @param comm_buffer_physical       Physical address of the buffer to convey into MMRAM, of which content must
///                                    start with self::EfiMmCommunicateHeader.
///  @param comm_buffer_virtual        Virtual address of the buffer to convey into MMRAM, of which content must
///                                    start with self::EfiMmCommunicateHeader.
///
///  @retval Status::SUCCESS           The message was successfully posted.
///  @retval Status::INVALID_PARAMETER comm_buffer_physical was null or comm_buffer_virtual was null.
///  @retval Status::BAD_BUFFER_SIZE   The buffer is too large for the MM implementation.
///                                    If this error is returned, the message_length field
///                                    in the comm_buffer header, is updated to reflect the maximum payload
///                                    size the implementation can accommodate.
///  @retval Status::ACCESS_DENIED     The communicate buffer parameters are in an address range that cannot be
///                                    accessed by the MM environment.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.9, Section IV-5.7.6
pub type Communicate3 = extern "efiapi" fn(
    this: *const Protocol,
    comm_buffer_physical: *mut c_void,
    comm_buffer_virtual: *mut c_void,
) -> efi::Status;

#[repr(C)]
/// MM Communication Protocol structure.
pub struct Protocol {
    /// Communicate with the MM environment (v3).
    /// See [`Communicate3`] for more details.
    pub communicate3: Communicate3,
}

#[repr(C)]
/// MM communication header structure.
pub struct EfiMmCommunicateHeader {
    /// Indicator GUID for MM core that the communication buffer is compliant with this v3 header.
    /// Must be COMMUNICATE_HEADER_V3_GUID.
    pub header_guid: efi::Guid,
    /// This is technically a read-only field, which is described by the caller to indicate the size of the entire
    /// buffer (in bytes) available for this communication transaction, including this communication header.
    pub buffer_size: u64,
    /// Reserved for future use.
    pub reserved: u64,
    /// Allows for disambiguation of the message format.
    pub message_guid: efi::Guid,
    /// Describes the size of MessageData (in bytes) and does not include the size of the header.
    pub message_size: u64,
    // Data follows the header that is message_size bytes in size.
}
