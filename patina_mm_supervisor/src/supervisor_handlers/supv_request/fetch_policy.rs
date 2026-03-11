//! FETCH_POLICY Request Handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use r_efi::efi;

use patina::{
    base::{UEFI_PAGE_SIZE, align_up},
    management_mode::protocol::mm_supervisor_request::MmSupervisorRequestHeader,
    uefi_size_to_pages,
};

use crate::{
    is_buffer_inside_mmram,
    mm_policy::{MemDescriptorV1_0, PolicyError, PolicyGate},
    read_cr3,
    state::security_state,
};

/// Handle a FETCH_POLICY request.
///
/// Returns the merged memory + firmware policy to the caller.
///
/// ## Behaviour
///
/// 1. **First-time call (before lock):** takes a memory policy snapshot, saves it,
///    and sets the ready-to-lock flag (whichever of `MmReadyToLock` or `FETCH_POLICY`
///    fires first performs this).
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
/// | MemDescriptorV1_0[0..N]          |  <- memory policy snapshot
/// |----------------------------------|
/// | SecurePolicyDataV1_0 + payload   |  <- firmware policy blob (raw copy)
/// |----------------------------------|
/// ```
pub(super) fn handle_fetch_policy(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("FETCH_POLICY request");

    // -- 0. Obtain the PolicyGate -------------------------------------
    let gate = match security_state().policy_gate() {
        Some(g) => g,
        None => {
            log::error!("FETCH_POLICY: POLICY_GATE not initialized");
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::NOT_READY;
        }
    };

    let cr3 = read_cr3();

    // -- 1. Ensure we have a snapshot (lock if not yet locked) ------------
    if !gate.is_locked() {
        // Policy requested prior to ready to lock - enforce lock now.
        log::info!("FETCH_POLICY: not yet locked - taking snapshot and locking now");
        // SAFETY: cr3 is valid and the memory policy buffer was configured during init.
        if let Err(e) = unsafe { gate.take_snapshot(cr3, is_buffer_inside_mmram) } {
            log::error!("FETCH_POLICY: take_snapshot failed: {:?}", e);
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::DEVICE_ERROR;
        }
    } else {
        // -- 2. Already locked - verify that current page table matches snapshot
        if let Err(status) = verify_policy_snapshot(gate, cr3) {
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return status;
        }
    }

    // -- 3. Write the merged policy into the comm buffer (after the header) -
    let payload_capacity = match comm_buffer_size.checked_sub(MmSupervisorRequestHeader::SIZE) {
        Some(c) => c,
        None => {
            log::error!("FETCH_POLICY: comm_buffer_size too small for header");
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::BUFFER_TOO_SMALL;
        }
    };

    // SAFETY: comm_buffer + header offset is valid writable memory.
    let dest = unsafe { comm_buffer.add(MmSupervisorRequestHeader::SIZE) };
    // SAFETY: `dest` points to `payload_capacity` bytes of writable comm-buffer memory (computed
    // above), satisfying `fetch_n_update_policy`'s requirement.
    let payload_written = match unsafe { gate.fetch_n_update_policy(dest, payload_capacity) } {
        Ok(n) => n,
        Err(PolicyError::InternalError) => {
            // Could be buffer-too-small, size overflow, or missing snapshot.
            log::error!("FETCH_POLICY: fetch_n_update_policy failed");
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::BUFFER_TOO_SMALL;
        }
        Err(e) => {
            log::error!("FETCH_POLICY: fetch_n_update_policy unexpected error: {:?}", e);
            *comm_buffer_size = MmSupervisorRequestHeader::SIZE;
            return efi::Status::DEVICE_ERROR;
        }
    };

    let total_response = MmSupervisorRequestHeader::SIZE + payload_written;
    *comm_buffer_size = total_response;
    log::info!(
        "FETCH_POLICY: response {} bytes (header={}, payload={})",
        total_response,
        MmSupervisorRequestHeader::SIZE,
        payload_written
    );

    efi::Status::SUCCESS
}

/// Walks the page table and compares the result against the saved snapshot
/// inside `PolicyGate`. Allocates a temporary scratch buffer from the page
/// allocator for the fresh walk.
///
/// Returns `Ok(())` if the tables match, or an `efi::Status` error on mismatch
/// or allocation failure.
fn verify_policy_snapshot(gate: &PolicyGate, cr3: u64) -> Result<(), efi::Status> {
    let saved_count = match gate.snapshot_count() {
        Some(c) => c,
        None => {
            log::warn!("verify_policy_snapshot: no snapshot available, skipping");
            return Ok(());
        }
    };

    let desc_size = core::mem::size_of::<MemDescriptorV1_0>();
    let needed_bytes = saved_count.checked_mul(desc_size).ok_or_else(|| {
        log::error!("verify_policy_snapshot: descriptor count overflow");
        efi::Status::DEVICE_ERROR
    })?;
    let aligned_bytes = align_up(needed_bytes, UEFI_PAGE_SIZE).map_err(|e| {
        log::error!("verify_policy_snapshot: failed to align scratch buffer size: {:?}", e);
        efi::Status::DEVICE_ERROR
    })?;
    let needed_pages = uefi_size_to_pages!(aligned_bytes);

    let scratch_base = security_state().page_allocator().allocate_pages(needed_pages).map_err(|e| {
        log::error!("verify_policy_snapshot: failed to allocate scratch buffer: {:?}", e);
        efi::Status::OUT_OF_RESOURCES
    })?;

    let scratch_ptr = scratch_base as *mut MemDescriptorV1_0;
    let scratch_max_count = aligned_bytes / desc_size;

    // SAFETY: scratch_ptr was just allocated and scratch_max_count is correct.
    let result = unsafe { gate.verify_snapshot(cr3, is_buffer_inside_mmram, scratch_ptr, scratch_max_count) };

    // Free the scratch buffer regardless of the result.
    let _ = security_state().page_allocator().free_pages(scratch_base, needed_pages);

    result.map_err(|e| {
        log::error!("verify_policy_snapshot: snapshot verification failed: {:?}", e);
        efi::Status::SECURITY_VIOLATION
    })
}
