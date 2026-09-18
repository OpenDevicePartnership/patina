//! Page Table Ownership Queries for the MM Supervisor Core
//!
//! Provides helpers to determine whether a memory range is owned by the supervisor
//! (CPL0) or the user module (CPL3), based on the page table's `Supervisor` attribute
//! (the X64 U/S bit).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{UEFI_PAGE_SIZE, align_range};
use patina_paging::{MemoryAttributes, PageTable};

use crate::state::security_state;

/// Result of a page table ownership query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageOwnership {
    /// The page is user-accessible (U/S = 1, `SpecialPurpose` clear).
    User,
    /// The page is supervisor-only (U/S = 0, `SpecialPurpose` set).
    Supervisor,
}

/// Queries the page table to determine the ownership (user vs supervisor) of an address.
///
/// The address and size are page-aligned before querying (rounded down / up respectively).
///
/// Checks the `Supervisor` attribute which maps to the U/S bit on X64:
///   - `Supervisor` set  => `PageOwnership::Supervisor` (U/S = 0)
///   - `Supervisor` clear => `PageOwnership::User` (U/S = 1)
///
/// Returns `None` if the range is empty or invalid, the page table is not initialized,
/// or the address is unmapped.
pub(crate) fn query_address_ownership(address: u64, size: u64) -> Option<PageOwnership> {
    query_address_ownership_with(address, size, |aligned_addr, aligned_size| {
        let page_table = security_state().lock_page_table();
        page_table.as_ref()?.query_memory_region(aligned_addr, aligned_size).ok()
    })
}

fn query_address_ownership_with(
    address: u64,
    size: u64,
    query: impl FnOnce(u64, u64) -> Option<MemoryAttributes>,
) -> Option<PageOwnership> {
    if size == 0 || address.checked_add(size).is_none() {
        return None;
    }

    let (aligned_addr, aligned_size) = align_range(address, size, UEFI_PAGE_SIZE as u64).ok()?;
    let aligned_end = aligned_addr.checked_add(aligned_size)?;
    let attrs = query(aligned_addr, aligned_size)?;
    log::trace!(
        "Queried page ownership for address range 0x{aligned_addr:016x}-0x{aligned_end:016x}: attributes={attrs:?}"
    );
    if attrs.contains(MemoryAttributes::Supervisor) {
        Some(PageOwnership::Supervisor)
    } else {
        Some(PageOwnership::User)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    #[test]
    fn test_page_ownership_is_copy_and_comparable() {
        let owner = PageOwnership::Supervisor;
        let copied = owner;
        assert_eq!(owner, copied);
        assert_ne!(PageOwnership::Supervisor, PageOwnership::User);
    }

    #[test]
    fn test_query_address_ownership_aligns_range_and_identifies_user_pages() {
        let queried_range = Cell::new(None);

        let ownership = query_address_ownership_with(0x1234, 0x1000, |address, size| {
            queried_range.set(Some((address, size)));
            Some(MemoryAttributes::Writeback | MemoryAttributes::ExecuteProtect)
        });

        assert_eq!(ownership, Some(PageOwnership::User));
        assert_eq!(queried_range.get(), Some((0x1000, 0x2000)));
    }

    #[test]
    fn test_query_address_ownership_identifies_supervisor_pages() {
        let ownership = query_address_ownership_with(0x2000, 0x1000, |_, _| {
            Some(MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly)
        });

        assert_eq!(ownership, Some(PageOwnership::Supervisor));
    }

    #[test]
    fn test_query_address_ownership_returns_none_when_query_fails() {
        assert_eq!(query_address_ownership_with(0x2000, 0x1000, |_, _| None), None);
    }

    #[test]
    fn test_query_address_ownership_rejects_invalid_ranges_without_querying() {
        for (address, size) in [(0x1000, 0), (u64::MAX, 1), (u64::MAX - 0xfff, 0xfff)] {
            let queried = Cell::new(false);

            assert_eq!(
                query_address_ownership_with(address, size, |_, _| {
                    queried.set(true);
                    Some(MemoryAttributes::empty())
                }),
                None
            );
            assert!(!queried.get(), "invalid range {address:#x}+{size:#x} was queried");
        }
    }
}
