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
    fn test_write_zeros_volatile_clears_exactly_the_range() {
        // Every start offset within a word and lengths across zero, a partial word and several
        // words, so the head, the aligned words and the tail are each exercised.
        for start in 0..8 {
            for len in [0, 1, 7, 8, 9, 23, 64] {
                let mut buffer = [0xA5A5_A5A5_A5A5_A5A5u64; 12];
                let bytes = buffer.as_mut_ptr().cast::<u8>();
                // SAFETY: `start + len` is at most 71, inside the 96-byte buffer.
                unsafe { write_zeros_volatile(bytes.add(start), len) };
                // SAFETY: the same 96 bytes, read once the writes are done.
                let after = unsafe { core::slice::from_raw_parts(bytes, 96) };
                for (offset, byte) in after.iter().enumerate() {
                    let expected = if (start..start + len).contains(&offset) { 0 } else { 0xA5 };
                    assert_eq!(*byte, expected, "byte {offset} for start {start}, len {len}");
                }
            }
        }
    }

    #[test]
    fn test_copy_volatile_copies_exactly_the_range() {
        // Source and destination offsets that share a word offset take the word path, and ones
        // that differ take the byte path. The lengths cover the head, the words and the tail.
        let source: [u8; 96] = core::array::from_fn(|index| index as u8 + 1);
        for src_start in 0..8 {
            for dst_start in 0..8 {
                for len in [0, 1, 7, 8, 9, 23, 64] {
                    let mut buffer = [0xA5A5_A5A5_A5A5_A5A5u64; 12];
                    let bytes = buffer.as_mut_ptr().cast::<u8>();
                    // SAFETY: both ranges end by byte 71 of their 96-byte buffers, which are
                    // separate arrays.
                    unsafe { copy_volatile(source.as_ptr().add(src_start), bytes.add(dst_start), len) };
                    // SAFETY: the same 96 bytes, read once the writes are done.
                    let after = unsafe { core::slice::from_raw_parts(bytes, 96) };
                    for (offset, byte) in after.iter().enumerate() {
                        let expected = if (dst_start..dst_start + len).contains(&offset) {
                            source[src_start + offset - dst_start]
                        } else {
                            0xA5
                        };
                        assert_eq!(*byte, expected, "byte {offset} for src {src_start}, dst {dst_start}, len {len}");
                    }
                }
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
