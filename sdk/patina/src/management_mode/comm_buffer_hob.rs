//! Management Mode (MM) Header and Buffer HOB Definitions
//!
//! Defines the header and buffer HOB structures necessary for the MM environment to be initialized and used by components
//! dependent on MM details.
//!
//! ## MM HOB Usage
//!
//! It is expected that the MM HOB buffer will be initialized by the environment that registers services for the
//! platform. The HOBs can have platform-fixed values assigned during their initialization. It should be common
//! for at least the communication buffers to be populated as a mutable HOB during boot time. It is
//! recommended for a "MM HOB" component to handle all MM HOB details with minimal other MM related
//! dependencies and lock the HOBs so they are available for components that depend on the immutable HOB
//! to perform MM operations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;
use zerocopy_derive::{FromBytes, Immutable, KnownLayout};

/// GUID for the MM communication buffer HOB (`gMmCommBufferHobGuid`).
///
/// `{ 0x6c2a2520, 0x0131, 0x4aee, { 0xa7, 0x50, 0xcc, 0x38, 0x4a, 0xac, 0xe8, 0xc6 } }`
pub const MM_COMM_BUFFER_HOB_GUID: BinaryGuid = BinaryGuid::from_string("6c2a2520-0131-4aee-a750-cc384aace8c6");

/// MM Common Buffer HOB Data Structure.
///
/// Describes the communication buffer region passed via HOB from PEI to MM.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MmCommonBufferHobData {
    /// Physical start address of the common region.
    pub physical_start: u64,
    /// Number of pages in the communication buffer region.
    pub number_of_pages: u64,
    /// Pointer to `MmCommBufferStatus` structure.
    pub status_buffer: u64,
}

/// MM Communication Buffer Status
///
/// Shared structure between DXE and MM environments to communicate the status
/// of MM communication operations. This structure is written by DXE before
/// triggering an MMI and read/written by MM during MMI processing.
///
/// This is a structure currently used in some MM Supervisor MM implementations.
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct MmCommBufferStatus {
    /// Whether the data in the fixed MM communication buffer is valid when entering from non-MM to MM.
    /// Must be set to TRUE before triggering MMI, will be set to FALSE by MM after processing.
    pub is_comm_buffer_valid: u8,

    /// The channel used to communicate with MM.
    /// FALSE = user buffer, TRUE = supervisor buffer
    pub talk_to_supervisor: u8,

    /// Padding to align to 8 bytes.
    /// This padding is necessary to match the structure layout defined in edk2 and mu_basecore.
    pub _padding: [u8; 6],

    /// The return status when returning from MM to non-MM.
    pub return_status: u64,

    /// The size in bytes of the output buffer when returning from MM to non-MM.
    pub return_buffer_size: u64,
}

impl Default for MmCommBufferStatus {
    #[coverage(off)]
    fn default() -> Self {
        Self::new()
    }
}

impl MmCommBufferStatus {
    /// Create a new mailbox status with all fields zeroed
    pub const fn new() -> Self {
        Self {
            is_comm_buffer_valid: 0,
            talk_to_supervisor: 0,
            _padding: [0; 6],
            return_status: 0,
            return_buffer_size: 0,
        }
    }
}
