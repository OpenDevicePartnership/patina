//! MM User Core Memory Allocator
//!
//! Provides a [`SyscallPageAllocator`] that implements [`PageAllocatorBackend`]
//! by issuing `syscall` instructions to the MM Supervisor for page allocation
//! and deallocation.
//!
//! The pool allocator, `PoolAllocator`, is wired up as the `#[global_allocator]`.
//!
//! ## Syscall ABI
//!
//! The MM Supervisor exposes page allocation via the following syscall indices
//! (defined in SysCallLib.h / `SyscallIndex` enum in the supervisor):
//!
//! | Syscall     | RAX       | RDX (arg1)     | R8 (arg2)      | R9 (arg3)   |
//! |-------------|-----------|----------------|----------------|-------------|
//! | AllocPage   | `0x10004` | alloc_type (0) | mem_type (6)   | page_count  |
//! | FreePage    | `0x10005` | address        | page_count     | 0           |
//!
//! The supervisor returns:
//! - RAX: result value
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
use patina::management_mode::supervisor::{SyscallIndex, raw_syscall};

/// `AllocateAnyPages` — allocate any available pages.
const ALLOCATE_ANY_PAGES: u64 = 0;

/// `EfiRuntimeServicesData` — the memory type used for MM pool allocations.
const RUNTIME_SERVICES_DATA: u64 = 6;

/// Validate that a given memory range is a valid MM communication buffer by
/// issuing the `MmIsCommBuffer` syscall to the supervisor.
///
/// Returns `true` if the supervisor confirms the range falls entirely within
/// the user communication buffer region.
pub fn is_comm_buffer(address: u64, size: u64) -> bool {
    let result = unsafe { raw_syscall(SyscallIndex::MmIsCommBuffer.as_u64(), address, size, 0) };
    result != 0
}

/// A page allocator backend that issues `syscall` instructions to the MM Supervisor.
///
/// This is used as the [`PageAllocatorBackend`] for the MM User Core's
/// [`PoolAllocator`](crate::pool_allocator::PoolAllocator) and global allocator.
///
/// ## Initialization
///
/// Call [`SyscallPageAllocator::set_initialized`] after the user core has been
/// set up and is ready to issue syscalls (i.e., during `StartUserCore` handling,
/// before driver dispatch begins).
pub struct SyscallPageAllocator {
    /// Whether the allocator has been activated. Before this is set, all
    /// allocations will fail immediately. This prevents accidental allocations
    /// before the syscall interface is ready.
    initialized: AtomicBool,
}

// SAFETY: SyscallPageAllocator uses an atomic flag and the syscall interface is
// re-entrant from the BSP.
unsafe impl Send for SyscallPageAllocator {}
unsafe impl Sync for SyscallPageAllocator {}

impl Default for SyscallPageAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SyscallPageAllocator {
    /// Creates a new uninitialized syscall page allocator.
    pub const fn new() -> Self {
        Self { initialized: AtomicBool::new(false) }
    }

    /// Marks the allocator as ready. Must be called after the syscall interface
    /// is available (i.e., early in `StartUserCore` handling).
    pub fn set_initialized(&self) {
        self.initialized.store(true, Ordering::Release);
        log::info!("SyscallPageAllocator initialized — heap is now available.");
    }
}

impl PageAllocatorBackend for SyscallPageAllocator {
    fn allocate_pages(&self, num_pages: usize) -> Result<u64, PageAllocError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(PageAllocError::NotInitialized);
        }

        if num_pages == 0 {
            return Err(PageAllocError::OutOfMemory);
        }

        let addr = unsafe {
            raw_syscall(SyscallIndex::AllocPage.as_u64(), ALLOCATE_ANY_PAGES, RUNTIME_SERVICES_DATA, num_pages as u64)
        };

        if addr == 0 {
            log::warn!("SyscallPageAllocator: AllocPage({} pages) returned a null address", num_pages);
            return Err(PageAllocError::OutOfMemory);
        }

        log::trace!("SyscallPageAllocator: allocated {} page(s) at 0x{:016x}", num_pages, addr);

        Ok(addr)
    }

    fn free_pages(&self, addr: u64, num_pages: usize) -> Result<(), PageAllocError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(PageAllocError::NotInitialized);
        }

        unsafe { raw_syscall(SyscallIndex::FreePage.as_u64(), addr, num_pages as u64, 0) };

        log::trace!("SyscallPageAllocator: freed {} page(s) at 0x{:016x}", num_pages, addr);

        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

/// Global page allocator instance for the user core.
///
/// This issues syscalls to the supervisor for actual page allocation.
/// Call [`SyscallPageAllocator::set_initialized`] during `StartUserCore`
/// to enable the heap.
pub static SYSCALL_PAGE_ALLOCATOR: SyscallPageAllocator = SyscallPageAllocator::new();

/// Global pool allocator instance.
///
/// Uses the shared [`PoolAllocator`] from `pool_allocator`,
/// backed by [`SyscallPageAllocator`] for page allocation via syscalls.
///
/// Only installed for the firmware (UEFI) target; host builds use the system allocator, since the
/// syscall-backed allocator cannot run on the host.
#[cfg(target_os = "uefi")]
#[global_allocator]
static GLOBAL_ALLOCATOR: PoolAllocator<SyscallPageAllocator> = PoolAllocator::new(&SYSCALL_PAGE_ALLOCATOR);
