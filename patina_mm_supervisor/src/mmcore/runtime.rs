//! `MmSupervisorCore` Runtime Phase
//!
//! The request dispatch the BSP runs and the holding pen the APs wait in, on every MMI after a
//! core's first. Each core returns once the request is served and every AP has left the pen.
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
    management_mode::{
        MmCommBufferStatus,
        supervisor::{MmUserRequest, UserCommandType},
    },
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

impl<P: PlatformInfo> MmSupervisorCore<P> {
    /// Serves one MMI on a core that has finished initialization.
    ///
    /// `cpu_id` is the APIC ID and `cpu_index` the dense, 0-based processor index.
    ///
    /// 1. Each AP marks itself `InHoldingPen` and enters the holding pen.
    /// 2. The BSP waits for every AP the MP Information HOB reports.
    /// 3. The BSP serves the pending request.
    /// 4. The BSP releases every AP and waits up to `AP_EXIT_TIMEOUT_US` for each to acknowledge.
    /// 5. Each AP marks itself `NotPresent` and acknowledges the BSP.
    ///
    /// ## Panics
    ///
    /// Panics on the BSP if an AP misses the arrival or exit window.
    pub(crate) fn enter_runtime(&'static self, cpu_id: u32, cpu_index: usize) {
        let is_bsp = is_bsp();

        if is_bsp {
            // An AP checks in only after per-core init, so a full holding pen means every core is
            // in this MMI.
            let expected_aps = self.number_of_cores().saturating_sub(1);

            self.wait_for_ap_arrival(expected_aps);

            log::trace!("BSP (CPU {cpu_id}) entering request serving routine...");
            self.bsp_request_loop(cpu_index);

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
            self.cpu_manager.set_ap_state(cpu_id, ApState::InHoldingPen);
            log::trace!("AP (CPU {cpu_id}) checked in, entering holding pen...");
            self.ap_holding_pen(cpu_id);

            self.cpu_manager.set_ap_state(cpu_id, ApState::NotPresent);
            self.cpu_manager.ack_exit_to_bsp();
        }
    }

    /// Waits for every AP to check in to the holding pen.
    ///
    /// ## Panics
    ///
    /// Panics if any AP misses the arrival window, since serving a request with only some cores
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
            let arrived = self.cpu_manager.count_aps_in_state(ApState::InHoldingPen);
            panic!(
                "MM Supervisor fail-secure: only {arrived}/{expected_aps} APs rendezvoused within the arrival window; \
                 refusing to service the request with a partial set of cores"
            );
        }
    }

    /// Serves the pending request, if any, on the BSP, and returns the target it was sent to.
    ///
    /// [`RequestTarget::select`] picks the target from the two status mailboxes. Returns
    /// [`RequestTarget::None`] when the communication buffers are not set up yet.
    fn bsp_request_loop(&self, cpu_index: usize) -> RequestTarget {
        let config = match security_state().comm_buffer_config() {
            Some(config) => config,
            None => {
                return RequestTarget::None;
            }
        };

        if config.user.status_address == 0 && config.supervisor.status_address == 0 {
            return RequestTarget::None;
        }

        // A mailbox that is not published reads as an idle status.
        let user_status = if config.user.status_address != 0 {
            // SAFETY: the address is non-zero and names an `MmCommBufferStatus` that init checked
            // is mapped supervisor-only. Code outside MM can change it, so it is read once.
            unsafe { core::ptr::read_volatile(config.user.status_address as *const MmCommBufferStatus) }
        } else {
            MmCommBufferStatus::new()
        };
        let supv_status = if config.supervisor.status_address != 0 {
            // SAFETY: as above, for the supervisor mailbox at `config.supervisor.status_address`.
            unsafe { core::ptr::read_volatile(config.supervisor.status_address as *const MmCommBufferStatus) }
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
                self.process_user_request(config, &user_status, cpu_index);
            }
            RequestTarget::Supervisor => {
                self.process_supervisor_request(config, cpu_index);
            }
        }

        target
    }

    /// Handles a request for the user core, synchronous or asynchronous.
    ///
    /// 1. Writes an `MmUserRequest` (entry context, status, internal buffer) into the
    ///    supervisor-to-user buffer.
    /// 2. For a synchronous MMI, copies the user buffer into its internal copy.
    /// 3. Demotes to the user entry point.
    /// 4. For a synchronous MMI, copies the internal copy back to the user buffer.
    /// 5. Writes the returned status to the user mailbox, rejecting a response larger than the
    ///    buffer.
    fn process_user_request(&self, config: &CommBufferConfig, status: &MmCommBufferStatus, cpu_index: usize) {
        // A valid user comm buffer marks a synchronous request.
        let sync_mmi = status.is_comm_buffer_valid != 0;
        log::trace!("Processing User request on CPU {cpu_index} (synchronous: {sync_mmi})");

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

        // Only the CPU fields are filled in. The user core reaches the services behind the
        // pointer fields through syscalls.
        let entry_context = EfiMmEntryContext {
            mm_startup_this_ap: 0,
            currently_executing_cpu: cpu_index as u64,
            number_of_cpus: self.number_of_cores() as u64,
            cpu_save_state_size: 0,
            cpu_save_state: 0,
        };

        let request = MmUserRequest {
            entry_context,
            status: *status,
            comm_buffer_address: config.user.internal_address,
            comm_buffer_size: config.user.size,
        };
        let request_size = core::mem::size_of::<MmUserRequest>() as u64;

        if config.supv_to_user_buffer_size < request_size {
            log::error!(
                "Supervisor-to-user buffer too small: {} < {} (request handoff)",
                config.supv_to_user_buffer_size,
                request_size
            );
            return;
        }

        // SAFETY: the supervisor allocated this user-owned buffer and checked above that it holds
        // the whole request.
        unsafe {
            with_user_access(|| {
                core::ptr::write_unaligned(config.supv_to_user_buffer as *mut MmUserRequest, request);
            });
        }

        if sync_mmi {
            // Ring 3 works on the internal copy of the user buffer.
            // SAFETY: both are the user-owned communication buffers the MM IPL published, each
            // `config.user.size` bytes.
            unsafe {
                with_user_access(|| {
                    crate::memory::copy_volatile(
                        config.user.external_address as *const u8,
                        config.user.internal_address as *mut u8,
                        config.user.size as usize,
                    );
                });
            }
            log::trace!(
                "Copied {} bytes from user buffer 0x{:x} to internal 0x{:x}",
                config.user.size,
                config.user.external_address,
                config.user.internal_address
            );
        }

        // SAFETY: `user_entry` is the non-zero user module entry point, `cpl3_stack` is this
        // CPU's Ring 3 stack, and `DEMOTED_ROUTINE_ARG_COUNT` matches the three arguments.
        let ret = unsafe {
            invoke_demoted_routine(
                cpu_index,
                user_entry,
                cpl3_stack,
                DEMOTED_ROUTINE_ARG_COUNT,
                UserCommandType::UserRequest as u64,
                config.supv_to_user_buffer,
                request_size,
            )
        };
        log::trace!("Returned from user request on CPU {cpu_index} with value: 0x{ret:016x}");

        if sync_mmi {
            // SAFETY: as for the copy in.
            unsafe {
                with_user_access(|| {
                    crate::memory::copy_volatile(
                        config.user.internal_address as *const u8,
                        config.user.external_address as *mut u8,
                        config.user.size as usize,
                    );
                });
            }
        }

        // Only the status is read back, so Ring 3 cannot change the buffer address or capacity.
        // SAFETY: the buffer was checked above to hold an `MmUserRequest`.
        let returned_status = unsafe {
            with_user_access(|| {
                core::ptr::addr_of!((*(config.supv_to_user_buffer as *const MmUserRequest)).status).read_unaligned()
            })
        };

        // Clearing is_comm_buffer_valid marks the request as complete.
        let mut final_status = returned_status;
        final_status.is_comm_buffer_valid = 0;

        // The non-MM caller reads `return_buffer_size` bytes of response. A size past the end of
        // the buffer cannot be trusted or truncated, so the request fails with no response.
        if final_status.return_buffer_size > config.user.size {
            log::error!(
                "User module reported a 0x{:x}-byte response for a 0x{:x}-byte communication buffer; rejecting",
                final_status.return_buffer_size,
                config.user.size
            );
            final_status.return_status = efi::Status::BAD_BUFFER_SIZE.as_usize() as u64;
            final_status.return_buffer_size = 0;
        }

        // SAFETY: `config.user.status_address` is the writable user status mailbox from the
        // communication buffer HOB.
        unsafe {
            let status_ptr = config.user.status_address as *mut MmCommBufferStatus;
            core::ptr::write_volatile(status_ptr, final_status);
        }
    }

    /// Handles a request for the supervisor.
    ///
    /// 1. Rejects the request with `ACCESS_DENIED` once `ExitBootServices` has been signaled.
    /// 2. Copies the external supervisor buffer into the zeroed internal buffer.
    /// 3. Reads the [`EfiMmCommunicateHeader`] and checks the message fits the buffer.
    /// 4. Calls the first built-in or [`PlatformInfo::mmi_handlers`] handler matching the GUID.
    /// 5. Rejects a reported response larger than the payload space.
    /// 6. Copies the response back and writes the status mailbox.
    fn process_supervisor_request(&self, config: &CommBufferConfig, cpu_index: usize) {
        log::trace!("Processing Supervisor request on CPU {cpu_index}...");

        if init_state().is_at_runtime() {
            log::error!("Supervisor buffer cannot be used for communication after ExitBootServices is signaled!!");
            self.write_supv_status(config, efi::Status::ACCESS_DENIED, 0);
            return;
        }

        let buffer_size = config.supervisor.size as usize;

        // SAFETY: both supervisor buffers come from the comm region HOB set up during init. Each
        // is `buffer_size` bytes and they do not overlap.
        unsafe {
            crate::memory::write_zeros_volatile(config.supervisor.internal_address as *mut u8, buffer_size);
            crate::memory::copy_volatile(
                config.supervisor.external_address as *const u8,
                config.supervisor.internal_address as *mut u8,
                buffer_size,
            );
        }

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
        let header =
            unsafe { core::ptr::read_unaligned(config.supervisor.internal_address as *const EfiMmCommunicateHeader) };

        let message_length = header.message_length();

        if message_length > buffer_size.saturating_sub(EfiMmCommunicateHeader::size()) {
            log::error!(
                "Message length 0x{:x} exceeds available buffer space 0x{:x}",
                message_length,
                buffer_size - EfiMmCommunicateHeader::size()
            );
            self.write_supv_status(config, efi::Status::BAD_BUFFER_SIZE, 0);
            return;
        }

        // SAFETY: the internal buffer is `buffer_size` bytes, checked above to hold at least a
        // header, so the payload offset stays inside it.
        let data_ptr = unsafe { (config.supervisor.internal_address as *mut u8).add(EfiMmCommunicateHeader::size()) };
        let mut data_size = message_length;

        let handler_guid = header.header_guid();
        let handler = DEFAULT_SUPERVISOR_MMI_HANDLERS
            .iter()
            .chain(P::mmi_handlers().iter())
            .find(|handler| handler.handler_guid == handler_guid);

        // A handler may itself return `NOT_FOUND`, so the warning keys off the lookup, not the
        // status.
        let dispatch_status = if let Some(handler) = handler {
            log::trace!(
                "Dispatching supervisor request to handler '{}' (GUID: {:?})",
                handler.name,
                handler.handler_guid
            );
            // SAFETY: `data_ptr` points to the payload after the header, and `data_size` is the
            // message length checked above to fit the internal buffer, which the supervisor owns.
            unsafe { (handler.handle)(data_ptr, &mut data_size) }
        } else {
            log::warn!("No handler found for supervisor request GUID: {handler_guid:?}");
            efi::Status::NOT_FOUND
        };

        // A handler reports its response length through `data_size`. One past the payload space
        // describes bytes that were never written, so the request fails with no response.
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

        // SAFETY: an oversized `data_size` returned above, so `total_response_size` is at most
        // `buffer_size`, the size of both buffers.
        unsafe {
            crate::memory::copy_volatile(
                config.supervisor.internal_address as *const u8,
                config.supervisor.external_address as *mut u8,
                total_response_size,
            );
        }
        log::trace!(
            "Copied {} bytes from internal buffer 0x{:x} back to external 0x{:x}",
            total_response_size,
            config.supervisor.internal_address,
            config.supervisor.external_address
        );

        // Any handler failure is reported to the caller as NOT_FOUND.
        let return_status =
            if dispatch_status == efi::Status::SUCCESS { efi::Status::SUCCESS } else { efi::Status::NOT_FOUND };
        self.write_supv_status(config, return_status, total_response_size as u64);
    }

    /// Writes the supervisor status mailbox: clears `is_comm_buffer_valid` and records the
    /// return status and response size.
    fn write_supv_status(&self, config: &CommBufferConfig, return_status: efi::Status, return_buffer_size: u64) {
        // SAFETY: `config.supervisor.status_address` is the writable supervisor status mailbox
        // from the comm region HOB.
        unsafe {
            let status_ptr = config.supervisor.status_address as *mut MmCommBufferStatus;
            let updated = MmCommBufferStatus {
                is_comm_buffer_valid: 0,
                _padding: [0; 7],
                return_status: return_status.as_usize() as u64,
                return_buffer_size,
            };
            core::ptr::write_volatile(status_ptr, updated);
        }
    }

    /// Runs the procedures the BSP posts to this AP's mailbox until the BSP releases the AP.
    fn ap_holding_pen(&'static self, cpu_id: u32) {
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

    /// Runs a procedure on an AP: directly if it is supervisor-owned, demoted to Ring 3 if it is
    /// user-owned.
    ///
    /// Choosing the ring from the address is sound only because `handle_start_ap_proc` refuses a
    /// procedure that is not user-owned, so nothing Ring 3 named can reach the supervisor branch.
    fn run_procedure_on_ap(&self, cpu_id: u32, procedure: u64, argument: u64) -> ApResponse {
        log::trace!("AP (CPU {cpu_id}) running procedure 0x{procedure:x} with arg 0x{argument:x}");

        if procedure == 0 {
            log::error!("AP (CPU {cpu_id}) received null procedure pointer");
            return ApResponse::Error(efi::Status::INVALID_PARAMETER);
        }

        let procedure_range = MemoryRange::new(procedure, core::mem::size_of::<usize>() as u64).ok();
        let is_user_range = match procedure_range.and_then(|range| query_address_ownership(&range)) {
            Some(PageOwnership::User) => true,
            Some(PageOwnership::Supervisor) => false,
            None => {
                log::error!(
                    "AP (CPU {cpu_id}) failed to query ownership for 0x{procedure:x} (unmapped or page table not ready)"
                );
                return ApResponse::Error(efi::Status::DEVICE_ERROR);
            }
        };

        if is_user_range {
            let cpu_index = if let Some(idx) = self.cpu_manager.find_cpu_index(cpu_id) {
                idx
            } else {
                log::error!("AP (CPU {cpu_id}) has no registered slot, cannot demote");
                return ApResponse::Error(efi::Status::DEVICE_ERROR);
            };

            let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
                Ok(stack) => stack,
                Err(err) => {
                    log::error!("AP (CPU {cpu_id}) failed to get CPL3 stack: {err}");
                    return ApResponse::Error(efi::Status::DEVICE_ERROR);
                }
            };

            let user_entry = match init_state().user_entry_point() {
                Some(entry) if entry != 0 => entry,
                _ => {
                    log::error!("User entry point not configured, cannot demote AP (CPU {cpu_id})");
                    return ApResponse::Error(efi::Status::DEVICE_ERROR);
                }
            };

            // The user core calls `procedure(argument)` with the EFIAPI calling convention.
            log::trace!(
                "AP (CPU {cpu_id}) demoting to user: proc=0x{procedure:x}, stack=0x{cpl3_stack:x}, arg=0x{argument:x}"
            );

            // SAFETY: `user_entry` is the non-zero user module entry point, `cpl3_stack` is this
            // CPU's Ring 3 stack, and `DEMOTED_ROUTINE_ARG_COUNT` matches the three arguments.
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
            // SAFETY: `procedure` is non-null and names a supervisor-owned function with the
            // `EfiApProcedure` ABI.
            let proc_fn: EfiApProcedure = unsafe { core::mem::transmute(procedure) };
            // SAFETY: as above.
            unsafe { proc_fn(argument as *mut core::ffi::c_void) };

            ApResponse::Success
        }
    }

    /// Type-erased AP startup entry for the syscall dispatcher.
    ///
    /// Stored as a plain `fn(u64, u64, u64) -> u64` through
    /// [`InitState::set_ap_startup_fn`](crate::state::InitState::set_ap_startup_fn), so the
    /// dispatcher can reach the supervisor without naming `P`.
    pub(crate) fn start_ap_procedure_trampoline(cpu_index: u64, procedure: u64, argument: u64) -> u64 {
        let core = Self::instance();
        core.start_ap_procedure(cpu_index, procedure, argument)
    }

    /// Validates a procedure and runs it on one AP, waiting up to `AP_TIMEOUT_US` for it to
    /// finish. Returns the EFI status as a `u64`.
    ///
    /// Checks:
    /// - `cpu_index` is below the core count.
    /// - A CPU is registered at `cpu_index`, and it is not the BSP.
    /// - `procedure` is non-null.
    /// - The AP's mailbox is empty.
    pub(crate) fn start_ap_procedure(&self, cpu_index: u64, procedure: u64, argument: u64) -> u64 {
        let cpu_index = cpu_index as usize;

        let number_of_cores = self.number_of_cores();
        if cpu_index >= number_of_cores {
            log::error!("START_AP: CpuIndex({cpu_index}) >= number_of_cores({number_of_cores})");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        let cpu_id = if let Some(id) = self.cpu_manager.get_cpu_id_by_index(cpu_index) {
            id
        } else {
            log::error!("START_AP: CpuIndex({cpu_index}) has no registered CPU");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        };

        if self.cpu_manager.is_bsp(cpu_id) {
            log::error!("START_AP: CpuIndex({cpu_index}) is the BSP, cannot start as AP");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        if procedure == 0 {
            log::error!("START_AP: Null procedure pointer");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        let command = ApCommand { procedure, argument };
        if self.mailbox_manager.send_command(cpu_index, command).is_err() {
            log::error!("START_AP: AP (CPU {cpu_id}, index {cpu_index}) is busy or mailbox unavailable");
            return efi::Status::INVALID_PARAMETER.as_usize() as u64;
        }

        log::trace!("START_AP: Dispatched proc=0x{procedure:x} arg=0x{argument:x} to CPU {cpu_id} (index {cpu_index})");

        match self.mailbox_manager.wait_response(cpu_index, AP_TIMEOUT_US) {
            Some(ApResponse::Success) => {
                log::trace!("START_AP: AP (CPU {cpu_id}) completed successfully");
                efi::Status::SUCCESS.as_usize() as u64
            }
            Some(ApResponse::Error(status)) => {
                log::error!("START_AP: AP (CPU {cpu_id}) returned error: {status:?}");
                status.as_usize() as u64
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
