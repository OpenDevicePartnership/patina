//! Scoped Access to User Owned Memory
//!
//! Lets the supervisor read or write Ring 3 memory for the length of one closure.
//! SMAP is the hardware barrier that otherwise stops Ring 0 from touching that memory,
//! and [`crate::intrinsics`] holds the `stac` and `clac` wrappers that lift and restore
//! it.
//!
//! Callers go through [`with_user_access`], which pairs
//! [`disable_smap`](crate::intrinsics::disable_smap) and
//! [`enable_smap`](crate::intrinsics::enable_smap) through a guard, so the barrier is
//! restored on an early return or a panic. Calling the two directly risks an unbalanced
//! pair, which would leave Ring 3 memory reachable from the supervisor for the rest of
//! the MMI.
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

/// Runs `access` with SMAP temporarily disabled so the supervisor can read or
/// write user-owned memory, restoring SMAP protection when the guard is dropped.
///
/// ## Safety
///
/// Lifting SMAP removes the hardware barrier that stops Ring 0 from touching user-owned
/// memory, so the caller must ensure that every access `access` performs targets a valid,
/// correctly-owned user range that it has already validated (for example through
/// [`query_address_ownership`](crate::memory::page_ownership::query_address_ownership)). Calls must not
/// be nested, and `access` must not migrate
/// to another CPU or return while a further access still depends on SMAP being lifted.
pub(crate) unsafe fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
    // SAFETY: the closure is scoped to the guard's lifetime, and the caller guarantees it only
    // accesses valid, correctly-owned user memory.
    let _user_access = unsafe { UserAccessGuard::new() };
    access()
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_with_user_access_runs_the_closure_and_restores_smap() {
        // SAFETY: the closures touch no memory at all, so there is no user range to validate.
        unsafe {
            assert_eq!(with_user_access(|| 42), 42);
            // The guard is reusable because it is balanced on drop.
            assert_eq!(with_user_access(|| 7), 7);
        }
    }
}
