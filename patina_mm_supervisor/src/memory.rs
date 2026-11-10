//! Memory Management for the MM Supervisor Core
//!
//! - [`mmram`] - the MMRAM regions and the bound the SMRR covers
//! - [`page_allocator`] - page allocation from MMRAM, with supervisor or user ownership
//! - [`paging_allocator`] - the pool that backs page table structures
//! - [`page_ownership`] - which ring owns a mapped range
//! - [`locked_state`] - the page allocator's bookkeeping, behind its lock
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

/// Errors reported by the page allocator and the paging pool allocator.
///
/// Not every variant can arise from both: `NotAllocated`, `InvalidAddress`, `TypeMismatch`,
/// `MapFailed` and `UnmapFailed` come from the page allocator, `PoolTooSmall` comes from the
/// paging pool, and `InvalidSize` can come from either.
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
    /// The pages were allocated with a different allocation type than the caller expected.
    TypeMismatch,
    /// The range could not be mapped as R/W, non-executable data in the page table.
    MapFailed,
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
            Self::TypeMismatch => write!(f, "the pages were allocated with a different allocation type"),
            Self::MapFailed => write!(f, "the range could not be mapped as data in the page table"),
            Self::UnmapFailed => {
                write!(f, "the freed range could not be made inaccessible in the page table, so it stays allocated")
            }
            Self::InvalidSize => write!(f, "the requested allocation size is invalid"),
            Self::PoolTooSmall => write!(f, "the pool region is too small"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_error_renders_each_variant_distinctly() {
        // Both allocators report through this one type, so a reader has only the message to tell
        // which condition was hit. The table is exhaustive, so a new variant without a Display
        // arm fails to compile here rather than rendering as another variant's text.
        let all = [
            AllocError::NotInitialized,
            AllocError::AlreadyInitialized,
            AllocError::OutOfMemory,
            AllocError::InvalidAlignment,
            AllocError::InvalidAddress,
            AllocError::NotAllocated,
            AllocError::TypeMismatch,
            AllocError::MapFailed,
            AllocError::UnmapFailed,
            AllocError::InvalidSize,
            AllocError::PoolTooSmall,
        ];

        let mut rendered = [""; 11];
        for (slot, error) in rendered.iter_mut().zip(all) {
            let text: &'static str = match error {
                AllocError::NotInitialized => "the allocator has not been initialized",
                AllocError::AlreadyInitialized => "the allocator is already initialized",
                AllocError::OutOfMemory => "no free pages are available to satisfy the request",
                AllocError::InvalidAlignment => "the requested address or alignment is not page aligned",
                AllocError::InvalidAddress => "the address is not within any known SMRAM region",
                AllocError::NotAllocated => "the address was not previously allocated",
                AllocError::TypeMismatch => "the pages were allocated with a different allocation type",
                AllocError::MapFailed => "the range could not be mapped as data in the page table",
                AllocError::UnmapFailed => {
                    "the freed range could not be made inaccessible in the page table, so it stays allocated"
                }
                AllocError::InvalidSize => "the requested allocation size is invalid",
                AllocError::PoolTooSmall => "the pool region is too small",
            };
            assert_eq!(format!("{error}"), text);
            *slot = text;
        }

        // The two "not initialized" readings are the pair most easily confused, so they must not
        // render the same way.
        assert_ne!(format!("{}", AllocError::NotInitialized), format!("{}", AllocError::AlreadyInitialized));
        for (index, left) in rendered.iter().enumerate() {
            for right in rendered.iter().skip(index + 1) {
                assert_ne!(left, right, "two AllocError variants render identically");
            }
        }
    }

    #[test]
    fn test_alloc_error_is_an_error_without_a_source() {
        use core::error::Error;

        // AllocError is a leaf: it names the condition itself rather than wrapping another error.
        assert!(AllocError::OutOfMemory.source().is_none());
    }
}
