//! `FETCH_POLICY` request handler.
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
    error::{MmSupervisorError, MmSupervisorResult},
    handlers::UnblockedMemoryTracker,
    intrinsics::read_cr3,
    memory::mmram::is_range_inside_mmram,
    policy::{MemDescriptorV1_0, PolicyGate, PolicyGateError},
    state::security_state,
};

/// The state a `FETCH_POLICY` request reads and the effects it has.
trait FetchPolicyContext {
    /// The policy gate type.
    type Gate;

    /// Returns the policy gate, or `None` if none was installed.
    fn policy_gate(&self) -> Option<&Self::Gate>;
    /// Returns whether the gate is locked, which selects verify over snapshot.
    fn is_locked(&self, gate: &Self::Gate) -> bool;
    /// Records the memory policy baseline on the first fetch, before the gate locks.
    fn take_snapshot(&self, gate: &Self::Gate) -> MmSupervisorResult<()>;
    /// Refuses any further unblock requests.
    fn lock_unblocked_memory(&self);
    /// Re-walks the page table and compares it against the recorded baseline.
    fn verify_snapshot(&self, gate: &Self::Gate) -> Result<(), efi::Status>;
    /// Writes the merged firmware and memory policy into `destination`, returning the byte count.
    fn fetch_policy(&self, gate: &Self::Gate, destination: &mut [u8]) -> MmSupervisorResult<usize>;
}

/// The [`FetchPolicyContext`] backed by the supervisor's global state.
struct SupervisorFetchPolicyContext<'a> {
    /// The installed policy gate, if initialization installed one.
    gate: Option<&'a PolicyGate>,
    /// Tracker that stops accepting unblock requests once the gate locks.
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

    fn take_snapshot(&self, gate: &Self::Gate) -> MmSupervisorResult<()> {
        // SAFETY: CR3 points to the active page table in MM, and initialization configured the
        // memory policy buffer.
        unsafe { gate.take_snapshot(read_cr3(), is_range_inside_mmram) }.map(|_| ())
    }

    fn lock_unblocked_memory(&self) {
        self.unblocked_tracker.set_core_init_complete();
    }

    fn verify_snapshot(&self, gate: &Self::Gate) -> Result<(), efi::Status> {
        verify_policy_snapshot(&SupervisorSnapshotVerificationContext { gate, cr3: read_cr3() })
    }

    fn fetch_policy(&self, gate: &Self::Gate, destination: &mut [u8]) -> MmSupervisorResult<usize> {
        // SAFETY: `destination` is a mutable slice, so it is writable for its full length.
        unsafe { gate.fetch_and_update_policy(destination.as_mut_ptr(), destination.len()) }
    }
}

/// The scratch allocation and comparison a snapshot verification needs.
trait SnapshotVerificationContext {
    /// Returns the number of descriptors in the recorded baseline, or `None` if there is none.
    fn snapshot_count(&self) -> Option<usize>;
    /// Allocates `pages` of scratch for a fresh page table walk.
    fn allocate_scratch(&self, pages: usize) -> Result<u64, ()>;
    /// Walks the page table into `scratch` and compares it against the baseline.
    fn verify_snapshot(&self, scratch: *mut MemDescriptorV1_0, max_count: usize) -> MmSupervisorResult<()>;
    /// Frees the scratch pages. Runs whether or not the comparison passed.
    fn free_scratch(&self, base: u64, pages: usize) -> Result<(), ()>;
}

/// The [`SnapshotVerificationContext`] backed by the supervisor's page allocator.
struct SupervisorSnapshotVerificationContext<'a> {
    /// The gate holding the baseline to compare against.
    gate: &'a PolicyGate,
    /// The active page table root for this MM invocation.
    cr3: u64,
}

impl SnapshotVerificationContext for SupervisorSnapshotVerificationContext<'_> {
    fn snapshot_count(&self) -> Option<usize> {
        self.gate.snapshot_count()
    }

    fn allocate_scratch(&self, pages: usize) -> Result<u64, ()> {
        security_state().page_allocator().allocate_pages(pages).map_err(|err| {
            log::error!("verify_policy_snapshot: failed to allocate scratch buffer: {err:?}");
        })
    }

    fn verify_snapshot(&self, scratch: *mut MemDescriptorV1_0, max_count: usize) -> MmSupervisorResult<()> {
        // SAFETY: `scratch` was allocated by the page allocator for `max_count` descriptors, and
        // CR3 identifies the active, stable page table for this MM invocation.
        unsafe { self.gate.verify_snapshot(self.cr3, is_range_inside_mmram, scratch, max_count) }
    }

    fn free_scratch(&self, base: u64, pages: usize) -> Result<(), ()> {
        security_state().page_allocator().free_pages(base, pages).map_err(|err| {
            log::error!("verify_policy_snapshot: failed to free scratch buffer: {err:?}");
        })
    }
}

/// Handles a `FETCH_POLICY` request by returning the merged firmware and memory policy.
///
/// 1. Before lock, the call takes the memory policy snapshot and closes the unblock memory
///    channel. Whichever of `MmReadyToLock` and `FETCH_POLICY` comes first does this.
/// 2. After lock, the call walks the page table again and compares it with the snapshot. Any
///    difference is a security violation.
/// 3. The snapshot is merged with the firmware policy blob and written to `comm_buffer`:
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
    if comm_buffer.is_null() {
        log::error!("FETCH_POLICY: communication buffer is null");
        *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: the communication handler contract makes `comm_buffer` writable for
    // `comm_buffer_size` bytes, and it is non-null (checked above).
    let buffer = unsafe { core::slice::from_raw_parts_mut(comm_buffer, *comm_buffer_size) };
    let context = SupervisorFetchPolicyContext {
        gate: security_state().policy_gate(),
        unblocked_tracker: security_state().unblocked_tracker(),
    };
    let (status, response_size) = process_fetch_policy(buffer, &context);
    *comm_buffer_size = response_size;
    status
}

/// Serves a `FETCH_POLICY` request into `comm_buffer`, returning the status and the number of
/// bytes the response occupies.
fn process_fetch_policy<C: FetchPolicyContext>(comm_buffer: &mut [u8], context: &C) -> (efi::Status, usize) {
    let Some(payload) = comm_buffer.get_mut(MmSupervisorRequestHeader::SIZE..) else {
        log::error!("FETCH_POLICY: communication buffer is too small for the request header");
        return (efi::Status::BUFFER_TOO_SMALL, MmSupervisorRequestHeader::SIZE);
    };

    let Some(gate) = context.policy_gate() else {
        log::error!("FETCH_POLICY: policy gate not initialized");
        return (efi::Status::NOT_READY, MmSupervisorRequestHeader::SIZE);
    };

    // Steps 1 and 2: after lock, verify against the snapshot; before lock, take it and lock.
    if context.is_locked(gate) {
        if let Err(status) = context.verify_snapshot(gate) {
            return (status, MmSupervisorRequestHeader::SIZE);
        }
    } else {
        log::info!("FETCH_POLICY: not yet locked - taking snapshot and locking now");
        if let Err(err) = context.take_snapshot(gate) {
            log::error!("FETCH_POLICY: take_snapshot failed: {err:?}");
            return (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE);
        }
        context.lock_unblocked_memory();
    }

    // Step 3: write the merged policy after the header.
    let payload_written = match context.fetch_policy(gate, payload) {
        Ok(written) => written,
        Err(MmSupervisorError::PolicyGate(PolicyGateError::BufferTooSmall)) => {
            log::error!("FETCH_POLICY: communication buffer is too small for the policy");
            return (efi::Status::BUFFER_TOO_SMALL, MmSupervisorRequestHeader::SIZE);
        }
        Err(err) => {
            log::error!("FETCH_POLICY: fetch_and_update_policy unexpected error: {err:?}");
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
    log::debug!(
        "FETCH_POLICY: response {} bytes (header={}, payload={})",
        total_response,
        MmSupervisorRequestHeader::SIZE,
        payload_written
    );

    (efi::Status::SUCCESS, total_response)
}

/// Walks the page table into a scratch buffer from `context` and compares it with the saved
/// snapshot in the `PolicyGate`.
///
/// ## Errors
///
/// Returns `SECURITY_VIOLATION` if there is no snapshot or the tables differ,
/// `OUT_OF_RESOURCES` if the scratch buffer cannot be allocated, and `DEVICE_ERROR` if a size
/// overflows or the scratch buffer cannot be freed.
fn verify_policy_snapshot<C: SnapshotVerificationContext>(context: &C) -> Result<(), efi::Status> {
    let Some(saved_count) = context.snapshot_count() else {
        // The gate reported itself locked without a snapshot. Nothing to compare against is not
        // a pass.
        log::error!("verify_policy_snapshot: no snapshot to verify against");
        return Err(efi::Status::SECURITY_VIOLATION);
    };

    let desc_size = core::mem::size_of::<MemDescriptorV1_0>();
    let needed_bytes = saved_count.checked_mul(desc_size).ok_or_else(|| {
        log::error!("verify_policy_snapshot: descriptor count overflow");
        efi::Status::DEVICE_ERROR
    })?;
    let aligned_bytes = align_up(needed_bytes, UEFI_PAGE_SIZE).map_err(|err| {
        log::error!("verify_policy_snapshot: failed to align scratch buffer size: {err:?}");
        efi::Status::DEVICE_ERROR
    })?;
    let needed_pages = uefi_size_to_pages!(aligned_bytes);

    let scratch_base = context.allocate_scratch(needed_pages).map_err(|()| efi::Status::OUT_OF_RESOURCES)?;

    let scratch_ptr = scratch_base as *mut MemDescriptorV1_0;
    let scratch_max_count = aligned_bytes / desc_size;

    let result = context.verify_snapshot(scratch_ptr, scratch_max_count);

    // Free the scratch buffer regardless of the verification result.
    let free_result = context.free_scratch(scratch_base, needed_pages);

    if let Err(err) = result {
        log::error!("verify_policy_snapshot: snapshot verification failed: {err:?}");
        return Err(efi::Status::SECURITY_VIOLATION);
    }
    if free_result.is_err() {
        return Err(efi::Status::DEVICE_ERROR);
    }

    Ok(())
}
