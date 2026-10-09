//! System-level MMI Handlers
//!
//! Contains handlers for system events such as the DXE MM Ready-to-Lock transition
//! and the `ExitBootServices` hand-off to the OS.
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
    memory::mmram::is_buffer_inside_mmram,
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
/// snapshot reports; the transition only logs that error and maps it to a status, so it does not
/// constrain the type beyond [`Debug`].
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
        // SAFETY: CR3 points to the active PML4 table inside MM, and the memory
        // policy buffer was configured during initialization.
        unsafe { gate.take_snapshot(cr3, is_buffer_inside_mmram) }.map(|_| ())
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
///
/// Reports `NOT_READY` when no policy gate is installed, `SUCCESS` without repeating the work
/// when the gate is already locked, and `DEVICE_ERROR` when the snapshot fails. The unblocked
/// memory tracker is locked only after a successful snapshot, so a failure leaves the supervisor
/// in the state it was in rather than half locked.
fn process_mm_ready_to_lock<C: ReadyToLockContext>(context: &C) -> efi::Status {
    let gate = if let Some(gate) = context.policy_gate() {
        gate
    } else {
        log::error!("MmReadyToLock: POLICY_GATE not initialized");
        return efi::Status::NOT_READY;
    };

    // If already locked, this is a no-op (idempotent).
    if context.is_locked(gate) {
        log::warn!("MmReadyToLock: already locked, ignoring duplicate");
        return efi::Status::SUCCESS;
    }

    // Take a snapshot and mark as locked.
    if let Err(e) = context.take_snapshot(gate) {
        log::error!("MmReadyToLock: take_snapshot failed: {e:?}");
        return efi::Status::DEVICE_ERROR;
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
///
/// Always reports `SUCCESS`. A repeat notification is logged and otherwise ignored, because the
/// flag is one-way and the producer is outside the supervisor's trust boundary.
fn process_mm_exit_boot_services(state: &InitState) -> efi::Status {
    // Idempotent: if ExitBootServices was already signaled, warn and succeed
    // without re-arming so duplicate notifications are tolerated.
    if !state.mark_at_runtime() {
        log::warn!("MmExitBootServices: ExitBootServices event is signaled more than once??");
    }

    efi::Status::SUCCESS
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::cell::Cell;

    use super::*;
    use crate::policy::{PolicyGateError, SecurePolicyDataV1_0};

    #[derive(Clone, Copy, Debug)]
    struct TestSnapshotError;

    struct TestGate {
        locked: bool,
    }

    struct TestReadyToLockContext {
        gate: Option<TestGate>,
        snapshot_result: Result<(), TestSnapshotError>,
        snapshot_calls: Cell<usize>,
        lock_unblocked_memory_calls: Cell<usize>,
    }

    impl TestReadyToLockContext {
        fn new(gate: Option<TestGate>, snapshot_result: Result<(), TestSnapshotError>) -> Self {
            Self { gate, snapshot_result, snapshot_calls: Cell::new(0), lock_unblocked_memory_calls: Cell::new(0) }
        }
    }

    impl ReadyToLockContext for TestReadyToLockContext {
        type Gate = TestGate;
        type SnapshotError = TestSnapshotError;

        fn policy_gate(&self) -> Option<&Self::Gate> {
            self.gate.as_ref()
        }

        fn is_locked(&self, gate: &Self::Gate) -> bool {
            gate.locked
        }

        fn take_snapshot(&self, _gate: &Self::Gate) -> Result<(), Self::SnapshotError> {
            self.snapshot_calls.set(self.snapshot_calls.get() + 1);
            self.snapshot_result
        }

        fn lock_unblocked_memory(&self) {
            self.lock_unblocked_memory_calls.set(self.lock_unblocked_memory_calls.get() + 1);
        }
    }

    fn unlocked_policy_gate() -> PolicyGate {
        let policy = Box::leak(Box::new(SecurePolicyDataV1_0 {
            version_major: 1,
            size: core::mem::size_of::<SecurePolicyDataV1_0>() as u32,
            ..Default::default()
        }));
        // SAFETY: `policy` is a valid V1.0 policy header and is leaked for the duration of the
        // process, so it outlives the returned gate.
        unsafe { PolicyGate::new(core::ptr::from_ref(policy).cast(), core::mem::size_of::<SecurePolicyDataV1_0>()) }
            .expect("test policy must be valid")
    }

    #[test]
    fn ready_to_lock_is_not_ready_without_a_policy_gate() {
        let context = TestReadyToLockContext::new(None, Ok(()));

        assert_eq!(process_mm_ready_to_lock(&context), efi::Status::NOT_READY);
        assert_eq!(context.snapshot_calls.get(), 0);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
    }

    #[test]
    fn ready_to_lock_is_idempotent_when_the_gate_is_already_locked() {
        let context = TestReadyToLockContext::new(Some(TestGate { locked: true }), Ok(()));

        assert_eq!(process_mm_ready_to_lock(&context), efi::Status::SUCCESS);
        assert_eq!(context.snapshot_calls.get(), 0);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
    }

    #[test]
    fn ready_to_lock_reports_snapshot_failure_without_locking_unblocked_memory() {
        let context = TestReadyToLockContext::new(Some(TestGate { locked: false }), Err(TestSnapshotError));

        assert_eq!(process_mm_ready_to_lock(&context), efi::Status::DEVICE_ERROR);
        assert_eq!(context.snapshot_calls.get(), 1);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
    }

    #[test]
    fn ready_to_lock_locks_unblocked_memory_after_a_successful_snapshot() {
        let context = TestReadyToLockContext::new(Some(TestGate { locked: false }), Ok(()));

        assert_eq!(process_mm_ready_to_lock(&context), efi::Status::SUCCESS);
        assert_eq!(context.snapshot_calls.get(), 1);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 1);
    }

    #[test]
    fn supervisor_ready_to_lock_context_delegates_to_its_real_state() {
        let gate = unlocked_policy_gate();
        let tracker = UnblockedMemoryTracker::new();
        let context = SupervisorReadyToLockContext { gate: Some(&gate), unblocked_tracker: &tracker };

        let context_gate = context.policy_gate().expect("test context must expose its policy gate");
        assert!(!context.is_locked(context_gate));
        assert_eq!(context.take_snapshot(context_gate), Err(PolicyGateError::InternalError.into()));
        assert!(!tracker.is_core_init_complete());
        context.lock_unblocked_memory();
        assert!(tracker.is_core_init_complete());
    }

    #[test]
    fn ready_to_lock_entry_point_uses_the_global_context() {
        let mut size = 0;
        let status = mm_ready_to_lock_handler(core::ptr::null_mut(), &mut size);

        // Other unit tests may have already installed the one-time global policy gate. With no
        // gate the entry point is not ready; with the test gate installed, its unconfigured
        // snapshot buffer produces a device error.
        assert!(status == efi::Status::NOT_READY || status == efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn exit_boot_services_marks_runtime_and_tolerates_duplicates() {
        let state = InitState::new();

        assert!(!state.is_at_runtime());
        assert_eq!(process_mm_exit_boot_services(&state), efi::Status::SUCCESS);
        assert!(state.is_at_runtime());
        assert_eq!(process_mm_exit_boot_services(&state), efi::Status::SUCCESS);
        assert!(state.is_at_runtime());
    }

    #[test]
    fn exit_boot_services_entry_point_marks_the_global_state() {
        let mut size = 0;

        assert_eq!(mm_exit_boot_services_handler(core::ptr::null_mut(), &mut size), efi::Status::SUCCESS);
        assert!(init_state().is_at_runtime());
    }
}
