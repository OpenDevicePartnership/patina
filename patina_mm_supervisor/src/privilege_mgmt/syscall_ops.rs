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
    use serial_test::serial;

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

    // `start_ap_procedure` and `save_state_read_phase2` reach process-global state that other
    // tests in this binary also touch, so the tests covering them are serialized below.

    /// Byte offsets of the pieces of the synthetic policy blob built by [`install_test_policy`].
    const ROOTS_OFFSET: u32 = 40;
    const MSR_DESC_OFFSET: u32 = ROOTS_OFFSET + 3 * 24;
    const IO_DESC_OFFSET: u32 = MSR_DESC_OFFSET + 8;
    const INSTRUCTION_DESC_OFFSET: u32 = IO_DESC_OFFSET + 8;
    const POLICY_SIZE: u32 = INSTRUCTION_DESC_OFFSET + 8;

    /// MSR, I/O port and instruction the synthetic policy permits. Everything else is denied,
    /// because each policy root uses allow-list semantics.
    const ALLOWED_MSR: u32 = 0x1B;
    const ALLOWED_PORT: u16 = 0xB2;

    /// Appends a `PolicyRootV1` describing `count` descriptors of `policy_type` at `offset`.
    fn push_policy_root(buf: &mut Vec<u8>, policy_type: u32, offset: u32, count: u32) {
        buf.extend_from_slice(&1u32.to_le_bytes()); // version
        buf.extend_from_slice(&24u32.to_le_bytes()); // policy_root_size
        buf.extend_from_slice(&policy_type.to_le_bytes());
        buf.extend_from_slice(&offset.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.push(crate::mm_policy::ACCESS_ATTR_ALLOW);
        buf.extend_from_slice(&[0u8; 3]); // reserved
    }

    /// Builds a minimal but valid V1.0 firmware policy and installs it as the global policy gate.
    ///
    /// The blob is laid out by hand (the policy structures are `repr(C)` and padding free) so the
    /// test exercises the same parsing the supervisor performs on a real firmware policy.
    fn install_test_policy() {
        let mut buf: Vec<u8> = Vec::new();

        // Header: `SecurePolicyDataV1_0`.
        buf.extend_from_slice(&0u16.to_le_bytes()); // version_minor
        buf.extend_from_slice(&1u16.to_le_bytes()); // version_major
        buf.extend_from_slice(&POLICY_SIZE.to_le_bytes()); // size
        buf.extend_from_slice(&0u32.to_le_bytes()); // memory_policy_offset
        buf.extend_from_slice(&0u32.to_le_bytes()); // memory_policy_count
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&0u32.to_le_bytes()); // capabilities
        buf.extend_from_slice(&0u64.to_le_bytes()); // reserved
        buf.extend_from_slice(&ROOTS_OFFSET.to_le_bytes()); // policy_root_offset
        buf.extend_from_slice(&3u32.to_le_bytes()); // policy_root_count

        push_policy_root(&mut buf, crate::mm_policy::TYPE_MSR, MSR_DESC_OFFSET, 1);
        push_policy_root(&mut buf, crate::mm_policy::TYPE_IO, IO_DESC_OFFSET, 1);
        push_policy_root(&mut buf, crate::mm_policy::TYPE_INSTRUCTION, INSTRUCTION_DESC_OFFSET, 1);

        // `MsrDescriptorV1_0`: read and write of a single MSR.
        buf.extend_from_slice(&ALLOWED_MSR.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // length
        buf.extend_from_slice(
            &((crate::mm_policy::RESOURCE_ATTR_READ | crate::mm_policy::RESOURCE_ATTR_WRITE) as u16).to_le_bytes(),
        );

        // `IoDescriptorV1_0`: read and write of a single byte-wide port.
        buf.extend_from_slice(&ALLOWED_PORT.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // length_or_width
        buf.extend_from_slice(
            &((crate::mm_policy::RESOURCE_ATTR_READ | crate::mm_policy::RESOURCE_ATTR_WRITE) as u16).to_le_bytes(),
        );
        buf.extend_from_slice(&0u16.to_le_bytes()); // reserved

        // `InstructionDescriptorV1_0`: execution of `CLI` only.
        buf.extend_from_slice(&Instruction::Cli.as_index().to_le_bytes());
        buf.extend_from_slice(&(crate::mm_policy::RESOURCE_ATTR_EXECUTE as u16).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved

        assert_eq!(buf.len(), POLICY_SIZE as usize, "policy blob layout must match its offsets");

        // The gate borrows the blob for the lifetime of the process, matching the firmware case
        // where the policy lives in a reserved region.
        let policy: &'static [u8] = Vec::leak(buf);

        // SAFETY: `policy` points at a valid V1.0 policy blob (built above) that lives for the
        // rest of the process, which is what `PolicyGate::new` requires.
        let gate = unsafe { crate::mm_policy::PolicyGate::new(policy.as_ptr()) }.expect("policy blob is valid");
        security_state().set_policy_gate(gate);
    }

    /// Stand-in AP startup function used when this test wins the one-time registration race.
    fn test_ap_startup(_cpu_index: u64, _procedure: u64, _argument: u64) -> u64 {
        Status::UNSUPPORTED.as_usize() as u64
    }

    #[test]
    #[serial]
    fn test_firmware_ops_policy_queries_follow_the_global_gate() {
        let ops = FirmwareOps;

        // Until the gate is installed every query fails closed, so Ring 3 cannot slip a request
        // through during bring-up.
        assert_eq!(ops.check_msr(ALLOWED_MSR, AccessType::Read), PolicyDecision::Unavailable);
        assert_eq!(ops.check_io(ALLOWED_PORT, IoWidth::Byte, AccessType::Read), PolicyDecision::Unavailable);
        assert_eq!(ops.check_instruction(Instruction::Cli), PolicyDecision::Unavailable);

        install_test_policy();

        // Permitted by the policy...
        assert_eq!(ops.check_msr(ALLOWED_MSR, AccessType::Read), PolicyDecision::Allowed);
        assert_eq!(ops.check_msr(ALLOWED_MSR, AccessType::Write), PolicyDecision::Allowed);
        assert_eq!(ops.check_io(ALLOWED_PORT, IoWidth::Byte, AccessType::Read), PolicyDecision::Allowed);
        assert_eq!(ops.check_instruction(Instruction::Cli), PolicyDecision::Allowed);

        // ...and everything outside the allow list is denied.
        assert_eq!(ops.check_msr(0x200, AccessType::Read), PolicyDecision::Denied(PolicyError::AccessDenied));
        assert_eq!(
            ops.check_io(0x70, IoWidth::Byte, AccessType::Read),
            PolicyDecision::Denied(PolicyError::AccessDenied)
        );
        assert_eq!(ops.check_instruction(Instruction::Hlt), PolicyDecision::Denied(PolicyError::AccessDenied));

        // A wider access than the descriptor covers is denied even on an allowed port.
        assert_eq!(
            ops.check_io(ALLOWED_PORT, IoWidth::Dword, AccessType::Read),
            PolicyDecision::Denied(PolicyError::AccessDenied)
        );
    }

    #[test]
    #[serial]
    fn test_firmware_ops_delegates_save_state_phase2_and_ap_startup() {
        let ops = FirmwareOps;

        // Phase 2 without a completed phase 1 must be rejected rather than reading save state.
        assert_eq!(ops.save_state_read_phase2(0x1000, 8, 0x2000), Err(Status::INVALID_PARAMETER));

        // Make sure an AP startup function is registered; this wins only if no other test got
        // there first, and either way the delegation must report a status rather than `None`.
        init_state().set_ap_startup_fn(test_ap_startup);
        assert!(init_state().ap_startup_fn().is_some());

        // `u64::MAX` is never a registered CPU index, so the registered function rejects it
        // without dispatching any work to a processor.
        let status = ops.start_ap_procedure(u64::MAX, 0x1000, 0);
        assert!(status.is_some(), "a registered startup function must yield a status");
        assert_ne!(status, Some(0), "an invalid CPU index must not report success");
    }
}
