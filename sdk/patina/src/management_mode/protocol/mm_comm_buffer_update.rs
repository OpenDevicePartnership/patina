//! Management Mode (MM) Communication Buffer Update Support
//!
//! This module provides support for an optional mechanism that might be used in some MM environments to
//! move communication buffers during boot and broadcast the new move to listeners.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::BinaryGuid;
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// GUID for the MM Communication Buffer Update Protocol
pub const GUID: BinaryGuid = BinaryGuid::from_string("2a22e38f-9d1c-49d0-bdce-7ddac16da45d");

/// MM Communication Buffer
///
/// The MM communicate buffer facilitates data sharing between non-MM and MM code.
///
/// The MM IPL code allocates a "fixed" runtime type memory as the MM communication buffer,
/// and communicates its address and size to MM Core via MmCommBuffer GUIDed HOB.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct MmCommBuffer {
    /// Physical address of the communication buffer
    pub physical_start: u64,
    /// Size of the communication buffer in UEFI pages (4KB each)
    pub number_of_pages: u64,
    /// The address of a MM_COMM_BUFFER_STATUS structure.
    pub status: u64,
}

/// MM Communication Buffer Update Protocol
///
/// Protocol interface for updating MM communication buffer information.
///
/// This structure is used by firmware to communicate updated communication buffer
/// details to consumers of the MM communication service.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct MmCommBufferUpdateProtocol {
    /// Version of this structure
    pub version: u64,
    /// MM communication buffer information
    pub updated_comm_buffer: MmCommBuffer,
}
