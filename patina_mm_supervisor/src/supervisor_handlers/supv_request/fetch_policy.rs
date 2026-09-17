//! `FETCH_POLICY` Request Handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::standard::efi;

use patina::{
    UEFI_PAGE_SIZE, align_up, management_mode::protocol::mm_supervisor_request::MmSupervisorRequestHeader,
    uefi_size_to_pages,
};

use crate::{
    intrinsics::read_cr3,
    is_buffer_inside_mmram,
    mm_policy::{MemDescriptorV1_0, PolicyError, PolicyGate},
    state::security_state,
    supervisor_handlers::UnblockedMemoryTracker,
};

trait FetchPolicyContext {
    type Gate;

    fn policy_gate(&self) -> Option<&Self::Gate>;
    fn is_locked(&self, gate: &Self::Gate) -> bool;
    fn take_snapshot(&self, gate: &Self::Gate) -> Result<(), PolicyError>;
    fn lock_unblocked_memory(&self);
    fn verify_snapshot(&self, gate: &Self::Gate) -> Result<(), efi::Status>;
    fn fetch_policy(&self, gate: &Self::Gate, destination: &mut [u8]) -> Result<usize, PolicyError>;
}

struct SupervisorFetchPolicyContext<'a> {
    gate: Option<&'a PolicyGate>,
    unblocked_tracker: &'a UnblockedMemoryTracker,
}

impl FetchPolicyContext for SupervisorFetchPolicyContext<'_> {
    type Gate = PolicyGate;

    fn policy_gate(&self) -> Option<&Self::Gate> {
        self.gate
    }

    fn is_locked(&self, gate: &Self::Gate) -> bool {
        gate.is_locked()
    }

    fn take_snapshot(&self, gate: &Self::Gate) -> Result<(), PolicyError> {
        // SAFETY: CR3 points to the active PML4 table inside MM, and the memory policy buffer was
        // configured during initialization.
        unsafe { gate.take_snapshot(read_cr3(), is_buffer_inside_mmram) }.map(|_| ())
    }

    fn lock_unblocked_memory(&self) {
        self.unblocked_tracker.set_core_init_complete();
    }

    fn verify_snapshot(&self, gate: &Self::Gate) -> Result<(), efi::Status> {
        verify_policy_snapshot(gate, read_cr3())
    }

    fn fetch_policy(&self, gate: &Self::Gate, destination: &mut [u8]) -> Result<usize, PolicyError> {
        // SAFETY: `destination` is a live mutable slice and therefore writable for its full length.
        unsafe { gate.fetch_n_update_policy(destination.as_mut_ptr(), destination.len()) }
    }
}

trait SnapshotVerificationContext {
    fn snapshot_count(&self) -> Option<usize>;
    fn allocate_scratch(&self, pages: usize) -> Result<u64, ()>;
    fn verify_snapshot(&self, scratch: *mut MemDescriptorV1_0, max_count: usize) -> Result<(), PolicyError>;
    fn free_scratch(&self, base: u64, pages: usize) -> Result<(), ()>;
}

struct SupervisorSnapshotVerificationContext<'a> {
    gate: &'a PolicyGate,
    cr3: u64,
}

impl SnapshotVerificationContext for SupervisorSnapshotVerificationContext<'_> {
    fn snapshot_count(&self) -> Option<usize> {
        self.gate.snapshot_count()
    }

    fn allocate_scratch(&self, pages: usize) -> Result<u64, ()> {
        security_state().page_allocator().allocate_pages(pages).map_err(|e| {
            log::error!("verify_policy_snapshot: failed to allocate scratch buffer: {e:?}");
        })
    }

    fn verify_snapshot(&self, scratch: *mut MemDescriptorV1_0, max_count: usize) -> Result<(), PolicyError> {
        // SAFETY: `scratch` was allocated by the page allocator for `max_count` descriptors, and
        // CR3 identifies the active, stable page table for this MM invocation.
        unsafe { self.gate.verify_snapshot(self.cr3, is_buffer_inside_mmram, scratch, max_count) }
    }

    fn free_scratch(&self, base: u64, pages: usize) -> Result<(), ()> {
        security_state().page_allocator().free_pages(base, pages).map_err(|e| {
            log::error!("verify_policy_snapshot: failed to free scratch buffer: {e:?}");
        })
    }
}

/// Handle a `FETCH_POLICY` request.
///
/// Returns the merged memory + firmware policy to the caller.
///
/// ## Behaviour
///
/// 1. **First-time call (before lock):** takes a memory policy snapshot, saves it,
///    and closes the unblock-memory channel (whichever of `MmReadyToLock` or
///    `FETCH_POLICY` fires first performs this transition).
/// 2. **Subsequent calls (after lock):** re-walks the page table and compares the
///    fresh result against the saved snapshot. Any discrepancy is a security
///    violation.
/// 3. **Merges** the memory policy snapshot with the static firmware policy blob
///    from `POLICY_GATE` and writes the combined result into `comm_buffer`.
///
/// ## Response layout
///
/// ```text
/// |----------------------------------|
/// | MmSupervisorRequestHeader (24 B) |
/// |----------------------------------|
/// | SecurePolicyDataV1_0 + payload   |  <- firmware policy blob
/// |----------------------------------|
/// | MemDescriptorV1_0[0..N]          |  <- memory policy snapshot
/// |----------------------------------|
/// ```
pub(super) fn handle_fetch_policy(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("FETCH_POLICY request");

    if comm_buffer.is_null() {
        log::error!("FETCH_POLICY: communication buffer is null");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: The communication handler contract guarantees that `comm_buffer` is writable for
    // `comm_buffer_size` bytes. The null check above satisfies `from_raw_parts_mut` for all lengths.
    let buffer = unsafe { core::slice::from_raw_parts_mut(comm_buffer, *comm_buffer_size) };
    let context = SupervisorFetchPolicyContext {
        gate: security_state().policy_gate(),
        unblocked_tracker: security_state().unblocked_tracker(),
    };
    let (status, response_size) = process_fetch_policy(buffer, &context);
    *comm_buffer_size = response_size;
    status
}

fn process_fetch_policy<C: FetchPolicyContext>(comm_buffer: &mut [u8], context: &C) -> (efi::Status, usize) {
    let Some(payload) = comm_buffer.get_mut(MmSupervisorRequestHeader::SIZE..) else {
        log::error!("FETCH_POLICY: communication buffer is too small for the request header");
        return (efi::Status::BUFFER_TOO_SMALL, MmSupervisorRequestHeader::SIZE);
    };

    let gate = if let Some(gate) = context.policy_gate() {
        gate
    } else {
        log::error!("FETCH_POLICY: POLICY_GATE not initialized");
        return (efi::Status::NOT_READY, MmSupervisorRequestHeader::SIZE);
    };

    // -- 1. Ensure we have a snapshot (lock if not yet locked) ------------
    if context.is_locked(gate) {
        // -- 2. Already locked - verify that current page table matches snapshot
        if let Err(status) = context.verify_snapshot(gate) {
            return (status, MmSupervisorRequestHeader::SIZE);
        }
    } else {
        // Policy requested prior to ready to lock - enforce lock now.
        log::info!("FETCH_POLICY: not yet locked - taking snapshot and locking now");
        if let Err(e) = context.take_snapshot(gate) {
            log::error!("FETCH_POLICY: take_snapshot failed: {e:?}");
            return (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE);
        }
        context.lock_unblocked_memory();
    }

    // -- 3. Write the merged policy into the comm buffer (after the header) -
    let payload_written = match context.fetch_policy(gate, payload) {
        Ok(n) => n,
        Err(PolicyError::InternalError) => {
            // Could be buffer-too-small, size overflow, or missing snapshot.
            log::error!("FETCH_POLICY: fetch_n_update_policy failed");
            return (efi::Status::BUFFER_TOO_SMALL, MmSupervisorRequestHeader::SIZE);
        }
        Err(e) => {
            log::error!("FETCH_POLICY: fetch_n_update_policy unexpected error: {e:?}");
            return (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE);
        }
    };

    if payload_written > payload.len() {
        log::error!(
            "FETCH_POLICY: policy writer reported {payload_written} bytes for a {}-byte destination",
            payload.len()
        );
        return (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE);
    }

    let total_response = MmSupervisorRequestHeader::SIZE + payload_written;
    log::info!(
        "FETCH_POLICY: response {} bytes (header={}, payload={})",
        total_response,
        MmSupervisorRequestHeader::SIZE,
        payload_written
    );

    (efi::Status::SUCCESS, total_response)
}

/// Walks the page table and compares the result against the saved snapshot
/// inside `PolicyGate`. Allocates a temporary scratch buffer from the page
/// allocator for the fresh walk.
///
/// Returns `Ok(())` if the tables match, or an `efi::Status` error on mismatch
/// or allocation failure.
fn verify_policy_snapshot(gate: &PolicyGate, cr3: u64) -> Result<(), efi::Status> {
    let context = SupervisorSnapshotVerificationContext { gate, cr3 };
    verify_policy_snapshot_with_context(&context)
}

fn verify_policy_snapshot_with_context<C: SnapshotVerificationContext>(context: &C) -> Result<(), efi::Status> {
    let saved_count = if let Some(c) = context.snapshot_count() {
        c
    } else {
        log::warn!("verify_policy_snapshot: no snapshot available, skipping");
        return Ok(());
    };

    let desc_size = core::mem::size_of::<MemDescriptorV1_0>();
    let needed_bytes = saved_count.checked_mul(desc_size).ok_or_else(|| {
        log::error!("verify_policy_snapshot: descriptor count overflow");
        efi::Status::DEVICE_ERROR
    })?;
    let aligned_bytes = align_up(needed_bytes, UEFI_PAGE_SIZE).map_err(|e| {
        log::error!("verify_policy_snapshot: failed to align scratch buffer size: {e:?}");
        efi::Status::DEVICE_ERROR
    })?;
    let needed_pages = uefi_size_to_pages!(aligned_bytes);

    let scratch_base = context.allocate_scratch(needed_pages).map_err(|()| efi::Status::OUT_OF_RESOURCES)?;

    let scratch_ptr = scratch_base as *mut MemDescriptorV1_0;
    let scratch_max_count = aligned_bytes / desc_size;

    let result = context.verify_snapshot(scratch_ptr, scratch_max_count);

    // Free the scratch buffer regardless of the verification result.
    let free_result = context.free_scratch(scratch_base, needed_pages);

    if let Err(e) = result {
        log::error!("verify_policy_snapshot: snapshot verification failed: {e:?}");
        return Err(efi::Status::SECURITY_VIOLATION);
    }
    if free_result.is_err() {
        return Err(efi::Status::DEVICE_ERROR);
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::cell::Cell;

    use super::*;
    use crate::mm_policy::SecurePolicyDataV1_0;

    struct TestGate {
        locked: bool,
    }

    struct TestFetchPolicyContext {
        gate: Option<TestGate>,
        snapshot_result: Result<(), PolicyError>,
        verification_result: Result<(), efi::Status>,
        fetch_result: Result<usize, PolicyError>,
        payload: &'static [u8],
        gate_lookups: Cell<usize>,
        snapshot_calls: Cell<usize>,
        lock_unblocked_memory_calls: Cell<usize>,
        verification_calls: Cell<usize>,
        fetch_calls: Cell<usize>,
    }

    struct TestSnapshotVerificationContext {
        snapshot_count: Option<usize>,
        allocation_result: Result<u64, ()>,
        verification_result: Result<(), PolicyError>,
        free_result: Result<(), ()>,
        allocated_pages: Cell<Option<usize>>,
        scratch: Cell<Option<(*mut MemDescriptorV1_0, usize)>>,
        freed: Cell<Option<(u64, usize)>>,
    }

    impl TestSnapshotVerificationContext {
        fn new(snapshot_count: Option<usize>) -> Self {
            Self {
                snapshot_count,
                allocation_result: Ok(0x1000),
                verification_result: Ok(()),
                free_result: Ok(()),
                allocated_pages: Cell::new(None),
                scratch: Cell::new(None),
                freed: Cell::new(None),
            }
        }
    }

    impl SnapshotVerificationContext for TestSnapshotVerificationContext {
        fn snapshot_count(&self) -> Option<usize> {
            self.snapshot_count
        }

        fn allocate_scratch(&self, pages: usize) -> Result<u64, ()> {
            self.allocated_pages.set(Some(pages));
            self.allocation_result
        }

        fn verify_snapshot(&self, scratch: *mut MemDescriptorV1_0, max_count: usize) -> Result<(), PolicyError> {
            self.scratch.set(Some((scratch, max_count)));
            self.verification_result
        }

        fn free_scratch(&self, base: u64, pages: usize) -> Result<(), ()> {
            self.freed.set(Some((base, pages)));
            self.free_result
        }
    }

    impl TestFetchPolicyContext {
        fn unlocked() -> Self {
            Self {
                gate: Some(TestGate { locked: false }),
                snapshot_result: Ok(()),
                verification_result: Ok(()),
                fetch_result: Ok(0),
                payload: &[],
                gate_lookups: Cell::new(0),
                snapshot_calls: Cell::new(0),
                lock_unblocked_memory_calls: Cell::new(0),
                verification_calls: Cell::new(0),
                fetch_calls: Cell::new(0),
            }
        }
    }

    impl FetchPolicyContext for TestFetchPolicyContext {
        type Gate = TestGate;

        fn policy_gate(&self) -> Option<&Self::Gate> {
            self.gate_lookups.set(self.gate_lookups.get() + 1);
            self.gate.as_ref()
        }

        fn is_locked(&self, gate: &Self::Gate) -> bool {
            gate.locked
        }

        fn take_snapshot(&self, _gate: &Self::Gate) -> Result<(), PolicyError> {
            self.snapshot_calls.set(self.snapshot_calls.get() + 1);
            self.snapshot_result
        }

        fn lock_unblocked_memory(&self) {
            self.lock_unblocked_memory_calls.set(self.lock_unblocked_memory_calls.get() + 1);
        }

        fn verify_snapshot(&self, _gate: &Self::Gate) -> Result<(), efi::Status> {
            self.verification_calls.set(self.verification_calls.get() + 1);
            self.verification_result
        }

        fn fetch_policy(&self, _gate: &Self::Gate, destination: &mut [u8]) -> Result<usize, PolicyError> {
            self.fetch_calls.set(self.fetch_calls.get() + 1);
            if let Ok(written) = self.fetch_result {
                let Some(source) = self.payload.get(..written) else {
                    return self.fetch_result;
                };
                let Some(target) = destination.get_mut(..written) else {
                    return self.fetch_result;
                };
                target.copy_from_slice(source);
            }
            self.fetch_result
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
        unsafe { PolicyGate::new(core::ptr::from_ref(policy).cast()) }.expect("test policy must be valid")
    }

    #[test]
    fn handler_rejects_a_null_buffer() {
        let mut size = MmSupervisorRequestHeader::SIZE;

        assert_eq!(handle_fetch_policy(core::ptr::null_mut(), &mut size), efi::Status::INVALID_PARAMETER);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }

    #[test]
    fn handler_rejects_a_short_non_null_buffer() {
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE - 1];
        let mut size = buffer.len();

        assert_eq!(handle_fetch_policy(buffer.as_mut_ptr(), &mut size), efi::Status::BUFFER_TOO_SMALL);
        assert_eq!(size, MmSupervisorRequestHeader::SIZE);
    }

    #[test]
    fn supervisor_fetch_policy_context_delegates_to_its_real_state() {
        let gate = unlocked_policy_gate();
        let tracker = UnblockedMemoryTracker::new();
        let context = SupervisorFetchPolicyContext { gate: Some(&gate), unblocked_tracker: &tracker };
        let context_gate = context.policy_gate().expect("test context must expose its policy gate");
        let mut destination = [0u8; 40];

        assert!(!context.is_locked(context_gate));
        assert_eq!(context.take_snapshot(context_gate), Err(PolicyError::InternalError));
        assert_eq!(context.verify_snapshot(context_gate), Ok(()));
        assert_eq!(context.fetch_policy(context_gate, &mut destination), Err(PolicyError::InternalError));
        assert!(!tracker.is_core_init_complete());
        context.lock_unblocked_memory();
        assert!(tracker.is_core_init_complete());

        let verification_context = SupervisorSnapshotVerificationContext { gate: context_gate, cr3: 0 };
        assert_eq!(verification_context.snapshot_count(), None);
        assert_eq!(verification_context.allocate_scratch(0), Err(()));
        assert_eq!(verification_context.verify_snapshot(core::ptr::null_mut(), 0), Ok(()));
        assert_eq!(verification_context.free_scratch(1, 0), Err(()));
    }

    #[test]
    fn snapshot_verification_skips_work_without_a_saved_snapshot() {
        let context = TestSnapshotVerificationContext::new(None);

        assert_eq!(verify_policy_snapshot_with_context(&context), Ok(()));
        assert_eq!(context.allocated_pages.get(), None);
        assert_eq!(context.scratch.get(), None);
        assert_eq!(context.freed.get(), None);
    }

    #[test]
    fn snapshot_verification_rejects_descriptor_size_overflow() {
        let context = TestSnapshotVerificationContext::new(Some(usize::MAX));

        assert_eq!(verify_policy_snapshot_with_context(&context), Err(efi::Status::DEVICE_ERROR));
        assert_eq!(context.allocated_pages.get(), None);
    }

    #[test]
    fn snapshot_verification_rejects_scratch_alignment_overflow() {
        let descriptor_size = core::mem::size_of::<MemDescriptorV1_0>();
        let context = TestSnapshotVerificationContext::new(Some(usize::MAX / descriptor_size));

        assert_eq!(verify_policy_snapshot_with_context(&context), Err(efi::Status::DEVICE_ERROR));
        assert_eq!(context.allocated_pages.get(), None);
    }

    #[test]
    fn snapshot_verification_reports_allocation_failure() {
        let mut context = TestSnapshotVerificationContext::new(Some(1));
        context.allocation_result = Err(());

        assert_eq!(verify_policy_snapshot_with_context(&context), Err(efi::Status::OUT_OF_RESOURCES));
        assert_eq!(context.allocated_pages.get(), Some(1));
        assert_eq!(context.scratch.get(), None);
        assert_eq!(context.freed.get(), None);
    }

    #[test]
    fn snapshot_verification_frees_scratch_after_success() {
        let context = TestSnapshotVerificationContext::new(Some(1));

        assert_eq!(verify_policy_snapshot_with_context(&context), Ok(()));
        assert_eq!(context.allocated_pages.get(), Some(1));
        assert_eq!(context.scratch.get(), Some((0x1000 as *mut MemDescriptorV1_0, 170)));
        assert_eq!(context.freed.get(), Some((0x1000, 1)));
    }

    #[test]
    fn snapshot_verification_prioritizes_a_policy_mismatch_over_cleanup_failure() {
        let mut context = TestSnapshotVerificationContext::new(Some(1));
        context.verification_result = Err(PolicyError::AccessDenied);
        context.free_result = Err(());

        assert_eq!(verify_policy_snapshot_with_context(&context), Err(efi::Status::SECURITY_VIOLATION));
        assert_eq!(context.freed.get(), Some((0x1000, 1)));
    }

    #[test]
    fn snapshot_verification_reports_cleanup_failure_after_a_successful_comparison() {
        let mut context = TestSnapshotVerificationContext::new(Some(1));
        context.free_result = Err(());

        assert_eq!(verify_policy_snapshot_with_context(&context), Err(efi::Status::DEVICE_ERROR));
        assert_eq!(context.freed.get(), Some((0x1000, 1)));
    }

    #[test]
    fn rejects_a_buffer_smaller_than_the_header_before_reading_policy_state() {
        let context = TestFetchPolicyContext::unlocked();
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE - 1];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::BUFFER_TOO_SMALL, MmSupervisorRequestHeader::SIZE)
        );
        assert_eq!(context.gate_lookups.get(), 0);
    }

    #[test]
    fn reports_not_ready_without_a_policy_gate() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.gate = None;
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::NOT_READY, MmSupervisorRequestHeader::SIZE)
        );
        assert_eq!(context.snapshot_calls.get(), 0);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
        assert_eq!(context.verification_calls.get(), 0);
        assert_eq!(context.fetch_calls.get(), 0);
    }

    #[test]
    fn reports_snapshot_failures_without_writing_a_policy() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.snapshot_result = Err(PolicyError::InternalError);
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE)
        );
        assert_eq!(context.snapshot_calls.get(), 1);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
        assert_eq!(context.verification_calls.get(), 0);
        assert_eq!(context.fetch_calls.get(), 0);
    }

    #[test]
    fn returns_snapshot_verification_errors_without_writing_a_policy() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.gate = Some(TestGate { locked: true });
        context.verification_result = Err(efi::Status::SECURITY_VIOLATION);
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::SECURITY_VIOLATION, MmSupervisorRequestHeader::SIZE)
        );
        assert_eq!(context.snapshot_calls.get(), 0);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
        assert_eq!(context.verification_calls.get(), 1);
        assert_eq!(context.fetch_calls.get(), 0);
    }

    #[test]
    fn maps_internal_policy_copy_errors_to_buffer_too_small() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.fetch_result = Err(PolicyError::InternalError);
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::BUFFER_TOO_SMALL, MmSupervisorRequestHeader::SIZE)
        );
        assert_eq!(context.snapshot_calls.get(), 1);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 1);
        assert_eq!(context.fetch_calls.get(), 1);
    }

    #[test]
    fn maps_unexpected_policy_copy_errors_to_device_error() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.fetch_result = Err(PolicyError::PolicyRootNotFound);
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE)
        );
        assert_eq!(context.fetch_calls.get(), 1);
    }

    #[test]
    fn rejects_a_policy_writer_that_overreports_the_destination_size() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.fetch_result = Ok(2);
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE + 1];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE)
        );
    }

    #[test]
    fn snapshots_and_returns_policy_data_when_the_gate_is_unlocked() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.fetch_result = Ok(4);
        context.payload = b"test";
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE + 4];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::SUCCESS, MmSupervisorRequestHeader::SIZE + 4)
        );
        assert_eq!(context.snapshot_calls.get(), 1);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 1);
        assert_eq!(context.verification_calls.get(), 0);
        assert_eq!(context.fetch_calls.get(), 1);
        assert_eq!(
            buffer.get(MmSupervisorRequestHeader::SIZE..).expect("response must contain a policy payload"),
            b"test"
        );
    }

    #[test]
    fn verifies_and_returns_policy_data_when_the_gate_is_locked() {
        let mut context = TestFetchPolicyContext::unlocked();
        context.gate = Some(TestGate { locked: true });
        context.fetch_result = Ok(1);
        context.payload = b"x";
        let mut buffer = [0u8; MmSupervisorRequestHeader::SIZE + 1];

        assert_eq!(
            process_fetch_policy(&mut buffer, &context),
            (efi::Status::SUCCESS, MmSupervisorRequestHeader::SIZE + 1)
        );
        assert_eq!(context.snapshot_calls.get(), 0);
        assert_eq!(context.lock_unblocked_memory_calls.get(), 0);
        assert_eq!(context.verification_calls.get(), 1);
        assert_eq!(context.fetch_calls.get(), 1);
    }
}
