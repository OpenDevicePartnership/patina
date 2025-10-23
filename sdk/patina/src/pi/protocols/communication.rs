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

use core::ffi::c_void;
use r_efi::{efi, system};

/// MM Communication Protocol GUID.
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xc68ed8e2, 0x9dc6, 0x4cbd, 0x9d, 0x94, &[0xdb, 0x65, 0xac, 0xc5, 0xc3, 0x32]);

/// MM Initialization GUID.
pub const EFI_MM_INITIALIZATION_GUID: efi::Guid =
    efi::Guid::from_fields(0x99be0d8f, 0x3548, 0x48aa, 0xb5, 0x77, &[0xfc, 0xfb, 0xa5, 0x6a, 0x67, 0xf7]);

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
    pub header_guid: r_efi::base::Guid,
    /// Describes the size of Data (in bytes) and does not include the size of the header.
    pub message_length: usize,
    // Comm buffer data follows the header
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
/// MM initialization header structure.
pub struct EfiMmInitializationHeader {
    /// To avoid confusion in interpreting frames, the communication buffer should always begin with the header.
    pub comm_header: EfiMmCommunicateHeader,
    /// Describes the size of Data (in bytes) and does not include the size of the header.
    pub system_table: *mut system::SystemTable,
}
