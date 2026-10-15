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
    error::{MmSupervisorError, MmSupervisorResult},
    handlers::UnblockedMemoryTracker,
    intrinsics::read_cr3,
    memory::mmram::is_range_inside_mmram,
    policy::{MemDescriptorV1_0, PolicyGate, PolicyGateError},
    state::security_state,
};

/// The state a `FETCH_POLICY` request reads and the effects it has.
trait FetchPolicyContext {
    /// The policy gate type this context supplies.
    type Gate;

    /// Returns the policy gate, or `None` when none was installed.
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

/// The live [`FetchPolicyContext`], reading the supervisor's own global state.
struct SupervisorFetchPolicyContext<'a> {
    /// The installed policy gate, if initialization got far enough to install one.
    gate: Option<&'a PolicyGate>,
    /// The tracker that stops accepting unblock requests once the gate locks.
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
        // SAFETY: CR3 points to the active root page table inside MM, and the
        // memory policy buffer was configured during initialization.
        unsafe { gate.take_snapshot(read_cr3(), is_range_inside_mmram) }.map(|_| ())
    }

    fn lock_unblocked_memory(&self) {
        self.unblocked_tracker.set_core_init_complete();
    }

    fn verify_snapshot(&self, gate: &Self::Gate) -> Result<(), efi::Status> {
        verify_policy_snapshot(gate, read_cr3())
    }

    fn fetch_policy(&self, gate: &Self::Gate, destination: &mut [u8]) -> MmSupervisorResult<usize> {
        // SAFETY: `destination` is a live mutable slice and therefore writable for its full length.
        unsafe { gate.fetch_and_update_policy(destination.as_mut_ptr(), destination.len()) }
    }
}

/// The scratch allocation and comparison a snapshot verification needs.
trait SnapshotVerificationContext {
    /// Returns how many descriptors the recorded baseline holds, or `None` when there is none.
    fn snapshot_count(&self) -> Option<usize>;
    /// Allocates `pages` of scratch for a fresh page table walk.
    fn allocate_scratch(&self, pages: usize) -> Result<u64, ()>;
    /// Walks the page table into `scratch` and compares it against the baseline.
    fn verify_snapshot(&self, scratch: *mut MemDescriptorV1_0, max_count: usize) -> MmSupervisorResult<()>;
    /// Returns the scratch pages, which runs whether or not the comparison passed.
    fn free_scratch(&self, base: u64, pages: usize) -> Result<(), ()>;
}

/// The live [`SnapshotVerificationContext`], allocating from the supervisor's page allocator.
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

/// Serves a `FETCH_POLICY` request into `comm_buffer`, returning the status and the number of
/// bytes the response occupies.
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
        if let Err(err) = context.take_snapshot(gate) {
            log::error!("FETCH_POLICY: take_snapshot failed: {err:?}");
            return (efi::Status::DEVICE_ERROR, MmSupervisorRequestHeader::SIZE);
        }
        context.lock_unblocked_memory();
    }

    // -- 3. Write the merged policy into the comm buffer (after the header) -
    let payload_written = match context.fetch_policy(gate, payload) {
        Ok(written) => written,
        Err(MmSupervisorError::PolicyGate(PolicyGateError::InternalError)) => {
            // Could be buffer-too-small, size overflow, or missing snapshot.
            log::error!("FETCH_POLICY: fetch_and_update_policy failed");
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

/// Runs a snapshot verification against `context`.
fn verify_policy_snapshot_with_context<C: SnapshotVerificationContext>(context: &C) -> Result<(), efi::Status> {
    let Some(saved_count) = context.snapshot_count() else {
        // Reaching here means the gate reported itself locked without a snapshot behind it.
        // Nothing to compare against is not a pass.
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
