//! Syscall Side Effects
//!
//! Every syscall handler in [`super::syscall_dispatcher`] follows the same shape: validate the
//! request coming from Ring 3, ask the firmware policy whether it is permitted, and only then
//! perform a privileged action — executing an instruction, touching an I/O port or MSR, or
//! consulting supervisor-global state.
//!
//! This module isolates that second half behind the [`SyscallOps`] trait so that:
//!
//! - all privileged instructions and all supervisor-global state access live in a single
//!   implementation ([`FirmwareOps`]), keeping `unsafe` out of the request validation logic, and
//! - the dispatcher can be exercised on a host (where `rdmsr`, `cli`, `in`/`out` and friends
//!   would fault) against a test implementation of the trait.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::arch::asm;

use crate::{
    CommBufferConfig, PageOwnership,
    mem::{AllocationType, page_allocator::PageAllocError},
    mm_policy::{AccessType, Instruction, IoWidth, PolicyError},
    state::{init_state, security_state},
};

use super::SyscallResult;

/// The outcome of a firmware policy query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// The firmware policy permits the operation.
    Allowed,
    /// The firmware policy denies the operation.
    Denied(PolicyError),
    /// The policy gate has not been initialized, so nothing can be permitted yet.
    Unavailable,
}

impl From<Result<(), PolicyError>> for PolicyDecision {
    fn from(result: Result<(), PolicyError>) -> Self {
        match result {
            Ok(()) => PolicyDecision::Allowed,
            Err(err) => PolicyDecision::Denied(err),
        }
    }
}

/// The side effects a syscall handler may perform on behalf of Ring 3.
///
/// The handlers themselves only validate and sequence requests; everything that touches
/// hardware or supervisor-global state goes through this trait. [`FirmwareOps`] is the
/// implementation used by the running supervisor.
pub trait SyscallOps {
    /// Asks the firmware policy whether `msr` may be accessed with `access`.
    fn check_msr(&self, msr: u32, access: AccessType) -> PolicyDecision;

    /// Asks the firmware policy whether I/O `port` may be accessed with `width` and `access`.
    fn check_io(&self, port: u16, width: IoWidth, access: AccessType) -> PolicyDecision;

    /// Asks the firmware policy whether `instruction` may be executed.
    fn check_instruction(&self, instruction: Instruction) -> PolicyDecision;

    /// Reads the model-specific register `msr`.
    ///
    /// ## Safety
    ///
    /// The caller must have cleared this MSR read with [`SyscallOps::check_msr`]; reading an
    /// arbitrary MSR can fault or expose supervisor-private state to Ring 3.
    unsafe fn read_msr(&self, msr: u32) -> u64;

    /// Writes `value` to the model-specific register `msr`.
    ///
    /// ## Safety
    ///
    /// The caller must have cleared this MSR write with [`SyscallOps::check_msr`]; writing an
    /// arbitrary MSR can fault or reconfigure the platform underneath the supervisor.
    unsafe fn write_msr(&self, msr: u32, value: u64);

    /// Reads `width` bytes from I/O `port`.
    ///
    /// ## Safety
    ///
    /// The caller must have cleared this access with [`SyscallOps::check_io`]; I/O reads can have
    /// side effects on the addressed device.
    unsafe fn io_read(&self, port: u16, width: IoWidth) -> u64;

    /// Writes the low `width` bytes of `value` to I/O `port`.
    ///
    /// ## Safety
    ///
    /// The caller must have cleared this access with [`SyscallOps::check_io`]; I/O writes can have
    /// side effects on the addressed device.
    unsafe fn io_write(&self, port: u16, width: IoWidth, value: u64);

    /// Executes a privileged `instruction` on behalf of Ring 3.
    ///
    /// ## Safety
    ///
    /// The caller must have cleared the instruction with [`SyscallOps::check_instruction`]; these
    /// instructions change processor state visible to the whole platform.
    unsafe fn execute_instruction(&self, instruction: Instruction);

    /// Returns whether the current processor is the bootstrap processor.
    fn is_bsp(&self) -> bool;

    /// Allocates `page_count` pages of user-owned (Ring 3) memory.
    fn allocate_user_pages(&self, page_count: usize) -> Result<u64, PageAllocError>;

    /// Frees `page_count` user-owned pages starting at `addr`, rejecting non-user allocations.
    fn free_user_pages(&self, addr: u64, page_count: usize) -> Result<(), PageAllocError>;

    /// Returns how the page at `addr` was allocated, or `None` if it is not allocated.
    fn allocation_type(&self, addr: u64) -> Option<AllocationType>;

    /// Returns the page table ownership of `size` bytes at `addr`, or `None` if unmapped.
    fn query_address_ownership(&self, addr: u64, size: u64) -> Option<PageOwnership>;

    /// Dispatches `procedure` to the AP at `cpu_index`, returning its status.
    ///
    /// Returns `None` when no AP startup function has been registered.
    fn start_ap_procedure(&self, cpu_index: u64, procedure: u64, argument: u64) -> Option<u64>;

    /// Runs phase 1 of the two-phase save-state read.
    fn save_state_read_phase1(&self, protocol: u64, register: u64, cpu_index: u64) -> SyscallResult;

    /// Runs phase 2 of the two-phase save-state read.
    fn save_state_read_phase2(&self, protocol: u64, width: u64, buffer: u64) -> SyscallResult;

    /// Returns whether `size` bytes at `addr` fall inside a region unblocked for MM access.
    fn is_within_unblocked_region(&self, addr: u64, size: u64) -> bool;

    /// Returns the communication buffer configuration, if it has been published.
    fn comm_buffer_config(&self) -> Option<CommBufferConfig>;
}

/// The [`SyscallOps`] implementation used by the running supervisor.
///
/// This is the only place where syscall handling executes privileged instructions or reaches
/// into the supervisor's global state.
#[derive(Debug, Clone, Copy, Default)]
pub struct FirmwareOps;

impl SyscallOps for FirmwareOps {
    fn check_msr(&self, msr: u32, access: AccessType) -> PolicyDecision {
        match security_state().policy_gate() {
            Some(gate) => gate.is_msr_allowed(msr, access).into(),
            None => PolicyDecision::Unavailable,
        }
    }

    fn check_io(&self, port: u16, width: IoWidth, access: AccessType) -> PolicyDecision {
        match security_state().policy_gate() {
            Some(gate) => gate.is_io_allowed(u32::from(port), width, access).into(),
            None => PolicyDecision::Unavailable,
        }
    }

    fn check_instruction(&self, instruction: Instruction) -> PolicyDecision {
        match security_state().policy_gate() {
            Some(gate) => gate.is_instruction_allowed(instruction).into(),
            None => PolicyDecision::Unavailable,
        }
    }

    // Executes the privileged `rdmsr` instruction, which faults outside ring 0 and cannot run in
    // a host-based unit test.
    unsafe fn read_msr(&self, msr: u32) -> u64 {
        // SAFETY: the caller validated this MSR against the firmware policy, as required by the
        // contract of `SyscallOps::read_msr`.
        unsafe { crate::intrinsics::read_msr(msr) }
    }

    // Executes the privileged `wrmsr` instruction, which faults outside ring 0 and cannot run in
    // a host-based unit test.
    unsafe fn write_msr(&self, msr: u32, value: u64) {
        // SAFETY: the caller validated this MSR against the firmware policy, as required by the
        // contract of `SyscallOps::write_msr`.
        unsafe { crate::intrinsics::write_msr(msr, value) };
    }

    // Executes `in`, which faults outside ring 0 and cannot run in a host-based unit test.
    unsafe fn io_read(&self, port: u16, width: IoWidth) -> u64 {
        let value: u64;
        // SAFETY: the caller validated this port and width against the firmware policy, as
        // required by the contract of `SyscallOps::io_read`. Each `in` reads only the requested
        // port and touches no memory (nomem, nostack).
        unsafe {
            match width {
                IoWidth::Byte => {
                    let data: u8;
                    asm!("in al, dx", out("al") data, in("dx") port, options(nomem, nostack));
                    value = u64::from(data);
                }
                IoWidth::Word => {
                    let data: u16;
                    asm!("in ax, dx", out("ax") data, in("dx") port, options(nomem, nostack));
                    value = u64::from(data);
                }
                IoWidth::Dword => {
                    let data: u32;
                    asm!("in eax, dx", out("eax") data, in("dx") port, options(nomem, nostack));
                    value = u64::from(data);
                }
            }
        }
        value
    }

    // Executes `out`, which faults outside ring 0 and cannot run in a host-based unit test.
    unsafe fn io_write(&self, port: u16, width: IoWidth, value: u64) {
        // SAFETY: the caller validated this port and width against the firmware policy, as
        // required by the contract of `SyscallOps::io_write`. Each `out` writes only the
        // requested port and touches no memory (nomem, nostack).
        unsafe {
            match width {
                IoWidth::Byte => asm!("out dx, al", in("dx") port, in("al") value as u8, options(nomem, nostack)),
                IoWidth::Word => asm!("out dx, ax", in("dx") port, in("ax") value as u16, options(nomem, nostack)),
                IoWidth::Dword => asm!("out dx, eax", in("dx") port, in("eax") value as u32, options(nomem, nostack)),
            }
        }
    }

    // Executes privileged instructions that fault outside ring 0 and cannot run in a host-based
    // unit test.
    unsafe fn execute_instruction(&self, instruction: Instruction) {
        // SAFETY: the caller validated the instruction against the firmware policy, as required by
        // the contract of `SyscallOps::execute_instruction`. Each instruction only updates
        // processor state (interrupt flag, caches, halt) and touches no memory (nomem, nostack).
        unsafe {
            match instruction {
                Instruction::Cli => asm!("cli", options(nomem, nostack)),
                Instruction::Wbinvd => asm!("wbinvd", options(nomem, nostack)),
                Instruction::Hlt => asm!("hlt", options(nomem, nostack)),
            }
        }
    }

    // Reads the APIC base MSR, which faults outside ring 0 and cannot run in a host-based unit
    // test.
    fn is_bsp(&self) -> bool {
        crate::is_bsp()
    }

    fn allocate_user_pages(&self, page_count: usize) -> Result<u64, PageAllocError> {
        security_state().page_allocator().allocate_pages_with_type(page_count, AllocationType::User)
    }

    fn free_user_pages(&self, addr: u64, page_count: usize) -> Result<(), PageAllocError> {
        security_state().page_allocator().free_pages_checked(addr, page_count, AllocationType::User)
    }

    fn allocation_type(&self, addr: u64) -> Option<AllocationType> {
        security_state().page_allocator().get_allocation_type(addr)
    }

    fn query_address_ownership(&self, addr: u64, size: u64) -> Option<PageOwnership> {
        crate::query_address_ownership(addr, size)
    }

    fn start_ap_procedure(&self, cpu_index: u64, procedure: u64, argument: u64) -> Option<u64> {
        let start_fn = init_state().ap_startup_fn()?;
        log::info!(
            "START_AP_PROC: Dispatching to AP startup function at {:p} for CPU {}",
            start_fn as *const (),
            cpu_index
        );
        Some(start_fn(cpu_index, procedure, argument))
    }

    fn save_state_read_phase1(&self, protocol: u64, register: u64, cpu_index: u64) -> SyscallResult {
        crate::save_state::save_state_read_phase1(protocol, register, cpu_index)
    }

    fn save_state_read_phase2(&self, protocol: u64, width: u64, buffer: u64) -> SyscallResult {
        crate::save_state::save_state_read_phase2(protocol, width, buffer)
    }

    fn is_within_unblocked_region(&self, addr: u64, size: u64) -> bool {
        security_state().unblocked_tracker().is_within_unblocked_region(addr, size)
    }

    fn comm_buffer_config(&self) -> Option<CommBufferConfig> {
        security_state().comm_buffer_config().copied()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::standard::efi::Status;

    #[test]
    fn test_policy_decision_from_gate_result() {
        assert_eq!(PolicyDecision::from(Ok(())), PolicyDecision::Allowed);
        assert_eq!(
            PolicyDecision::from(Err(PolicyError::AccessDenied)),
            PolicyDecision::Denied(PolicyError::AccessDenied)
        );
        assert_eq!(
            PolicyDecision::from(Err(PolicyError::PolicyRootNotFound)),
            PolicyDecision::Denied(PolicyError::PolicyRootNotFound)
        );
    }

    #[test]
    fn test_firmware_ops_reports_uninitialized_policy_gate() {
        // The global policy gate is never initialized in host tests, so every policy query must
        // fail closed with `Unavailable` rather than silently allowing the access.
        let ops = FirmwareOps;

        assert_eq!(ops.check_msr(0x1B, AccessType::Read), PolicyDecision::Unavailable);
        assert_eq!(ops.check_io(0xB2, IoWidth::Byte, AccessType::Write), PolicyDecision::Unavailable);
        assert_eq!(ops.check_instruction(Instruction::Cli), PolicyDecision::Unavailable);
    }

    #[test]
    fn test_firmware_ops_fails_closed_before_state_is_initialized() {
        // Ring 3 can issue syscalls before the supervisor finishes bringing its state up. None of
        // these may hand out memory, claim ownership of an address, or report a buffer as valid
        // while the backing state is still uninitialized.
        let ops = FirmwareOps;

        // The page allocator refuses to serve or release memory it does not own yet.
        assert_eq!(ops.allocate_user_pages(1), Err(PageAllocError::NotInitialized));
        assert_eq!(ops.free_user_pages(0x1000, 1), Err(PageAllocError::NotInitialized));
        assert_eq!(ops.allocation_type(0x1000), None);

        // With no page table installed, ownership of an address is unknown rather than "user".
        assert_eq!(ops.query_address_ownership(0x1000, 0x1000), None);

        // Nothing has been unblocked and no communication buffer has been published. Note the
        // tracker is deliberately permissive until core initialization completes (see
        // `UnblockedMemoryTracker::is_memory_blocked`), so this reports the bootstrap answer.
        assert!(ops.is_within_unblocked_region(0x1000, 0x1000));
        assert!(ops.comm_buffer_config().is_none());

        // Save-state metadata is published during initialization, so phase 1 is not ready.
        assert_eq!(ops.save_state_read_phase1(0x1000, 38, 0), Err(Status::NOT_READY));
    }

    // `start_ap_procedure` and `save_state_read_phase2` are deliberately left uncovered rather
    // than excluded from coverage: both depend on process-global state that other tests in this
    // binary mutate (`set_instance` registers an AP startup function; the save-state tests own
    // the phase 1/2 hand-off slot), so exercising them here would be order dependent. They are
    // covered through the dispatcher instead, via `SyscallOps` test implementations.
}
