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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;
    use crate::policy::{MemDescriptorV1_0, PolicyGateError, SecurePolicyDataV1_0};
    use crate::test_support::PageAlignedMemory;

    /// Supplies a real, empty root page table in place of the privileged CR3 read.
    pub(super) fn read_cr3() -> u64 {
        std::thread_local! { static ROOT: PageAlignedMemory = PageAlignedMemory::new(1); }
        ROOT.with(PageAlignedMemory::base)
    }

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

    fn install_gate_with_snapshot_buffer() -> &'static PolicyGate {
        let mut gate = unlocked_policy_gate();
        let buffer = Box::leak(Box::new(MemDescriptorV1_0::default()));
        gate.set_memory_policy_buffer(buffer, 1).unwrap();
        security_state().set_policy_gate(gate);
        security_state().policy_gate().unwrap()
    }

    #[test]
    #[should_panic(expected = "policy gate must be initialized")]
    fn ready_to_lock_panics_without_a_policy_gate() {
        let context = TestReadyToLockContext::new(None, Ok(()));

        process_mm_ready_to_lock(&context);
    }

    #[test]
    fn ready_to_lock_is_idempotent_when_the_gate_is_already_locked() {
        let context = TestReadyToLockContext::new(Some(TestGate { locked: true }), Ok(()));

        assert_eq!(process_mm_ready_to_lock(&context), efi::Status::SUCCESS);
        assert_eq!(context.snapshot_calls.get(), 0);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
    }

    #[test]
    fn ready_to_lock_panics_on_snapshot_failure() {
        let context = TestReadyToLockContext::new(Some(TestGate { locked: false }), Err(TestSnapshotError));

        let panic = catch_unwind(AssertUnwindSafe(|| process_mm_ready_to_lock(&context)))
            .expect_err("a failed snapshot must panic");
        let message = panic.downcast_ref::<String>().expect("panic payload must be a formatted message");
        assert!(message.contains("memory policy snapshot failed"), "unexpected panic message: {message}");
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
        // The global gate is one-time, so this is a no-op when another test installed it first.
        // Either way the installed gate has no snapshot buffer configured, so the snapshot fails.
        security_state().set_policy_gate(unlocked_policy_gate());

        let mut size = 0;
        let result = catch_unwind(AssertUnwindSafe(|| mm_ready_to_lock_handler(core::ptr::null_mut(), &mut size)));

        assert!(result.is_err(), "a failed snapshot must panic");
    }

    #[test]
    fn exit_boot_services_locks_before_runtime_and_tolerates_duplicates() {
        let gate = install_gate_with_snapshot_buffer();
        let tracker = security_state().unblocked_tracker();
        let mut size = 0;

        assert!(!gate.is_locked());
        assert!(!tracker.is_core_init_complete());
        assert!(!init_state().is_at_runtime());
        assert_eq!(mm_exit_boot_services_handler(core::ptr::null_mut(), &mut size), efi::Status::SUCCESS);
        assert_eq!(gate.snapshot_count(), Some(0));
        assert!(tracker.is_core_init_complete());
        assert!(init_state().is_at_runtime());

        assert_eq!(mm_exit_boot_services_handler(core::ptr::null_mut(), &mut size), efi::Status::SUCCESS);
    }

    #[test]
    fn exit_boot_services_preserves_an_existing_snapshot() {
        let gate = install_gate_with_snapshot_buffer();
        let mut size = 0;
        assert_eq!(mm_ready_to_lock_handler(core::ptr::null_mut(), &mut size), efi::Status::SUCCESS);
        assert_eq!(gate.snapshot_count(), Some(0));
        assert_eq!(mm_exit_boot_services_handler(core::ptr::null_mut(), &mut size), efi::Status::SUCCESS);
        assert_eq!(gate.snapshot_count(), Some(0));
        assert!(init_state().is_at_runtime());
    }

    #[test]
    fn exit_boot_services_does_not_enter_runtime_on_snapshot_failure() {
        security_state().set_policy_gate(unlocked_policy_gate()); // No snapshot buffer.
        let mut size = 0;
        let result = catch_unwind(AssertUnwindSafe(|| mm_exit_boot_services_handler(core::ptr::null_mut(), &mut size)));
        assert!(result.is_err());
        assert!(!security_state().unblocked_tracker().is_core_init_complete());
        assert!(!init_state().is_at_runtime());
    }

    #[test]
    fn exit_boot_services_entry_point_does_not_enter_runtime_without_a_policy_gate() {
        let mut size = 0;
        let result = catch_unwind(AssertUnwindSafe(|| mm_exit_boot_services_handler(core::ptr::null_mut(), &mut size)));
        assert!(result.is_err());
        assert!(!init_state().is_at_runtime());
    }
}
