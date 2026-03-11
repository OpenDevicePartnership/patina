//! MM User Core Memory Allocator
//!
//! Provides [`SyscallPageAllocator`], a [`PageAllocatorBackend`] that allocates and frees
//! pages through syscalls to the MM Supervisor. On the UEFI target a `PoolAllocator` on
//! top of it is the `#[global_allocator]`.
//!
//! ## Syscall ABI
//!
//! The page syscalls (see `SyscallIndex` in `patina::management_mode::supervisor`):
//!
//! | Syscall       | RAX     | RDX (arg1)       | R8 (arg2)      | R9 (arg3)    |
//! |---------------|---------|------------------|----------------|--------------|
//! | `AllocPage`   | 0x10004 | `alloc_type` (0) | `mem_type` (6) | `page_count` |
//! | `FreePage`    | 0x10005 | address          | `page_count`   | 0            |
//!
//! The supervisor returns the result in RAX.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "uefi")]
use crate::pool_allocator::PoolAllocator;
use crate::pool_allocator::{PageAllocError, PageAllocatorBackend};
use patina::management_mode::supervisor::SyscallIndex;
#[cfg(not(test))]
use patina::management_mode::supervisor::raw_syscall;
use patina::standard::efi;

/// `AllocateAnyPages`: allocate any available pages.
const ALLOCATE_ANY_PAGES: u64 = efi::ALLOCATE_ANY_PAGES as u64;

/// `EfiRuntimeServicesData`: the memory type used for MM pool allocations.
const RUNTIME_SERVICES_DATA: u64 = efi::RUNTIME_SERVICES_DATA as u64;

/// Issues a supervisor syscall.
///
/// Host test builds call the controllable [`mock`] instead. The SDK's host stub of
/// `raw_syscall` returns `EFI_UNSUPPORTED`, a non-zero value that [`is_comm_buffer`] would
/// read as the supervisor accepting the range, so it cannot stand in for the real syscall.
///
/// ## Safety
///
/// Transfers control to the supervisor; the arguments must be valid for `call_index`.
unsafe fn syscall(call_index: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    #[cfg(not(test))]
    // SAFETY: Forwarded from the caller, which upholds the ABI contract for this syscall index.
    let result = unsafe { raw_syscall(call_index, arg1, arg2, arg3) };

    #[cfg(test)]
    let result = mock::syscall(call_index, arg1, arg2, arg3);

    result
}

/// Asks the supervisor, through the `MmIsCommBuffer` syscall, whether a range lies
/// entirely within the user communication buffer.
pub fn is_comm_buffer(address: u64, size: u64) -> bool {
    // SAFETY: `MmIsCommBuffer` only checks the range; it does not access the memory.
    let result = unsafe { syscall(SyscallIndex::MmIsCommBuffer.as_u64(), address, size, 0) };
    result != 0
}

/// A page allocator backend that allocates pages through MM Supervisor syscalls.
///
/// It backs the user core's [`PoolAllocator`](crate::pool_allocator::PoolAllocator) and
/// global allocator. Every request fails until
/// [`set_initialized`](SyscallPageAllocator::set_initialized) is called during
/// `StartUserCore`, before driver dispatch.
pub struct SyscallPageAllocator {
    /// Whether the syscall interface is ready. Until it is set, every request fails.
    initialized: AtomicBool,
}

impl Default for SyscallPageAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SyscallPageAllocator {
    /// Creates an allocator that fails every request until it is initialized.
    pub const fn new() -> Self {
        Self { initialized: AtomicBool::new(false) }
    }

    /// Marks the allocator ready. Call it once the syscall interface is available, early in
    /// `StartUserCore`.
    pub fn set_initialized(&self) {
        self.initialized.store(true, Ordering::Release);
        log::info!("SyscallPageAllocator initialized - heap is now available.");
    }
}

impl PageAllocatorBackend for SyscallPageAllocator {
    fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError> {
        if !self.initialized.load(Ordering::Acquire) {
            log::error!("SyscallPageAllocator: AllocPage({num_pages} pages) before the syscall interface is ready");
            return Err(PageAllocError::NotInitialized);
        }

        if num_pages == 0 {
            log::error!("SyscallPageAllocator: AllocPage rejected, page count is 0");
            return Err(PageAllocError::OutOfMemory);
        }

        // SAFETY: `AllocPage` takes scalar arguments and returns an address, and the
        // allocator was checked above to be initialized.
        let addr = unsafe {
            syscall(SyscallIndex::AllocPage.as_u64(), ALLOCATE_ANY_PAGES, RUNTIME_SERVICES_DATA, num_pages as u64)
        };

        if addr == 0 {
            log::warn!("SyscallPageAllocator: AllocPage({num_pages} pages) returned a null address");
            return Err(PageAllocError::OutOfMemory);
        }

        log::trace!("SyscallPageAllocator: allocated {num_pages} page(s) at 0x{addr:016x}");

        Ok(addr)
    }

    fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
        if !self.initialized.load(Ordering::Acquire) {
            log::error!(
                "SyscallPageAllocator: FreePage(0x{addr:016x}, {num_pages} pages) before the syscall interface is ready"
            );
            return Err(PageAllocError::NotInitialized);
        }

        // SAFETY: The caller of `free_pages` guarantees `addr`/`num_pages` describe a region
        // previously returned by `allocate_pages`, which the supervisor then reclaims.
        let status = unsafe { syscall(SyscallIndex::FreePage.as_u64(), addr, num_pages as u64, 0) };
        if status != 0 {
            log::error!("SyscallPageAllocator: FreePage(0x{addr:016x}, {num_pages} pages) failed with 0x{status:x}");
            return Err(PageAllocError::SyscallFailed(status));
        }

        log::trace!("SyscallPageAllocator: freed {num_pages} page(s) at 0x{addr:016x}");

        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

/// Global page allocator for the user core. [`SyscallPageAllocator::set_initialized`]
/// enables it during `StartUserCore`.
pub static SYSCALL_PAGE_ALLOCATOR: SyscallPageAllocator = SyscallPageAllocator::new();

/// Global pool allocator, a [`PoolAllocator`] backed by [`SYSCALL_PAGE_ALLOCATOR`].
///
/// Only installed for the UEFI target. Host builds use the system allocator, because the
/// syscall allocator cannot run on the host.
#[cfg(target_os = "uefi")]
#[global_allocator]
static GLOBAL_ALLOCATOR: PoolAllocator<SyscallPageAllocator> = PoolAllocator::new(&SYSCALL_PAGE_ALLOCATOR);
