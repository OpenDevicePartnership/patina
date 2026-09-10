//! DXE Core Compatibility Memory Manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::ffi::c_void;

use patina::{
    component::service::{IntoService, Service, compat_memory::CompatMemoryManager, memory::MemoryError},
    standard::efi,
};
use patina_test::{patina_test, u_assert};

use crate::allocator::{core_allocate_pool, core_free_pool};

/// Provides compatibility with memory management patterns used by C code.
#[derive(IntoService)]
#[service(dyn CompatMemoryManager)]
pub(crate) struct CoreCompatMemoryManager;

impl CompatMemoryManager for CoreCompatMemoryManager {
    /// # Safety
    ///
    /// See [`CompatMemoryManager::free_pool`].
    unsafe fn free_pool(&self, address: usize) -> Result<(), MemoryError> {
        // SAFETY: Caller contract is enforced with trait docs. Forwarded to core_free_pool unchanged.
        match unsafe { core_free_pool(address as *mut c_void) } {
            Ok(()) => Ok(()),
            Err(_) => Err(MemoryError::InvalidAddress),
        }
    }
}

#[patina_test]
#[cfg_attr(coverage, coverage(off))]
fn compat_memory_manager_frees_pool_allocation_test(
    cm: Service<dyn CompatMemoryManager>,
) -> patina_test::error::Result {
    let ptr = core_allocate_pool(efi::BOOT_SERVICES_DATA, 8).expect("pool allocation should succeed for test");
    // SAFETY: ptr was just allocated by core_allocate_pool above.
    let result = unsafe { cm.free_pool(ptr as usize) };
    u_assert!(result.is_ok(), "Failed to free a valid pool allocation.");

    // SAFETY: address is intentionally wrong, was not allocated, and only used to exercise the error path.
    let result = unsafe { cm.free_pool(0x1) };
    u_assert!(result.is_err(), "Freeing a bogus address should have failed.");

    Ok(())
}
