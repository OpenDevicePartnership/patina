//! Communication Buffer Configuration for the MM Supervisor Core
//!
//! Defines the communication buffer layout extracted from the MM Supervisor
//! `PassDown` HOB and shared across the supervisor for routing user- and
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
use patina_paging::x64::{disable_write_protection, enable_write_protection};
use zerocopy::FromBytes;
use zerocopy_derive::Immutable;

use crate::{
    error::MmSupervisorResult,
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
    /// The internal copy of a communication buffer could not be allocated.
    AllocationFailed,
    /// One or more communication buffers were left at address zero.
    Missing,
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
            Self::AllocationFailed => write!(f, "the internal copy of the buffer could not be allocated"),
            Self::Missing => write!(f, "one or more communication buffers are not properly initialized"),
        }
    }
}

/// Communication buffer configuration extracted from `PassDown` HOB.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommBufferConfig {
    /// The channel carrying supervisor-targeted requests.
    pub supervisor: CommChannel,
    /// The channel carrying user-targeted requests.
    pub user: CommChannel,
    /// MM Supervisor to User buffer.
    pub supv_to_user_buffer: u64,
    /// Size of Supervisor to User buffer.
    pub supv_to_user_buffer_size: u64,
}

/// MM Common Region HOB Data Structure
///
/// Describes the supervisor MM communication region published by the C MM
/// IPL under `gMmCommonRegionHobGuid`.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub struct MmCommonRegionHobData {
    /// Region type discriminator. Always `MM_SUPERVISOR_BUFFER_T` (0) for
    /// the HOB the supervisor consumes.
    pub region_type: u64,
    /// Base address of the communication buffer region.
    pub addr: u64,
    /// Number of pages in the communication buffer region.
    pub number_of_pages: u64,
    /// Address of the `MmCommBufferStatus` structure that pairs with this region.
    pub status_addr: u64,
}

/// One communication channel between the supervisor and the code on the other side of it.
///
/// A channel is the region the MM IPL named, the supervisor's own copy of it, and the status
/// mailbox that pairs with them. [`CommBufferConfig`] holds the two the supervisor runs: the
/// supervisor channel from the MM Common Region HOB, and the user channel from the MM
/// Communication Buffer HOB.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommChannel {
    /// Address of the external region the HOB named.
    pub external: u64,
    /// Size of that region in bytes.
    pub size: u64,
    /// Address of the supervisor's internal copy of it.
    pub internal: u64,
    /// Address of the `MmCommBufferStatus` that pairs with it.
    pub status: u64,
}

impl CommChannel {
    /// Builds the supervisor channel from an `MM_COMMON_REGION_HOB_GUID` payload.
    ///
    /// The page count stays local, because nothing past the allocation needs it: `size` carries
    /// the same length in the unit every later reader works in.
    pub(crate) fn from_supv_hob(data: &[u8]) -> MmSupervisorResult<Self> {
        log::info!("Found MM Common Region HOB (supervisor)");

        let (hob, _) =
            MmCommonRegionHobData::read_from_prefix(data).map_err(|_| CommBufferError::CommRegionHobTooSmall {
                found: data.len(),
                expected: core::mem::size_of::<MmCommonRegionHobData>(),
            })?;

        let external = hob.addr;
        let status = hob.status_addr;
        let pages = hob.number_of_pages;

        let page_count = usize::try_from(pages).map_err(|_| {
            log::error!("Supervisor communication buffer page count {pages} does not fit the target architecture");
            CommBufferError::InvalidSize { pages }
        })?;
        let buffer = MemoryRange::from_pages(external, pages).map_err(|err| {
            log::error!("Supervisor communication buffer of {pages} pages at 0x{external:016x} is invalid: {err}");
            CommBufferError::InvalidSize { pages }
        })?;
        let status_buffer =
            MemoryRange::new(status, core::mem::size_of::<MmCommBufferStatus>() as u64).map_err(|err| {
                log::error!("Supervisor status buffer at 0x{status:016x} is invalid: {err}");
                CommBufferError::InvalidStatusBuffer { address: status }
            })?;

        require_external_comm_buffer(&buffer, "Supervisor communication buffer");
        require_external_comm_buffer(&status_buffer, "Supervisor status buffer");

        let internal = security_state()
            .page_allocator()
            .allocate_pages_with_type(page_count, AllocationType::Supervisor)
            .map_err(|err| {
                log::error!("Failed to allocate internal supervisor common buffer: {err:?}");
                CommBufferError::AllocationFailed
            })?;

        Ok(Self { external, size: buffer.size(), internal, status })
    }

    /// Builds the user channel from an `MM_COMM_BUFFER_HOB_GUID` payload.
    ///
    /// Unlike [`CommChannel::from_supv_hob`], this writes back into the HOB: `physical_start` is
    /// replaced with the internal copy so the user module names it during its own HOB walk after
    /// demotion.
    ///
    /// That write is why the payload arrives as a pointer rather than a slice. The HOB list is
    /// walked through shared references, so the caller holds a `&[u8]` and cannot turn it into a
    /// `&mut [u8]`. It takes the address instead and ends the shared borrow before calling. The
    /// HOB pages may also be mapped read-only, which the store below handles by clearing
    /// `CR0.WP`, and a `&mut [u8]` would claim an ordinary write would succeed.
    ///
    /// ## Safety
    ///
    /// `data` must be non-null and point to `data_len` readable, writable bytes in the
    /// original HOB buffer. No references to those bytes may be live while this
    /// function runs because `physical_start` is overwritten in place.
    pub(crate) unsafe fn from_user_hob(data: *mut u8, data_len: usize) -> MmSupervisorResult<Self> {
        log::info!("Found MM Communication Buffer HOB");

        // SAFETY: the caller guarantees that `data` points to `data_len` readable bytes.
        // `MmCommonBufferHobData` is `Copy`, so `hob` below is an owned copy rather than a borrow
        // of the payload, and `bytes` is never read again. No shared reference to those bytes is
        // live by the time the payload is rewritten through `data` at the end of this function.
        let bytes = unsafe { core::slice::from_raw_parts(data.cast_const(), data_len) };
        let (hob, _) = MmCommonBufferHobData::read_from_prefix(bytes).map_err(|_| {
            CommBufferError::CommunicationBufferHobTooSmall {
                found: data_len,
                expected: core::mem::size_of::<MmCommonBufferHobData>(),
            }
        })?;

        let external = hob.physical_start;
        let status = hob.status_buffer;
        let pages = hob.number_of_pages;

        let page_count = usize::try_from(pages).map_err(|_| {
            log::error!("User communication buffer page count {pages} does not fit the target architecture");
            CommBufferError::InvalidSize { pages }
        })?;
        let buffer = MemoryRange::from_pages(external, pages).map_err(|err| {
            log::error!("User communication buffer of {pages} pages at 0x{external:016x} is invalid: {err}");
            CommBufferError::InvalidSize { pages }
        })?;
        let status_buffer =
            MemoryRange::new(status, core::mem::size_of::<MmCommBufferStatus>() as u64).map_err(|err| {
                log::error!("User status buffer at 0x{status:016x} is invalid: {err}");
                CommBufferError::InvalidStatusBuffer { address: status }
            })?;

        require_external_comm_buffer(&buffer, "User communication buffer");
        require_external_comm_buffer(&status_buffer, "User status buffer");

        let internal = security_state()
            .page_allocator()
            .allocate_pages_with_type(page_count, AllocationType::User)
            .map_err(|err| {
                log::error!("Failed to allocate internal user common buffer: {err:?}");
                CommBufferError::AllocationFailed
            })?;

        let channel = Self { external, size: buffer.size(), internal, status };

        // TODO: Remove the logic that overwrites the HOB's physical_start with the internal buffer address
        // so the user module sees it after demotion.
        // SAFETY:
        // - `data` points to the original writable HOB buffer and parsing above proved it contains
        //   `MmCommonBufferHobData`, so `physical_start` lies within the allocation. `addr_of_mut!`
        //   avoids forming a reference to the packed field, and `write_volatile` keeps the store from
        //   being elided.
        // - The HOB pages may be mapped read-only, so `disable_write_protection` clears `CR0.WP` to
        //   permit the supervisor store. This runs in Ring 0 during single-threaded BSP init in the MM
        //   (SMM) environment, where interrupts are masked, satisfying the privilege/atomicity
        //   requirements of `disable_write_protection`. `enable_write_protection` is handed exactly the
        //   value returned by `disable_write_protection`, restoring `CR0.WP` before this block returns
        //   and bounding the unprotected window to the single field write.
        unsafe {
            let hob_ptr = data.cast::<MmCommonBufferHobData>();
            let original_cr0 = disable_write_protection();
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*hob_ptr).physical_start), channel.internal);
            enable_write_protection(original_cr0);
        }

        Ok(channel)
    }
}

/// Requires that `range` is usable as an external communication buffer:
/// entirely outside MMRAM, and mapped supervisor-only in the active page table.
///
/// The MM IPL names these buffers and sits outside the supervisor's trust boundary. The
/// supervisor copies a response back into the buffer, so any part of it inside MMRAM turns that
/// copy into an MMRAM write with a payload chosen outside MM - hence overlap rather than
/// containment, since a partly-inside buffer carries the same primitive in its tail. Ring 3 works
/// on the internal copy, so direct access here would let a demoted driver rewrite a request, or
/// its status mailbox, while it is being serviced.
fn require_external_comm_buffer(range: &MemoryRange, description: &str) {
    assert!(!range_overlaps_mmram(range), "{description} at {range} overlaps MMRAM; it must lie entirely outside");

    match query_address_ownership(range) {
        Some(PageOwnership::Supervisor) => {}
        Some(PageOwnership::User) => {
            panic!("{description} at {range} is mapped user-accessible; it must be supervisor-only")
        }
        None => panic!("{description} at {range} is unmapped or not uniformly mapped"),
    }
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
