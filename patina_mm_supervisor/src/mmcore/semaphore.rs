//! Counting Rendezvous Semaphores for the MM Supervisor Core
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU32, Ordering};

/// Signals a counting rendezvous semaphore (atomic increment).
pub(crate) fn sem_signal(sem: &AtomicU32) {
    sem.fetch_add(1, Ordering::AcqRel);
}

/// Consumes one count if the semaphore is positive. Non-blocking; returns whether a
/// count was taken.
pub(crate) fn sem_try_take(sem: &AtomicU32) -> bool {
    let mut value = sem.load(Ordering::Acquire);
    while value != 0 {
        match sem.compare_exchange_weak(value, value - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(current) => value = current,
        }
    }
    false
}
