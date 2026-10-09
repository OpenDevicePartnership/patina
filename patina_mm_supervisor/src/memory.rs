//! Memory Management
//!
//! What the MM Supervisor Core knows about memory, and the allocators it carves that memory
//! up with. [`AllocError`] is the failure both allocators report.
//! - [`mmram`] - a region of MMRAM, the bound established over them, and where a range sits
//!   relative to that bound
//! - [`page_ownership`] - whether an address range belongs to Ring 0 or Ring 3
//! - [`page_allocator`] - SMRAM page-granularity allocator for general use
//! - [`paging_allocator`] - dedicated bump allocator for page table structures
//! - [`locked_state`] - the locked view over the bookkeeping the page allocator keeps in SMRAM
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
            AllocError::UnmapFailed,
            AllocError::InvalidSize,
            AllocError::PoolTooSmall,
        ];

        let mut rendered = [""; 9];
        for (slot, error) in rendered.iter_mut().zip(all) {
            let text: &'static str = match error {
                AllocError::NotInitialized => "the allocator has not been initialized",
                AllocError::AlreadyInitialized => "the allocator is already initialized",
                AllocError::OutOfMemory => "no free pages are available to satisfy the request",
                AllocError::InvalidAlignment => "the requested address or alignment is not page aligned",
                AllocError::InvalidAddress => "the address is not within any known SMRAM region",
                AllocError::NotAllocated => "the address was not previously allocated",
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
        for (i, left) in rendered.iter().enumerate() {
            for right in rendered.iter().skip(i + 1) {
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
