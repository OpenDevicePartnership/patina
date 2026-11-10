//! Communication Buffer Configuration for the MM Supervisor Core
//!
//! Defines the communication buffer layout built from the MM Common Region HOB
//! (`gMmCommonRegionHobGuid`, supervisor channel) and the `MM_COMM_BUFFER_HOB_GUID`
//! HOB (user channel), and shared across the supervisor for routing user- and
//! supervisor-targeted requests.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use patina::MemoryRange;
use patina::management_mode::{MmCommBufferStatus, comm_buffer_hob::MmCommonBufferHobData};
use zerocopy::FromBytes;
use zerocopy_derive::Immutable;

use crate::{
    error::MmSupervisorResult,
    intrinsics::with_write_protection_disabled,
    memory::{
        AllocationType,
        mmram::range_overlaps_mmram,
        page_ownership::{PageOwnership, query_address_ownership},
    },
    state::security_state,
};

/// GUID for `gMmCommonRegionHobGuid`.
///
/// `{ 0xd4ffc718, 0xfb82, 0x4274, { 0x9a, 0xfc, 0xaa, 0x8b, 0x1e, 0xef, 0x52, 0x93 } }`
pub(crate) const MM_COMMON_REGION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("d4ffc718-fb82-4274-9afc-aa8b1eef5293");

/// Errors produced while building a communication channel from the HOB list.
///
/// The MM IPL describes both buffers from outside the supervisor's trust boundary, so each
/// field is checked before the internal copy is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommBufferError {
    /// The MM Common Region HOB, which describes the supervisor channel, was not present in
    /// the HOB list.
    CommRegionHobMissing,
    /// The MM Communication Buffer HOB, which describes the user channel, was not present in
    /// the HOB list.
    CommunicationBufferHobMissing,
    /// The MM Common Region HOB payload is smaller than [`MmCommonRegionHobData`].
    CommRegionHobTooSmall {
        /// Bytes the HOB actually carries.
        found: usize,
        /// Bytes the structure requires.
        expected: usize,
    },
    /// The MM Communication Buffer HOB payload is smaller than [`MmCommonBufferHobData`].
    CommunicationBufferHobTooSmall {
        /// Bytes the HOB actually carries.
        found: usize,
        /// Bytes the structure requires.
        expected: usize,
    },
    /// The page count is zero, does not fit the target architecture, or produces an
    /// address range that overflows.
    InvalidSize {
        /// The page count the HOB reported.
        pages: u64,
    },
    /// The status buffer address leaves no room for an `MmCommBufferStatus` below the end of the
    /// address space.
    InvalidStatusBuffer {
        /// The status buffer address the HOB reported.
        address: u64,
    },
    /// An external buffer overlaps MMRAM. It must lie entirely outside.
    OverlapsMmram {
        /// The buffer the HOB described.
        range: MemoryRange,
    },
    /// An external buffer is mapped user-accessible. It must be supervisor-only.
    UserAccessible {
        /// The buffer the HOB described.
        range: MemoryRange,
    },
    /// An external buffer is unmapped, or its pages do not share one mapping.
    NotUniformlyMapped {
        /// The buffer the HOB described.
        range: MemoryRange,
    },
    /// The internal copy of a communication buffer could not be allocated.
    AllocationFailed,
    /// The HOB names address zero for a buffer.
    NullAddress {
        /// Which buffer the HOB left at zero.
        buffer: &'static str,
    },
}

impl core::error::Error for CommBufferError {}

impl fmt::Display for CommBufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommRegionHobMissing => write!(f, "the MM Common Region HOB is missing from the HOB list"),
            Self::CommunicationBufferHobMissing => {
                write!(f, "the MM Communication Buffer HOB is missing from the HOB list")
            }
            Self::CommRegionHobTooSmall { found, expected } => {
                write!(f, "the MM Common Region HOB payload is {found} bytes, but {expected} are required")
            }
            Self::CommunicationBufferHobTooSmall { found, expected } => {
                write!(f, "the MM Communication Buffer HOB payload is {found} bytes, but {expected} are required")
            }
            Self::InvalidSize { pages } => write!(f, "the page count {pages} does not describe a usable buffer"),
            Self::InvalidStatusBuffer { address } => {
                write!(f, "the status buffer address 0x{address:016x} does not describe a usable buffer")
            }
            Self::OverlapsMmram { range } => {
                write!(f, "the buffer at {range} overlaps MMRAM; it must lie entirely outside")
            }
            Self::UserAccessible { range } => {
                write!(f, "the buffer at {range} is mapped user-accessible; it must be supervisor-only")
            }
            Self::NotUniformlyMapped { range } => {
                write!(f, "the buffer at {range} is unmapped or not uniformly mapped")
            }
            Self::AllocationFailed => write!(f, "the internal copy of the buffer could not be allocated"),
            Self::NullAddress { buffer } => write!(f, "the HOB names address zero for the {buffer}"),
        }
    }
}

/// Communication buffer configuration built from the MM Common Region HOB (supervisor channel)
/// and the `MM_COMM_BUFFER_HOB_GUID` HOB (user channel).
#[derive(Debug, Clone, Copy, Default)]
pub struct CommBufferConfig {
    /// The channel carrying supervisor-targeted requests.
    pub supervisor: CommChannel,
    /// The channel carrying user-targeted requests.
    pub user: CommChannel,
    /// Address of the page the supervisor uses to pass the entry context and status to the user
    /// core.
    pub supv_to_user_buffer: u64,
    /// Size of that page in bytes.
    pub supv_to_user_buffer_size: u64,
}

/// MM Common Region HOB Data Structure
///
/// Describes the supervisor MM communication region published by the C MM
/// IPL under `gMmCommonRegionHobGuid`.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub struct MmCommonRegionHobData {
    /// Region type discriminator. The MM IPL sets it to `MM_SUPERVISOR_BUFFER_T` (0) for the
    /// HOB the supervisor consumes; the supervisor does not check it.
    pub region_type: u64,
    /// Physical start address of the communication buffer region.
    pub physical_start: u64,
    /// Number of pages in the communication buffer region.
    pub number_of_pages: u64,
    /// Address of the `MmCommBufferStatus` structure that pairs with this region.
    pub status_address: u64,
}

/// One communication channel between the supervisor and the code on the other side of it.
///
/// A channel is the region the MM IPL named, the supervisor's own copy of it, and the status
/// mailbox that pairs with them. [`CommBufferConfig`] holds the two the supervisor runs: the
/// supervisor channel from the MM Common Region HOB, and the user channel from the MM
/// Communication Buffer HOB.
///
/// `external_address` and `status_address` are non-MM memory; handlers only work on `internal_address`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommChannel {
    /// Non-MM buffer outside MMRAM that the caller writes requests to and reads responses from.
    pub external_address: u64,
    /// The supervisor's copy of `external_address` inside MMRAM.
    pub internal_address: u64,
    /// Size in bytes of both `external_address` and `internal_address`.
    pub size: u64,
    /// Non-MM `MmCommBufferStatus` mailbox outside MMRAM.
    pub status_address: u64,
}

impl fmt::Display for CommChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "external_address=0x{:x} internal_address=0x{:x} size=0x{:x} status_address=0x{:x}",
            self.external_address, self.internal_address, self.size, self.status_address
        )
    }
}

impl CommChannel {
    /// Builds the supervisor channel from an `MM_COMMON_REGION_HOB_GUID` payload.
    ///
    /// The page count stays local, because nothing past the allocation needs it: `size` carries
    /// the same length in bytes.
    pub(crate) fn from_supv_hob(data: &[u8]) -> MmSupervisorResult<Self> {
        log::info!("Found MM Common Region HOB (supervisor)");

        let (hob, _) =
            MmCommonRegionHobData::read_from_prefix(data).map_err(|_| CommBufferError::CommRegionHobTooSmall {
                found: data.len(),
                expected: core::mem::size_of::<MmCommonRegionHobData>(),
            })?;

        let external_address = hob.physical_start;
        let status_address = hob.status_address;
        let pages = hob.number_of_pages;

        if external_address == 0 {
            log::error!("Supervisor communication buffer address is zero");
            return Err(CommBufferError::NullAddress { buffer: "supervisor communication buffer" }.into());
        }
        if status_address == 0 {
            log::error!("Supervisor status buffer address is zero");
            return Err(CommBufferError::NullAddress { buffer: "supervisor status buffer" }.into());
        }

        let page_count = usize::try_from(pages).map_err(|_| {
            log::error!("Supervisor communication buffer page count {pages} does not fit the target architecture");
            CommBufferError::InvalidSize { pages }
        })?;
        let buffer = MemoryRange::from_pages(external_address, pages).map_err(|err| {
            log::error!(
                "Supervisor communication buffer of {pages} pages at 0x{external_address:016x} is invalid: {err}"
            );
            CommBufferError::InvalidSize { pages }
        })?;
        let status_buffer = MemoryRange::new(status_address, core::mem::size_of::<MmCommBufferStatus>() as u64)
            .map_err(|err| {
                log::error!("Supervisor status buffer at 0x{status_address:016x} is invalid: {err}");
                CommBufferError::InvalidStatusBuffer { address: status_address }
            })?;

        require_external_comm_buffer(&buffer, "Supervisor communication buffer")?;
        require_external_comm_buffer(&status_buffer, "Supervisor status buffer")?;

        let internal_address = security_state()
            .page_allocator()
            .allocate_pages_with_type(page_count, AllocationType::Supervisor)
            .map_err(|err| {
                log::error!("Failed to allocate internal supervisor common buffer: {err:?}");
                CommBufferError::AllocationFailed
            })?;

        Ok(Self { external_address, internal_address, size: buffer.size(), status_address })
    }

    /// Builds the user channel from an `MM_COMM_BUFFER_HOB_GUID` payload.
    ///
    /// The user core finds its buffer through this HOB, so the caller must then point the HOB at
    /// the internal copy with [`CommChannel::redirect_user_hob`].
    pub(crate) fn from_user_hob(data: &[u8]) -> MmSupervisorResult<Self> {
        log::info!("Found MM Communication Buffer HOB");

        let (hob, _) = MmCommonBufferHobData::read_from_prefix(data).map_err(|_| {
            CommBufferError::CommunicationBufferHobTooSmall {
                found: data.len(),
                expected: core::mem::size_of::<MmCommonBufferHobData>(),
            }
        })?;

        let external_address = hob.physical_start;
        let status_address = hob.status_buffer;
        let pages = hob.number_of_pages;

        if external_address == 0 {
            log::error!("User communication buffer address is zero");
            return Err(CommBufferError::NullAddress { buffer: "user communication buffer" }.into());
        }
        if status_address == 0 {
            log::error!("User status buffer address is zero");
            return Err(CommBufferError::NullAddress { buffer: "user status buffer" }.into());
        }

        let page_count = usize::try_from(pages).map_err(|_| {
            log::error!("User communication buffer page count {pages} does not fit the target architecture");
            CommBufferError::InvalidSize { pages }
        })?;
        let buffer = MemoryRange::from_pages(external_address, pages).map_err(|err| {
            log::error!("User communication buffer of {pages} pages at 0x{external_address:016x} is invalid: {err}");
            CommBufferError::InvalidSize { pages }
        })?;
        let status_buffer = MemoryRange::new(status_address, core::mem::size_of::<MmCommBufferStatus>() as u64)
            .map_err(|err| {
                log::error!("User status buffer at 0x{status_address:016x} is invalid: {err}");
                CommBufferError::InvalidStatusBuffer { address: status_address }
            })?;

        require_external_comm_buffer(&buffer, "User communication buffer")?;
        require_external_comm_buffer(&status_buffer, "User status buffer")?;

        let internal_address = security_state()
            .page_allocator()
            .allocate_pages_with_type(page_count, AllocationType::User)
            .map_err(|err| {
                log::error!("Failed to allocate internal user common buffer: {err:?}");
                CommBufferError::AllocationFailed
            })?;

        Ok(Self { external_address, internal_address, size: buffer.size(), status_address })
    }

    /// Points the `MM_COMM_BUFFER_HOB_GUID` HOB's `physical_start` at this channel's internal copy,
    /// so the user core uses it instead of the non-MM buffer.
    ///
    /// ## Errors
    ///
    /// Returns `CommunicationBufferHobTooSmall` without writing if `data_len` cannot hold an
    /// [`MmCommonBufferHobData`].
    ///
    /// ## Safety
    ///
    /// `data` must point to `data_len` writable bytes of the original HOB payload, with no live
    /// references to them.
    pub(crate) unsafe fn redirect_user_hob(&self, data: *mut u8, data_len: usize) -> MmSupervisorResult<()> {
        let expected = core::mem::size_of::<MmCommonBufferHobData>();
        if data_len < expected {
            return Err(CommBufferError::CommunicationBufferHobTooSmall { found: data_len, expected }.into());
        }

        // TODO: Pass the internal copy to the user core directly instead of rewriting the HOB.
        let hob_ptr = data.cast::<MmCommonBufferHobData>();
        // SAFETY: `data` holds a writable `MmCommonBufferHobData`, checked above. The struct is
        // packed, so `addr_of_mut!` takes the field's address without a reference.
        let physical_start = unsafe { core::ptr::addr_of_mut!((*hob_ptr).physical_start) }.cast::<[u8; 8]>();
        let store = || {
            // SAFETY: `physical_start` is a writable 8-byte field of the HOB, and `[u8; 8]` has no
            // alignment requirement.
            unsafe { core::ptr::write_volatile(physical_start, self.internal_address.to_le_bytes()) };
        };
        // SAFETY: `CR0.WP` is cleared only for this store, because the HOB pages may be
        // read-only, during single-threaded BSP init in Ring 0.
        unsafe { with_write_protection_disabled(store) };

        Ok(())
    }
}

/// Requires that `range` is usable as an external communication buffer:
/// entirely outside MMRAM, and mapped supervisor-only in the active page table.
///
/// The MM IPL names these buffers and sits outside the supervisor's trust boundary. The
/// supervisor copies a response back into the buffer, so any part of it inside MMRAM turns that
/// copy into an MMRAM write with a payload chosen outside MM. Overlap is checked rather than
/// containment because a partly-inside buffer carries the same primitive in its tail. Ring 3 works
/// on the internal copy, so direct access here would let a demoted driver rewrite a request, or
/// its status mailbox, while it is being serviced.
///
/// ## Errors
///
/// Returns `OverlapsMmram` if `range` overlaps MMRAM, and `UserAccessible` or
/// `NotUniformlyMapped` if it is not mapped supervisor-only. `description` names the buffer in
/// the log.
fn require_external_comm_buffer(range: &MemoryRange, description: &str) -> Result<(), CommBufferError> {
    let error = if range_overlaps_mmram(range) {
        CommBufferError::OverlapsMmram { range: *range }
    } else {
        match query_address_ownership(range) {
            Some(PageOwnership::Supervisor) => return Ok(()),
            Some(PageOwnership::User) => CommBufferError::UserAccessible { range: *range },
            None => CommBufferError::NotUniformlyMapped { range: *range },
        }
    };
    log::error!("{description}: {error}");
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comm_buffer_config_default_is_zeroed() {
        let cfg = CommBufferConfig::default();
        assert_eq!(cfg.supv_comm_buffer, 0);
        assert_eq!(cfg.supv_comm_buffer_internal, 0);
        assert_eq!(cfg.supv_comm_buffer_size, 0);
        assert_eq!(cfg.user_comm_buffer, 0);
        assert_eq!(cfg.user_comm_buffer_internal, 0);
        assert_eq!(cfg.user_comm_buffer_size, 0);
        assert_eq!(cfg.user_status_buffer, 0);
        assert_eq!(cfg.supv_status_buffer, 0);
        assert_eq!(cfg.supv_to_user_buffer, 0);
        assert_eq!(cfg.supv_to_user_buffer_size, 0);
    }

    #[test]
    fn test_comm_buffer_config_is_copy() {
        let cfg = CommBufferConfig { supv_comm_buffer: 0x1000, user_comm_buffer: 0x2000, ..Default::default() };
        let copied = cfg; // relies on `Copy`
        assert_eq!(copied.supv_comm_buffer, 0x1000);
        assert_eq!(copied.user_comm_buffer, 0x2000);
        // `cfg` is still usable after the copy.
        assert_eq!(cfg.supv_comm_buffer, 0x1000);
    }
}
