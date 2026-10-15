//! Page Table Ownership Queries for the MM Supervisor Core
//!
//! Provides helpers to determine whether a memory range is owned by the
//! supervisor (CPL0) or the user module (CPL3), based on the page table's
//! `Supervisor` attribute (the X64 U/S bit).
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
    /// The page is user-accessible (U/S = 1, `SpecialPurpose` clear).
    User,
    /// The page is supervisor-only (U/S = 0, `SpecialPurpose` set).
    Supervisor,
}

/// Queries the page table to determine the ownership (user vs supervisor) of `range`.
///
/// The range is widened to whole pages before querying.
///
/// Checks the `Supervisor` attribute which maps to the U/S bit on X64:
///   - `Supervisor` set  => `PageOwnership::Supervisor` (U/S = 0)
///   - `Supervisor` clear => `PageOwnership::User` (U/S = 1)
///
/// Returns `None` if the range cannot be widened to whole pages, the page table is not
/// initialized, or the range is unmapped.
pub(crate) fn query_address_ownership(range: &MemoryRange) -> Option<PageOwnership> {
    let pages = range.align_outward(UEFI_PAGE_SIZE as u64).ok()?;
    let attrs = {
        let page_table = security_state().lock_page_table();
        page_table.as_ref()?.query_memory_region(pages.base(), pages.size()).ok()?
    };
    log::trace!("Queried page ownership for address range {pages}: attributes={attrs:?}");
    if attrs.contains(MemoryAttributes::Supervisor) {
        Some(PageOwnership::Supervisor)
    } else {
        Some(PageOwnership::User)
    }
}
