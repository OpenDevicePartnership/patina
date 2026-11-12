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
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::UEFI_PAGE_SIZE;
    use serial_test::serial;

    use crate::memory;
    use crate::test_support;
    use crate::test_support::*;
    use patina_paging::{MemoryAttributes, PageTable};

    #[test]
    fn test_comm_buffer_config_default_is_zeroed() {
        let cfg = CommBufferConfig::default();
        assert_eq!(cfg.supervisor, CommChannel::default());
        assert_eq!(cfg.user, CommChannel::default());
        assert_eq!(cfg.supervisor.external_address, 0);
        assert_eq!(cfg.supervisor.internal_address, 0);
        assert_eq!(cfg.supervisor.size, 0);
        assert_eq!(cfg.supervisor.status_address, 0);
        assert_eq!(cfg.user.external_address, 0);
        assert_eq!(cfg.user.internal_address, 0);
        assert_eq!(cfg.user.size, 0);
        assert_eq!(cfg.user.status_address, 0);
        assert_eq!(cfg.supv_to_user_buffer, 0);
        assert_eq!(cfg.supv_to_user_buffer_size, 0);
    }

    #[test]
    fn test_comm_buffer_config_is_copy() {
        let cfg = CommBufferConfig {
            supervisor: CommChannel { external_address: 0x1000, ..Default::default() },
            user: CommChannel { external_address: 0x2000, ..Default::default() },
            ..Default::default()
        };
        let copied = cfg; // relies on `Copy`
        assert_eq!(copied.supervisor.external_address, 0x1000);
        assert_eq!(copied.user.external_address, 0x2000);
        // `cfg` is still usable after the copy.
        assert_eq!(cfg.supervisor.external_address, 0x1000);
    }

    /// Installs MMRAM and an empty page table, and returns the memory backing MMRAM.
    fn comm_buffer_state() -> PageAlignedMemory {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);
        mmram
    }

    /// Runs [`require_external_comm_buffer`] and returns its error message, or `None` if it
    /// accepted the buffer.
    fn rejection(range: MemoryRange) -> Option<String> {
        require_external_comm_buffer(&range, "Test buffer").err().map(|error| error.to_string())
    }

    /// Asserts that [`require_external_comm_buffer`] refuses the buffer with a message containing
    /// `expected`.
    fn assert_rejected(range: MemoryRange, expected: &str) {
        let message = rejection(range).expect("the buffer was accepted");
        assert!(message.contains(expected), "expected a rejection containing {expected:?}, got {message:?}");
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_accepts_a_supervisor_mapped_buffer_outside_mmram() {
        let _mmram = comm_buffer_state();
        let external = PageAlignedMemory::new(2);
        map_supervisor_only(&external);

        assert_eq!(rejection(external.range()), None);
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_rejects_a_buffer_touching_mmram() {
        // The copy-back would otherwise turn a payload chosen outside MM into an MMRAM write. The
        // MMRAM pages are not mapped in this page table, so these are refused for touching MMRAM
        // before the ownership query could refuse them as unmapped.
        let mmram = comm_buffer_state();
        let page = UEFI_PAGE_SIZE as u64;

        assert_rejected(MemoryRange::new(mmram.base(), page).unwrap(), "overlaps MMRAM");
        // Only the tail of this buffer reaches MMRAM.
        assert_rejected(MemoryRange::new(mmram.base() - page, 2 * page).unwrap(), "overlaps MMRAM");
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

        assert_rejected(external.range(), "is mapped user-accessible");
    }

    #[test]
    #[serial]
    fn test_require_external_comm_buffer_rejects_an_unmapped_buffer() {
        let _mmram = comm_buffer_state();
        let external = PageAlignedMemory::new(2);

        assert_rejected(external.range(), "is unmapped");
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

        assert_rejected(external.range(), "not uniformly mapped");
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

        assert_eq!(channel.external_address, external.base());
        assert_eq!(channel.size, 2 * UEFI_PAGE_SIZE as u64);
        assert_eq!(channel.status_address, status);
        // Ring 3 works on the internal copy, so it comes from supervisor-owned MMRAM.
        assert_eq!(
            security_state().page_allocator().get_allocation_type(channel.internal_address),
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

        let range = MemoryRange::from_pages(mmram.base(), 1).expect("one page at the MMRAM base");
        assert_eq!(CommChannel::from_supv_hob(&data), Err(CommBufferError::OverlapsMmram { range }.into()));
    }

    #[test]
    #[serial]
    fn test_from_user_hob_preserves_the_external_buffer_descriptor() {
        test_support::init_test_logger();
        let mmram = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 8);
        init_global_state_over(&mmram);

        let external = PageAlignedMemory::new(3);
        map_supervisor_only(&external);
        let status = external.base() + 2 * UEFI_PAGE_SIZE as u64;
        let data = user_comm_buffer_hob_data(external.base(), 2, status);
        let original = data;

        let channel = CommChannel::from_user_hob(&data).expect("build the user channel");

        assert_eq!(channel.external_address, external.base());
        assert_eq!(channel.size, 2 * UEFI_PAGE_SIZE as u64);
        assert_eq!(channel.status_address, status);
        assert_eq!(
            security_state().page_allocator().get_allocation_type(channel.internal_address),
            Some(AllocationType::User)
        );

        assert_ne!(channel.internal_address, channel.external_address);
        assert_eq!(data, original, "the HOB must continue to describe the external buffer and status mailbox");
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
        let data = user_comm_buffer_hob_data(external.base(), 1, external.base() + UEFI_PAGE_SIZE as u64);

        let result = CommChannel::from_user_hob(&data);

        let range = MemoryRange::from_pages(external.base(), 1).expect("one page at the buffer base");
        assert_eq!(result, Err(CommBufferError::UserAccessible { range }.into()));
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_truncated_data() {
        let supv = supv_comm_buffer_hob_data(0x10_0000, 1, 0x20_0000);
        let user = user_comm_buffer_hob_data(0x30_0000, 1, 0x40_0000);
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
        let user_result = CommChannel::from_user_hob(&user[..user_len]);
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

            let user = user_comm_buffer_hob_data(address, pages, 0x20_0000);
            assert_eq!(CommChannel::from_user_hob(&user), Err(CommBufferError::InvalidSize { pages }.into()));
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

        let user = user_comm_buffer_hob_data(0x1000, 1, status);
        assert_eq!(
            CommChannel::from_user_hob(&user),
            Err(CommBufferError::InvalidStatusBuffer { address: status }.into())
        );
    }

    #[test]
    fn test_parse_communication_buffer_hobs_reject_a_zero_address() {
        // Zero is checked before anything else is validated, so no global state is reached.
        let cases = [
            (0, 0x20_0000, "supervisor communication buffer", "user communication buffer"),
            (0x10_0000, 0, "supervisor status buffer", "user status buffer"),
        ];
        for (address, status, supv_buffer, user_buffer) in cases {
            assert_eq!(
                CommChannel::from_supv_hob(&supv_comm_buffer_hob_data(address, 1, status)),
                Err(CommBufferError::NullAddress { buffer: supv_buffer }.into())
            );
            assert_eq!(
                CommChannel::from_user_hob(&user_comm_buffer_hob_data(address, 1, status)),
                Err(CommBufferError::NullAddress { buffer: user_buffer }.into())
            );
        }
    }

    #[test]
    fn test_comm_channel_displays_every_address() {
        let channel =
            CommChannel { external_address: 0x1000, internal_address: 0x2000, size: 0x3000, status_address: 0x4000 };

        assert_eq!(
            format!("{channel}"),
            "external_address=0x1000 internal_address=0x2000 size=0x3000 status_address=0x4000"
        );
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
            CommBufferError::OverlapsMmram { range: MemoryRange::new(0x1000, 0x1000).expect("valid range") },
            CommBufferError::UserAccessible { range: MemoryRange::new(0x1000, 0x1000).expect("valid range") },
            CommBufferError::NotUniformlyMapped { range: MemoryRange::new(0x1000, 0x1000).expect("valid range") },
            CommBufferError::AllocationFailed,
            CommBufferError::NullAddress { buffer: "user status buffer" },
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
