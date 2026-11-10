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

use crate::{
    error::MmSupervisorError,
    handlers::UnblockedMemoryTracker,
    intrinsics::read_cr3,
    memory::mmram::is_range_inside_mmram,
    policy::PolicyGate,
    state::{InitState, init_state, security_state},
};

/// The state the Ready-to-Lock transition reads and the two effects it has.
///
/// The transition decides over supervisor-global state: whether a policy gate exists, whether it
/// is already locked, and whether a snapshot can be taken. Putting that state behind a trait
/// keeps [`process_mm_ready_to_lock`] free of global access, so tests can drive each ordering it
/// enforces.
///
/// `SnapshotError` only needs [`Debug`] because the transition only formats it into its panic
/// message.
trait ReadyToLockContext {
    /// The policy gate type.
    type Gate;
    /// The error a failed snapshot reports.
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
/// Afterwards no new memory regions can be unblocked, and the memory policy snapshot in the
/// `PolicyGate` is the reference baseline.
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

    // A duplicate notification is a no-op.
    if context.is_locked(gate) {
        log::warn!("MmReadyToLock: already locked, ignoring duplicate");
        return efi::Status::SUCCESS;
    }

    // Taking the snapshot locks the gate.
    if let Err(err) = context.take_snapshot(gate) {
        panic!("MmReadyToLock: memory policy snapshot failed: {err:?}");
    }

    // Unblock requests are refused from here on.
    context.lock_unblocked_memory();

    efi::Status::SUCCESS
}

/// Handles `ExitBootServices` from the non-MM environment.
///
/// Marks the supervisor as at runtime. After that the runtime gate in the dispatch loop rejects
/// supervisor requests.
pub(crate) fn mm_exit_boot_services_handler(_comm_buffer: *mut u8, _comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmExitBootServicesHandler invoked");

    process_mm_exit_boot_services(init_state())
}

/// Marks the supervisor as being at runtime.
fn process_mm_exit_boot_services(state: &InitState) -> efi::Status {
    // A duplicate notification succeeds without re-arming.
    if !state.mark_at_runtime() {
        log::debug!("MmExitBootServices: duplicate ExitBootServices notification ignored");
    }

    efi::Status::SUCCESS
}
