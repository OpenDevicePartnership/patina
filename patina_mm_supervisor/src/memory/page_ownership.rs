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
