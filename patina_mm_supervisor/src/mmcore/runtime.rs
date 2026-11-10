//! `MmSupervisorCore` Runtime Phase
//!
//! The dispatch loop the BSP runs and the holding pen the APs wait in, entered
//! on every MMI after the first. Each core returns to its caller once the BSP has
//! served the request and every AP has left the holding pen.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::standard::efi;
use patina::{
    MemoryRange,
    management_mode::{MmCommBufferStatus, supervisor::UserCommandType},
    pi::{mm_cis::EfiMmEntryContext, protocol::communication::EfiMmCommunicateHeader},
};

use super::{
    cpu::ApState,
    mailbox::{ApCommand, ApResponse},
    request_target::RequestTarget,
};
use crate::{
    MmSupervisorCore, PlatformInfo,
    comm_buffer::CommBufferConfig,
    handlers::DEFAULT_SUPERVISOR_MMI_HANDLERS,
    intrinsics::is_bsp,
    memory::page_ownership::{PageOwnership, query_address_ownership},
    privilege_mgmt::{DEMOTED_ROUTINE_ARG_COUNT, invoke_demoted_routine},
    state::{init_state, security_state},
    user_access_guard::with_user_access,
};

/// Timeout for waiting for APs to arrive in the holding pen (1 second).
const AP_ARRIVAL_TIMEOUT_US: u64 = 1_000_000;

/// Timeout for waiting for released APs to acknowledge leaving the holding pen (1 second).
const AP_EXIT_TIMEOUT_US: u64 = 1_000_000;

/// Timeout for waiting for an AP to complete a dispatched procedure (10 seconds).
const AP_TIMEOUT_US: u64 = 10_000_000;

impl<P: PlatformInfo, const MAX_CPUS: usize> MmSupervisorCore<P, MAX_CPUS> {
    /// Serves one MMI on a core that has finished initialization.
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
            let expected_aps = self.cpu_manager.registered_count().saturating_sub(1);

            // Wait for all registered APs to check in (set state to InHoldingPen).
            self.wait_for_ap_arrival(expected_aps);

            // Every registered AP is in MM, so the request can be served.
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
            log::trace!("AP (CPU {cpu_id}) checked in, entering holding pen...");
            self.ap_holding_pen(cpu_id);

            // Check out: clear the InHoldingPen state and acknowledge the BSP.
            self.cpu_manager.set_ap_state(cpu_id, ApState::NotPresent);
            self.cpu_manager.ack_exit_to_bsp();
        }
    }

    /// Waits for every registered AP to check in to the holding pen.
    ///
    /// ## Panics
    ///
    /// Panics if any AP misses the arrival window. Serving a request with only some of the cores
    /// in MM is not safe.
    fn wait_for_ap_arrival(&self, expected_aps: usize) {
        if expected_aps == 0 {
            return;
        }

        let all_arrived = super::perf_timer::spin_until(AP_ARRIVAL_TIMEOUT_US, || {
            self.cpu_manager.count_aps_in_state(ApState::InHoldingPen) >= expected_aps
        });

        if all_arrived {
            log::trace!("All {expected_aps} AP(s) arrived");
        } else {
            // Every core must be in the holding pen before the BSP serves a request, so a core that
            // misses the window is a security-fatal condition.
            let arrived = self.cpu_manager.count_aps_in_state(ApState::InHoldingPen);
            panic!(
                "MM Supervisor fail-secure: only {arrived}/{expected_aps} APs rendezvoused within the arrival window; \
                 refusing to service the request with a partial set of cores"
            );
        }
    }

    /// Serves the pending request, if any, on the BSP.
    ///
    /// Reads the user and supervisor `MmCommBufferStatus` mailboxes and lets
    /// [`RequestTarget::select`] pick the target. A user request is handled by demoting to the
    /// user entry point, with a synchronous request's buffer copied into the user internal buffer
    /// first. A supervisor request goes
    /// to the supervisor handlers. When neither mailbox is valid the MMI is asynchronous and still
    /// takes the user path, so the user core's async handlers run.
    ///
    /// Returns the target that was served, or [`RequestTarget::None`] when the communication
    /// buffers are not set up yet.
    fn bsp_request_loop(&self, cpu_index: usize) -> RequestTarget {
        let config = match security_state().comm_buffer_config() {
            Some(config) => config,
            None => {
                return RequestTarget::None;
            }
        };

        // Bail out only if neither status mailbox is wired up yet.
        if config.user.status == 0 && config.supervisor.status == 0 {
            return RequestTarget::None;
        }

        // A mailbox that is not published reads as an all-zero (idle) status.
        let user_status = if config.user.status != 0 {
            // SAFETY: `config.user.status` is non-zero here and, provided by the MM IPL, references
            // an MMRAM-resident `MmCommBufferStatus`, so the volatile read is valid.
            unsafe { core::ptr::read_volatile(config.user.status as *const MmCommBufferStatus) }
        } else {
            MmCommBufferStatus::new()
        };
        let supv_status = if config.supervisor.status != 0 {
            // SAFETY: `config.supervisor.status` is non-zero here and, provided by the MM IPL, references
            // an MMRAM-resident `MmCommBufferStatus`, so the volatile read is valid.
            unsafe { core::ptr::read_volatile(config.supervisor.status as *const MmCommBufferStatus) }
        } else {
            MmCommBufferStatus::new()
        };

        let target = RequestTarget::select(&user_status, &supv_status);

        log::trace!(
            "Processing request: user_valid={}, supv_valid={}, target={:?}",
            user_status.is_comm_buffer_valid,
            supv_status.is_comm_buffer_valid,
            target
        );

        match target {
            RequestTarget::None => {}
            RequestTarget::User => {
                // A synchronous user MMI or an async dispatch.
                self.process_user_request(config, &user_status, cpu_index);
            }
            RequestTarget::Supervisor => {
                self.process_supervisor_request(config, cpu_index);
            }
        }

        target
    }

    /// Handles a request for the user core.
    ///
    /// 1. Builds a fresh `EfiMmEntryContext` with the current CPU index and CPU count
    /// 2. Copies the `EfiMmEntryContext` into the supervisor-to-user data buffer
    /// 3. Appends the `MmCommBufferStatus` immediately after the context
    /// 4. For synchronous MMIs, copies the user comm buffer to the internal copy
    /// 5. Demotes to the user entry point via `invoke_demoted_routine`
    /// 6. On return, copies back the user comm buffer and reads the updated status
    /// 7. Writes the status to the user mailbox, rejecting a response larger than the buffer
    fn process_user_request(&self, config: &CommBufferConfig, status: &MmCommBufferStatus, cpu_index: usize) {
        log::trace!("Processing User request on CPU {cpu_index} (synchronous: {})", status.is_comm_buffer_valid != 0);

        if config.user.external == 0 || config.user.internal == 0 {
            log::error!("User communication buffer not configured");
            return;
        }

        if config.supv_to_user_buffer == 0 {
            log::error!("Supervisor-to-user data buffer not configured");
            return;
        }

        let user_entry = match init_state().user_entry_point() {
            Some(entry) if entry != 0 => entry,
            _ => {
                log::error!("User entry point not configured, cannot demote");
                return;
            }
        };

        let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
            Ok(stack) => stack,
            Err(err) => {
                log::error!("Failed to get CPL3 stack for CPU {cpu_index}: {err}");
                return;
            }
        };

        // Only the CPU fields are filled in. The C structure's pointers (mm_startup_this_ap,
        // cpu_save_state, cpu_save_state_size) have no meaning here: the user core reaches those
        // services through syscalls.
        let entry_context = EfiMmEntryContext {
            mm_startup_this_ap: 0,
            currently_executing_cpu: cpu_index as u64,
            number_of_cpus: self.cpu_manager.registered_count() as u64,
            cpu_save_state_size: 0,
            cpu_save_state: 0,
        };

        // The user core reads the context and status from the supervisor-to-user buffer after
        // demotion.
        let context_size = core::mem::size_of::<EfiMmEntryContext>();
        let status_size = core::mem::size_of::<MmCommBufferStatus>();

        if (config.supv_to_user_buffer_size as usize) < context_size + status_size {
            log::error!(
                "Supervisor-to-user buffer too small: {} < {} (context) + {} (status)",
                config.supv_to_user_buffer_size,
                context_size,
                status_size
            );
            return;
        }

        // SAFETY: `supv_to_user_buffer` is the user-owned buffer published by MM IPL and was
        // verified above to hold `context_size + status_size` bytes, so both copies stay inside
        // it and every access made while SMAP is lifted targets that user range.
        unsafe {
            with_user_access(|| {
                core::ptr::copy_nonoverlapping(
                    (&raw const entry_context).cast::<u8>(),
                    config.supv_to_user_buffer as *mut u8,
                    context_size,
                );

                // The status follows the context.
                core::ptr::copy_nonoverlapping(
                    core::ptr::from_ref::<MmCommBufferStatus>(status).cast::<u8>(),
                    (config.supv_to_user_buffer as *mut u8).add(context_size),
                    status_size,
                );
            });
        }

        // A valid user comm buffer marks a synchronous request. Otherwise the MMI is asynchronous.
        let sync_mmi = status.is_comm_buffer_valid;

        if sync_mmi != 0 {
            // Ring 3 works on the internal copy of the user buffer.
            // SAFETY: both buffers are the user-owned communication buffers published by MM IPL,
            // and both are `config.user.size` bytes, so the copy made while SMAP is lifted
            // stays inside those user ranges.
            unsafe {
                with_user_access(|| {
                    core::ptr::copy_nonoverlapping(
                        config.user.external as *const u8,
                        config.user.internal as *mut u8,
                        config.user.size as usize,
                    );
                });
            }
            log::trace!(
                "Copied {} bytes from user buffer 0x{:x} to internal 0x{:x}",
                config.user.size,
                config.user.external,
                config.user.internal
            );
        }

        // Arguments to the user entry point: the command type, the supervisor-to-user buffer
        // (context followed by status), and the size of the context.
        // SAFETY: `user_entry` was validated to be non-zero above and points to the user
        // module entry published by MM IPL. `cpl3_stack` is the per-CPU Ring 3 stack returned
        // by `get_cpl3_stack`. The arg count (3) matches the three argument values passed.
        let ret = unsafe {
            invoke_demoted_routine(
                cpu_index,
                user_entry,
                cpl3_stack,
                DEMOTED_ROUTINE_ARG_COUNT,
                UserCommandType::UserRequest as u64,
                config.supv_to_user_buffer,
                context_size as u64,
            )
        };
        log::trace!("Returned from user request on CPU {cpu_index} with value: 0x{ret:016x}");

        // Copy the response back out to the caller's buffer.
        if sync_mmi != 0 {
            // SAFETY: as for the copy in, both buffers are the user-owned communication buffers
            // published by MM IPL and both are `config.user.size` bytes.
            unsafe {
                with_user_access(|| {
                    core::ptr::copy_nonoverlapping(
                        config.user.internal as *const u8,
                        config.user.external as *mut u8,
                        config.user.size as usize,
                    );
                });
            }
        }

        // The user core may have updated return_status and return_buffer_size.
        // SAFETY: `supv_to_user_buffer` is the user-owned buffer verified above to hold
        // `context_size + status_size` bytes, so the status read while SMAP is lifted stays
        // inside that user range.
        let returned_status = unsafe {
            with_user_access(|| {
                core::ptr::read(
                    (config.supv_to_user_buffer as *const u8).add(context_size).cast::<MmCommBufferStatus>(),
                )
            })
        };

        // Clearing is_comm_buffer_valid marks the request as complete.
        let mut final_status = returned_status;
        final_status.is_comm_buffer_valid = 0;

        // Ring 3 filled in `return_buffer_size`, and the non-MM caller uses it to read the
        // response out of the communication buffer. If it runs past the end of that buffer, the
        // supervisor cannot tell which bytes the user core meant, and truncating would return
        // part of a response it never agreed to send. Report the failure and return nothing.
        if final_status.return_buffer_size > config.user.size {
            log::error!(
                "User module reported a 0x{:x}-byte response for a 0x{:x}-byte communication buffer; rejecting",
                final_status.return_buffer_size,
                config.user.size
            );
            final_status.return_status = efi::Status::BAD_BUFFER_SIZE.as_usize() as u64;
            final_status.return_buffer_size = 0;
        }

        // SAFETY: `config.user.status` is the user status mailbox from the comm buffer HOB, which
        // is valid and writable.
        unsafe {
            let status_ptr = config.user.status as *mut MmCommBufferStatus;
            core::ptr::write_volatile(status_ptr, final_status);
        }
    }

    /// Handles a request for the supervisor.
    ///
    /// Matches the GUID in the [`EfiMmCommunicateHeader`] against the core's built-in handlers
    /// and then the platform handlers from [`PlatformInfo::mmi_handlers`], and invokes the first
    /// match. Steps:
    ///
    /// 0. Reject the request with `ACCESS_DENIED` if `ExitBootServices` has already been signaled
    /// 1. Zero the internal buffer and copy the external supervisor buffer into it
    /// 2. Parse the `EfiMmCommunicateHeader` (GUID and message length) from the internal buffer
    /// 3. Validate message length does not exceed the buffer size
    /// 4. Iterate the default handlers then [`PlatformInfo::mmi_handlers`] to find a handler
    ///    matching the header GUID
    /// 5. Call the handler with a pointer to the data payload and mutable size
    /// 6. Refuse the request if the handler reported more than the payload space it was given
    /// 7. Copy the internal buffer back to the external buffer
    /// 8. Update the status buffer with return status and total response size
    fn process_supervisor_request(&self, config: &CommBufferConfig, cpu_index: usize) {
        log::trace!("Processing Supervisor request on CPU {cpu_index}...");

        if init_state().is_at_runtime() {
            log::error!("Supervisor buffer cannot be used for communication after ExitBootServices is signaled!!");
            self.write_supv_status(config, efi::Status::ACCESS_DENIED, 0);
            return;
        }

        if config.supervisor.external == 0 || config.supervisor.internal == 0 {
            log::error!("Supervisor communication buffer not configured");
            return;
        }

        let buffer_size = config.supervisor.size as usize;

        // SAFETY: both supervisor buffers come from the comm region HOB set up during init. Each
        // is `buffer_size` bytes and they do not overlap.
        unsafe {
            core::ptr::write_bytes(config.supervisor.internal as *mut u8, 0, buffer_size);
            core::ptr::copy_nonoverlapping(
                config.supervisor.external as *const u8,
                config.supervisor.internal as *mut u8,
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
            self.write_supv_status(config, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        // SAFETY: the buffer holds at least a header, checked above. The header is packed, so it
        // is read unaligned.
        let header = unsafe { core::ptr::read_unaligned(config.supervisor.internal as *const EfiMmCommunicateHeader) };

        let message_length = header.message_length();

        // Validate message length doesn't exceed the buffer
        if message_length > buffer_size.saturating_sub(EfiMmCommunicateHeader::size()) {
            log::error!(
                "Message length 0x{:x} exceeds available buffer space 0x{:x}",
                message_length,
                buffer_size - EfiMmCommunicateHeader::size()
            );
            self.write_supv_status(config, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        // The payload follows the header.
        // SAFETY: `config.supervisor.internal` is a valid buffer of `buffer_size` bytes, and
        // `buffer_size >= EfiMmCommunicateHeader::size()` was checked above, so offsetting by
        // the header size stays within the same allocation.
        let data_ptr = unsafe { (config.supervisor.internal as *mut u8).add(EfiMmCommunicateHeader::size()) };
        let mut data_size = message_length;

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
            self.write_supv_status(config, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        let total_response_size = data_size + EfiMmCommunicateHeader::size();

        // SAFETY: both buffers are `buffer_size` bytes and an oversized `data_size` returned
        // above, so `total_response_size` is at most `buffer_size` and the copy stays inside
        // both allocations.
        unsafe {
            core::ptr::copy_nonoverlapping(
                config.supervisor.internal as *const u8,
                config.supervisor.external as *mut u8,
                total_response_size,
            );
        }
        log::trace!(
            "Copied {} bytes from internal buffer 0x{:x} back to external 0x{:x}",
            total_response_size,
            config.supervisor.internal,
            config.supervisor.external
        );

        // Any handler failure is reported to the caller as NOT_FOUND.
        let return_status =
            if dispatch_status == efi::Status::SUCCESS { efi::Status::SUCCESS } else { efi::Status::NOT_FOUND };
        self.write_supv_status(config, return_status, total_response_size as u64);
    }

    /// Writes the supervisor status mailbox: clears `is_comm_buffer_valid` and records the
    /// return status and response size.
    fn write_supv_status(&self, config: &CommBufferConfig, return_status: efi::Status, return_buffer_size: u64) {
        // SAFETY: `config.supervisor.status` is the supervisor status mailbox from the comm region
        // HOB, which is valid and writable.
        unsafe {
            let status_ptr = config.supervisor.status as *mut MmCommBufferStatus;
            let updated = MmCommBufferStatus {
                is_comm_buffer_valid: 0,
                _padding: [0; 7],
                return_status: return_status.as_usize() as u64,
                return_buffer_size,
            };
            core::ptr::write_volatile(status_ptr, updated);
        }
    }

    /// Runs the procedures the BSP posts to this AP's mailbox until the BSP releases this AP
    /// through its rendezvous semaphore.
    fn ap_holding_pen(&'static self, cpu_id: u32) {
        // Each CPU owns the mailbox at its dense slot index.
        let cpu_index = if let Some(idx) = self.cpu_manager.find_cpu_index(cpu_id) {
            idx
        } else {
            log::error!("AP (CPU {cpu_id}) has no registered slot; skipping holding pen");
            return;
        };
        log::trace!("AP (CPU {cpu_id}) in holding pen, polling mailbox...");

        loop {
            if let Some(command) = self.mailbox_manager.check_mailbox(cpu_index) {
                log::trace!("AP (CPU {cpu_id}) received command: {command:?}");

                let response = self.execute_ap_command(cpu_id, &command);
                self.mailbox_manager.post_response(cpu_index, response);
            }

            if self.cpu_manager.take_release_by_index(cpu_index) {
                break;
            }
            core::hint::spin_loop();
        }

        log::trace!("AP (CPU {cpu_id}) exiting holding pen");
    }

    /// Runs a command received by an AP, marking the AP busy while it runs.
    fn execute_ap_command(&self, cpu_id: u32, command: &ApCommand) -> ApResponse {
        let ApCommand { procedure, argument } = *command;
        self.cpu_manager.set_ap_state(cpu_id, ApState::Busy);
        let response = self.run_procedure_on_ap(cpu_id, procedure, argument);
        self.cpu_manager.set_ap_state(cpu_id, ApState::InHoldingPen);
        response
    }

    /// Runs a procedure on an AP, demoting to Ring 3 if the procedure is user-owned.
    ///
    /// This is the AP side of [`ApCommand`] and mirrors the C `ProcedureWrapper`: it
    /// checks who owns the procedure's page and either calls it directly (supervisor-owned) or
    /// demotes to Ring 3 (user-owned).
    ///
    /// Choosing the ring from the address is sound only because `handle_start_ap_proc` refuses a
    /// procedure that is not user-owned, so nothing Ring 3 named can reach the supervisor branch.
    fn run_procedure_on_ap(&self, cpu_id: u32, procedure: u64, argument: u64) -> ApResponse {
        log::trace!("AP (CPU {cpu_id}) running procedure 0x{procedure:x} with arg 0x{argument:x}");

        if procedure == 0 {
            log::error!("AP (CPU {cpu_id}) received null procedure pointer");
            return ApResponse::Error(efi::Status::INVALID_PARAMETER.as_usize() as u32);
        }

        let procedure_range = MemoryRange::new(procedure, core::mem::size_of::<usize>() as u64).ok();
        let is_user_range = match procedure_range.and_then(|range| query_address_ownership(&range)) {
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
            let cpu_index = if let Some(idx) = self.cpu_manager.find_cpu_index(cpu_id) {
                idx
            } else {
                log::error!("AP (CPU {cpu_id}) has no registered slot, cannot demote");
                return ApResponse::Error(efi::Status::DEVICE_ERROR.as_usize() as u32);
            };

            let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
                Ok(stack) => stack,
                Err(err) => {
                    log::error!("AP (CPU {cpu_id}) failed to get CPL3 stack: {err}");
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

            // The user core calls the procedure as void (EFIAPI *)(void *ProcedureArgument).
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
                    DEMOTED_ROUTINE_ARG_COUNT,
                    UserCommandType::UserApProcedure as u64,
                    procedure,
                    argument,
                )
            };

            log::trace!("AP (CPU {cpu_id}) returned from demoted procedure: 0x{ret:x}");
            ApResponse::Success
        } else {
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
    /// It is instantiated for the concrete `P: PlatformInfo` type and stored as a
    /// `fn(u64, u64, u64) -> u64` through
    /// [`InitState::set_ap_startup_fn`](crate::state::InitState::set_ap_startup_fn), which is how
    /// the dispatcher reaches the supervisor without naming its generic parameters.
    pub(crate) fn start_ap_procedure_trampoline(cpu_index: u64, procedure: u64, argument: u64) -> u64 {
        let core = Self::instance();
        core.start_ap_procedure(cpu_index, procedure, argument)
    }

    /// Validates a procedure and runs it on one AP, waiting for it to finish.
    ///
    /// Follows the C `InternalSmmStartupThisAp`:
    /// 1. CPU index is within range of registered CPUs
    /// 2. CPU at that index is present (registered)
    /// 3. CPU is not the BSP
    /// 4. Procedure pointer is non-null
    /// 5. Sends the command via the mailbox (fails if AP is busy)
    /// 6. Waits for the AP to complete (blocking)
    ///
    /// Returns the EFI status as a `u64`.
    pub(crate) fn start_ap_procedure(&self, cpu_index: u64, procedure: u64, argument: u64) -> u64 {
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

        // 5. Send the procedure to the AP via mailbox
        //    This will fail if the AP's mailbox is not empty (AP is busy).
        let command = ApCommand { procedure, argument };
        if self.mailbox_manager.send_command(cpu_index, command).is_err() {
            log::error!("START_AP: AP (CPU {cpu_id}, index {cpu_index}) is busy or mailbox unavailable");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::trace!("START_AP: Dispatched proc=0x{procedure:x} arg=0x{argument:x} to CPU {cpu_id} (index {cpu_index})");

        // 6. Wait for the AP to finish, up to AP_TIMEOUT_US.
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
