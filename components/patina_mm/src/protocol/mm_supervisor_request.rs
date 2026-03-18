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

use patina::BinaryGuid;

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
/// 0x08    4     request     - Request type (see [`requests`] module)
/// 0x0C    4     reserved    - Reserved for alignment, must be 0
/// 0x10    8     result      - Return status (0 = success, set by supervisor on response)
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
pub struct MmSupervisorRequestHeader {
    /// Signature to identify the request ('MSUP' as little-endian).
    pub signature: u32,
    /// Revision of the request protocol.
    pub revision: u32,
    /// The specific request type (see [`requests`] module constants).
    pub request: u32,
    /// Reserved for alignment, must be 0.
    pub reserved: u32,
    /// Result status. Set by the supervisor on response (0 = success).
    pub result: u64,
}

impl MmSupervisorRequestHeader {
    /// Size of the header in bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Validates the header signature and revision.
    pub fn is_valid(&self) -> bool {
        self.signature == SIGNATURE && self.revision <= REVISION
    }
}

/// Response from MM Supervisor version info request.
///
/// Returned as the payload following an [`MmSupervisorRequestHeader`] when the request
/// type is [`requests::VERSION_INFO`].
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
}

// ============================================================================
// Protocol Constants
// ============================================================================

/// The expected signature value ('MSUP' as little-endian u32).
pub const SIGNATURE: u32 = 0x5055534D;

/// Current revision of the request protocol.
pub const REVISION: u32 = 1;

/// Standard MM Supervisor request types.
///
/// Each variant corresponds to a specific supervisor operation. The enum is `#[repr(u32)]`
/// to match the wire format of [`MmSupervisorRequestHeader::request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
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
    /// The highest valid request type value.
    pub const MAX: u64 = Self::CommUpdate as u64;
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

/// Deprecated module — use [`RequestType`] enum variants instead.
///
/// Kept temporarily for backward compatibility.
pub mod requests {
    use super::RequestType;
    /// Request to unblock memory regions.
    pub const UNBLOCK_MEM: u32 = RequestType::UnblockMem as u32;
    /// Request to fetch security policy.
    pub const FETCH_POLICY: u32 = RequestType::FetchPolicy as u32;
    /// Request for version information.
    pub const VERSION_INFO: u32 = RequestType::VersionInfo as u32;
    /// Request to update communication buffer.
    pub const COMM_UPDATE: u32 = RequestType::CommUpdate as u32;
}

/// Response status constants.
pub mod responses {
    /// Operation completed successfully.
    pub const SUCCESS: u64 = 0;
    /// Operation failed with error.
    pub const ERROR: u64 = 0xFFFFFFFFFFFFFFFF;
}

// ============================================================================
// Unblock Memory Params
// ============================================================================

use r_efi::efi;

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
    /// Size of this structure in bytes (56).
    pub const SIZE: usize = core::mem::size_of::<Self>();
}
