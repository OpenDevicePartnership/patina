//! Memory Management sub module used by MM Supervisor Core
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub(crate) mod locked_state;
pub(crate) mod mmram;
pub(crate) mod page_allocator;
pub(crate) mod page_ownership;
pub(crate) mod paging_allocator;

pub(crate) use page_allocator::{AllocationType, PageAllocator};
pub(crate) use paging_allocator::{DEFAULT_PAGING_POOL_PAGES, PagingPoolAllocator, SharedPagingAllocator};

/// The UEFI page size as a step for [`MemoryRange::step_by`](patina::MemoryRange::step_by).
pub(crate) const PAGE_STEP: core::num::NonZeroU64 =
    core::num::NonZeroU64::new(patina::UEFI_PAGE_SIZE as u64).expect("the UEFI page size is not zero");

/// Errors reported by the memory allocators.
///
/// Both allocators share this type. They differ in the backing structure, not in the ways a
/// request can fail, and the overlapping half of two separate enums only forced callers to convert
/// between spellings of the same condition.
///
/// Not every variant can arise from every allocator: `NotAllocated`, `InvalidAddress` and
/// `UnmapFailed` come from the page allocator's free path, while `InvalidSize` and `PoolTooSmall`
/// come from the paging pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocError {
    /// The allocator has not been initialized.
    NotInitialized,
    /// The allocator has already been initialized.
    AlreadyInitialized,
    /// No free pages available to satisfy the request.
    OutOfMemory,
    /// The requested address or alignment is not page aligned.
    InvalidAlignment,
    /// The address is not within any known SMRAM region.
    InvalidAddress,
    /// The address was not previously allocated.
    NotAllocated,
    /// The freed range could not be made inaccessible in the page table, so it was left allocated.
    UnmapFailed,
    /// Invalid allocation size requested.
    InvalidSize,
    /// The pool region is too small.
    PoolTooSmall,
}

impl core::error::Error for AllocError {}

impl core::fmt::Display for AllocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "the allocator has not been initialized"),
            Self::AlreadyInitialized => write!(f, "the allocator is already initialized"),
            Self::OutOfMemory => write!(f, "no free pages are available to satisfy the request"),
            Self::InvalidAlignment => write!(f, "the requested address or alignment is not page aligned"),
            Self::InvalidAddress => write!(f, "the address is not within any known SMRAM region"),
            Self::NotAllocated => write!(f, "the address was not previously allocated"),
            Self::UnmapFailed => {
                write!(f, "the freed range could not be made inaccessible in the page table, so it stays allocated")
            }
            Self::InvalidSize => write!(f, "the requested allocation size is invalid"),
            Self::PoolTooSmall => write!(f, "the pool region is too small"),
        }
    }
}
