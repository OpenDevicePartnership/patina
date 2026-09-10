//! Compatibility Memory Manager Service Definitions.
//!
//! This module defines a [`CompatMemoryManager`] service for performing UEFI memory operations
//! that are made available for interoperability with pre-existing memory management patterns to
//! Patina from within Patina components.
//!
//! For example, a Patina component may be given pool allocated memory from a protocol that it is
//! expected to free by the contract specified in the protocol interface. In a Patina/Pure Rust
//! call stack, pool allocation is not directly managed by components, however, in this case a
//! mechanism is needed to do something other than simply let the allocation leak. These are the
//! type of "compatibility" scenarios that this service is intended to address.
//!
//! Rust-to-Rust pool memory should keep using `Box`/`Vec`/
//! [`MemoryManager::get_allocator`](super::memory::MemoryManager::get_allocator) instead. This
//! service is not a replacement for those; it exists only to provide memory management support
//! for operations unique to the C/Rust boundary.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::ptr::NonNull;

use super::memory::MemoryError;
use crate::log_debug_assert;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Compatibility for pre-existing memory management patterns.
///
/// See the [module documentation](self) for when to use this service.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait CompatMemoryManager {
    /// Frees the pool memory at `address`.
    ///
    /// # Safety
    ///
    /// `address` must be the address of memory allocated by a UEFI pool allocation (for example a
    /// C driver's call through `EFI_BOOT_SERVICES.AllocatePool()`, or a UEFI interface call
    /// documented to return a pool-allocated, "caller-must-free" out-parameter), and must not have
    /// already been freed.
    unsafe fn free_pool(&self, address: usize) -> Result<(), MemoryError>;
}

/// A guard over a foreign UEFI pool allocation, freeing it through [`CompatMemoryManager`] when dropped.
///
/// Unlike [`PageAllocation`](super::memory::PageAllocation), this guard only ever hands back a raw
/// pointer and intentionally not a `&T`/`Box<T>`. Since `AllocatePool()` only guarantees 8-byte
/// alignment, a typed conversion could be unsound for `T`.
#[must_use]
pub struct PoolAllocation {
    blob: NonNull<u8>,
    memory_manager: &'static dyn CompatMemoryManager,
}

impl PoolAllocation {
    /// Creates a new guard over an existing pool allocation.
    ///
    /// # Safety
    ///
    /// Has the same contract as [`CompatMemoryManager::free_pool`] where `blob` must be the address
    /// of a valid, not-yet-freed UEFI pool allocation.
    pub unsafe fn new(blob: NonNull<u8>, memory_manager: &'static dyn CompatMemoryManager) -> Self {
        Self { blob, memory_manager }
    }

    /// Returns the raw pointer to the pool allocation.
    ///
    /// Use `read_unaligned()`/`write_unaligned()`/zerocopy for typed access. Pool memory is only
    /// guaranteed 8-byte aligned.
    pub fn as_ptr(&self) -> NonNull<u8> {
        self.blob
    }
}

impl Drop for PoolAllocation {
    fn drop(&mut self) {
        let address = self.blob.addr().get();
        // SAFETY: `blob` was validated as a not-yet-freed pool allocation by the caller of `new`,
        // and Drop runs at most once, so this cannot double-free through this guard.
        unsafe {
            if self.memory_manager.free_pool(address).is_err() {
                log_debug_assert!("Failed to free pool allocation at {address:x}!");
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use alloc::boxed::Box;

    use super::*;

    #[test]
    fn test_pool_allocation_frees_on_drop() {
        let mut mock = MockCompatMemoryManager::new();
        mock.expect_free_pool().times(1).returning(|_| Ok(()));
        let memory_manager: &'static dyn CompatMemoryManager = Box::leak(Box::new(mock));

        let blob = NonNull::new(0x1000usize as *mut u8).unwrap();
        // SAFETY: The address is only used for testing and is not dereferenced.
        let guard = unsafe { PoolAllocation::new(blob, memory_manager) };
        assert_eq!(guard.as_ptr(), blob);
        drop(guard);
    }

    #[test]
    fn test_pool_allocation_logs_but_does_not_panic_on_free_failure() {
        let mut mock = MockCompatMemoryManager::new();
        mock.expect_free_pool().times(1).returning(|_| Err(MemoryError::InvalidAddress));
        let memory_manager: &'static dyn CompatMemoryManager = Box::leak(Box::new(mock));

        let blob = NonNull::new(0x2000usize as *mut u8).unwrap();
        // SAFETY: The address is only used for testing and is not dereferenced.
        let guard = unsafe { PoolAllocation::new(blob, memory_manager) };
        drop(guard);
    }
}
