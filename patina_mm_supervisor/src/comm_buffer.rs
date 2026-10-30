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

use patina::management_mode::{MmCommBufferStatus, comm_buffer_hob::MmCommonBufferHobData};
use patina::{MemoryRange, UEFI_PAGE_SIZE};
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
        let buffer = MemoryRange::from_pages(external, pages, UEFI_PAGE_SIZE as u64).map_err(|err| {
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
            .map_err(|e| {
                log::error!("Failed to allocate internal supervisor common buffer: {e:?}");
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
        let buffer = MemoryRange::from_pages(external, pages, UEFI_PAGE_SIZE as u64).map_err(|err| {
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
            .map_err(|e| {
                log::error!("Failed to allocate internal user common buffer: {e:?}");
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
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use crate::memory;
    use crate::test_support;
    use crate::test_support::*;
    use patina_paging::{MemoryAttributes, PageTable};

    #[test]
    fn test_comm_buffer_config_default_is_zeroed() {
        let cfg = CommBufferConfig::default();
        assert_eq!(cfg.supervisor, CommChannel::default());
        assert_eq!(cfg.user, CommChannel::default());
        assert_eq!(cfg.supervisor.external, 0);
        assert_eq!(cfg.supervisor.internal, 0);
        assert_eq!(cfg.supervisor.size, 0);
        assert_eq!(cfg.supervisor.status, 0);
        assert_eq!(cfg.user.external, 0);
        assert_eq!(cfg.user.internal, 0);
        assert_eq!(cfg.user.size, 0);
        assert_eq!(cfg.user.status, 0);
        assert_eq!(cfg.supv_to_user_buffer, 0);
        assert_eq!(cfg.supv_to_user_buffer_size, 0);
    }

    #[test]
    fn test_comm_buffer_config_is_copy() {
        let cfg = CommBufferConfig {
            supervisor: CommChannel { external: 0x1000, ..Default::default() },
            user: CommChannel { external: 0x2000, ..Default::default() },
            ..Default::default()
        };
        let copied = cfg; // relies on `Copy`
        assert_eq!(copied.supervisor.external, 0x1000);
        assert_eq!(copied.user.external, 0x2000);
        // `cfg` is still usable after the copy.
        assert_eq!(cfg.supervisor.external, 0x1000);
    }

    /// Installs MMRAM and an empty page table, and returns the memory backing MMRAM.
    fn comm_buffer_state() -> PageAlignedMemory {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);
        mmram
    }

    /// Runs [`require_external_comm_buffer`] and returns its panic message, or `None` if it
    /// accepted the buffer.
    fn rejection(address: u64, size: u64) -> Option<String> {
        let range = MemoryRange::new(address, size).expect("a valid test range");
        catch_unwind(|| require_external_comm_buffer(&range, "Test buffer")).err().map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|message| (*message).to_string()))
                .unwrap_or_default()
        })
    }

    /// Asserts that [`require_external_comm_buffer`] refuses the buffer with a message containing
    /// `expected`.
    fn assert_rejected(address: u64, size: u64, expected: &str) {
        let message = rejection(address, size).expect("the buffer was accepted");
        assert!(message.contains(expected), "expected a rejection containing {expected:?}, got {message:?}");
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_accepts_a_supervisor_mapped_buffer_outside_mmram() {
        let _mmram = comm_buffer_state();
        let external = PageAlignedMemory::new(2);
        map_supervisor_only(&external);

        assert_eq!(rejection(external.base(), external.size()), None);
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_rejects_a_buffer_touching_mmram() {
        // The copy-back would otherwise turn a payload chosen outside MM into an MMRAM write. The
        // MMRAM pages are not mapped in this page table, so these are refused for touching MMRAM
        // before the ownership query could refuse them as unmapped.
        let mmram = comm_buffer_state();
        let page = UEFI_PAGE_SIZE as u64;

        assert_rejected(mmram.base(), page, "overlaps MMRAM");
        // Only the tail of this buffer reaches MMRAM.
        assert_rejected(mmram.base() - page, 2 * page, "overlaps MMRAM");
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_rejects_a_user_mapped_buffer() {
        let _mmram = comm_buffer_state();
        let external = PageAlignedMemory::new(2);
        {
            let mut pt_guard = security_state().lock_page_table();
            let pt = pt_guard.as_mut().expect("a page table is installed");
            pt.map_memory_region(external.base(), external.size(), MemoryAttributes::ExecuteProtect)
                .expect("map the external buffer");
        }

        assert_rejected(external.base(), external.size(), "is mapped user-accessible");
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_rejects_an_unmapped_buffer() {
        let _mmram = comm_buffer_state();
        let external = PageAlignedMemory::new(2);

        assert_rejected(external.base(), external.size(), "is unmapped");
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_queries_the_whole_buffer() {
        // Only the first page is mapped supervisor-only. A check that looked at the first page
        // alone would accept a buffer whose tail Ring 3 cannot be shown not to reach.
        let _mmram = comm_buffer_state();
        let external = PageAlignedMemory::new(2);
        {
            let attrs = MemoryAttributes::ExecuteProtect | MemoryAttributes::Supervisor;
            let mut pt_guard = security_state().lock_page_table();
            let pt = pt_guard.as_mut().expect("a page table is installed");
            pt.map_memory_region(external.base(), UEFI_PAGE_SIZE as u64, attrs).expect("map the first page");
        }

        assert_rejected(external.base(), external.size(), "not uniformly mapped");
    }

    #[test]
    #[serial]
    fn test_from_supv_hob_builds_a_channel_from_a_buffer_outside_mmram() {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);

        // The MM IPL's buffer lives outside MMRAM and is mapped supervisor-only.
        let external = PageAlignedMemory::new(3);
        map_supervisor_only(&external);
        let status = external.base() + 2 * UEFI_PAGE_SIZE as u64;
        let data = supv_comm_buffer_hob_data(external.base(), 2, status);

        let channel = CommChannel::from_supv_hob(&data).expect("build the supervisor channel");

        assert_eq!(channel.external, external.base());
        assert_eq!(channel.size, 2 * UEFI_PAGE_SIZE as u64);
        assert_eq!(channel.status, status);
        // Ring 3 works on the internal copy, so it comes from supervisor-owned MMRAM.
        assert_eq!(
            security_state().page_allocator().get_allocation_type(channel.internal),
            Some(AllocationType::Supervisor)
        );
    }

    #[test]
    #[serial]
    fn test_from_supv_hob_rejects_a_buffer_inside_mmram() {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);

        // A buffer the MM IPL placed inside MMRAM turns the supervisor's response copy into an
        // MMRAM write with a payload chosen outside MM.
        let data = supv_comm_buffer_hob_data(mmram.base(), 1, mmram.base());

        let result = catch_unwind(|| CommChannel::from_supv_hob(&data));

        assert!(result.is_err(), "a communication buffer inside MMRAM produced a channel");
    }

    #[test]
    #[serial]
    fn test_from_user_hob_redirects_the_hob_to_the_internal_copy() {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);

        let external = PageAlignedMemory::new(3);
        map_supervisor_only(&external);
        let status = external.base() + 2 * UEFI_PAGE_SIZE as u64;
        let mut data = user_comm_buffer_hob_data(external.base(), 2, status);

        // SAFETY: `data` is a live, writable payload of exactly one `MmCommonBufferHobData`, and
        // no references into it are held across the call.
        let channel =
            unsafe { CommChannel::from_user_hob(data.as_mut_ptr(), data.len()) }.expect("build the user channel");

        assert_eq!(channel.external, external.base());
        assert_eq!(channel.size, 2 * UEFI_PAGE_SIZE as u64);
        assert_eq!(channel.status, status);
        assert_eq!(security_state().page_allocator().get_allocation_type(channel.internal), Some(AllocationType::User));
        // The user module reads the HOB after demotion, so it must name the internal copy.
        assert_eq!(u64::from_ne_bytes(data[0..8].try_into().expect("eight bytes")), channel.internal);
    }

    #[test]
    #[serial]
    fn test_from_user_hob_rejects_a_user_mapped_buffer() {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);

        // A user-accessible buffer would let a demoted driver rewrite a request while it is
        // being serviced.
        let external = PageAlignedMemory::new(2);
        {
            let mut pt_guard = security_state().lock_page_table();
            let pt = pt_guard.as_mut().expect("a page table is installed");
            pt.map_memory_region(external.base(), external.size(), MemoryAttributes::ExecuteProtect)
                .expect("map the external buffer");
        }
        let mut data = user_comm_buffer_hob_data(external.base(), 1, external.base() + UEFI_PAGE_SIZE as u64);

        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `data` is a live, writable payload of exactly one `MmCommonBufferHobData`.
            unsafe { CommChannel::from_user_hob(data.as_mut_ptr(), data.len()) }
        }));

        assert!(result.is_err(), "a user-accessible communication buffer produced a channel");
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_truncated_data() {
        let supv = supv_comm_buffer_hob_data(0x10_0000, 1, 0x20_0000);
        let mut user = user_comm_buffer_hob_data(0x30_0000, 1, 0x40_0000);
        let user_len = user.len() - 1;

        // A short payload is rejected while reading the HOB, before anything is allocated, so
        // these reach no global state.
        assert_eq!(
            CommChannel::from_supv_hob(&supv[..supv.len() - 1]),
            Err(CommBufferError::CommRegionHobTooSmall {
                found: supv.len() - 1,
                expected: size_of::<MmCommonRegionHobData>()
            }
            .into())
        );
        // SAFETY: `user` is a live, writable payload and no references into it are held across
        // the call. The truncated length stops the read before the HOB is rewritten.
        let user_result = unsafe { CommChannel::from_user_hob(user.as_mut_ptr(), user_len) };
        assert_eq!(
            user_result,
            Err(CommBufferError::CommunicationBufferHobTooSmall {
                found: user_len,
                expected: size_of::<MmCommonBufferHobData>()
            }
            .into())
        );
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_invalid_ranges() {
        for (address, pages) in [(0x1000, 0), (0x1000, u64::MAX), (u64::MAX - 0xFFF, 1)] {
            // The range is checked before the external buffer is required or anything is
            // allocated, so a bad page count is reported without reaching global state.
            assert_eq!(
                CommChannel::from_supv_hob(&supv_comm_buffer_hob_data(address, pages, 0x20_0000)),
                Err(CommBufferError::InvalidSize { pages }.into())
            );

            let mut user = user_comm_buffer_hob_data(address, pages, 0x20_0000);
            let user_len = user.len();
            // SAFETY: `user` is a live, writable payload and no references into it are held
            // across the call. The range check stops the work before the HOB is rewritten.
            let user_result = unsafe { CommChannel::from_user_hob(user.as_mut_ptr(), user_len) };
            assert_eq!(user_result, Err(CommBufferError::InvalidSize { pages }.into()));
        }
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_a_status_buffer_past_the_end_of_the_address_space() {
        // The status range is checked before either buffer is required or anything is allocated,
        // so this is reported without reaching global state.
        let status = u64::MAX - 1;
        assert_eq!(
            CommChannel::from_supv_hob(&supv_comm_buffer_hob_data(0x1000, 1, status)),
            Err(CommBufferError::InvalidStatusBuffer { address: status }.into())
        );

        let mut user = user_comm_buffer_hob_data(0x1000, 1, status);
        let user_len = user.len();
        // SAFETY: `user` is a live, writable payload and no references into it are held across
        // the call. The range check stops the work before the HOB is rewritten.
        let user_result = unsafe { CommChannel::from_user_hob(user.as_mut_ptr(), user_len) };
        assert_eq!(user_result, Err(CommBufferError::InvalidStatusBuffer { address: status }.into()));
    }

    #[test]
    fn test_comm_buffer_error_displays_each_variant() {
        let errors = [
            CommBufferError::CommRegionHobMissing,
            CommBufferError::CommunicationBufferHobMissing,
            CommBufferError::CommRegionHobTooSmall { found: 31, expected: 32 },
            CommBufferError::CommunicationBufferHobTooSmall { found: 31, expected: 32 },
            CommBufferError::InvalidSize { pages: 0 },
            CommBufferError::InvalidStatusBuffer { address: u64::MAX },
            CommBufferError::AllocationFailed,
            CommBufferError::Missing,
        ];
        for err in errors {
            assert!(!format!("{err}").is_empty(), "every variant must render a message");
        }

        // The two short payload variants carry the same sizes, so the HOB each one names is the
        // only thing telling them apart in the log.
        assert_ne!(
            format!("{}", CommBufferError::CommRegionHobTooSmall { found: 31, expected: 32 }),
            format!("{}", CommBufferError::CommunicationBufferHobTooSmall { found: 31, expected: 32 })
        );
    }

    #[test]
    fn test_comm_region_hob_layout_matches_c_abi() {
        assert_eq!(size_of::<MmCommonRegionHobData>(), 32);
    }
}
