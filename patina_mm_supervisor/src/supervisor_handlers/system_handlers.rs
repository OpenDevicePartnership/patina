//! System-level MMI Handlers
//!
//! Contains handlers for system events such as the DXE MM Ready-to-Lock transition
//! and the ExitBootServices hand-off to the OS.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use r_efi::efi;

use crate::{
    is_buffer_inside_mmram, read_cr3,
    state::{init_state, security_state},
};

/// MmReadyToLock handler implementation.
///
/// Called when the DXE phase signals that MM should transition to a locked state.
/// After this runs, no new memory regions can be unblocked and the memory policy
/// snapshot stored inside `PolicyGate` is considered the reference baseline.
pub(crate) fn mm_ready_to_lock_handler(_comm_buffer: *mut u8, _comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmReadyToLockHandler invoked");

    let gate = match security_state().policy_gate() {
        Some(g) => g,
        None => {
            log::error!("MmReadyToLock: POLICY_GATE not initialized");
            return efi::Status::NOT_READY;
        }
    };

    // If already locked, this is a no-op (idempotent).
    if gate.is_locked() {
        log::warn!("MmReadyToLock: already locked, ignoring duplicate");
        return efi::Status::SUCCESS;
    }

    // Take a snapshot and mark as locked.
    let cr3 = read_cr3();
    // SAFETY: cr3 points to the active PML4 table inside SMM,
    // and the memory policy buffer was configured during init.
    if let Err(e) = unsafe { gate.take_snapshot(cr3, is_buffer_inside_mmram) } {
        log::error!("MmReadyToLock: take_snapshot failed: {:?}", e);
        return efi::Status::DEVICE_ERROR;
    }

    // And mark the unblock memory tracker as locked as well since unblock memory is no longer allowed after this point.
    security_state().unblocked_tracker().set_core_init_complete();

    efi::Status::SUCCESS
}

/// ExitBootServices handler implementation.
///
/// Called when the non-MM environment signals ExitBootServices. This marks the
/// supervisor as being at runtime, after which the supervisor communication
/// channel is closed and supervisor-targeted requests are rejected (the runtime
/// gate in the dispatch loop denies them).
pub(crate) fn mm_exit_boot_services_handler(_comm_buffer: *mut u8, _comm_buffer_size: &mut usize) -> efi::Status {
    log::info!("MmExitBootServicesHandler invoked");

    // Idempotent: if ExitBootServices was already signaled, warn and succeed
    // without re-arming so duplicate notifications are tolerated.
    if !init_state().mark_at_runtime() {
        log::warn!("MmExitBootServices: ExitBootServices event is signaled more than once??");
    }

    efi::Status::SUCCESS
}
