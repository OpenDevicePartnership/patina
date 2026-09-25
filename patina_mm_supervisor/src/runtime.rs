//! MM Supervisor Core Runtime Dispatch
//!
//! This module contains the runtime request processing logic for the MM Supervisor Core,
//! including the BSP request loop, user/supervisor request dispatch, AP holding pen,
//! and AP procedure management.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::standard::efi;
use patina::{
    management_mode::{MmCommBufferStatus, supervisor::UserCommandType},
    pi::{mm_cis::EfiMmEntryContext, protocol::communication::EfiMmCommunicateHeader},
};

use crate::{
    AP_ARRIVAL_TIMEOUT_US, AP_EXIT_TIMEOUT_US, AP_TIMEOUT_US, CommBufferConfig, MmSupervisorCore, PageOwnership,
    PlatformInfo,
    cpu::ApState,
    intrinsics::is_bsp,
    mailbox::{ApCommand, ApResponse},
    privilege_mgmt::invoke_demoted_routine,
    query_address_ownership,
    state::{DEFAULT_SUPERVISOR_MMI_HANDLERS, init_state, security_state},
};

/// Helper function to disable the SMAP bit in EFLAGS to allow supervisor code to access user memory when needed.
///
/// ## Safety
///
/// Disabling SMAP removes the hardware barrier that stops the supervisor (Ring 0) from
/// reading or writing user-owned (Ring 3) memory. The caller must re-enable SMAP via
/// [`enable_smap`] once the user-memory access completes, and must ensure every access
/// performed while SMAP is lifted targets valid, correctly-owned user memory. Prefer
/// [`with_user_access`], which guarantees the disable/enable pair is balanced.
unsafe fn disable_smap() {
    // SAFETY: `stac` only sets the AC flag in EFLAGS; it touches no memory and clobbers
    // no registers (hence `nostack, preserves_flags`). It is a privileged instruction that
    // is valid in the Ring 0 supervisor context this code always runs in.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "stac", // Set AC flag to enable access to user memory
            options(nostack, preserves_flags)
        );
    }
}

/// Helper function to re-enable the SMAP bit in EFLAGS after accessing user memory.
///
/// ## Safety
///
/// This mutates the privileged EFLAGS.AC state and must only be called to close a region
/// opened by [`disable_smap`]. Callers must ensure no further user-memory access that
/// relies on SMAP being lifted happens after this returns. Prefer [`with_user_access`],
/// which guarantees the disable/enable pair is balanced.
unsafe fn enable_smap() {
    // SAFETY: `clac` only clears the AC flag in EFLAGS; it touches no memory and clobbers
    // no registers (hence `nostack, preserves_flags`). It is a privileged instruction that
    // is valid in the Ring 0 supervisor context this code always runs in.
    #[cfg(not(test))]
    unsafe {
        core::arch::asm!(
            "clac", // Clear AC flag to re-enable SMAP protections
            options(nostack, preserves_flags)
        );
    }
}

/// Keeps SMAP disabled while the guard is alive and restores it when dropped.
#[must_use = "SMAP is re-enabled when the guard is dropped"]
struct UserAccessGuard;

impl UserAccessGuard {
    /// Disables SMAP until the returned guard is dropped.
    ///
    /// ## Safety
    ///
    /// The guarded scope must only access valid, correctly-owned user memory. Guards must
    /// not be nested, and the guard must remain on the CPU where it was created.
    unsafe fn new() -> Self {
        // SAFETY: the caller upholds the user-memory access requirements for the guard's lifetime.
        unsafe { disable_smap() };
        Self
    }
}

impl Drop for UserAccessGuard {
    fn drop(&mut self) {
        // SAFETY: this guard can only be constructed by `new`, which disables SMAP once.
        unsafe { enable_smap() };
    }
}

/// Runs `access` with SMAP temporarily disabled so the supervisor can read or
/// write user-owned memory, restoring SMAP protection when the guard is dropped.
///
/// ## Safety
///
/// Lifting SMAP removes the hardware barrier that stops Ring 0 from touching user-owned
/// memory, so the caller must ensure that every access `access` performs targets a valid,
/// correctly-owned user range that it has already validated (for example through
/// [`query_address_ownership`]). Calls must not be nested, and `access` must not migrate
/// to another CPU or return while a further access still depends on SMAP being lifted.
pub(crate) unsafe fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
    // SAFETY: the closure is scoped to the guard's lifetime, and the caller guarantees it only
    // accesses valid, correctly-owned user memory.
    let _user_access = unsafe { UserAccessGuard::new() };
    access()
}

impl<P: PlatformInfo, const MAX_CPUS: usize> MmSupervisorCore<P, MAX_CPUS> {
    /// Enter runtime mode (called on subsequent entries after init is complete).
    ///
    /// `cpu_id` is the APIC ID; `cpu_index` is the dense, 0-based UEFI processor index that
    /// selects this CPU's per-core resources (Ring 3 stack, mailbox).
    ///
    /// Implements the MP synchronization protocol:
    /// 1. APs check in by setting their state to `InHoldingPen` and entering the holding pen
    /// 2. BSP waits for all registered APs, all cores must be in MM before servicing a request
    /// 3. BSP processes the pending request via `bsp_request_loop`
    /// 4. BSP releases every AP via the per-CPU rendezvous semaphore
    /// 5. BSP waits, bounded by `AP_EXIT_TIMEOUT_US`, for every released AP to acknowledge it has left
    /// 6. Each AP clears its `InHoldingPen` state and acknowledges the BSP
    pub(crate) fn enter_runtime(&'static self, cpu_id: u32, cpu_index: usize) {
        let is_bsp = is_bsp();

        if is_bsp {
            log::info!("BSP (CPU {cpu_id}) waiting for APs to arrive...");

            // Wait for all registered APs to check in (set state to InHoldingPen).
            let expected_aps = self.cpu_manager.registered_count().saturating_sub(1);
            self.wait_for_ap_arrival(expected_aps);

            // Every registered AP is now in MM - service the request.
            log::trace!("BSP (CPU {cpu_id}) entering request serving routine...");
            self.bsp_request_loop(cpu_index);

            // Exit barrier: release every penned AP and wait for each to acknowledge it has left.
            log::trace!("BSP (CPU {cpu_id}) releasing all APs from the holding pen...");
            self.cpu_manager.release_all_aps();
            let acknowledged = self.cpu_manager.wait_for_ap_exit_acks(expected_aps, AP_EXIT_TIMEOUT_US);
            assert!(
                acknowledged == expected_aps,
                "MM Supervisor fail-secure: only {acknowledged}/{expected_aps} APs acknowledged leaving the holding \
                 pen within the exit window; refusing to resume the platform with cores still in MM"
            );

            self.mailbox_manager.reset_all();
        } else {
            // AP: check in by marking state, then enter holding pen.
            self.cpu_manager.set_ap_state(cpu_id, ApState::InHoldingPen);
            log::info!("AP (CPU {cpu_id}) checked in, entering holding pen...");
            self.ap_holding_pen(cpu_id);

            // Check out: clear the InHoldingPen state now that this AP has left the pen and let BSP know we are out.
            self.cpu_manager.set_ap_state(cpu_id, ApState::NotPresent);
            self.cpu_manager.ack_exit_to_bsp();
        }
    }

    /// Waits for all registered APs to rendezvous in the holding pen, enforcing the
    /// all APs arrival guarantee.
    ///
    /// TODO: A fully robust implementation would first re-assert the MMI via SMI IPI to
    /// force delayed/blocked stragglers in before failing. That force-in needs Local APIC
    /// IPI support the supervisor does not yet provide; until then a straggler that misses
    /// the window halts the platform instead of being pulled in.
    ///
    /// ## Panics
    ///
    /// Panics (fail-secure halt) if not all registered APs arrive within
    /// `AP_ARRIVAL_TIMEOUT_US`.
    fn wait_for_ap_arrival(&self, expected_aps: usize) {
        if expected_aps == 0 {
            return;
        }

        let all_arrived = crate::perf_timer::spin_until(AP_ARRIVAL_TIMEOUT_US, || {
            self.cpu_manager.count_aps_in_state(ApState::InHoldingPen) >= expected_aps
        });

        if all_arrived {
            log::info!("All {expected_aps} APs arrived");
        } else {
            // All cores have to rendezvous in the holding pen before the BSP services a request. If any core
            // fails to arrive within the window, this is a security-fatal condition.
            let arrived = self.cpu_manager.count_aps_in_state(ApState::InHoldingPen);
            panic!(
                "MM Supervisor fail-secure: only {arrived}/{expected_aps} APs rendezvoused within the arrival window; \
                 refusing to service the request with a partial set of cores"
            );
        }
    }

    /// The main request serving loop for the BSP.
    /// It manages other CPUs and processes pending requests from the communication buffer.
    ///
    /// Two parallel `MmCommBufferStatus` mailboxes are consulted — one for the
    /// user channel and one for the supervisor channel. The user mailbox is
    /// checked first; if neither mailbox is valid the request is treated as
    /// an asynchronous MMI and dispatched through the user path so the
    /// user-core's async handler chain still runs.
    ///
    /// - If targeting User: copies user comm buffer to internal, then demotes to user entry point
    /// - If targeting Supervisor: dispatches to the request dispatcher
    fn bsp_request_loop(&self, cpu_index: usize) {
        // Get communication buffer configuration
        let config = match security_state().comm_buffer_config() {
            Some(c) => c,
            None => {
                // Not yet initialized, nothing to process
                return;
            }
        };

        // Bail out only if neither status mailbox is wired up yet.
        if config.user_status_buffer == 0 && config.supv_status_buffer == 0 {
            return;
        }

        // Read both status mailboxes. A buffer that hasn't been published yet
        // is treated as an all-zero (idle) status.
        let user_status = if config.user_status_buffer != 0 {
            // SAFETY: `user_status_buffer` is non-zero here and, provided by the MM IPL, references
            // an MMRAM-resident `MmCommBufferStatus`, so the volatile read is valid.
            unsafe { core::ptr::read_volatile(config.user_status_buffer as *const MmCommBufferStatus) }
        } else {
            MmCommBufferStatus::new()
        };
        let supv_status = if config.supv_status_buffer != 0 {
            // SAFETY: `supv_status_buffer` is non-zero here and, provided by the MM IPL, references
            // an MMRAM-resident `MmCommBufferStatus`, so the volatile read is valid.
            unsafe { core::ptr::read_volatile(config.supv_status_buffer as *const MmCommBufferStatus) }
        } else {
            MmCommBufferStatus::new()
        };

        let target = crate::RequestTarget::select(&user_status, &supv_status);

        log::trace!(
            "Processing request: user_valid={}, supv_valid={}, target={:?}",
            user_status.is_comm_buffer_valid,
            supv_status.is_comm_buffer_valid,
            target
        );

        match target {
            crate::RequestTarget::None => {
                // No pending request
            }
            crate::RequestTarget::User => {
                // Request targets the User module (sync user MMI or async dispatch)
                self.process_user_request(config, &user_status, cpu_index);
            }
            crate::RequestTarget::Supervisor => {
                // Request targets the Supervisor
                self.process_supervisor_request(config, &supv_status, cpu_index);
            }
        }
    }

    /// Process a request targeting the User module.
    ///
    /// This function implements the user-mode MMI dispatch pathway:
    /// 1. Builds a fresh `EfiMmEntryContext` with the current CPU index and CPU count
    /// 2. Copies the `EfiMmEntryContext` into the supervisor-to-user data buffer
    /// 3. Appends the `MmCommBufferStatus` immediately after the context
    /// 4. For synchronous MMIs, copies the user comm buffer to the internal copy
    /// 5. Demotes to the user entry point via `invoke_demoted_routine`
    /// 6. On return, copies back the user comm buffer and reads the updated status
    fn process_user_request(&self, config: &CommBufferConfig, status: &MmCommBufferStatus, cpu_index: usize) {
        log::info!("Processing User request...");

        // Validate buffers
        if config.user_comm_buffer == 0 || config.user_comm_buffer_internal == 0 {
            log::error!("User communication buffer not configured");
            return;
        }

        if config.supv_to_user_buffer == 0 {
            log::error!("Supervisor-to-user data buffer not configured");
            return;
        }

        // Get user entry point
        let user_entry = match init_state().user_entry_point() {
            Some(entry) if entry != 0 => entry,
            _ => {
                log::error!("User entry point not configured, cannot demote");
                return;
            }
        };

        // Demote to user entry point to process the request
        let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
            Ok(stack) => stack,
            Err(e) => {
                log::error!("Failed to get CPL3 stack for CPU {cpu_index}: {e:?}");
                return;
            }
        };

        // Build a fresh EfiMmEntryContext with only the fields the user actually needs.
        // The legacy C structure carried pointers (mm_startup_this_ap, cpu_save_state,
        // cpu_save_state_size) that are meaningless in the Rust supervisor model — the
        // user module accesses those services through syscalls instead.
        let entry_context = EfiMmEntryContext {
            mm_startup_this_ap: 0,
            currently_executing_cpu: cpu_index as u64,
            number_of_cpus: self.cpu_manager.registered_count() as u64,
            cpu_save_state_size: 0,
            cpu_save_state: 0,
        };

        // Copy the EfiMmEntryContext + MmCommBufferStatus into the supervisor-to-user
        // data buffer so the user can read processor information after demotion.
        let context_size = core::mem::size_of::<EfiMmEntryContext>();
        let status_size = core::mem::size_of::<MmCommBufferStatus>();

        // Validate the supervisor-to-user buffer is large enough for context + status
        if (config.supv_to_user_buffer_size as usize) < context_size + status_size {
            log::error!(
                "Supervisor-to-user buffer too small: {} < {} (context) + {} (status)",
                config.supv_to_user_buffer_size,
                context_size,
                status_size
            );
            return;
        }

        // Copy the context + status into the supervisor-to-user buffer with SMAP lifted.
        // SAFETY: `supv_to_user_buffer` is the user-owned buffer published by MM IPL and was
        // verified above to hold `context_size + status_size` bytes, so both copies stay inside
        // it and every access made while SMAP is lifted targets that user range.
        unsafe {
            with_user_access(|| {
                // Copy the EfiMmEntryContext to the start of the supervisor-to-user buffer
                core::ptr::copy_nonoverlapping(
                    &raw const entry_context as *const u8,
                    config.supv_to_user_buffer as *mut u8,
                    context_size,
                );

                // Copy the MmCommBufferStatus right after the context
                core::ptr::copy_nonoverlapping(
                    core::ptr::from_ref::<MmCommBufferStatus>(status).cast::<u8>(),
                    (config.supv_to_user_buffer as *mut u8).add(context_size),
                    status_size,
                );
            });
        }

        // Determine whether this is synchronous or asynchronous request
        let sync_mmi = status.is_comm_buffer_valid;

        if sync_mmi != 0 {
            // Copy user buffer to user internal buffer for processing in Ring 3
            // SAFETY: both buffers are the user-owned communication buffers published by MM IPL,
            // and both are `user_comm_buffer_size` bytes, so the copy made while SMAP is lifted
            // stays inside those user ranges.
            unsafe {
                with_user_access(|| {
                    core::ptr::copy_nonoverlapping(
                        config.user_comm_buffer as *const u8,
                        config.user_comm_buffer_internal as *mut u8,
                        config.user_comm_buffer_size as usize,
                    );
                });
            }
            log::trace!(
                "Copied {} bytes from user buffer 0x{:x} to internal 0x{:x}",
                config.user_comm_buffer_size,
                config.user_comm_buffer,
                config.user_comm_buffer_internal
            );
        }

        log::info!("User request is synchronous: {}", sync_mmi != 0);

        // Invoke the demoted user entry point with:
        //   arg1: UserCommandType::UserRequest (command type)
        //   arg2: supv_to_user_buffer (pointer to EfiMmEntryContext + MmCommBufferStatus)
        //   arg3: sizeof(EfiMmEntryContext) (size of the context portion)
        // SAFETY: `user_entry` was validated to be non-zero above and points to the user
        // module entry published by MM IPL. `cpl3_stack` is the per-CPU Ring 3 stack returned
        // by `get_cpl3_stack`. The arg count (3) matches the three argument values passed.
        let ret = unsafe {
            invoke_demoted_routine(
                cpu_index,
                user_entry,
                cpl3_stack,
                3,
                UserCommandType::UserRequest as u64,
                config.supv_to_user_buffer,
                context_size as u64,
            )
        };
        log::info!("Returned from user request with value: 0x{ret}");

        // Copy the response from the internal buffer back to the user buffer
        if sync_mmi != 0 {
            // SAFETY: as for the copy in, both buffers are the user-owned communication buffers
            // published by MM IPL and both are `user_comm_buffer_size` bytes.
            unsafe {
                with_user_access(|| {
                    core::ptr::copy_nonoverlapping(
                        config.user_comm_buffer_internal as *const u8,
                        config.user_comm_buffer as *mut u8,
                        config.user_comm_buffer_size as usize,
                    );
                });
            }
        }

        // Read the updated MmCommBufferStatus back from the supervisor-to-user buffer
        // (the user may have modified return_status and return_buffer_size)
        // SAFETY: `supv_to_user_buffer` is the user-owned buffer verified above to hold
        // `context_size + status_size` bytes, so the status read while SMAP is lifted stays
        // inside that user range.
        let returned_status = unsafe {
            with_user_access(|| {
                core::ptr::read((config.supv_to_user_buffer as *const u8).add(context_size) as *const MmCommBufferStatus)
            })
        };

        // Write the returned status back to the user status mailbox, clearing
        // is_comm_buffer_valid to indicate processing is complete
        let mut final_status = returned_status;
        final_status.is_comm_buffer_valid = 0;

        // Ring 3 filled in `return_buffer_size`, and the non-MM caller uses it to read the
        // response out of the communication buffer. A value past the end of that buffer is not a
        // response the caller can be given any part of: the supervisor cannot tell which bytes
        // the user module meant, so truncating would hand back a prefix of something it never
        // agreed to send. Report the failure instead and return nothing.
        if final_status.return_buffer_size > config.user_comm_buffer_size {
            log::error!(
                "User module reported a 0x{:x}-byte response for a 0x{:x}-byte communication buffer; rejecting",
                final_status.return_buffer_size,
                config.user_comm_buffer_size
            );
            final_status.return_status = efi::Status::BAD_BUFFER_SIZE.as_usize() as u64;
            final_status.return_buffer_size = 0;
        }

        // SAFETY: user_status_buffer is valid and writable
        unsafe {
            let status_ptr = config.user_status_buffer as *mut MmCommBufferStatus;
            core::ptr::write_volatile(status_ptr, final_status);
        }
    }

    /// Process a request targeting the Supervisor.
    ///
    /// Parses the [`EfiMmCommunicateHeader`] from the supervisor communication buffer,
    /// matches the header GUID against the core's built-in handlers followed by the
    /// platform handlers from [`PlatformInfo::mmi_handlers`], and invokes the first
    /// matching handler. This lets platforms link in additional handlers without
    /// modifying the core.
    ///
    /// ## Dispatch Flow
    ///
    /// 0. Reject the request with `ACCESS_DENIED` if `ExitBootServices` has already been signaled
    /// 1. Zero the internal buffer and copy the external supervisor buffer into it
    /// 2. Parse the `EfiMmCommunicateHeader` (GUID + message length) from the internal buffer
    /// 3. Validate message length does not exceed the buffer size
    /// 4. Iterate the default handlers then [`PlatformInfo::mmi_handlers`] to find a handler
    ///    matching the header GUID
    /// 5. Call the handler with a pointer to the data payload and mutable size
    /// 6. Refuse the request if the handler reported more than the payload space it was given
    /// 7. Copy the internal buffer back to the external buffer
    /// 8. Update the status buffer with return status and total response size
    fn process_supervisor_request(&self, config: &CommBufferConfig, status: &MmCommBufferStatus, cpu_index: usize) {
        log::trace!("Processing Supervisor request on CPU {cpu_index}...");

        // Deny any request here after ExitBootServices.
        if init_state().is_at_runtime() {
            log::error!("Supervisor buffer cannot be used for communication after ExitBootServices is signaled!!");
            self.write_supv_status(config, status, efi::Status::ACCESS_DENIED, 0);
            return;
        }

        // Validate buffers
        if config.supv_comm_buffer == 0 || config.supv_comm_buffer_internal == 0 {
            log::error!("Supervisor communication buffer not configured");
            return;
        }

        let buffer_size = config.supv_comm_buffer_size as usize;

        // Zero the internal buffer then copy the external supervisor buffer into it
        // SAFETY: Buffers are provided by MM IPL and are guaranteed valid and non-overlapping
        unsafe {
            core::ptr::write_bytes(config.supv_comm_buffer_internal as *mut u8, 0, buffer_size);
            core::ptr::copy_nonoverlapping(
                config.supv_comm_buffer as *const u8,
                config.supv_comm_buffer_internal as *mut u8,
                buffer_size,
            );
        }

        // Parse the EfiMmCommunicateHeader from the internal buffer
        if buffer_size < EfiMmCommunicateHeader::size() {
            log::error!(
                "Supervisor buffer too small for communicate header: {} < {}",
                buffer_size,
                EfiMmCommunicateHeader::size()
            );
            self.write_supv_status(config, status, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        // SAFETY: We verified the buffer is large enough for the header.
        // The header is packed so we use read_unaligned.
        let header =
            unsafe { core::ptr::read_unaligned(config.supv_comm_buffer_internal as *const EfiMmCommunicateHeader) };

        let message_length = header.message_length();

        // Validate message length doesn't exceed the buffer
        if message_length > buffer_size.saturating_sub(EfiMmCommunicateHeader::size()) {
            log::error!(
                "Message length 0x{:x} exceeds available buffer space 0x{:x}",
                message_length,
                buffer_size - EfiMmCommunicateHeader::size()
            );
            self.write_supv_status(config, status, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        // Compute pointer to the data payload (after the header)
        // SAFETY: `supv_comm_buffer_internal` is a valid buffer of `buffer_size` bytes, and we
        // verified above that `buffer_size >= EfiMmCommunicateHeader::size()`, so offsetting by
        // the header size stays within the same allocation.
        let data_ptr = unsafe { (config.supv_comm_buffer_internal as *mut u8).add(EfiMmCommunicateHeader::size()) };
        let mut data_size = message_length;

        // Dispatch: iterate the default handlers followed by the platform handlers to find a match
        let handler_guid = header.header_guid();
        let mut dispatch_status = efi::Status::NOT_FOUND;

        for handler in DEFAULT_SUPERVISOR_MMI_HANDLERS.iter().chain(P::mmi_handlers().iter()) {
            if patina::Guid::from_ref(&handler.handler_guid) == handler_guid {
                log::trace!(
                    "Dispatching supervisor request to handler '{}' (GUID: {:?})",
                    handler.name,
                    handler.handler_guid
                );
                dispatch_status = (handler.handle)(data_ptr, &mut data_size);
                break;
            }
        }

        if dispatch_status == efi::Status::NOT_FOUND {
            log::warn!("No handler found for supervisor request GUID: {handler_guid:?}");
        }

        // A handler reports its response length back through `data_size`. A value past the
        // payload space it was given describes a response that was never written, so there is no
        // prefix worth copying out: report the failure and return nothing rather than handing the
        // non-MM caller a length that runs past the end of the communication buffer.
        let max_data_size = buffer_size - EfiMmCommunicateHeader::size();
        if data_size > max_data_size {
            log::error!(
                "Handler reported a 0x{data_size:x}-byte response for 0x{max_data_size:x} bytes of payload space; \
                 rejecting"
            );
            self.write_supv_status(config, status, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        // Compute the total response size (header + data) for the copy-back
        let total_response_size = data_size + EfiMmCommunicateHeader::size();

        // Copy the (possibly modified) internal buffer back to the external buffer
        // SAFETY: both buffers are `buffer_size` bytes and an oversized `data_size` returned
        // above, so `total_response_size` is at most `buffer_size` and the copy stays inside
        // both allocations.
        unsafe {
            core::ptr::copy_nonoverlapping(
                config.supv_comm_buffer_internal as *const u8,
                config.supv_comm_buffer as *mut u8,
                total_response_size,
            );
        }
        log::trace!(
            "Copied {} bytes from internal buffer 0x{:x} back to external 0x{:x}",
            total_response_size,
            config.supv_comm_buffer_internal,
            config.supv_comm_buffer
        );

        // Update the status buffer with return status and response size
        let return_status =
            if dispatch_status == efi::Status::SUCCESS { efi::Status::SUCCESS } else { efi::Status::NOT_FOUND };
        self.write_supv_status(config, status, return_status, total_response_size as u64);
    }

    /// Write the supervisor status buffer after processing a supervisor request.
    ///
    /// Clears `is_comm_buffer_valid`, sets return status and size on the
    /// supervisor mailbox.
    fn write_supv_status(
        &self,
        config: &CommBufferConfig,
        _status: &MmCommBufferStatus,
        return_status: efi::Status,
        return_buffer_size: u64,
    ) {
        // SAFETY: supv_status_buffer is valid and writable, set up by MM IPL
        unsafe {
            let status_ptr = config.supv_status_buffer as *mut MmCommBufferStatus;
            let updated = MmCommBufferStatus {
                is_comm_buffer_valid: 0,
                _padding: [0; 7],
                return_status: return_status.as_usize() as u64,
                return_buffer_size,
            };
            core::ptr::write_volatile(status_ptr, updated);
        }
    }

    /// The holding pen for APs.
    ///
    /// APs wait here servicing `RunProcedure` commands from the BSP. The pen exits when
    /// BSP releases this AP via its rendezvous semaphore.
    fn ap_holding_pen(&'static self, cpu_id: u32) {
        // Each CPU owns the mailbox at its dense slot index; resolve it once.
        let cpu_index = if let Some(idx) = self.cpu_manager.find_cpu_index(cpu_id) {
            idx
        } else {
            log::error!("AP (CPU {cpu_id}) has no registered slot; skipping holding pen");
            return;
        };
        log::trace!("AP (CPU {cpu_id}) in holding pen, polling mailbox...");

        loop {
            // Service any pending point-to-point command for this AP.
            if let Some(command) = self.mailbox_manager.check_mailbox(cpu_index) {
                log::trace!("AP (CPU {cpu_id}) received command: {command:?}");

                // Execute the command and post the response.
                let response = self.execute_ap_command(cpu_id, &command);
                self.mailbox_manager.post_response(cpu_index, response);
            }

            // Exit when the BSP releases us from the barrier.
            if self.cpu_manager.take_release_by_index(cpu_index) {
                break;
            }
            core::hint::spin_loop();
        }

        log::info!("AP (CPU {cpu_id}) exiting holding pen");
    }

    /// Execute a command received by an AP.
    fn execute_ap_command(&self, cpu_id: u32, command: &ApCommand) -> ApResponse {
        let ApCommand::RunProcedure { procedure, argument } = *command;
        self.cpu_manager.set_ap_state(cpu_id, ApState::Busy);
        let response = self.run_procedure_on_ap(cpu_id, procedure, argument);
        self.cpu_manager.set_ap_state(cpu_id, ApState::InHoldingPen);
        response
    }

    /// Run a procedure on an AP, demoting to user mode if the procedure is in user-owned range.
    ///
    /// This is the AP-side handler for `ApCommand::RunProcedure`. It mirrors the C
    /// `ProcedureWrapper` logic: inspects the procedure pointer ownership and either
    /// calls it directly (supervisor-owned) or demotes to Ring 3 (user-owned).
    ///
    /// Choosing the ring from the address is only sound because `handle_start_ap_proc` refuses a
    /// procedure that is not user-owned, so nothing Ring 3 named can reach the supervisor branch.
    fn run_procedure_on_ap(&self, cpu_id: u32, procedure: u64, argument: u64) -> ApResponse {
        log::trace!("AP (CPU {cpu_id}) running procedure 0x{procedure:x} with arg 0x{argument:x}");

        if procedure == 0 {
            log::error!("AP (CPU {cpu_id}) received null procedure pointer");
            return ApResponse::Error(efi::Status::INVALID_PARAMETER.as_usize() as u32);
        }

        // Determine if the procedure is in user-owned (Ring 3) range by querying the
        // page table via the centralized helper.
        let is_user_range = match query_address_ownership(procedure, core::mem::size_of::<usize>() as u64) {
            Some(PageOwnership::User) => true,
            Some(PageOwnership::Supervisor) => false,
            None => {
                log::error!(
                    "AP (CPU {cpu_id}) failed to query ownership for 0x{procedure:x} (unmapped or page table not ready)"
                );
                return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
            }
        };

        if is_user_range {
            // Resolve the cpu_index (slot index) for this APIC ID
            let cpu_index = if let Some(idx) = self.cpu_manager.find_cpu_index(cpu_id) {
                idx
            } else {
                log::error!("AP (CPU {cpu_id}) has no registered slot, cannot demote");
                return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
            };

            // Get the CPL3 stack for this CPU
            let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
                Ok(stack) => stack,
                Err(e) => {
                    log::error!("AP (CPU {cpu_id}) failed to get CPL3 stack: {e:?}");
                    return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
                }
            };

            let user_entry = match init_state().user_entry_point() {
                Some(entry) if entry != 0 => entry,
                _ => {
                    log::error!("User entry point not configured, cannot demote AP (CPU {cpu_id})");
                    return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
                }
            };

            // Demote to user mode and call the procedure
            // The procedure signature is: void (EFIAPI *)(void *ProcedureArgument)
            log::trace!(
                "AP (CPU {cpu_id}) demoting to user: proc=0x{procedure:x}, stack=0x{cpl3_stack:x}, arg=0x{argument:x}"
            );

            // SAFETY: `user_entry` was validated to be non-zero above and points to the user
            // module entry published by MM IPL. `cpl3_stack` is the per-CPU Ring 3 stack
            // returned by `get_cpl3_stack`. The arg count (3) matches the three argument values
            // passed.
            let ret = unsafe {
                invoke_demoted_routine(
                    cpu_index,
                    user_entry,
                    cpl3_stack,
                    3,
                    UserCommandType::UserApProcedure as u64,
                    procedure,
                    argument,
                )
            };

            log::trace!("AP (CPU {cpu_id}) returned from demoted procedure: 0x{ret:x}");
            ApResponse::Success
        } else {
            // Supervisor-owned: call directly in Ring 0
            log::trace!("AP (CPU {cpu_id}) calling supervisor procedure directly at 0x{procedure:x}");

            type EfiApProcedure = unsafe extern "efiapi" fn(*mut core::ffi::c_void);
            // SAFETY: The procedure pointer was validated to be non-null above and points to a
            // supervisor-owned (Ring 0) function following the `EfiApProcedure` ABI.
            let proc_fn: EfiApProcedure = unsafe { core::mem::transmute(procedure) };
            // SAFETY: `procedure` is a supervisor-owned (Ring 0) address validated by the BSP and
            // matching the `EfiApProcedure` ABI, so calling it with the provided argument is sound.
            unsafe { proc_fn(argument as *mut core::ffi::c_void) };

            ApResponse::Success
        }
    }

    /// Type-erased trampoline for AP startup, called from the syscall dispatcher.
    ///
    /// This function is conformed for the concrete `P: PlatformInfo` type
    /// and stored as a `fn(u64, u64, u64) -> u64` in [`AP_STARTUP_FN`].
    pub(crate) fn start_ap_procedure_trampoline(cpu_index: u64, procedure: u64, argument: u64) -> u64 {
        let core = Self::instance();
        core.start_ap_procedure(cpu_index, procedure, argument)
    }

    /// Validate and dispatch a procedure to a specific AP.
    ///
    /// Performs validation checks similar to the C `InternalSmmStartupThisAp`:
    /// 1. CPU index is within range of registered CPUs
    /// 2. CPU at that index is present (registered)
    /// 3. CPU is not the BSP
    /// 4. Procedure pointer is non-null
    /// 5. Sends the command via the mailbox (fails if AP is busy)
    /// 6. Waits for the AP to complete (blocking)
    fn start_ap_procedure(&self, cpu_index: u64, procedure: u64, argument: u64) -> u64 {
        let cpu_index = cpu_index as usize;

        // 1. Validate CPU index is within registered count
        let registered = self.cpu_manager.registered_count();
        if cpu_index >= registered {
            log::error!("START_AP: CpuIndex({cpu_index}) >= registered_count({registered})");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        // 2. Look up the APIC ID for this index
        let cpu_id = if let Some(id) = self.cpu_manager.get_cpu_id_by_index(cpu_index) {
            id
        } else {
            log::error!("START_AP: CpuIndex({cpu_index}) has no registered CPU");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        };

        // 3. Check that the target is not the BSP
        if self.cpu_manager.is_bsp(cpu_id) {
            log::error!("START_AP: CpuIndex({cpu_index}) is the BSP, cannot start as AP");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        // 4. Validate procedure pointer is non-null
        if procedure == 0 {
            log::error!("START_AP: Null procedure pointer");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        // 5. Send the RunProcedure command to the AP via mailbox
        //    This will fail if the AP's mailbox is not empty (AP is busy).
        let command = ApCommand::RunProcedure { procedure, argument };
        if self.mailbox_manager.send_command(cpu_index, command).is_err() {
            log::error!("START_AP: AP (CPU {cpu_id}, index {cpu_index}) is busy or mailbox unavailable");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::trace!("START_AP: Dispatched proc=0x{procedure:x} arg=0x{argument:x} to CPU {cpu_id} (index {cpu_index})");

        // 6. Wait for the AP to complete (blocking mode)
        //    Use a generous timeout (10 seconds = 10_000_000 microseconds)
        match self.mailbox_manager.wait_response(cpu_index, AP_TIMEOUT_US) {
            Some(ApResponse::Success) => {
                log::trace!("START_AP: AP (CPU {cpu_id}) completed successfully");
                efi::Status::SUCCESS.as_usize() as u64
            }
            Some(ApResponse::Error(code)) => {
                log::error!("START_AP: AP (CPU {cpu_id}) returned error: 0x{code:x}");
                u64::from(code)
            }
            Some(ApResponse::Busy) => {
                log::error!("START_AP: AP (CPU {cpu_id}) reported busy");
                efi::Status::NOT_READY.as_usize() as u64
            }
            Some(ApResponse::None) | None => {
                log::error!("START_AP: AP (CPU {cpu_id}) timed out or no response");
                efi::Status::TIMEOUT.as_usize() as u64
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{SupervisorMmiHandler, privilege_mgmt::mock};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use patina::Guid;

    /// GUID claimed by [`TestPlatform`]'s MMI handler; distinct from every default handler.
    const TEST_HANDLER_GUID: efi::Guid =
        efi::Guid::from_fields(0x1234_5678, 0x9abc, 0xdef0, 0x12, 0x34, &[0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0]);

    /// Number of times [`test_mmi_handler`] has been invoked in this process.
    static HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);
    /// Payload size the handler reports back, and the status it returns.
    static HANDLER_RESPONSE_SIZE: AtomicUsize = AtomicUsize::new(0);
    static HANDLER_SHOULD_FAIL: AtomicUsize = AtomicUsize::new(0);

    /// Records the call, stamps a byte into the payload, and resizes the response.
    fn test_mmi_handler(comm_buffer: *mut u8, comm_buffer_size: &mut usize) -> efi::Status {
        HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
        if *comm_buffer_size > 0 {
            // SAFETY: the dispatcher passes a pointer to `*comm_buffer_size` writable payload bytes.
            unsafe { comm_buffer.write(0xAB) };
        }
        *comm_buffer_size = HANDLER_RESPONSE_SIZE.load(Ordering::SeqCst);
        if HANDLER_SHOULD_FAIL.load(Ordering::SeqCst) == 0 { efi::Status::SUCCESS } else { efi::Status::DEVICE_ERROR }
    }

    static TEST_HANDLERS: &[SupervisorMmiHandler] =
        &[SupervisorMmiHandler { name: "TestHandler", handler_guid: TEST_HANDLER_GUID, handle: test_mmi_handler }];

    struct TestPlatform;

    impl crate::PlatformInfo for TestPlatform {
        fn mmi_handlers() -> &'static [SupervisorMmiHandler] {
            TEST_HANDLERS
        }
    }

    type TestCore = MmSupervisorCore<TestPlatform, 4>;

    /// Owns the heap allocations a [`CommBufferConfig`] points at, keeping them alive for a test.
    struct TestBuffers {
        supv_external: Vec<u8>,
        supv_internal: Vec<u8>,
        user_external: Vec<u8>,
        user_internal: Vec<u8>,
        supv_to_user: Vec<u8>,
        user_status: Box<MmCommBufferStatus>,
        supv_status: Box<MmCommBufferStatus>,
    }

    impl TestBuffers {
        fn new(comm_size: usize) -> Self {
            Self {
                supv_external: vec![0; comm_size],
                supv_internal: vec![0; comm_size],
                user_external: vec![0; comm_size],
                user_internal: vec![0; comm_size],
                supv_to_user: vec![0; 256],
                user_status: Box::new(MmCommBufferStatus::new()),
                supv_status: Box::new(MmCommBufferStatus::new()),
            }
        }

        fn config(&mut self) -> CommBufferConfig {
            CommBufferConfig {
                supv_comm_buffer: self.supv_external.as_mut_ptr() as u64,
                supv_comm_buffer_internal: self.supv_internal.as_mut_ptr() as u64,
                supv_comm_buffer_size: self.supv_external.len() as u64,
                user_comm_buffer: self.user_external.as_mut_ptr() as u64,
                user_comm_buffer_internal: self.user_internal.as_mut_ptr() as u64,
                user_comm_buffer_size: self.user_external.len() as u64,
                user_status_buffer: core::ptr::from_mut(self.user_status.as_mut()) as u64,
                supv_status_buffer: core::ptr::from_mut(self.supv_status.as_mut()) as u64,
                supv_to_user_buffer: self.supv_to_user.as_mut_ptr() as u64,
                supv_to_user_buffer_size: self.supv_to_user.len() as u64,
            }
        }

        /// Writes a communicate header plus `payload` into the external supervisor buffer.
        fn write_supv_request(&mut self, guid: efi::Guid, message_length: usize, payload: &[u8]) {
            let header = EfiMmCommunicateHeader::new(Guid::from_ref(&guid), message_length);
            let header_size = EfiMmCommunicateHeader::size();
            self.supv_external[..header_size].copy_from_slice(header.as_bytes());
            self.supv_external[header_size..header_size + payload.len()].copy_from_slice(payload);
        }
    }

    fn valid_status() -> MmCommBufferStatus {
        MmCommBufferStatus { is_comm_buffer_valid: 1, ..MmCommBufferStatus::new() }
    }

    #[test]
    fn test_with_user_access_runs_the_closure_and_restores_smap() {
        // SAFETY: the closures touch no memory at all, so there is no user range to validate.
        unsafe {
            assert_eq!(with_user_access(|| 42), 42);
            // The guard is reusable because it is balanced on drop.
            assert_eq!(with_user_access(|| 7), 7);
        }
    }

    #[test]
    fn test_wait_for_ap_arrival_returns_immediately_with_no_aps() {
        let core = TestCore::new();
        core.wait_for_ap_arrival(0);
    }

    #[test]
    fn test_wait_for_ap_arrival_succeeds_once_every_ap_checks_in() {
        let core = TestCore::new();
        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(core.cpu_manager.register_cpu(1, 1, false), Some(1));
        assert_eq!(core.cpu_manager.register_cpu(2, 2, false), Some(2));
        assert!(core.cpu_manager.set_ap_state(1, ApState::InHoldingPen));
        assert!(core.cpu_manager.set_ap_state(2, ApState::InHoldingPen));

        core.wait_for_ap_arrival(2);
    }

    #[test]
    #[should_panic(expected = "fail-secure")]
    fn test_wait_for_ap_arrival_halts_when_an_ap_is_missing() {
        let core = TestCore::new();
        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(core.cpu_manager.register_cpu(1, 1, false), Some(1));

        // The AP never reaches the holding pen, so the arrival window expires.
        core.wait_for_ap_arrival(1);
    }

    #[test]
    fn test_bsp_request_loop_returns_before_the_comm_buffer_is_published() {
        // The PassDown HOB has not been processed, so there is no configuration to act on.
        assert!(security_state().comm_buffer_config().is_none());
        TestCore::new().bsp_request_loop(0);
    }

    #[test]
    fn test_bsp_request_loop_dispatches_the_supervisor_mailbox() {
        let mut buffers = TestBuffers::new(256);
        *buffers.supv_status = valid_status();
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let config = buffers.config();
        security_state().set_comm_buffer_config(config);

        HANDLER_RESPONSE_SIZE.store(4, Ordering::SeqCst);
        TestCore::new().bsp_request_loop(0);

        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(buffers.supv_status.is_comm_buffer_valid, 0);
        assert_eq!(buffers.supv_status.return_status, efi::Status::SUCCESS.as_usize() as u64);
    }

    #[test]
    fn test_bsp_request_loop_ignores_unpublished_status_mailboxes() {
        let mut buffers = TestBuffers::new(256);
        let mut config = buffers.config();
        config.user_status_buffer = 0;
        config.supv_status_buffer = 0;
        security_state().set_comm_buffer_config(config);

        TestCore::new().bsp_request_loop(0);
        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_bsp_request_loop_treats_a_missing_user_mailbox_as_idle() {
        let mut buffers = TestBuffers::new(256);
        *buffers.supv_status = valid_status();
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let mut config = buffers.config();
        // Only the supervisor channel is published; the user mailbox reads as all-zero.
        config.user_status_buffer = 0;
        security_state().set_comm_buffer_config(config);

        HANDLER_RESPONSE_SIZE.store(4, Ordering::SeqCst);
        TestCore::new().bsp_request_loop(0);

        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_bsp_request_loop_routes_an_async_mmi_through_the_user_path() {
        init_state().set_user_entry_point(0x4000);
        let core = TestCore::new();
        core.syscall_interface.init(4, 0x8000, 0x1000).expect("syscall interface initializes");

        let mut buffers = TestBuffers::new(256);
        let mut config = buffers.config();
        // Neither mailbox is valid, so the request is an async MMI. Drop the supervisor
        // mailbox so the user channel is the only published one.
        config.supv_status_buffer = 0;
        security_state().set_comm_buffer_config(config);

        let calls = std::rc::Rc::new(core::cell::Cell::new(0));
        let observed = calls.clone();
        mock::set_handler(move |_, _, _, _, command, _, _| {
            observed.set(observed.get() + 1);
            assert_eq!(command, UserCommandType::UserRequest as u64);
            0
        });

        core.bsp_request_loop(0);
        mock::clear();

        assert_eq!(calls.get(), 1);
        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_process_supervisor_request_rejects_unconfigured_buffers() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        let mut config = buffers.config();
        config.supv_comm_buffer = 0;

        core.process_supervisor_request(&config, &valid_status(), 0);
        // The early return leaves the mailbox untouched.
        assert_eq!(buffers.supv_status.return_status, 0);
    }

    #[test]
    fn test_process_supervisor_request_rejects_a_buffer_too_small_for_a_header() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(EfiMmCommunicateHeader::size() - 1);
        let config = buffers.config();

        core.process_supervisor_request(&config, &valid_status(), 0);
        assert_eq!(buffers.supv_status.return_status, efi::Status::BAD_BUFFER_SIZE.as_usize() as u64);
    }

    #[test]
    fn test_process_supervisor_request_rejects_an_overlong_message() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(64);
        // A message length larger than the payload space the buffer can hold.
        buffers.write_supv_request(TEST_HANDLER_GUID, 1024, &[]);
        let config = buffers.config();

        core.process_supervisor_request(&config, &valid_status(), 0);
        assert_eq!(buffers.supv_status.return_status, efi::Status::BAD_BUFFER_SIZE.as_usize() as u64);
        assert_eq!(buffers.supv_status.is_comm_buffer_valid, 0);
    }

    #[test]
    fn test_process_supervisor_request_reports_an_unhandled_guid() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        let unknown = efi::Guid::from_fields(0xdead_beef, 0, 0, 0, 0, &[0; 6]);
        buffers.write_supv_request(unknown, 4, &[1, 2, 3, 4]);
        let config = buffers.config();

        core.process_supervisor_request(&config, &valid_status(), 0);

        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(buffers.supv_status.return_status, efi::Status::NOT_FOUND.as_usize() as u64);
    }

    #[test]
    fn test_process_supervisor_request_dispatches_to_a_platform_handler() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let config = buffers.config();
        HANDLER_RESPONSE_SIZE.store(8, Ordering::SeqCst);

        core.process_supervisor_request(&config, &valid_status(), 0);

        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 1);
        // The handler's payload edit is copied back to the external buffer.
        assert_eq!(buffers.supv_external[EfiMmCommunicateHeader::size()], 0xAB);
        assert_eq!(buffers.supv_status.return_status, efi::Status::SUCCESS.as_usize() as u64);
        assert_eq!(buffers.supv_status.return_buffer_size, (8 + EfiMmCommunicateHeader::size()) as u64);
    }

    #[test]
    fn test_process_supervisor_request_maps_a_handler_failure_to_not_found() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let config = buffers.config();
        HANDLER_RESPONSE_SIZE.store(4, Ordering::SeqCst);
        HANDLER_SHOULD_FAIL.store(1, Ordering::SeqCst);

        core.process_supervisor_request(&config, &valid_status(), 0);

        assert_eq!(buffers.supv_status.return_status, efi::Status::NOT_FOUND.as_usize() as u64);
    }

    #[test]
    fn test_process_supervisor_request_rejects_an_oversized_response() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(64);
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let config = buffers.config();
        // A handler that reports more than it was given must not have that size reach the caller,
        // which would use it to read past the end of the communication buffer.
        HANDLER_RESPONSE_SIZE.store(1024, Ordering::SeqCst);

        core.process_supervisor_request(&config, &valid_status(), 0);

        // Nothing is copied out and the caller is told why, rather than being handed a prefix of
        // a response the handler never agreed to send.
        assert_eq!(buffers.supv_status.return_status, efi::Status::BAD_BUFFER_SIZE.as_usize() as u64);
        assert_eq!(buffers.supv_status.return_buffer_size, 0);
    }

    #[test]
    fn test_process_supervisor_request_reports_an_oversized_response_without_overflowing() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(64);
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let config = buffers.config();
        // Adding the header to this size would wrap an unchecked `usize`.
        HANDLER_RESPONSE_SIZE.store(usize::MAX, Ordering::SeqCst);

        core.process_supervisor_request(&config, &valid_status(), 0);

        assert_eq!(buffers.supv_status.return_status, efi::Status::BAD_BUFFER_SIZE.as_usize() as u64);
        assert_eq!(buffers.supv_status.return_buffer_size, 0);
    }

    #[test]
    fn test_process_supervisor_request_is_denied_after_exit_boot_services() {
        assert!(init_state().mark_at_runtime());

        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        buffers.write_supv_request(TEST_HANDLER_GUID, 4, &[1, 2, 3, 4]);
        let config = buffers.config();

        core.process_supervisor_request(&config, &valid_status(), 0);

        assert_eq!(HANDLER_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(buffers.supv_status.return_status, efi::Status::ACCESS_DENIED.as_usize() as u64);
    }

    #[test]
    fn test_write_supv_status_clears_validity_and_records_the_result() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(64);
        *buffers.supv_status = valid_status();
        let config = buffers.config();

        core.write_supv_status(&config, &valid_status(), efi::Status::UNSUPPORTED, 0x40);

        assert_eq!(buffers.supv_status.is_comm_buffer_valid, 0);
        assert_eq!(buffers.supv_status.return_status, efi::Status::UNSUPPORTED.as_usize() as u64);
        assert_eq!(buffers.supv_status.return_buffer_size, 0x40);
    }

    #[test]
    fn test_process_user_request_requires_its_buffers() {
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);

        let mut no_comm = buffers.config();
        no_comm.user_comm_buffer = 0;
        core.process_user_request(&no_comm, &valid_status(), 0);

        let mut no_internal = buffers.config();
        no_internal.user_comm_buffer_internal = 0;
        core.process_user_request(&no_internal, &valid_status(), 0);

        let mut no_supv_to_user = buffers.config();
        no_supv_to_user.supv_to_user_buffer = 0;
        core.process_user_request(&no_supv_to_user, &valid_status(), 0);

        // Every path returned before touching the user mailbox.
        assert_eq!(buffers.user_status.return_status, 0);
    }

    #[test]
    fn test_process_user_request_requires_a_user_entry_point() {
        // The user module entry point is only published once the HOB list is parsed.
        assert!(init_state().user_entry_point().is_none());

        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        let config = buffers.config();

        core.process_user_request(&config, &valid_status(), 0);
        assert_eq!(buffers.user_status.return_status, 0);
    }

    #[test]
    fn test_process_user_request_requires_a_ring3_stack() {
        init_state().set_user_entry_point(0x4000);

        // The syscall interface is uninitialized, so no CPL3 stack can be resolved.
        let core = TestCore::new();
        let mut buffers = TestBuffers::new(256);
        let config = buffers.config();

        core.process_user_request(&config, &valid_status(), 0);
        assert_eq!(buffers.user_status.return_status, 0);
    }

    #[test]
    fn test_process_user_request_requires_room_for_the_entry_context() {
        init_state().set_user_entry_point(0x4000);
        let core = TestCore::new();
        core.syscall_interface.init(4, 0x8000, 0x1000).expect("syscall interface initializes");

        let mut buffers = TestBuffers::new(256);
        let mut config = buffers.config();
        config.supv_to_user_buffer_size = 1;

        core.process_user_request(&config, &valid_status(), 0);
        assert_eq!(buffers.user_status.return_status, 0);
    }

    #[test]
    fn test_process_user_request_round_trips_a_synchronous_request() {
        init_state().set_user_entry_point(0x4000);
        let core = TestCore::new();
        core.syscall_interface.init(4, 0x8000, 0x1000).expect("syscall interface initializes");
        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));

        let mut buffers = TestBuffers::new(256);
        buffers.user_external[..4].copy_from_slice(&[1, 2, 3, 4]);
        let config = buffers.config();
        let supv_to_user = config.supv_to_user_buffer;
        let context_size = core::mem::size_of::<EfiMmEntryContext>();

        // The mock stands in for the Ring 0 -> Ring 3 transition: it verifies the arguments,
        // edits the internal buffer, and reports a status through the shared window.
        mock::set_handler(move |_cpu, entry, _stack, arg_count, command, buffer, size| {
            assert_eq!(entry, 0x4000);
            assert_eq!(arg_count, 3);
            assert_eq!(command, UserCommandType::UserRequest as u64);
            assert_eq!(buffer, supv_to_user);
            assert_eq!(size, context_size as u64);

            // SAFETY: the supervisor placed an `MmCommBufferStatus` right after the context.
            unsafe {
                let status = (supv_to_user as *mut u8).add(context_size) as *mut MmCommBufferStatus;
                (*status).return_status = efi::Status::SUCCESS.as_usize() as u64;
                (*status).return_buffer_size = 4;
            }
            0
        });

        core.process_user_request(&config, &valid_status(), 0);
        mock::clear();

        // The request was staged into the internal buffer and the response copied back out.
        assert_eq!(&buffers.user_internal[..4], &[1, 2, 3, 4]);
        assert_eq!(buffers.user_status.is_comm_buffer_valid, 0);
        assert_eq!(buffers.user_status.return_status, efi::Status::SUCCESS.as_usize() as u64);
        assert_eq!(buffers.user_status.return_buffer_size, 4);

        // The entry context handed to the user names this CPU and the registered CPU count.
        // SAFETY: `process_user_request` wrote the context at the start of the shared window.
        let context = unsafe { core::ptr::read(supv_to_user as *const EfiMmEntryContext) };
        assert_eq!(context.currently_executing_cpu, 0);
        assert_eq!(context.number_of_cpus, 1);
    }

    #[test]
    fn test_process_user_request_rejects_an_oversized_response_from_ring_3() {
        init_state().set_user_entry_point(0x4000);
        let core = TestCore::new();
        core.syscall_interface.init(4, 0x8000, 0x1000).expect("syscall interface initializes");
        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));

        let mut buffers = TestBuffers::new(256);
        let config = buffers.config();
        let supv_to_user = config.supv_to_user_buffer;
        let context_size = core::mem::size_of::<EfiMmEntryContext>();

        // A user module that reports more than the communication buffer holds would otherwise
        // send the non-MM caller reading past the end of it.
        mock::set_handler(move |_cpu, _entry, _stack, _arg_count, _command, _buffer, _size| {
            // SAFETY: the supervisor placed an `MmCommBufferStatus` right after the context.
            unsafe {
                let status = (supv_to_user as *mut u8).add(context_size) as *mut MmCommBufferStatus;
                (*status).return_status = efi::Status::SUCCESS.as_usize() as u64;
                (*status).return_buffer_size = u64::MAX;
            }
            0
        });

        core.process_user_request(&config, &valid_status(), 0);
        mock::clear();

        assert_eq!(buffers.user_status.return_status, efi::Status::BAD_BUFFER_SIZE.as_usize() as u64);
        assert_eq!(buffers.user_status.return_buffer_size, 0);
    }

    #[test]
    fn test_process_user_request_keeps_a_response_that_fits() {
        init_state().set_user_entry_point(0x4000);
        let core = TestCore::new();
        core.syscall_interface.init(4, 0x8000, 0x1000).expect("syscall interface initializes");
        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));

        let mut buffers = TestBuffers::new(256);
        let config = buffers.config();
        let supv_to_user = config.supv_to_user_buffer;
        let context_size = core::mem::size_of::<EfiMmEntryContext>();

        // Exactly the buffer size is legitimate and must reach the caller untouched.
        mock::set_handler(move |_cpu, _entry, _stack, _arg_count, _command, _buffer, _size| {
            // SAFETY: the supervisor placed an `MmCommBufferStatus` right after the context.
            unsafe {
                let status = (supv_to_user as *mut u8).add(context_size) as *mut MmCommBufferStatus;
                (*status).return_buffer_size = 256;
            }
            0
        });

        core.process_user_request(&config, &valid_status(), 0);
        mock::clear();

        assert_eq!(buffers.user_status.return_buffer_size, 256);
    }

    #[test]
    fn test_process_user_request_skips_the_buffer_copy_for_an_async_mmi() {
        init_state().set_user_entry_point(0x4000);
        let core = TestCore::new();
        core.syscall_interface.init(4, 0x8000, 0x1000).expect("syscall interface initializes");

        let mut buffers = TestBuffers::new(256);
        buffers.user_external[..4].copy_from_slice(&[1, 2, 3, 4]);
        let config = buffers.config();

        // An async MMI has no valid comm buffer, so nothing is staged for the user.
        core.process_user_request(&config, &MmCommBufferStatus::new(), 0);

        assert_eq!(&buffers.user_internal[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_ap_holding_pen_skips_an_unregistered_cpu() {
        static CORE: TestCore = TestCore::new();
        CORE.ap_holding_pen(9);
    }

    #[test]
    fn test_ap_holding_pen_exits_once_released() {
        static CORE: TestCore = TestCore::new();
        assert_eq!(CORE.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(CORE.cpu_manager.register_cpu(1, 1, false), Some(1));
        // The BSP has already signalled the exit barrier, so the pen drains on the first poll.
        CORE.cpu_manager.release_all_aps();

        CORE.ap_holding_pen(1);
    }

    #[test]
    fn test_ap_holding_pen_services_a_pending_command_before_exiting() {
        static CORE: TestCore = TestCore::new();
        assert_eq!(CORE.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(CORE.cpu_manager.register_cpu(1, 1, false), Some(1));
        // A null procedure is rejected by the AP, which still posts a response.
        CORE.mailbox_manager.send_command(1, ApCommand::RunProcedure { procedure: 0, argument: 0 }).unwrap();
        CORE.cpu_manager.release_all_aps();

        CORE.ap_holding_pen(1);

        assert!(matches!(CORE.mailbox_manager.wait_response(1, 1_000), Some(ApResponse::Error(_))));
        assert_eq!(CORE.cpu_manager.get_ap_state(1), Some(ApState::InHoldingPen));
    }

    #[test]
    fn test_execute_ap_command_restores_the_holding_pen_state() {
        let core = TestCore::new();
        assert_eq!(core.cpu_manager.register_cpu(1, 0, false), Some(0));

        let response = core.execute_ap_command(1, &ApCommand::RunProcedure { procedure: 0, argument: 0 });

        assert!(matches!(response, ApResponse::Error(_)));
        assert_eq!(core.cpu_manager.get_ap_state(1), Some(ApState::InHoldingPen));
    }

    #[test]
    fn test_run_procedure_on_ap_rejects_a_null_procedure() {
        let core = TestCore::new();
        let response = core.run_procedure_on_ap(1, 0, 0);
        assert_eq!(response, ApResponse::Error(efi::Status::INVALID_PARAMETER.as_usize() as u32));
    }

    #[test]
    fn test_run_procedure_on_ap_rejects_an_unmapped_procedure() {
        // Without a page table the supervisor cannot prove which ring owns the procedure.
        *security_state().lock_page_table() = None;

        let core = TestCore::new();
        let response = core.run_procedure_on_ap(1, 0x1000, 0);
        assert_eq!(response, ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32));
    }

    #[test]
    fn test_start_ap_procedure_validates_the_target_cpu() {
        let core = TestCore::new();
        let invalid = efi::Status::INVALID_PARAMETER.as_usize() as u64;

        // No CPUs are registered yet, so every index is out of range.
        assert_eq!(core.start_ap_procedure(0, 0x1000, 0), invalid);

        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(core.cpu_manager.register_cpu(1, 1, false), Some(1));

        // Index 0 is the BSP, which cannot be told to run an AP procedure.
        assert_eq!(core.start_ap_procedure(0, 0x1000, 0), invalid);
        // Beyond the registered count.
        assert_eq!(core.start_ap_procedure(2, 0x1000, 0), invalid);
        // A null procedure pointer.
        assert_eq!(core.start_ap_procedure(1, 0, 0), invalid);
    }

    #[test]
    fn test_start_ap_procedure_rejects_an_unpopulated_slot() {
        let core = TestCore::new();
        // Registering out of order leaves slot 0 empty while raising the registered count.
        assert_eq!(core.cpu_manager.register_cpu(7, 1, false), Some(1));

        assert_eq!(core.start_ap_procedure(0, 0x1000, 0), efi::Status::INVALID_PARAMETER.as_usize() as u64);
    }

    #[test]
    fn test_start_ap_procedure_rejects_a_busy_mailbox() {
        let core = TestCore::new();
        assert_eq!(core.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(core.cpu_manager.register_cpu(1, 1, false), Some(1));
        // Occupy the mailbox so the dispatch has nowhere to post.
        core.mailbox_manager.send_command(1, ApCommand::RunProcedure { procedure: 0x2000, argument: 0 }).unwrap();

        assert_eq!(core.start_ap_procedure(1, 0x1000, 0), efi::Status::INVALID_PARAMETER.as_usize() as u64);
    }

    #[test]
    fn test_start_ap_procedure_reports_the_ap_response() {
        static CORE: TestCore = TestCore::new();
        assert_eq!(CORE.cpu_manager.register_cpu(0, 0, true), Some(0));
        assert_eq!(CORE.cpu_manager.register_cpu(1, 1, false), Some(1));

        for (posted, expected) in [
            (ApResponse::Success, efi::Status::SUCCESS.as_usize() as u64),
            (ApResponse::Error(0x1234), 0x1234),
            (ApResponse::Busy, efi::Status::NOT_READY.as_usize() as u64),
            (ApResponse::None, efi::Status::TIMEOUT.as_usize() as u64),
        ] {
            // Stand in for the AP: drain the command and answer it.
            let responder = std::thread::spawn(move || {
                while CORE.mailbox_manager.check_mailbox(1).is_none() {
                    core::hint::spin_loop();
                }
                CORE.mailbox_manager.post_response(1, posted);
            });

            assert_eq!(CORE.start_ap_procedure(1, 0x1000, 0x5678), expected);
            responder.join().expect("responder thread completes");
            CORE.mailbox_manager.reset_all();
        }
    }
}
