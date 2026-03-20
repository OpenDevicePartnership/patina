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

use crate::{BinaryGuid, Guid};
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
        Self { is_comm_buffer_valid: 0, talk_to_supervisor: 0, return_status: 0, return_buffer_size: 0 }
    }
}

/// UEFI MM Communicate Header
///
/// A standard header that must be present at the beginning of any MM communication buffer.
///
/// ## Notes
///
/// - This only supports V1 and V2 of the MM Communicate header format.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EfiMmCommunicateHeader {
    /// Allows for disambiguation of the message format.
    /// Used to identify the registered MM handlers that should be given the message.
    header_guid: BinaryGuid,
    /// The size of Data (in bytes) and does not include the size of the header.
    message_length: usize,
}

impl EfiMmCommunicateHeader {
    /// Create a new communicate header with the specified GUID and message length.
    pub fn new(header_guid: Guid, message_length: usize) -> Self {
        Self { header_guid: header_guid.to_efi_guid().into(), message_length }
    }

    /// Returns the communicate header as a slice of bytes using safe conversion.
    ///
    /// Useful if byte-level access to the header structure is needed.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: EfiMmCommunicateHeader is repr(C) with well-defined layout and size
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, Self::size()) }
    }

    /// Returns the size of the header in bytes.
    pub const fn size() -> usize {
        core::mem::size_of::<Self>()
    }

    /// Get the header GUID from the communication buffer.
    ///
    /// Returns `Some(guid)` if the buffer has been properly initialized with a GUID,
    /// or `None` if the buffer is not initialized.
    ///
    /// # Returns
    ///
    /// The GUID from the communication header if available.
    ///
    /// # Errors
    ///
    /// Returns an error if the communication buffer header cannot be read.
    pub fn header_guid(&self) -> Guid<'_> {
        Guid::from_ref(&self.header_guid)
    }

    /// Returns the message length from this communicate header.
    ///
    /// The length represents the size of the message data that follows the header.
    ///
    /// # Returns
    ///
    /// The length in bytes of the message data (excluding the header size).
    pub const fn message_length(&self) -> usize {
        self.message_length
    }
}

/// EFI_MM_ENTRY_CONTEXT structure.
///
/// Processor information and functionality needed by MM Foundation.
/// Matches the C `EFI_MM_ENTRY_CONTEXT` / `EFI_SMM_ENTRY_CONTEXT` from PI specification.
///
/// Layout (x86_64, all fields 8 bytes):
/// - `mm_startup_this_ap`: Function pointer for `EFI_MM_STARTUP_THIS_AP`
/// - `currently_executing_cpu`: Index of the processor executing the MM Foundation
/// - `number_of_cpus`: Total number of possible processors in the platform (1-based)
/// - `cpu_save_state_size`: Pointer to array of save state sizes per CPU
/// - `cpu_save_state`: Pointer to array of CPU save state pointers
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EfiMmEntryContext {
    /// Function pointer for EFI_MM_STARTUP_THIS_AP.
    pub mm_startup_this_ap: u64,
    /// Index of the currently executing CPU.
    pub currently_executing_cpu: u64,
    /// Total number of CPUs (1-based).
    pub number_of_cpus: u64,
    /// Pointer to array of per-CPU save state sizes.
    pub cpu_save_state_size: u64,
    /// Pointer to array of per-CPU save state pointers.
    pub cpu_save_state: u64,
}
