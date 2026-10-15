//! System-level MMI Handlers
//!
//! Contains handlers for system events such as the DXE MM Ready-to-Lock
//! transition and the `ExitBootServices` hand-off to the OS.
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
/// The transition is a sequence of decisions over supervisor-global state: whether a policy gate
/// exists, whether it is already locked, and whether a snapshot can be taken. Naming that state
/// as a trait keeps [`process_mm_ready_to_lock`] free of global access, so each ordering it
/// enforces can be driven directly in a test.
///
/// `Gate` is the policy gate the implementation holds, and `SnapshotError` is whatever its
/// snapshot reports; the transition only formats that error into its panic message, so it does
/// not constrain the type beyond [`Debug`].
trait ReadyToLockContext {
    /// The policy gate type this context supplies.
    type Gate;
    /// What a failed snapshot reports.
    type SnapshotError: Debug;

    /// Returns the policy gate, or `None` when none was installed.
    fn policy_gate(&self) -> Option<&Self::Gate>;
    /// Returns whether the gate has already been locked, which makes the transition a no-op.
    fn is_locked(&self, gate: &Self::Gate) -> bool;
    /// Records the memory policy baseline later fetches are compared against.
    fn take_snapshot(&self, gate: &Self::Gate) -> Result<(), Self::SnapshotError>;
    /// Refuses any further unblock requests.
    fn lock_unblocked_memory(&self);
}

/// The live [`ReadyToLockContext`], reading the supervisor's own global state.
struct SupervisorReadyToLockContext<'a> {
    /// The installed policy gate, if initialization got far enough to install one.
    gate: Option<&'a PolicyGate>,
    /// The tracker that stops accepting unblock requests once the transition completes.
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
        // SAFETY: CR3 points to the active root page table inside MM, and the memory
        // policy buffer was configured during initialization.
        unsafe { gate.take_snapshot(cr3, is_range_inside_mmram) }.map(|_| ())
    }

    fn lock_unblocked_memory(&self) {
        self.unblocked_tracker.set_core_init_complete();
    }
}

/// `MmReadyToLock` handler implementation.
///
/// Called when the DXE phase signals that MM should transition to a locked state.
/// After this runs, no new memory regions can be unblocked and the memory policy
/// snapshot stored inside `PolicyGate` is considered the reference baseline.
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

    // If already locked, this is a no-op (idempotent).
    if context.is_locked(gate) {
        log::warn!("MmReadyToLock: already locked, ignoring duplicate");
        return efi::Status::SUCCESS;
    }

    // Take a snapshot and mark as locked.
    if let Err(err) = context.take_snapshot(gate) {
        panic!("MmReadyToLock: memory policy snapshot failed: {err:?}");
    }

    // And mark the unblock memory tracker as locked as well since unblock memory is no longer allowed after this point.
    context.lock_unblocked_memory();

    efi::Status::SUCCESS
}

/// `ExitBootServices` handler implementation.
///
/// Called when the non-MM environment signals `ExitBootServices`. This marks the
/// supervisor as being at runtime, after which the supervisor communication
/// channel is closed and supervisor-targeted requests are rejected (the runtime
/// gate in the dispatch loop denies them).
pub(crate) fn mm_exit_boot_services_handler(_comm_buffer: *mut u8, _comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmExitBootServicesHandler invoked");

    process_mm_exit_boot_services(init_state())
}

/// Marks the supervisor as being at runtime.
fn process_mm_exit_boot_services(state: &InitState) -> efi::Status {
    // Idempotent: if ExitBootServices was already signaled, succeed without
    // re-arming so duplicate notifications are tolerated.
    if !state.mark_at_runtime() {
        log::debug!("MmExitBootServices: duplicate ExitBootServices notification ignored");
    }

    efi::Status::SUCCESS
}
