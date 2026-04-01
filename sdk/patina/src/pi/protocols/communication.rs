//! Communication Protocol
//!
//! Sends/receives a message for a registered handler.
//!
//! See <https://uefi.org/specs/PI/1.9/V4_UEFI_Protocols.html#efi-mm-communication-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{BinaryGuid, Guid};
use core::ffi::c_void;
use r_efi::efi;

/// MM Communication Protocol GUID.
pub const PROTOCOL_GUID: crate::BinaryGuid = crate::BinaryGuid::from_string("C68ED8E2-9DC6-4CBD-9D94-DB65ACC5C332");

/// MM Initialization GUID.
pub const EFI_MM_INITIALIZATION_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("99BE0D8F-3548-48AA-B577-FCFBA56A67F7");

/// Sends/receives a message for a registered handler.
///
/// This protocol provides runtime services for communicating between DXE drivers and a registered MMI handler.
///
/// This function provides a service to send and receive messages from a registered UEFI service. The
/// EFI_MM_COMMUNICATION_PROTOCOL driver is responsible for doing any of the copies such that the data lives in
/// boot-service-accessible RAM.
///
/// A given implementation of the EFI_MM_COMMUNICATION_PROTOCOL may choose to use the EFI_MM_CONTROL_PROTOCOL for
/// effecting the mode transition, or it may use some other method. The agent invoking the communication interface at
/// runtime may be virtually mapped. The MM infrastructure code and handlers, on the other hand, execute in physical
/// mode. As a result, the non- MM agent, which may be executing in the virtual-mode OS context as a result of an OS
/// invocation of the UEFI SetVirtualAddressMap() service, should use a contiguous memory buffer with a physical
/// address before invoking this service. If the virtual address of the buffer is used, the MM Driver may not know how
/// to do the appropriate virtual-to-physical conversion.
///
/// To avoid confusion in interpreting frames, the CommunicateBuffer parameter should always begin with
/// EFI_MM_COMMUNICATE_HEADER , which is defined in “Related Definitions” below. The header data is mandatory for
/// messages sent into the MM agent.
///
/// If the CommSize parameter is omitted the MessageLength field in the EFI_MM_COMMUNICATE_HEADER , in conjunction
/// with the size of the header itself, can be used to ascertain the total size of the communication payload. If the
/// MessageLength is zero, or too large for the MM implementation to manage, the MM implementation must update the
/// MessageLength to reflect the size of the Data buffer that it can tolerate.
///
/// If the CommSize parameter is passed into the call, but the integer it points to, has a value of 0, then this must
/// be updated to reflect the maximum size of the CommBuffer that the implementation can tolerate.
///
/// Once inside of MM, the MM infrastructure will call all registered handlers with the same HandlerType as the GUID
/// specified by HeaderGuid and the CommBuffer pointing to Data.
///
/// This function is not reentrant.
///
/// The standard header is used at the beginning of the EFI_MM_INITIALIZATION_HEADER structure during MM initialization.
///
///  @param this                The protocol instance.
///  @param comm_buffer         A pointer to the buffer to convey into MMRAM.
///  @param comm_size           The size of the data buffer being passed in. On exit, the size of data
///                             being returned. Zero if the handler does not wish to reply with any data.
///                             This parameter is optional and may be NULL.
///
///  @retval Status::SUCCESS           The message was successfully posted.
///  @retval Status::INVALID_PARAMETER The comm_buffer pointer was NULL.
///  @retval Status::BAD_BUFFER_SIZE   The buffer is too large for the MM implementation.
///                                    If this error is returned, the MessageLength field
///                                    in the comm_buffer header or the integer pointed by
///                                    comm_size, are updated to reflect the maximum payload
///                                    size the implementation can accommodate.
///  @retval Status::ACCESS_DENIED     The CommunicateBuffer parameter or comm_size parameter,
///                                    if not omitted, are in address range that cannot be
///                                    accessed by the MM environment.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.9, Section IV-5.7.1
pub type Communicate =
    extern "efiapi" fn(this: *const Protocol, comm_buffer: *mut c_void, comm_size: usize) -> efi::Status;

#[repr(C)]
/// MM Communication Protocol structure.
pub struct Protocol {
    /// Communicate with the MM environment.
    /// See [`Communicate`] for more details.
    pub communicate: Communicate,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
/// MM communication header structure.
pub struct EfiMmCommunicateHeader {
    /// To avoid confusion in interpreting frames, the communication buffer should always begin with the header.
    pub header_guid: BinaryGuid,
    /// Describes the size of Data (in bytes) and does not include the size of the header.
    pub message_length: usize,
    // Comm buffer data follows the header
}

impl EfiMmCommunicateHeader {
    /// Create a new communicate header with the specified GUID and message length.
    pub fn new(header_guid: Guid, message_length: usize) -> Self {
        Self { header_guid: header_guid.to_efi_guid().into(), message_length }
    }

    /// Returns the communicate header as a slice of bytes using safe conversion.
    /// Useful if byte-level access to the header structure is needed.
    ///
    /// # Returns
    ///
    /// A slice of bytes representing the header.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: EfiMmCommunicateHeader is repr(C) with well-defined layout and size
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, Self::size()) }
    }

    /// Function to get the size of the header in bytes.
    ///
    /// # Returns
    ///
    /// The size of the header in bytes.
    pub const fn size() -> usize {
        core::mem::size_of::<Self>()
    }

    /// Get the header GUID from the communication buffer.
    ///
    /// # Returns
    ///
    /// The GUID from the communication header.
    pub fn header_guid(&self) -> Guid<'_> {
        Guid::from_ref(&self.header_guid)
    }

    /// Returns the message length from this communicate header.
    /// The length represents the size of the message data that follows the header.
    ///
    /// # Returns
    ///
    /// The length in bytes of the message data (excluding the header size).
    pub const fn message_length(&self) -> usize {
        self.message_length
    }
}
