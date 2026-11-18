//! Page Table Ownership Queries for the MM Supervisor Core
//!
//! Determines whether a memory range is owned by the supervisor (CPL0) or the user module
//! (CPL3), from the page table's `Supervisor` attribute (the X64 U/S bit).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{MemoryRange, UEFI_PAGE_SIZE};
use patina_paging::{MemoryAttributes, PageTable};

use crate::state::security_state;

/// Result of a page table ownership query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageOwnership {
    /// The page is user-accessible (U/S = 1, `Supervisor` attribute clear).
    User,
    /// The page is supervisor-only (U/S = 0, `Supervisor` attribute set).
    Supervisor,
}

/// Returns which ring owns `range`, read from the page table.
///
/// The range is widened to whole pages. Returns `None` if that overflows, the page table is not
/// installed, or the range is unmapped.
pub(crate) fn query_address_ownership(range: &MemoryRange) -> Option<PageOwnership> {
    let attrs = query_attributes(range)?;
    if attrs.contains(MemoryAttributes::Supervisor) {
        Some(PageOwnership::Supervisor)
    } else {
        Some(PageOwnership::User)
    }
}

/// Returns whether every page of `range` is mapped user-accessible and writable.
///
/// Returns `false` under the same conditions [`query_address_ownership`] returns `None`.
pub(crate) fn is_user_writable(range: &MemoryRange) -> bool {
    query_attributes(range)
        .is_some_and(|attrs| !attrs.intersects(MemoryAttributes::Supervisor | MemoryAttributes::ReadOnly))
}

/// Returns the attributes of `range`, widened to whole pages, from the page table.
fn query_attributes(range: &MemoryRange) -> Option<MemoryAttributes> {
    let pages = range.align_outward(UEFI_PAGE_SIZE as u64).ok()?;
    let attrs = {
        let page_table = security_state().lock_page_table();
        page_table.as_ref()?.query_memory_region(pages.base(), pages.size()).ok()?
    };
    log::trace!("Queried page attributes for address range {pages}: attributes={attrs:?}");
    Some(attrs)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{PageAlignedMemory, init_global_state_over};

    #[test]
    fn test_page_ownership_is_copy_and_comparable() {
        let owner = PageOwnership::Supervisor;
        let copied = owner;
        assert_eq!(owner, copied);
        assert_ne!(PageOwnership::Supervisor, PageOwnership::User);
    }

    fn mr(base: u64, size: u64) -> MemoryRange {
        MemoryRange::new(base, size).unwrap()
    }

    fn map(range: &MemoryRange, attributes: MemoryAttributes) {
        let mut page_table = security_state().lock_page_table();
        let page_table = page_table.as_mut().expect("a page table is installed");
        page_table.map_memory_region(range.base(), range.size(), attributes).expect("map the test pages");
    }

    #[test]
    #[serial_test::serial]
    fn test_query_address_ownership_returns_none_without_page_table() {
        *security_state().lock_page_table() = None;
        assert_eq!(query_address_ownership(&mr(0x2000, 0x1000)), None);
    }

    #[test]
    #[serial_test::serial]
    fn test_query_address_ownership_identifies_user_and_supervisor_pages() {
        let pool = PageAlignedMemory::new(crate::memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&pool);
        let buffer = PageAlignedMemory::new(2);
        let range = mr(buffer.base() + 0x234, 0x1000);

        map(&buffer.range(), MemoryAttributes::ExecuteProtect);
        assert_eq!(query_address_ownership(&range), Some(PageOwnership::User));
        assert!(is_user_writable(&range));

        // A user page that is read-only is still user-owned, but cannot be written.
        map(&buffer.range(), MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadOnly);
        assert_eq!(query_address_ownership(&range), Some(PageOwnership::User));
        assert!(!is_user_writable(&range));

        map(&buffer.range(), MemoryAttributes::ExecuteProtect | MemoryAttributes::Supervisor);
        assert_eq!(query_address_ownership(&range), Some(PageOwnership::Supervisor));
        assert!(!is_user_writable(&range));

        *security_state().lock_page_table() = None;
        assert!(!is_user_writable(&range));
    }

    #[test]
    #[serial_test::serial]
    fn test_query_address_ownership_widens_range_to_whole_pages() {
        let pool = PageAlignedMemory::new(crate::memory::DEFAULT_PAGING_POOL_PAGES + 32);
        init_global_state_over(&pool);
        let buffer = PageAlignedMemory::new(2);
        map(&mr(buffer.base(), UEFI_PAGE_SIZE as u64), MemoryAttributes::ExecuteProtect);

        // Both ranges start inside the mapped first page. Only the second one reaches into the
        // unmapped second page, so only its query fails.
        assert_eq!(query_address_ownership(&mr(buffer.base() + 0x234, 0x10)), Some(PageOwnership::User));
        assert_eq!(query_address_ownership(&mr(buffer.base() + 0x234, 0x1000)), None);

        // A range in the last page cannot be widened, because its rounded end would pass u64::MAX.
        assert_eq!(query_address_ownership(&mr(u64::MAX - 0xfff, 0xfff)), None);

        *security_state().lock_page_table() = None;
    }
}
