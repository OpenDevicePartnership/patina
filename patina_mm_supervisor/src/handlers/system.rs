//! System-level MMI handlers.
//!
//! Handles the DXE MM Ready-to-Lock transition and the `ExitBootServices` hand-off to the OS.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::fmt::Debug;

use patina::standard::efi;

#[cfg(not(test))]
use crate::intrinsics::read_cr3;
#[cfg(test)]
use tests::read_cr3;

use crate::{
    error::MmSupervisorError,
    handlers::UnblockedMemoryTracker,
    memory::mmram::is_range_inside_mmram,
    policy::PolicyGate,
    state::{init_state, security_state},
};

/// The global state the Ready-to-Lock transition reads and changes, behind a trait so tests can
/// drive [`process_mm_ready_to_lock`].
trait ReadyToLockContext {
    /// The policy gate type.
    type Gate;
    /// The error a failed snapshot reports. Only formatted into the panic message.
    type SnapshotError: Debug;

    /// Returns the policy gate, or `None` if none was installed.
    fn policy_gate(&self) -> Option<&Self::Gate>;
    /// Returns whether the gate has already been locked, which makes the transition a no-op.
    fn is_locked(&self, gate: &Self::Gate) -> bool;
    /// Records the memory policy baseline later fetches are compared against.
    fn take_snapshot(&self, gate: &Self::Gate) -> Result<(), Self::SnapshotError>;
    /// Refuses any further unblock requests.
    fn lock_unblocked_memory(&self);
}

/// The [`ReadyToLockContext`] backed by the supervisor's global state.
struct SupervisorReadyToLockContext<'a> {
    /// The installed policy gate, if initialization installed one.
    gate: Option<&'a PolicyGate>,
    /// Tracker that stops accepting unblock requests once the transition completes.
    unblocked_tracker: &'a UnblockedMemoryTracker,
}

impl ReadyToLockContext for SupervisorReadyToLockContext<'_> {
    type Gate = PolicyGate;
    type SnapshotError = MmSupervisorError;

    fn policy_gate(&self) -> Option<&Self::Gate> {
        self.gate
    }

    fn is_locked(&self, gate: &Self::Gate) -> bool {
        gate.is_locked()
    }

    fn take_snapshot(&self, gate: &Self::Gate) -> Result<(), Self::SnapshotError> {
        let cr3 = read_cr3();
        // SAFETY: CR3 points to the active page table in MM, and initialization configured the
        // memory policy buffer.
        unsafe { gate.take_snapshot(cr3, is_range_inside_mmram) }.map(|_| ())
    }

    fn lock_unblocked_memory(&self) {
        self.unblocked_tracker.set_core_init_complete();
    }
}

/// Handles `MmReadyToLock`, which DXE signals when MM should lock.
///
/// Takes the memory policy snapshot, which locks the policy gate, and refuses further unblock
/// requests. A repeated notification is ignored.
///
/// ## Panics
///
/// Panics if no policy gate is installed or the snapshot fails.
pub(crate) fn mm_ready_to_lock_handler(_comm_buffer: *mut u8, _comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmReadyToLockHandler invoked");

    let context = SupervisorReadyToLockContext {
        gate: security_state().policy_gate(),
        unblocked_tracker: security_state().unblocked_tracker(),
    };
    process_mm_ready_to_lock(&context)
}

/// Runs the Ready-to-Lock transition against `context`.
fn process_mm_ready_to_lock<C: ReadyToLockContext>(context: &C) -> efi::Status {
    let gate = context.policy_gate().expect("MmReadyToLock: policy gate must be initialized during BSP init");

    if context.is_locked(gate) {
        log::warn!("MmReadyToLock: already locked, ignoring duplicate");
        return efi::Status::SUCCESS;
    }

    // Taking the snapshot locks the gate.
    if let Err(err) = context.take_snapshot(gate) {
        panic!("MmReadyToLock: memory policy snapshot failed: {err:?}");
    }

    context.lock_unblocked_memory();

    efi::Status::SUCCESS
}

/// Handles `ExitBootServices` from the non-MM environment.
///
/// Marks the supervisor as at runtime, after which the dispatch loop rejects supervisor
/// requests.
pub(crate) fn mm_exit_boot_services_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmExitBootServicesHandler invoked");

    let state = init_state();
    if state.is_at_runtime() {
        log::debug!("MmExitBootServices: duplicate ExitBootServices notification ignored");
        return efi::Status::SUCCESS;
    }

    // Invoke the ready to lock in case we are dealing with some screwed-up normal world behavior...
    let status = mm_ready_to_lock_handler(comm_buffer, comm_buffer_size);
    if status == efi::Status::SUCCESS {
        state.mark_at_runtime();
    }
    status
}
