//! Memory Management for the MM Supervisor Core
//!
//! - [`mmram`] - the MMRAM regions and the bound the SMRR covers
//! - [`page_allocator`] - page allocation from MMRAM, with supervisor or user ownership
//! - [`paging_allocator`] - the pool that backs page table structures
//! - [`page_ownership`] - which ring owns a mapped range
//! - [`locked_state`] - the page allocator's bookkeeping, behind its lock
//! - [`validation`] - the checks on the page table adopted from MM Init
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
pub(crate) mod validation;

pub(crate) use page_allocator::{AllocationType, PageAllocator};
pub(crate) use paging_allocator::{DEFAULT_PAGING_POOL_PAGES, PagingPoolAllocator, SharedPagingAllocator};

/// The UEFI page size as a step for [`MemoryRange::step_by`](patina::MemoryRange::step_by).
pub(crate) const PAGE_STEP: core::num::NonZeroU64 =
    core::num::NonZeroU64::new(patina::UEFI_PAGE_SIZE as u64).expect("the UEFI page size is not zero");

/// Writes zero to the `len` bytes at `base` with volatile stores.
///
/// The cleared bytes are read later by Ring 3, the non-MM world or another allocation, which the
/// compiler cannot see, so an ordinary `write_bytes` could be elided. The aligned middle is
/// cleared a `u64` at a time, any unaligned head or tail a byte at a time.
///
/// ## Safety
///
/// `base` must be valid for writes of `len` bytes.
pub(crate) unsafe fn write_zeros_volatile(base: *mut u8, len: usize) {
    const QWORD: usize = core::mem::size_of::<u64>();

    // `align_offset` may report no usable offset, which only means every byte is written alone.
    let head = base.align_offset(core::mem::align_of::<u64>()).min(len);
    let words = (len - head) / QWORD;
    let tail = head + words * QWORD;

    // SAFETY: the caller guarantees `[base, base + len)` is writable, and every offset below is
    // inside it. `base + head` is aligned for `u64`, so the word stores are aligned.
    unsafe {
        for offset in (0..head).chain(tail..len) {
            base.add(offset).write_volatile(0);
        }
        let word_base = base.add(head).cast::<u64>();
        for index in 0..words {
            word_base.add(index).write_volatile(0);
        }
    }
}

/// Copies `len` bytes from `src` to `dst` with volatile reads and writes.
///
/// Ring 3 or the non-MM world may change the source or read the destination, which the compiler
/// cannot see. Volatile accesses read each source byte exactly once and always store it, where
/// `copy_nonoverlapping` could be elided or re-read. When both pointers share an offset within a
/// `u64`, the aligned middle is copied a `u64` at a time; everything else a byte at a time.
///
/// ## Safety
///
/// `src` must be valid for reads of `len` bytes, `dst` must be valid for writes of `len` bytes,
/// and the two ranges must not overlap.
pub(crate) unsafe fn copy_volatile(src: *const u8, dst: *mut u8, len: usize) {
    const QWORD: usize = core::mem::size_of::<u64>();

    // Words need one offset that aligns both pointers. `align_offset` may report no usable
    // offset, which only means every byte is copied alone.
    let head = if src.addr() % QWORD == dst.addr() % QWORD {
        dst.align_offset(core::mem::align_of::<u64>()).min(len)
    } else {
        len
    };
    let words = (len - head) / QWORD;
    let tail = head + words * QWORD;

    // SAFETY: the caller guarantees both ranges are valid for `len` bytes and do not overlap, and
    // every offset below is inside them. `src + head` and `dst + head` are both aligned for `u64`
    // whenever `words` is not zero, so the word accesses are aligned.
    unsafe {
        for offset in 0..head {
            dst.add(offset).write_volatile(src.add(offset).read_volatile());
        }
        let (src_words, dst_words) = (src.add(head).cast::<u64>(), dst.add(head).cast::<u64>());
        for index in 0..words {
            dst_words.add(index).write_volatile(src_words.add(index).read_volatile());
        }
        for offset in tail..len {
            dst.add(offset).write_volatile(src.add(offset).read_volatile());
        }
    }
}

/// Errors reported by the page allocator and the paging pool allocator.
///
/// `NotAllocated`, `InvalidAddress`, `TypeMismatch`, `MapFailed` and `UnmapFailed` are page
/// allocator errors; `PoolTooSmall` is a paging pool error.
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
