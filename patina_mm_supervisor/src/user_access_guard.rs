//! Scoped Access to User Owned Memory
//!
//! Lifts SMAP for the length of one closure so the supervisor can access Ring 3 memory.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::intrinsics::{disable_smap, enable_smap};

/// Keeps SMAP disabled while the guard is alive and restores it when dropped.
#[must_use = "SMAP is re-enabled when the guard is dropped"]
struct UserAccessGuard;

impl UserAccessGuard {
    /// Disables SMAP until the returned guard is dropped.
    ///
    /// ## Safety
    ///
    /// The guarded scope must only access valid, correctly-owned user memory. Guards must
    /// not be nested, and the guard must remain on the CPU where it was created.
    unsafe fn new() -> Self {
        // SAFETY: the caller upholds the user-memory access requirements for the guard's lifetime.
        unsafe { disable_smap() };
        Self
    }
}

impl Drop for UserAccessGuard {
    fn drop(&mut self) {
        // SAFETY: this guard can only be constructed by `new`, which disables SMAP once.
        unsafe { enable_smap() };
    }
}

/// Runs `access` with SMAP lifted and restores SMAP when it returns.
///
/// ## Safety
///
/// Every access `access` makes must target a valid user range the caller has already
/// validated (for example with
/// [`query_address_ownership`](crate::memory::page_ownership::query_address_ownership)).
/// Calls must not be nested, and `access` must not migrate to another CPU.
pub(crate) unsafe fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
    // SAFETY: the guard lives exactly as long as `access`, and the caller guarantees `access`
    // only touches validated user memory.
    let _user_access = unsafe { UserAccessGuard::new() };
    access()
}
