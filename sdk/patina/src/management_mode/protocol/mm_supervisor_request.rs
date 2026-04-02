//! MM Supervisor Request Protocol Definitions
//!
//! This module provides the shared protocol structures and constants for MM Supervisor
//! request handling. These types define the communication contract between the supervisor
//! and its clients (DXE, tests, etc.).
//!
//! ## Overview
//!
//! The MM Supervisor uses a structured request/response protocol. Requests are sent via
//! the MM communicate buffer and consist of an [`MmSupervisorRequestHeader`] followed by
//! request-specific payload data. The supervisor processes the request and writes back
//! a response header (with result status) followed by response-specific data.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::BinaryGuid;
use r_efi::efi;
use zerocopy::FromBytes;

/// Signature value for the request header ('MSUP' as little-endian u32).
pub const SIGNATURE: u32 = u32::from_le_bytes([b'M', b'S', b'U', b'P']);

/// Current revision of the request protocol.
pub const REVISION: u32 = 1;

// GUID for gMmSupervisorRequestHandlerGuid
// { 0x8c633b23, 0x1260, 0x4ea6, { 0x83, 0xf, 0x7d, 0xdc, 0x97, 0x38, 0x21, 0x11 } }
/// GUID for the MM Supervisor Request Handler protocol.
pub const MM_SUPERVISOR_REQUEST_HANDLER_GUID: BinaryGuid =
    BinaryGuid::from_string("8c633b23-1260-4ea6-830f-7ddc97382111");

/// MM Supervisor request header.
///
/// This header is present at the start of every supervisor request buffer. It identifies
/// the request type and carries the result status on response.
///
/// ## Layout
///
/// ```text
/// Offset  Size  Field
/// 0x00    4     signature   - Must be [`SIGNATURE`] ('MSUP' as little-endian u32)
/// 0x04    4     revision    - Protocol revision, must be <= [`REVISION`]
/// 0x08    4     request     - Request type (see [`RequestType`] enum)
/// 0x0C    4     reserved    - Reserved for alignment, must be 0
/// 0x10    8     result      - Return status (0 = success, set by supervisor on response)
/// ```
#[derive(Debug, Clone, Copy, zerocopy_derive::FromBytes, zerocopy_derive::IntoBytes, zerocopy_derive::Immutable)]
#[repr(C)]
pub struct MmSupervisorRequestHeader {
    /// Signature to identify the request ('MSUP' as little-endian).
    pub signature: u32,
    /// Revision of the request protocol.
    pub revision: u32,
    /// The specific request type (see [`RequestType`] enum).
    pub request: u32,
    /// Reserved for alignment, must be 0.
    pub reserved: u32,
    /// Result status. The value of this field follows the [`efi::Status`] definitions.
    pub result: u64,
}

impl MmSupervisorRequestHeader {
    /// Size of the header in bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Validates the header signature and revision.
    pub fn is_valid(&self) -> bool {
        self.signature == SIGNATURE && self.revision <= REVISION
    }

    /// Reads a header from a byte slice.
    ///
    /// Returns `None` if the slice is too small or misaligned.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::read_from_bytes(bytes.get(..Self::SIZE)?).ok()
    }
}

/// Response from MM Supervisor version info request.
///
/// Returned as the payload following an [`MmSupervisorRequestHeader`] when the request
/// type is [`RequestType::VersionInfo`].
///
/// ## Layout
///
/// ```text
/// Offset  Size  Field
/// 0x00    4     version                       - Supervisor version
/// 0x04    4     patch_level                   - Supervisor patch level
/// 0x08    8     max_supervisor_request_level  - Highest supported request type
/// ```
#[derive(
    Debug,
    Clone,
    Copy,
    zerocopy_derive::FromBytes,
    zerocopy_derive::IntoBytes,
    zerocopy_derive::Immutable,
    zerocopy_derive::KnownLayout
)]
#[repr(C)]
pub struct MmSupervisorVersionInfo {
    /// Version of the MM Supervisor.
    pub version: u32,
    /// Patch level.
    pub patch_level: u32,
    /// Maximum supported supervisor request level (highest valid request type value).
    pub max_supervisor_request_level: u64,
}

impl MmSupervisorVersionInfo {
    /// Size of the version info structure in bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Reads version info from a byte slice.
    ///
    /// Returns `None` if the slice is too small or misaligned.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::read_from_bytes(bytes.get(..Self::SIZE)?).ok()
    }
}

/// MM Supervisor request types.
///
/// Each variant corresponds to a specific supervisor operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestType {
    /// Request to unblock memory regions.
    UnblockMem = 0x0001,
    /// Request to fetch security policy.
    FetchPolicy = 0x0002,
    /// Request for version information.
    VersionInfo = 0x0003,
    /// Request to update communication buffer.
    CommUpdate = 0x0004,
}

impl RequestType {
    /// Tries to convert a raw u64 value into a `RequestType`.
    ///
    /// Returns `Err(value)` if the value does not correspond to a valid request type.
    pub const MAX_REQUEST_TYPE: u64 = Self::CommUpdate as u64;
}

impl TryFrom<u32> for RequestType {
    type Error = u32;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0x0001 => Ok(Self::UnblockMem),
            0x0002 => Ok(Self::FetchPolicy),
            0x0003 => Ok(Self::VersionInfo),
            0x0004 => Ok(Self::CommUpdate),
            other => Err(other),
        }
    }
}

impl From<RequestType> for u32 {
    fn from(request_type: RequestType) -> Self {
        request_type as u32
    }
}

/// Standard MM Supervisor response types.
///
/// Each variant corresponds to a specific response status that the supervisor can return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    /// Error: Invalid request index.
    InvalidRequest,
    /// Error: Invalid data buffer.
    InvalidDataBuffer,
    /// Error: Communication buffer initialization failed.
    CommBufferInitError,
}

/// Maps `ResponseType` variants to corresponding [`efi::Status`] codes, because this is how
/// the supervisor request handlers map the [`MmSupervisorRequestHeader::result`] field in the
/// response header.
impl From<ResponseType> for efi::Status {
    fn from(response_type: ResponseType) -> Self {
        match response_type {
            ResponseType::InvalidRequest => efi::Status::INVALID_PARAMETER,
            ResponseType::InvalidDataBuffer => efi::Status::BUFFER_TOO_SMALL,
            ResponseType::CommBufferInitError => efi::Status::DEVICE_ERROR,
        }
    }
}

/// MM Supervisor Unblock Memory Parameters.
///
/// Matches the C `MM_SUPERVISOR_UNBLOCK_MEMORY_PARAMS` layout. The C header
/// defines this under `#pragma pack(push, 1)`, but because `efi::MemoryDescriptor`
/// (40 bytes) and `Guid` (16 bytes) are both naturally aligned, the packed
/// and natural layouts are identical (56 bytes total).
///
/// ## Layout
///
/// ```text
/// Offset  Size  Field
/// 0x00    40    memory_descriptor   - EFI_MEMORY_DESCRIPTOR (r-efi efi::MemoryDescriptor)
/// 0x28    16    identifier_guid     - Requester identification GUID
/// ```
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MmSupervisorUnblockMemoryParams {
    /// Memory descriptor identifying the region to unblock.
    pub memory_descriptor: efi::MemoryDescriptor,
    /// GUID identifying the requesting driver/module.
    pub identifier_guid: BinaryGuid,
}

impl MmSupervisorUnblockMemoryParams {
    /// Size of this structure in bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}
