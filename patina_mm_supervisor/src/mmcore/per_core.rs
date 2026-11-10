//! Per-Core Storage
//!
//! The per-core arrays of the [`CpuManager`](super::cpu::CpuManager) and the
//! [`MailboxManager`](super::mailbox::MailboxManager) are sized from the core count the MP
//! Information HOB reports. The BSP allocates their storage from the supervisor page allocator
//! during initialization, and the pages stay allocated for the supervisor's lifetime.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::mem::{align_of, size_of};

use patina::UEFI_PAGE_SIZE;

use crate::{
    error::MmSupervisorResult,
    memory::{AllocError, PageAllocator},
};

/// Allocates supervisor pages for `count` values of `T`, sets each to `T::default()`, and returns
/// them as a slice. The pages are never freed.
///
/// After the page table is installed, the page allocator maps the pages supervisor-only,
/// read/write and non-executable.
///
/// ## Errors
///
/// Returns [`AllocError::InvalidSize`] when `count` is zero or its byte size overflows, and the
/// page allocator's error when no run of free pages is large enough.
pub(crate) fn allocate_per_core_slice<T: Default>(
    page_allocator: &PageAllocator,
    count: usize,
) -> MmSupervisorResult<&'static [T]> {
    const { assert!(align_of::<T>() <= UEFI_PAGE_SIZE, "per-core storage starts on a page boundary") };

    let bytes = count.checked_mul(size_of::<T>()).ok_or(AllocError::InvalidSize)?;
    // `div_ceil` instead of `uefi_size_to_pages!`, whose `bytes + UEFI_PAGE_MASK` can overflow
    // for a byte count that `checked_mul` accepted.
    let base = page_allocator.allocate_pages(bytes.div_ceil(UEFI_PAGE_SIZE))?;
    let address = usize::try_from(base).map_err(|_| AllocError::InvalidAddress)?;
    let first = core::ptr::with_exposed_provenance_mut::<T>(address);

    // SAFETY: `allocate_pages` returned a page-aligned run of at least `bytes` bytes that nothing
    // else owns, so all `count` elements are in bounds, and a page boundary satisfies the
    // alignment of `T`, as the assertion above checks. `write` stores each value without reading
    // or dropping the old contents of the pages. Every element is written before the slice is
    // formed, and the pages are never freed, so the slice can be `'static`.
    unsafe {
        for index in 0..count {
            first.add(index).write(T::default());
        }
        Ok(core::slice::from_raw_parts(first, count))
    }
}
