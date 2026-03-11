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

use patina::{
    management_mode::{MmCommBufferStatus, supervisor::UserCommandType},
    pi::{mm_cis::EfiMmEntryContext, protocols::communication::EfiMmCommunicateHeader},
};
use r_efi::efi;

use crate::{
    AP_ARRIVAL_TIMEOUT_US, AP_TIMEOUT_US, CommBufferConfig, MmSupervisorCore, PageOwnership, PlatformInfo,
    cpu::{ApState, is_bsp},
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
    #[cfg(all(not(test), target_arch = "x86_64"))]
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
    #[cfg(all(not(test), target_arch = "x86_64"))]
    unsafe {
        core::arch::asm!(
            "clac", // Clear AC flag to re-enable SMAP protections
            options(nostack, preserves_flags)
        );
    }
}

/// Runs `access` with SMAP temporarily disabled so the supervisor can read or
/// write user-owned memory, restoring SMAP protection when it returns.
fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
    // SAFETY: `disable_smap`/`enable_smap` are called as a balanced pair around `access`,
    // upholding the invariant that SMAP protection is always restored before returning. The
    // caller of `with_user_access` is responsible for ensuring `access` only touches valid,
    // correctly-owned user memory while SMAP is lifted.
    unsafe {
        disable_smap();
        let result = access();
        enable_smap();
        result
    }
}

impl<P: PlatformInfo, const MAX_CPUS: usize> MmSupervisorCore<P, MAX_CPUS> {
    /// Enter runtime mode (called on subsequent entries after init is complete).
    ///
    /// Implements the MP synchronization protocol:
    /// 1. APs check in by setting their state to `InHoldingPen` and entering the holding pen
    /// 2. BSP waits for all registered APs, all cores must be in MM before servicing a request
    /// 3. BSP processes the pending request via `bsp_request_loop`
    /// 4. BSP releases every AP via the per-CPU rendezvous semaphore
    /// 5. BSP waits indefinitely for every released AP to acknowledge it has left
    /// 6. Each AP clears its `InHoldingPen` state and acknowledges the BSP
    pub(crate) fn enter_runtime(&'static self, cpu_id: u32) {
        let is_bsp = is_bsp();

        if is_bsp {
            log::info!("BSP (CPU {}) waiting for APs to arrive...", cpu_id);

            // Wait for all registered APs to check in (set state to InHoldingPen).
            let expected_aps = self.cpu_manager.registered_count().saturating_sub(1);
            self.wait_for_ap_arrival(expected_aps);

            // Every registered AP is now in MM - service the request.
            log::trace!("BSP (CPU {}) entering request serving routine...", cpu_id);
            self.bsp_request_loop(cpu_id as usize);

            // Exit barrier: release every penned AP and wait for each to acknowledge it has left.
            log::trace!("BSP (CPU {}) releasing all APs from the holding pen...", cpu_id);
            self.cpu_manager.release_all_aps();
            self.cpu_manager.wait_for_ap_exit_acks(expected_aps);

            self.mailbox_manager.reset_all();
        } else {
            // AP: check in by marking state, then enter holding pen.
            self.cpu_manager.set_ap_state(cpu_id, ApState::InHoldingPen);
            log::info!("AP (CPU {}) checked in, entering holding pen...", cpu_id);
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

        let all_arrived = crate::perf_timer::spin_until::<P::CpuInfo, _>(AP_ARRIVAL_TIMEOUT_US, || {
            self.cpu_manager.count_aps_in_state(ApState::InHoldingPen) >= expected_aps
        });

        if all_arrived {
            log::trace!("All {} APs arrived", expected_aps);
        } else {
            // All cores have to rendezvous in the holding pen before the BSP services a request. If any core
            // fails to arrive within the window, this is a security-fatal condition.
            let arrived = self.cpu_manager.count_aps_in_state(ApState::InHoldingPen);
            panic!(
                "MM Supervisor fail-secure: only {}/{} APs rendezvoused within the arrival window; \
                 refusing to service the request with a partial set of cores",
                arrived, expected_aps
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
                log::error!("Failed to get CPL3 stack for CPU {}: {:?}", cpu_index, e);
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
        // SAFETY: supv_to_user_buffer is valid and large enough, verified above.
        with_user_access(|| unsafe {
            // Copy the EfiMmEntryContext to the start of the supervisor-to-user buffer
            core::ptr::copy_nonoverlapping(
                &entry_context as *const EfiMmEntryContext as *const u8,
                config.supv_to_user_buffer as *mut u8,
                context_size,
            );

            // Copy the MmCommBufferStatus right after the context
            core::ptr::copy_nonoverlapping(
                status as *const MmCommBufferStatus as *const u8,
                (config.supv_to_user_buffer as *mut u8).add(context_size),
                status_size,
            );
        });

        // Determine whether this is synchronous or asynchronous request
        let sync_mmi = status.is_comm_buffer_valid;

        if sync_mmi != 0 {
            // Copy user buffer to user internal buffer for processing in Ring 3
            // SAFETY: Buffers are provided by MM IPL and are guaranteed valid
            with_user_access(|| unsafe {
                core::ptr::copy_nonoverlapping(
                    config.user_comm_buffer as *const u8,
                    config.user_comm_buffer_internal as *mut u8,
                    config.user_comm_buffer_size as usize,
                );
            });
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
        log::info!("Returned from user request with value: 0x{}", ret);

        // Copy the response from the internal buffer back to the user buffer
        if sync_mmi != 0 {
            // SAFETY: Buffers are provided by MM IPL and are guaranteed valid.
            with_user_access(|| unsafe {
                core::ptr::copy_nonoverlapping(
                    config.user_comm_buffer_internal as *const u8,
                    config.user_comm_buffer as *mut u8,
                    config.user_comm_buffer_size as usize,
                );
            });
        }

        // Read the updated MmCommBufferStatus back from the supervisor-to-user buffer
        // (the user may have modified return_status and return_buffer_size)
        // SAFETY: supv_to_user_buffer is valid and the status is at offset context_size
        let returned_status = with_user_access(|| unsafe {
            core::ptr::read((config.supv_to_user_buffer as *const u8).add(context_size) as *const MmCommBufferStatus)
        });

        // Write the returned status back to the user status mailbox, clearing
        // is_comm_buffer_valid to indicate processing is complete
        // SAFETY: user_status_buffer is valid and writable
        unsafe {
            let status_ptr = config.user_status_buffer as *mut MmCommBufferStatus;
            let mut final_status = returned_status;
            final_status.is_comm_buffer_valid = 0;
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
    /// 0. Reject the request with `ACCESS_DENIED` if ExitBootServices has already been signaled
    /// 1. Zero the internal buffer and copy the external supervisor buffer into it
    /// 2. Parse the `EfiMmCommunicateHeader` (GUID + message length) from the internal buffer
    /// 3. Validate message length does not exceed the buffer size
    /// 4. Iterate the default handlers then [`PlatformInfo::mmi_handlers`] to find a handler
    ///    matching the header GUID
    /// 5. Call the handler with a pointer to the data payload and mutable size
    /// 6. Update the status buffer with return status and total response size
    /// 7. Copy the internal buffer back to the external buffer
    fn process_supervisor_request(&self, config: &CommBufferConfig, status: &MmCommBufferStatus, cpu_index: usize) {
        log::trace!("Processing Supervisor request on CPU {}...", cpu_index);

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
            log::warn!("No handler found for supervisor request GUID: {:?}", handler_guid);
        }

        // Compute the total response size (header + data) for the copy-back
        let total_response_size = data_size + EfiMmCommunicateHeader::size();

        // Copy the (possibly modified) internal buffer back to the external buffer
        if total_response_size <= buffer_size {
            // SAFETY: Both buffers are valid and total_response_size is within bounds
            unsafe {
                core::ptr::copy_nonoverlapping(
                    config.supv_comm_buffer_internal as *const u8,
                    config.supv_comm_buffer as *mut u8,
                    total_response_size,
                );
            }
        } else {
            log::error!("Response size 0x{:x} exceeds buffer capacity 0x{:x}", total_response_size, buffer_size);
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
    /// APs wait here servicing RunProcedure commands from the BSP. The pen exits when
    /// BSP releases this AP via its rendezvous semaphore.
    fn ap_holding_pen(&'static self, cpu_id: u32) {
        // Each CPU owns the mailbox at its dense slot index; resolve it once.
        let cpu_index = match self.cpu_manager.find_cpu_index(cpu_id) {
            Some(idx) => idx,
            None => {
                log::error!("AP (CPU {}) has no registered slot; skipping holding pen", cpu_id);
                return;
            }
        };
        log::trace!("AP (CPU {}) in holding pen, polling mailbox...", cpu_id);

        loop {
            // Service any pending point-to-point command for this AP.
            if let Some(command) = self.mailbox_manager.check_mailbox(cpu_index) {
                log::trace!("AP (CPU {}) received command: {:?}", cpu_id, command);

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

        log::trace!("AP (CPU {}) exiting holding pen", cpu_id);
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
    fn run_procedure_on_ap(&self, cpu_id: u32, procedure: u64, argument: u64) -> ApResponse {
        log::trace!("AP (CPU {}) running procedure 0x{:x} with arg 0x{:x}", cpu_id, procedure, argument);

        if procedure == 0 {
            log::error!("AP (CPU {}) received null procedure pointer", cpu_id);
            return ApResponse::Error(efi::Status::INVALID_PARAMETER.as_usize() as u32);
        }

        // Determine if the procedure is in user-owned (Ring 3) range by querying the
        // page table via the centralized helper.
        let is_user_range = match query_address_ownership(procedure, core::mem::size_of::<usize>() as u64) {
            Some(PageOwnership::User) => true,
            Some(PageOwnership::Supervisor) => false,
            None => {
                log::error!(
                    "AP (CPU {}) failed to query ownership for 0x{:x} (unmapped or page table not ready)",
                    cpu_id,
                    procedure
                );
                return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
            }
        };

        if is_user_range {
            // Resolve the cpu_index (slot index) for this APIC ID
            let cpu_index = match self.cpu_manager.find_cpu_index(cpu_id) {
                Some(idx) => idx,
                None => {
                    log::error!("AP (CPU {}) has no registered slot, cannot demote", cpu_id);
                    return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
                }
            };

            // Get the CPL3 stack for this CPU
            let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
                Ok(stack) => stack,
                Err(e) => {
                    log::error!("AP (CPU {}) failed to get CPL3 stack: {:?}", cpu_id, e);
                    return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
                }
            };

            let user_entry = match init_state().user_entry_point() {
                Some(entry) if entry != 0 => entry,
                _ => {
                    log::error!("User entry point not configured, cannot demote AP (CPU {})", cpu_id);
                    return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
                }
            };

            // Demote to user mode and call the procedure
            // The procedure signature is: void (EFIAPI *)(void *ProcedureArgument)
            log::trace!(
                "AP (CPU {}) demoting to user: proc=0x{:x}, stack=0x{:x}, arg=0x{:x}",
                cpu_id,
                procedure,
                cpl3_stack,
                argument
            );

            // SAFETY: `user_entry` was validated to be non-zero above and points to the user
            // module entry published by MM IPL. `cpl3_stack` is the per-CPU Ring 3 stack
            // returned by `get_cpl3_stack`. The arg count (3) matches the three argument values
            // passed.
            let _ret = unsafe {
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

            log::trace!("AP (CPU {}) returned from demoted procedure: 0x{:x}", cpu_id, _ret);
            ApResponse::Success
        } else {
            // Supervisor-owned: call directly in Ring 0
            log::trace!("AP (CPU {}) calling supervisor procedure directly at 0x{:x}", cpu_id, procedure);

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
            log::error!("START_AP: CpuIndex({}) >= registered_count({})", cpu_index, registered);
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        // 2. Look up the APIC ID for this index
        let cpu_id = match self.cpu_manager.get_cpu_id_by_index(cpu_index) {
            Some(id) => id,
            None => {
                log::error!("START_AP: CpuIndex({}) has no registered CPU", cpu_index);
                return efi::Status::INVALID_PARAMETER.as_usize() as u64;
            }
        };

        // 3. Check that the target is not the BSP
        if self.cpu_manager.is_bsp(cpu_id) {
            log::error!("START_AP: CpuIndex({}) is the BSP, cannot start as AP", cpu_index);
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
            log::error!("START_AP: AP (CPU {}, index {}) is busy or mailbox unavailable", cpu_id, cpu_index);
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::trace!(
            "START_AP: Dispatched proc=0x{:x} arg=0x{:x} to CPU {} (index {})",
            procedure,
            argument,
            cpu_id,
            cpu_index
        );

        // 6. Wait for the AP to complete (blocking mode)
        //    Use a generous timeout (10 seconds = 10_000_000 microseconds)
        match self.mailbox_manager.wait_response(cpu_index, AP_TIMEOUT_US) {
            Some(ApResponse::Success) => {
                log::trace!("START_AP: AP (CPU {}) completed successfully", cpu_id);
                efi::Status::SUCCESS.as_usize() as u64
            }
            Some(ApResponse::Error(code)) => {
                log::error!("START_AP: AP (CPU {}) returned error: 0x{:x}", cpu_id, code);
                code as u64
            }
            Some(ApResponse::Busy) => {
                log::error!("START_AP: AP (CPU {}) reported busy", cpu_id);
                efi::Status::NOT_READY.as_usize() as u64
            }
            Some(ApResponse::None) | None => {
                log::error!("START_AP: AP (CPU {}) timed out or no response", cpu_id);
                efi::Status::TIMEOUT.as_usize() as u64
            }
        }
    }
}
