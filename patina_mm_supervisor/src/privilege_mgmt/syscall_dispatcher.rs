//! Syscall dispatcher.
//!
//! Handles syscalls from Ring 3. The `syscall` instruction jumps to the address in
//! `MSR_IA32_LSTAR` (the `syscall_center` assembly stub), which calls this dispatcher.
//!
//! ## Syscall Interface
//!
//! The syscall uses a custom calling convention:
//! - RAX: call index (`SyscallIndex`)
//! - RDX: argument 1
//! - R8:  argument 2
//! - R9:  argument 3
//! - RCX: caller return address (set by the syscall instruction)
//! - R11: RFLAGS (set by the syscall instruction)
//!
//! ## Security
//!
//! Every handler must validate its arguments and check that any memory pointer lies in a
//! user-accessible region.
//!
//! ## Side Effects
//!
//! Handlers never touch hardware or supervisor-global state directly. After validating a
//! request they call the [`SyscallOps`] implementation the dispatcher was built with. The
//! supervisor uses [`FirmwareOps`]; tests substitute their own implementation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::standard::efi::{ALLOCATE_ANY_PAGES, AllocateType, MemoryType, RUNTIME_SERVICES_DATA, Status};

use patina::{MemoryRange, UEFI_PAGE_SIZE, management_mode::supervisor::SyscallIndex};

use crate::{
    memory::page_ownership::PageOwnership,
    policy::{AccessType, Instruction, IoWidth},
};

use super::{
    SyscallResult,
    syscall_ops::{FirmwareOps, PolicyDecision, SyscallOps},
};

// The syscall entry assembly is built only for the UEFI target so host builds (tests, doctests)
// link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("syscall_entry.asm"));

/// `EFI_MM_IO_WIDTH` value for an 8-bit I/O access.
const MM_IO_UINT8: u64 = 0;
/// `EFI_MM_IO_WIDTH` value for a 16-bit I/O access.
const MM_IO_UINT16: u64 = 1;
/// `EFI_MM_IO_WIDTH` value for a 32-bit I/O access.
const MM_IO_UINT32: u64 = 2;

/// Converts an `EFI_MM_IO_WIDTH` value to an [`IoWidth`]. Other widths return `None`.
fn efi_io_width_to_io_width(width: u64) -> Option<IoWidth> {
    match width {
        MM_IO_UINT8 => Some(IoWidth::Byte),
        MM_IO_UINT16 => Some(IoWidth::Word),
        MM_IO_UINT32 => Some(IoWidth::Dword),
        _ => None,
    }
}

/// Converts a raw syscall argument into an I/O port address.
///
/// The x86 I/O space is 16 bits wide, so a larger value is a malformed request. Rejecting it
/// here keeps the policy gate from validating a truncated port.
fn io_port_from_arg(port: u64) -> Option<u16> {
    u16::try_from(port).ok()
}

/// Converts a raw syscall argument into a `u32`, such as an MSR index or an EFI enum value.
///
/// A value with any of bits 63:32 set is a malformed request. Rejecting it keeps the policy gate
/// and the handlers from acting on a truncated value. `operation` names the syscall in the log.
fn u32_from_arg(operation: &str, value: u64) -> Result<u32, Status> {
    u32::try_from(value).map_err(|_| {
        log::error!("{operation}: argument 0x{value:x} does not fit in 32 bits");
        Status::INVALID_PARAMETER
    })
}

/// Returns `Ok` only if the firmware policy allowed the operation.
///
/// Otherwise returns the status the syscall reports to Ring 3: `ACCESS_DENIED` when the policy
/// denies the operation, and `NOT_READY` when no policy gate is installed. Ring 3 sees only that
/// status, so the reason is logged here, with `operation` naming the syscall.
fn check_policy(operation: impl core::fmt::Display, decision: PolicyDecision) -> Result<(), Status> {
    match decision {
        PolicyDecision::Allowed => Ok(()),
        PolicyDecision::Denied(err) => {
            log::error!("{operation}: Blocked by policy: {err:?}");
            Err(Status::ACCESS_DENIED)
        }
        PolicyDecision::Unavailable => {
            log::error!("{operation}: Policy gate not initialized");
            Err(Status::NOT_READY)
        }
    }
}

/// Context for a syscall invocation.
#[derive(Debug, Clone, Copy)]
pub struct SyscallContext {
    /// The syscall index (from RAX).
    pub call_index: u64,
    /// First argument (from RDX).
    pub arg1: u64,
    /// Second argument (from R8).
    pub arg2: u64,
    /// Third argument (from R9).
    pub arg3: u64,
    /// Caller return address (from RCX, set by the syscall instruction).
    pub caller_addr: u64,
    /// Ring 3 stack pointer at syscall entry.
    pub ring3_stack_ptr: u64,
}

/// Dispatches syscalls from Ring 3.
///
/// Owns the [`SyscallOps`] implementation that carries out validated requests. The supervisor
/// uses [`FirmwareOps`]; unit tests substitute a test implementation to run the validation logic
/// without privileged instructions.
pub struct SyscallDispatcher<O: SyscallOps = FirmwareOps> {
    /// Performs the privileged work of a validated syscall.
    ops: O,
}

impl SyscallDispatcher<FirmwareOps> {
    /// Creates a syscall dispatcher that operates on real hardware and supervisor state.
    pub const fn new() -> Self {
        Self::with_ops(FirmwareOps)
    }
}

impl<O: SyscallOps> SyscallDispatcher<O> {
    /// Creates a syscall dispatcher backed by the given [`SyscallOps`] implementation.
    pub const fn with_ops(ops: O) -> Self {
        Self { ops }
    }

    /// Validates the syscall index and runs its handler. The result is returned to Ring 3 in
    /// RAX.
    ///
    /// Save-state read failures are returned as `Ok` with the EFI status as the value.
    ///
    /// ## Errors
    ///
    /// Returns `UNSUPPORTED` for an unknown syscall index.
    ///
    /// ## Panics
    ///
    /// Panics if any other handler fails, or for the `LegacyMax` index, so Ring 3 cannot
    /// continue after a rejected privileged request.
    pub fn dispatch(&self, ctx: &SyscallContext) -> SyscallResult {
        let Ok(index) = SyscallIndex::try_from(ctx.call_index) else {
            log::error!("Unknown syscall index: 0x{:x}", ctx.call_index);
            return Err(Status::UNSUPPORTED);
        };

        log::trace!(
            "Syscall: {:?} (0x{:x}), args: 0x{:x}, 0x{:x}, 0x{:x}, caller: 0x{:x}, stack: 0x{:x}",
            index,
            ctx.call_index,
            ctx.arg1,
            ctx.arg2,
            ctx.arg3,
            ctx.caller_addr,
            ctx.ring3_stack_ptr
        );

        let result = match index {
            SyscallIndex::RdMsr => self.handle_rdmsr(ctx),
            SyscallIndex::WrMsr => self.handle_wrmsr(ctx),
            SyscallIndex::Cli => self.handle_instruction(Instruction::Cli),
            SyscallIndex::IoRead => self.handle_io_read(ctx),
            SyscallIndex::IoWrite => self.handle_io_write(ctx),
            SyscallIndex::Wbinvd => self.handle_instruction(Instruction::Wbinvd),
            SyscallIndex::Hlt => self.handle_instruction(Instruction::Hlt),
            SyscallIndex::SaveStateRead => self.handle_save_state_read(ctx),
            SyscallIndex::LegacyMax => panic!("Invalid syscall index: LegacyMax is not a real syscall"),
            SyscallIndex::AllocPage => self.handle_alloc_page(ctx),
            SyscallIndex::FreePage => self.handle_free_page(ctx),
            SyscallIndex::StartApProc => self.handle_start_ap_proc(ctx),
            SyscallIndex::SaveStateRead2 => self.handle_save_state_read2(ctx),
            SyscallIndex::MmMemoryUnblocked => self.handle_mm_memory_unblocked(ctx),
            SyscallIndex::MmIsCommBuffer => self.handle_mm_is_comm_buffer(ctx),
        };

        match result {
            // Both save-state phases report failures back to the caller as an EFI status in RAX,
            // which is what `EFI_MM_CPU_PROTOCOL.ReadSaveState` returns to its own caller.
            Err(err) if matches!(index, SyscallIndex::SaveStateRead | SyscallIndex::SaveStateRead2) => {
                log::trace!("Syscall {:?} returned value=0x{:x}", index, err.as_usize());
                Ok(err.as_usize() as u64)
            }
            Err(err) => {
                panic!("Syscall: {index:?} failed with error: {err:?}");
            }
            _ => result,
        }
    }

    /// Handles the MSR read syscall: checks the read against firmware policy, then runs rdmsr.
    ///
    /// - Arg1: MSR index
    /// - Returns: MSR value (RAX)
    fn handle_rdmsr(&self, ctx: &SyscallContext) -> SyscallResult {
        let msr_index = u32_from_arg("RDMSR", ctx.arg1)?;
        log::trace!("RDMSR: msr=0x{msr_index:x}");

        check_policy("RDMSR", self.ops.check_msr(msr_index, AccessType::Read))?;

        // SAFETY: the policy gate authorized reading this MSR, as `SyscallOps::read_msr` requires.
        let value = unsafe { self.ops.read_msr(msr_index) };
        log::trace!("RDMSR: MSR 0x{msr_index:x} = 0x{value:x}");
        Ok(value)
    }

    /// Handles the MSR write syscall: checks the write against firmware policy, then runs wrmsr.
    ///
    /// - Arg1: MSR index
    /// - Arg2: Value to write
    fn handle_wrmsr(&self, ctx: &SyscallContext) -> SyscallResult {
        let msr_index = u32_from_arg("WRMSR", ctx.arg1)?;
        let value = ctx.arg2;
        log::trace!("WRMSR: msr=0x{msr_index:x}, value=0x{value:x}");

        check_policy("WRMSR", self.ops.check_msr(msr_index, AccessType::Write))?;

        // SAFETY: the policy gate authorized writing this MSR, as `SyscallOps::write_msr` requires.
        unsafe { self.ops.write_msr(msr_index, value) };
        log::trace!("WRMSR: MSR 0x{msr_index:x} written with 0x{value:x}");
        Ok(0)
    }

    /// Handles the syscalls that run a privileged instruction (CLI, WBINVD or HLT) for Ring 3,
    /// after checking it against firmware policy.
    fn handle_instruction(&self, instruction: Instruction) -> SyscallResult {
        log::trace!("{instruction:?}");

        check_policy(instruction, self.ops.check_instruction(instruction))?;

        // SAFETY: the policy gate authorized this instruction, as
        // `SyscallOps::execute_instruction` requires.
        unsafe { self.ops.execute_instruction(instruction) };
        log::trace!("{instruction:?}: Executed");
        Ok(0)
    }

    /// Handles the I/O port read syscall: checks the read against firmware policy, then runs
    /// the in instruction.
    ///
    /// - Arg1: I/O port address
    /// - Arg2: `EFI_MM_IO_WIDTH` (0=UINT8, 1=UINT16, 2=UINT32)
    /// - Returns: Value read from the port (RAX)
    ///
    /// A port outside the 16-bit I/O space is rejected with `EFI_INVALID_PARAMETER` rather than
    /// truncated, so the supervisor never accesses a port the caller did not ask for.
    fn handle_io_read(&self, ctx: &SyscallContext) -> SyscallResult {
        let port = ctx.arg1;
        let efi_width = ctx.arg2;
        log::trace!("IO_READ: port=0x{port:x}, width={efi_width}");

        let Some(io_width) = efi_io_width_to_io_width(efi_width) else {
            log::error!("IO_READ: Invalid IO width: {efi_width}");
            return Err(Status::INVALID_PARAMETER);
        };

        // Reject ports outside the 16-bit I/O space before the policy gate sees them.
        let Some(port_addr) = io_port_from_arg(port) else {
            log::error!("IO_READ: Port 0x{port:x} is outside the 16-bit I/O space");
            return Err(Status::INVALID_PARAMETER);
        };

        check_policy("IO_READ", self.ops.check_io(port_addr, io_width, AccessType::Read))?;

        // SAFETY: the policy gate authorized reading this port and width, as
        // `SyscallOps::io_read` requires.
        let value = unsafe { self.ops.io_read(port_addr, io_width) };

        log::trace!("IO_READ: port=0x{port:x} => 0x{value:x}");
        Ok(value)
    }

    /// Handles the I/O port write syscall: checks the write against firmware policy, then runs
    /// the out instruction.
    ///
    /// - Arg1: I/O port address
    /// - Arg2: `EFI_MM_IO_WIDTH` (0=UINT8, 1=UINT16, 2=UINT32)
    /// - Arg3: Value to write
    ///
    /// A port outside the 16-bit I/O space is rejected with `EFI_INVALID_PARAMETER` rather than
    /// truncated, so the supervisor never writes to a port the caller did not ask for.
    fn handle_io_write(&self, ctx: &SyscallContext) -> SyscallResult {
        let port = ctx.arg1;
        let efi_width = ctx.arg2;
        let value = ctx.arg3;
        log::trace!("IO_WRITE: port=0x{port:x}, width={efi_width}, value=0x{value:x}");

        let Some(io_width) = efi_io_width_to_io_width(efi_width) else {
            log::error!("IO_WRITE: Invalid IO width: {efi_width}");
            return Err(Status::INVALID_PARAMETER);
        };

        // Reject ports outside the 16-bit I/O space before the policy gate sees them.
        let Some(port_addr) = io_port_from_arg(port) else {
            log::error!("IO_WRITE: Port 0x{port:x} is outside the 16-bit I/O space");
            return Err(Status::INVALID_PARAMETER);
        };

        check_policy("IO_WRITE", self.ops.check_io(port_addr, io_width, AccessType::Write))?;

        // SAFETY: the policy gate authorized writing this port and width, as
        // `SyscallOps::io_write` requires.
        unsafe { self.ops.io_write(port_addr, io_width, value) };

        log::trace!("IO_WRITE: port=0x{port:x} <= 0x{value:x}");
        Ok(0)
    }

    /// Handles the legacy save state read syscall (phase 1 of the two-phase read).
    ///
    /// - Arg1: User MM CPU protocol pointer
    /// - Arg2: Register to be read (`EFI_MM_SAVE_STATE_REGISTER`)
    /// - Arg3: CPU index to read from
    fn handle_save_state_read(&self, ctx: &SyscallContext) -> SyscallResult {
        log::trace!("SAVE_STATE_READ: protocol=0x{:x}, register={}, cpu={}", ctx.arg1, ctx.arg2, ctx.arg3);

        if ctx.arg1 == 0 {
            log::error!("SAVE_STATE_READ: Null protocol pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        self.ops.save_state_read_phase1(ctx.arg1, ctx.arg2, ctx.arg3)
    }

    /// Handles the page allocation syscall.
    ///
    /// - Arg1: Allocate type (`EFI_ALLOCATE_TYPE`)
    /// - Arg2: Memory type (must be `EfiRuntimeServicesData`)
    /// - Arg3: Page count
    /// - Returns: Allocated physical address (RAX)
    fn handle_alloc_page(&self, ctx: &SyscallContext) -> SyscallResult {
        let alloc_type: AllocateType = u32_from_arg("ALLOC_PAGE", ctx.arg1)?;
        let mem_type: MemoryType = u32_from_arg("ALLOC_PAGE", ctx.arg2)?;
        let page_count = ctx.arg3;
        log::trace!("ALLOC_PAGE: alloc_type={alloc_type}, mem_type={mem_type}, count={page_count}");

        // Only the BSP may allocate pages, because allocation updates the page table.
        if !self.ops.is_bsp() {
            log::error!("ALLOC_PAGE: AP cannot allocate pages");
            return Err(Status::ACCESS_DENIED);
        }

        if mem_type != RUNTIME_SERVICES_DATA {
            log::error!("ALLOC_PAGE: Invalid memory type: {mem_type}");
            return Err(Status::INVALID_PARAMETER);
        }

        // The page allocator supports only AllocateAnyPages.
        if alloc_type != ALLOCATE_ANY_PAGES {
            log::error!("ALLOC_PAGE: Only AllocateAnyPages (0) is supported, got {alloc_type}");
            return Err(Status::UNSUPPORTED);
        }

        if page_count == 0 {
            log::error!("ALLOC_PAGE: Zero page count");
            return Err(Status::INVALID_PARAMETER);
        }

        match self.ops.allocate_user_pages(page_count as usize) {
            Ok(addr) => {
                log::trace!("ALLOC_PAGE: Allocated {page_count} page(s) at 0x{addr:x}");
                Ok(addr)
            }
            Err(err) => {
                log::error!("ALLOC_PAGE: Allocation failed: {err:?}");
                Err(Status::OUT_OF_RESOURCES)
            }
        }
    }

    /// Handles the page free syscall, matching the `SMM_FREE_PAGE` case of the C implementation.
    ///
    /// - Arg1: Physical address to free
    /// - Arg2: Number of pages
    fn handle_free_page(&self, ctx: &SyscallContext) -> SyscallResult {
        let (addr, page_count) = (ctx.arg1, ctx.arg2);
        log::trace!("FREE_PAGE: addr=0x{addr:x}, count={page_count}");

        // Zero pages, or a range that runs past the end of the address space, is never valid.
        let Ok(range) = MemoryRange::from_pages(addr, page_count) else {
            log::error!("FREE_PAGE: {page_count} page(s) at 0x{addr:x} is not a valid range");
            return Err(Status::INVALID_PARAMETER);
        };

        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("FREE_PAGE: Address 0x{addr:x} is not page-aligned");
            return Err(Status::INVALID_PARAMETER);
        }

        // Ring 3 may free only its own (User) allocations, never supervisor allocations.
        match self.ops.allocation_type(addr) {
            Some(crate::memory::AllocationType::User) => {}
            Some(crate::memory::AllocationType::Supervisor) => {
                log::error!("FREE_PAGE: Address 0x{addr:x} is a supervisor allocation - access denied");
                return Err(Status::SECURITY_VIOLATION);
            }
            None => {
                log::error!("FREE_PAGE: Address 0x{addr:x} is not allocated");
                return Err(Status::INVALID_PARAMETER);
            }
        }

        // The checked free fails if any page in the range is not a User allocation.
        match self.ops.free_user_pages(addr, page_count as usize) {
            Ok(()) => {
                log::trace!("FREE_PAGE: Freed {range}");
                Ok(0)
            }
            Err(err) => {
                log::error!("FREE_PAGE: Free failed: {err:?}");
                Err(Status::SECURITY_VIOLATION)
            }
        }
    }

    /// Handles the start AP procedure syscall.
    ///
    /// Validates the request and calls the platform AP startup function registered during
    /// [`MmSupervisorCore`](crate::MmSupervisorCore) initialization. Checks done here:
    /// - Caller is the BSP
    /// - Procedure pointer is non-null and user-owned
    /// - Argument pointer, if non-null, is user-owned
    ///
    /// The registered function, which has the CPU and mailbox managers, does the rest of the
    /// validation (CPU index range, BSP target, AP busy) and the dispatch.
    ///
    /// - Arg1: Procedure function pointer
    /// - Arg2: CPU index
    /// - Arg3: Argument pointer
    fn handle_start_ap_proc(&self, ctx: &SyscallContext) -> SyscallResult {
        let procedure = ctx.arg1;
        let cpu_index = ctx.arg2;
        let argument = ctx.arg3;

        log::trace!("START_AP_PROC: proc=0x{procedure:x}, cpu={cpu_index}, arg=0x{argument:x}");

        // 1. Caller is the BSP. An AP reaching here is already running a procedure the BSP
        //    dispatched to it. Letting it dispatch in turn would nest the MP state machine: it
        //    could contend for a mailbox the BSP is filling, target itself and spin the full AP
        //    timeout waiting for a response it cannot post, or leave a second AP busy while the
        //    BSP believes every AP is parked.
        if !self.ops.is_bsp() {
            log::error!("START_AP_PROC: only the BSP may dispatch a procedure to an AP");
            return Err(Status::ACCESS_DENIED);
        }

        // 2. Procedure pointer is non-null.
        if procedure == 0 {
            log::error!("START_AP_PROC: Null procedure pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        // 3. The procedure runs demoted on the target AP, so it must be user-owned. Accepting any
        //    mapped address would let Ring 3 name a supervisor address to run.
        let procedure_size = core::mem::size_of::<usize>() as u64;
        let pointer_owner = |address| {
            MemoryRange::new(address, procedure_size).ok().and_then(|range| self.ops.query_address_ownership(&range))
        };
        match pointer_owner(procedure) {
            Some(PageOwnership::User) => {}
            Some(owner) => {
                log::error!("START_AP_PROC: Procedure 0x{procedure:x} is owned by {owner:?}, expected User");
                return Err(Status::SECURITY_VIOLATION);
            }
            None => {
                log::error!("START_AP_PROC: Procedure 0x{procedure:x} not in mapped memory");
                return Err(Status::INVALID_PARAMETER);
            }
        }

        // 4. The argument reaches the same Ring 3 procedure, which cannot read supervisor memory,
        //    so a non-null argument must be user-owned too.
        if argument != 0 {
            match pointer_owner(argument) {
                Some(PageOwnership::User) => {}
                Some(owner) => {
                    log::error!("START_AP_PROC: Argument 0x{argument:x} is owned by {owner:?}, expected User");
                    return Err(Status::SECURITY_VIOLATION);
                }
                None => {
                    log::error!("START_AP_PROC: Argument 0x{argument:x} not in mapped memory");
                    return Err(Status::INVALID_PARAMETER);
                }
            }
        }

        // 5. Call the registered AP startup function.
        match self.ops.start_ap_procedure(cpu_index, procedure, argument) {
            Some(0) => Ok(0),
            Some(status) => Err(Status::from_usize(status as usize)),
            None => {
                log::error!("START_AP_PROC: AP startup not initialized");
                Err(Status::NOT_READY)
            }
        }
    }

    /// Handles the extended save state read syscall (phase 2 of the two-phase read).
    ///
    /// - Arg1: User MM CPU protocol pointer
    /// - Arg2: Width of buffer to read in bytes
    /// - Arg3: User buffer to hold return data
    fn handle_save_state_read2(&self, ctx: &SyscallContext) -> SyscallResult {
        if ctx.arg1 == 0 {
            log::error!("SAVE_STATE_READ2: Null protocol pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        self.ops.save_state_read_phase2(ctx.arg1, ctx.arg2, ctx.arg3)
    }

    /// Handles the MM memory unblocked syscall: reports whether a range lies in an unblocked
    /// region and in user-owned memory.
    ///
    /// - Arg1: Physical address
    /// - Arg2: Size in bytes
    /// - Returns: 1 (TRUE) if valid, 0 (FALSE) otherwise
    fn handle_mm_memory_unblocked(&self, ctx: &SyscallContext) -> SyscallResult {
        let (addr, size) = (ctx.arg1, ctx.arg2);
        log::trace!("MM_MEMORY_UNBLOCKED: addr=0x{addr:x}, size=0x{size:x}");

        // An empty buffer, or one that runs past the end of the address space, is never valid.
        let Ok(range) = MemoryRange::new(addr, size) else {
            log::trace!("MM_MEMORY_UNBLOCKED: addr=0x{addr:x} size=0x{size:x} is not a valid range");
            return Ok(0); // FALSE
        };

        let is_valid = self.ops.is_within_unblocked_region(&range);

        if !is_valid {
            log::trace!("MM_MEMORY_UNBLOCKED: {range} not in unblocked region");
            return Ok(0); // FALSE
        }

        // The buffer must also be user-owned.
        if let Some(owner) = self.ops.query_address_ownership(&range) {
            if owner != PageOwnership::User {
                log::trace!("MM_MEMORY_UNBLOCKED: {range} owned by {owner:?} - not valid");
                return Ok(0); // FALSE
            }
        } else {
            log::trace!("MM_MEMORY_UNBLOCKED: {range} not in mapped memory");
            return Ok(0); // FALSE
        }

        log::trace!("MM_MEMORY_UNBLOCKED: {range} is valid");
        Ok(1) // TRUE
    }

    /// Handles the MM is-communication-buffer syscall: reports whether a range lies entirely
    /// inside the user communication buffer.
    ///
    /// - Arg1: Buffer address
    /// - Arg2: Buffer size
    /// - Returns: 1 (TRUE) if valid comm buffer, 0 (FALSE) otherwise
    fn handle_mm_is_comm_buffer(&self, ctx: &SyscallContext) -> SyscallResult {
        let (address, size) = (ctx.arg1, ctx.arg2);
        log::trace!("MM_IS_COMM_BUFFER: addr=0x{address:x}, size=0x{size:x}");

        // An empty buffer, or one that runs past the end of the address space, is never valid.
        let Ok(range) = MemoryRange::new(address, size) else {
            log::trace!("MM_IS_COMM_BUFFER: addr=0x{address:x} size=0x{size:x} is not a valid range");
            return Ok(0); // FALSE
        };

        let Some(config) = self.ops.comm_buffer_config() else {
            log::error!("MM_IS_COMM_BUFFER: Comm buffer config not initialized");
            return Ok(0); // FALSE
        };

        // The range must lie entirely within the user comm buffer.
        let is_valid = MemoryRange::new(config.user.internal_address, config.user.size)
            .is_ok_and(|buffer| buffer.contains_range(&range));

        log::trace!("MM_IS_COMM_BUFFER: {range} => {is_valid}");
        if is_valid { Ok(1) } else { Ok(0) }
    }
}

/// C-compatible syscall dispatcher entry point.
///
/// Called from the assembly syscall entry stub (`syscall_center`). The parameters carry the
/// syscall registers described in the module docs, and the return value goes to Ring 3 in RAX.
///
/// ## Panics
///
/// Panics when [`SyscallDispatcher::dispatch`] fails or panics.
#[unsafe(no_mangle)]
pub extern "efiapi" fn syscall_dispatcher(
    call_index: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    caller_addr: u64,
    ring3_stack_ptr: u64,
) -> u64 {
    let ctx = SyscallContext { call_index, arg1, arg2, arg3, caller_addr, ring3_stack_ptr };

    // `dispatch` panics on a failed privileged request and returns save-state failures as a
    // status value, so the only error left is an unknown syscall index.
    SyscallDispatcher::new()
        .dispatch(&ctx)
        .unwrap_or_else(|status| panic!("Syscall 0x{call_index:x} failed with error: {status:?}"))
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::comm_buffer::{CommBufferConfig, CommChannel};
    use crate::error::MmSupervisorResult;
    use crate::memory::{AllocError, AllocationType};
    use crate::policy::PolicyGateError;
    use core::cell::RefCell;

    /// An action a handler asked its [`SyscallOps`] implementation to perform.
    ///
    /// Recording policy queries alongside the privileged work lets a test assert both that the
    /// work happened and that it only happened after the request cleared the policy gate.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Effect {
        CheckMsr(u32, AccessType),
        CheckIo(u16, IoWidth, AccessType),
        CheckInstruction(Instruction),
        ReadMsr(u32),
        WriteMsr(u32, u64),
        IoRead(u16, IoWidth),
        IoWrite(u16, IoWidth, u64),
        Execute(Instruction),
        AllocateUserPages(usize),
        FreeUserPages(u64, usize),
        StartAp(u64, u64, u64),
        SaveStatePhase1(u64, u64, u64),
        SaveStatePhase2(u64, u64, u64),
    }

    /// A [`SyscallOps`] implementation that records what a handler asked for and answers with
    /// caller-configured values, so handler validation can run on a host.
    struct MockOps {
        msr_policy: PolicyDecision,
        io_policy: PolicyDecision,
        instruction_policy: PolicyDecision,
        msr_value: u64,
        io_value: u64,
        is_bsp: bool,
        allocate_result: MmSupervisorResult<u64>,
        free_result: MmSupervisorResult<()>,
        allocation_type: Option<AllocationType>,
        /// Mapped addresses and their ownership; any other address is treated as unmapped.
        mapped: Vec<(u64, PageOwnership)>,
        ap_status: Option<u64>,
        phase1_result: SyscallResult,
        phase2_result: SyscallResult,
        unblocked: bool,
        comm_buffer: Option<CommBufferConfig>,
        effects: RefCell<Vec<Effect>>,
    }

    impl Default for MockOps {
        fn default() -> Self {
            Self {
                msr_policy: PolicyDecision::Allowed,
                io_policy: PolicyDecision::Allowed,
                instruction_policy: PolicyDecision::Allowed,
                msr_value: 0,
                io_value: 0,
                is_bsp: true,
                allocate_result: Ok(0x4000),
                free_result: Ok(()),
                allocation_type: Some(AllocationType::User),
                mapped: Vec::new(),
                ap_status: Some(0),
                phase1_result: Ok(0),
                phase2_result: Ok(0),
                unblocked: true,
                comm_buffer: None,
                effects: RefCell::new(Vec::new()),
            }
        }
    }

    impl MockOps {
        fn record(&self, effect: Effect) {
            self.effects.borrow_mut().push(effect);
        }

        fn effects(&self) -> Vec<Effect> {
            self.effects.borrow().clone()
        }
    }

    impl SyscallOps for MockOps {
        fn check_msr(&self, msr: u32, access: AccessType) -> PolicyDecision {
            self.record(Effect::CheckMsr(msr, access));
            self.msr_policy
        }

        fn check_io(&self, port: u16, width: IoWidth, access: AccessType) -> PolicyDecision {
            self.record(Effect::CheckIo(port, width, access));
            self.io_policy
        }

        fn check_instruction(&self, instruction: Instruction) -> PolicyDecision {
            self.record(Effect::CheckInstruction(instruction));
            self.instruction_policy
        }

        unsafe fn read_msr(&self, msr: u32) -> u64 {
            self.record(Effect::ReadMsr(msr));
            self.msr_value
        }

        unsafe fn write_msr(&self, msr: u32, value: u64) {
            self.record(Effect::WriteMsr(msr, value));
        }

        unsafe fn io_read(&self, port: u16, width: IoWidth) -> u64 {
            self.record(Effect::IoRead(port, width));
            self.io_value
        }

        unsafe fn io_write(&self, port: u16, width: IoWidth, value: u64) {
            self.record(Effect::IoWrite(port, width, value));
        }

        unsafe fn execute_instruction(&self, instruction: Instruction) {
            self.record(Effect::Execute(instruction));
        }

        fn is_bsp(&self) -> bool {
            self.is_bsp
        }

        fn allocate_user_pages(&self, page_count: usize) -> MmSupervisorResult<u64> {
            self.record(Effect::AllocateUserPages(page_count));
            self.allocate_result
        }

        fn free_user_pages(&self, addr: u64, page_count: usize) -> MmSupervisorResult<()> {
            self.record(Effect::FreeUserPages(addr, page_count));
            self.free_result
        }

        fn allocation_type(&self, _addr: u64) -> Option<AllocationType> {
            self.allocation_type
        }

        fn query_address_ownership(&self, range: &MemoryRange) -> Option<PageOwnership> {
            self.mapped.iter().find(|(mapped, _)| *mapped == range.base()).map(|(_, owner)| *owner)
        }

        fn start_ap_procedure(&self, cpu_index: u64, procedure: u64, argument: u64) -> Option<u64> {
            self.record(Effect::StartAp(cpu_index, procedure, argument));
            self.ap_status
        }

        fn save_state_read_phase1(&self, protocol: u64, register: u64, cpu_index: u64) -> SyscallResult {
            self.record(Effect::SaveStatePhase1(protocol, register, cpu_index));
            self.phase1_result
        }

        fn save_state_read_phase2(&self, protocol: u64, width: u64, buffer: u64) -> SyscallResult {
            self.record(Effect::SaveStatePhase2(protocol, width, buffer));
            self.phase2_result
        }

        fn is_within_unblocked_region(&self, _range: &MemoryRange) -> bool {
            self.unblocked
        }

        fn comm_buffer_config(&self) -> Option<CommBufferConfig> {
            self.comm_buffer
        }
    }

    /// Builds a dispatcher over a mock, which tests reach through `dispatcher.ops`.
    fn new_dispatcher(ops: MockOps) -> SyscallDispatcher<MockOps> {
        SyscallDispatcher::with_ops(ops)
    }

    /// Builds a syscall context with recognizable caller/stack values.
    fn ctx(call_index: u64, arg1: u64, arg2: u64, arg3: u64) -> SyscallContext {
        SyscallContext { call_index, arg1, arg2, arg3, caller_addr: 0xCA11_0000, ring3_stack_ptr: 0x57AC_0000 }
    }

    #[test]
    fn test_syscall_index_roundtrip() {
        for idx in
            [SyscallIndex::RdMsr, SyscallIndex::WrMsr, SyscallIndex::Cli, SyscallIndex::IoRead, SyscallIndex::IoWrite]
        {
            assert_eq!(SyscallIndex::try_from(idx.as_u64()), Ok(idx));
        }
    }

    #[test]
    fn test_unknown_syscall_index() {
        // Values that fall in the gaps between defined indices map to `None`.
        assert_eq!(SyscallIndex::try_from(0x0008), Err(0x0008));
        assert_eq!(SyscallIndex::try_from(0x10000), Err(0x10000));
        assert_eq!(SyscallIndex::try_from(0xDEAD_BEEF), Err(0xDEAD_BEEF));
    }

    #[test]
    fn test_efi_io_width_conversion() {
        assert_eq!(efi_io_width_to_io_width(MM_IO_UINT8), Some(IoWidth::Byte));
        assert_eq!(efi_io_width_to_io_width(MM_IO_UINT16), Some(IoWidth::Word));
        assert_eq!(efi_io_width_to_io_width(MM_IO_UINT32), Some(IoWidth::Dword));
        // 3 is EFI's MM_IO_UINT64, which the supervisor does not support, and anything above it
        // is not a width at all.
        assert_eq!(efi_io_width_to_io_width(3), None);
        assert_eq!(efi_io_width_to_io_width(u64::MAX), None);
    }

    #[test]
    fn test_io_port_from_arg_rejects_ports_outside_io_space() {
        assert_eq!(io_port_from_arg(0), Some(0));
        assert_eq!(io_port_from_arg(0xB2), Some(0xB2));
        assert_eq!(io_port_from_arg(0xFFFF), Some(0xFFFF));
        assert_eq!(io_port_from_arg(0x1_0000), None);
        // Without the range check these would silently alias port 0.
        assert_eq!(io_port_from_arg(0x1_0000_0000), None);
        assert_eq!(io_port_from_arg(u64::MAX), None);
    }

    #[test]
    fn test_check_policy_maps_decisions_to_status() {
        assert_eq!(check_policy("TEST", PolicyDecision::Allowed), Ok(()));
        assert_eq!(
            check_policy("TEST", PolicyDecision::Denied(PolicyGateError::AccessDenied.into())),
            Err(Status::ACCESS_DENIED)
        );
        assert_eq!(check_policy("TEST", PolicyDecision::Unavailable), Err(Status::NOT_READY));
    }

    #[test]
    fn test_dispatch_rejects_unknown_index() {
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.dispatch(&ctx(0x0008, 0, 0, 0)), Err(Status::UNSUPPORTED));
        assert!(dispatcher.ops.effects().is_empty());
    }

    /// One dispatch routing expectation for [`test_dispatch_routes_to_handlers`].
    struct RoutingCase {
        index: SyscallIndex,
        ops: MockOps,
        args: (u64, u64, u64),
        expected: SyscallResult,
        /// The side effect the handler must produce, for the handlers that act on the platform.
        effect: Option<Effect>,
    }

    #[test]
    fn test_dispatch_routes_to_handlers() {
        let any = u64::from(ALLOCATE_ANY_PAGES);
        let data = u64::from(RUNTIME_SERVICES_DATA);
        let comm_buffer = CommBufferConfig {
            user: CommChannel { internal_address: 0x1_0000, size: 0x1000, ..Default::default() },
            ..Default::default()
        };

        // Every real index must reach the handler that performs the matching side effect. The
        // last two handlers answer a question rather than acting, so they are pinned by result.
        let cases = [
            RoutingCase {
                index: SyscallIndex::RdMsr,
                ops: MockOps::default(),
                args: (0x1B, 0, 0),
                expected: Ok(0),
                effect: Some(Effect::ReadMsr(0x1B)),
            },
            RoutingCase {
                index: SyscallIndex::WrMsr,
                ops: MockOps::default(),
                args: (0x1B, 0x5A, 0),
                expected: Ok(0),
                effect: Some(Effect::WriteMsr(0x1B, 0x5A)),
            },
            RoutingCase {
                index: SyscallIndex::Cli,
                ops: MockOps::default(),
                args: (0, 0, 0),
                expected: Ok(0),
                effect: Some(Effect::Execute(Instruction::Cli)),
            },
            RoutingCase {
                index: SyscallIndex::Wbinvd,
                ops: MockOps::default(),
                args: (0, 0, 0),
                expected: Ok(0),
                effect: Some(Effect::Execute(Instruction::Wbinvd)),
            },
            RoutingCase {
                index: SyscallIndex::Hlt,
                ops: MockOps::default(),
                args: (0, 0, 0),
                expected: Ok(0),
                effect: Some(Effect::Execute(Instruction::Hlt)),
            },
            RoutingCase {
                index: SyscallIndex::IoRead,
                ops: MockOps::default(),
                args: (0xB2, MM_IO_UINT8, 0),
                expected: Ok(0),
                effect: Some(Effect::IoRead(0xB2, IoWidth::Byte)),
            },
            RoutingCase {
                index: SyscallIndex::IoWrite,
                ops: MockOps::default(),
                args: (0xB2, MM_IO_UINT8, 0x5A),
                expected: Ok(0),
                effect: Some(Effect::IoWrite(0xB2, IoWidth::Byte, 0x5A)),
            },
            RoutingCase {
                index: SyscallIndex::SaveStateRead,
                ops: MockOps::default(),
                args: (0x1000, 38, 1),
                expected: Ok(0),
                effect: Some(Effect::SaveStatePhase1(0x1000, 38, 1)),
            },
            RoutingCase {
                index: SyscallIndex::AllocPage,
                ops: MockOps { allocate_result: Ok(0x8000), ..Default::default() },
                args: (any, data, 1),
                expected: Ok(0x8000),
                effect: Some(Effect::AllocateUserPages(1)),
            },
            RoutingCase {
                index: SyscallIndex::FreePage,
                ops: MockOps::default(),
                args: (0x2000, 1, 0),
                expected: Ok(0),
                effect: Some(Effect::FreeUserPages(0x2000, 1)),
            },
            RoutingCase {
                index: SyscallIndex::StartApProc,
                ops: MockOps { mapped: vec![(0x1000, PageOwnership::User)], ..Default::default() },
                args: (0x1000, 1, 0),
                expected: Ok(0),
                effect: Some(Effect::StartAp(1, 0x1000, 0)),
            },
            RoutingCase {
                index: SyscallIndex::SaveStateRead2,
                ops: MockOps::default(),
                args: (0x1000, 8, 0x2000),
                expected: Ok(0),
                effect: Some(Effect::SaveStatePhase2(0x1000, 8, 0x2000)),
            },
            RoutingCase {
                index: SyscallIndex::MmMemoryUnblocked,
                ops: MockOps { mapped: vec![(0x5000, PageOwnership::User)], ..Default::default() },
                args: (0x5000, 0x100, 0),
                expected: Ok(1),
                effect: None,
            },
            RoutingCase {
                index: SyscallIndex::MmIsCommBuffer,
                ops: MockOps { comm_buffer: Some(comm_buffer), ..Default::default() },
                args: (0x1_0000, 0x100, 0),
                expected: Ok(1),
                effect: None,
            },
        ];

        for case in cases {
            let index = case.index;
            let (arg1, arg2, arg3) = case.args;
            let dispatcher = new_dispatcher(case.ops);

            assert_eq!(
                dispatcher.dispatch(&ctx(index.as_u64(), arg1, arg2, arg3)),
                case.expected,
                "wrong result for {index:?}"
            );
            if let Some(effect) = case.effect {
                assert!(dispatcher.ops.effects().contains(&effect), "{index:?} did not produce {effect:?}");
            }
        }
    }

    #[test]
    fn test_syscall_dispatcher_entry_point() {
        // The `efiapi` entry point the assembly stub calls builds a dispatcher over the real
        // `FirmwareOps`. `MmIsCommBuffer` is the one syscall that reaches a definite answer
        // without hardware: with no communication buffer published it reports FALSE.
        assert_eq!(syscall_dispatcher(SyscallIndex::MmIsCommBuffer.as_u64(), 0x1_0000, 0x100, 0, 0xCA11, 0x57AC), 0);
    }

    #[test]
    #[should_panic(expected = "failed with error")]
    fn test_dispatch_panics_when_a_handler_fails() {
        // Ring 3 cannot be allowed to continue after a rejected privileged request, so every
        // syscall except the two save-state phases turns a handler error into a panic.
        let dispatcher = new_dispatcher(MockOps { msr_policy: PolicyDecision::Unavailable, ..Default::default() });
        let _ = dispatcher.dispatch(&ctx(SyscallIndex::RdMsr.as_u64(), 0x1B, 0, 0));
    }

    #[test]
    #[should_panic(expected = "LegacyMax is not a real syscall")]
    fn test_dispatch_panics_on_legacy_max() {
        let dispatcher = new_dispatcher(MockOps::default());
        let _ = dispatcher.dispatch(&ctx(SyscallIndex::LegacyMax.as_u64(), 0, 0, 0));
    }

    #[test]
    fn test_dispatch_returns_status_code_for_save_state_read_errors() {
        // Both save-state phases report failures back to the caller instead of panicking. A
        // register the save state does not define fails in phase 1.
        let dispatcher = new_dispatcher(MockOps { phase1_result: Err(Status::NOT_FOUND), ..Default::default() });
        assert_eq!(
            dispatcher.dispatch(&ctx(SyscallIndex::SaveStateRead.as_u64(), 0x1000, 999, 0)),
            Ok(Status::NOT_FOUND.as_usize() as u64)
        );

        let dispatcher = new_dispatcher(MockOps { phase2_result: Err(Status::ACCESS_DENIED), ..Default::default() });
        assert_eq!(
            dispatcher.dispatch(&ctx(SyscallIndex::SaveStateRead2.as_u64(), 0x1000, 8, 0x2000)),
            Ok(Status::ACCESS_DENIED.as_usize() as u64)
        );
    }

    #[test]
    fn test_rdmsr_reads_only_after_policy_allows() {
        let dispatcher = new_dispatcher(MockOps { msr_value: 0xDEAD_BEEF, ..Default::default() });

        assert_eq!(dispatcher.handle_rdmsr(&ctx(0, 0x1B, 0, 0)), Ok(0xDEAD_BEEF));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::CheckMsr(0x1B, AccessType::Read), Effect::ReadMsr(0x1B)]);
    }

    #[test]
    fn test_rdmsr_denied_by_policy() {
        let dispatcher = new_dispatcher(MockOps {
            msr_policy: PolicyDecision::Denied(PolicyGateError::AccessDenied.into()),
            ..Default::default()
        });

        assert_eq!(dispatcher.handle_rdmsr(&ctx(0, 0x1B, 0, 0)), Err(Status::ACCESS_DENIED));
        // The MSR must not be read once the policy denies the request.
        assert_eq!(dispatcher.ops.effects(), vec![Effect::CheckMsr(0x1B, AccessType::Read)]);
    }

    #[test]
    fn test_rdmsr_without_policy_gate() {
        let dispatcher = new_dispatcher(MockOps { msr_policy: PolicyDecision::Unavailable, ..Default::default() });

        assert_eq!(dispatcher.handle_rdmsr(&ctx(0, 0x1B, 0, 0)), Err(Status::NOT_READY));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::CheckMsr(0x1B, AccessType::Read)]);
    }

    #[test]
    fn test_wrmsr_writes_only_after_policy_allows() {
        let dispatcher = new_dispatcher(MockOps::default());

        assert_eq!(dispatcher.handle_wrmsr(&ctx(0, 0x1B, 0x1234_5678_9ABC_DEF0, 0)), Ok(0));
        assert_eq!(
            dispatcher.ops.effects(),
            vec![Effect::CheckMsr(0x1B, AccessType::Write), Effect::WriteMsr(0x1B, 0x1234_5678_9ABC_DEF0)]
        );
    }

    #[test]
    fn test_msr_index_over_32_bits_is_rejected_before_the_policy_check() {
        let dispatcher = new_dispatcher(MockOps::default());
        let index = 0x1_0000_001B;

        assert_eq!(dispatcher.handle_rdmsr(&ctx(0, index, 0, 0)), Err(Status::INVALID_PARAMETER));
        assert_eq!(dispatcher.handle_wrmsr(&ctx(0, index, 0x5A, 0)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty(), "a truncated MSR index must not reach the policy gate");
    }

    #[test]
    fn test_wrmsr_denied_by_policy() {
        let dispatcher = new_dispatcher(MockOps {
            msr_policy: PolicyDecision::Denied(PolicyGateError::AccessDenied.into()),
            ..Default::default()
        });

        assert_eq!(dispatcher.handle_wrmsr(&ctx(0, 0x1B, 0x5A, 0)), Err(Status::ACCESS_DENIED));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::CheckMsr(0x1B, AccessType::Write)]);
    }

    #[test]
    fn test_privileged_instructions_run_only_after_policy_allows() {
        for instruction in [Instruction::Cli, Instruction::Wbinvd, Instruction::Hlt] {
            let dispatcher = new_dispatcher(MockOps::default());

            assert_eq!(dispatcher.handle_instruction(instruction), Ok(0));
            assert_eq!(
                dispatcher.ops.effects(),
                vec![Effect::CheckInstruction(instruction), Effect::Execute(instruction)],
                "unexpected effects for {instruction:?}"
            );
        }
    }

    #[test]
    fn test_privileged_instructions_respect_policy_denial() {
        let denied = new_dispatcher(MockOps {
            instruction_policy: PolicyDecision::Denied(PolicyGateError::AccessDenied.into()),
            ..Default::default()
        });
        assert_eq!(denied.handle_instruction(Instruction::Cli), Err(Status::ACCESS_DENIED));
        assert_eq!(denied.ops.effects(), vec![Effect::CheckInstruction(Instruction::Cli)]);

        let open_gate =
            new_dispatcher(MockOps { instruction_policy: PolicyDecision::Unavailable, ..Default::default() });
        assert_eq!(open_gate.handle_instruction(Instruction::Hlt), Err(Status::NOT_READY));
        assert_eq!(open_gate.ops.effects(), vec![Effect::CheckInstruction(Instruction::Hlt)]);
    }

    #[test]
    fn test_io_read_widths() {
        for (efi_width, width) in
            [(MM_IO_UINT8, IoWidth::Byte), (MM_IO_UINT16, IoWidth::Word), (MM_IO_UINT32, IoWidth::Dword)]
        {
            let dispatcher = new_dispatcher(MockOps { io_value: 0x1234_5678, ..Default::default() });

            assert_eq!(dispatcher.handle_io_read(&ctx(0, 0xCF8, efi_width, 0)), Ok(0x1234_5678));
            assert_eq!(
                dispatcher.ops.effects(),
                vec![Effect::CheckIo(0xCF8, width, AccessType::Read), Effect::IoRead(0xCF8, width)]
            );
        }
    }

    #[test]
    fn test_io_read_rejects_invalid_requests() {
        // Unsupported width.
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_io_read(&ctx(0, 0xCF8, 3, 0)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty());

        // Port outside the 16-bit I/O space: rejected before the policy gate sees a truncated port.
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_io_read(&ctx(0, 0x1_0000_0000, MM_IO_UINT8, 0)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty());

        // Denied by policy.
        let dispatcher = new_dispatcher(MockOps {
            io_policy: PolicyDecision::Denied(PolicyGateError::AccessDenied.into()),
            ..Default::default()
        });
        assert_eq!(dispatcher.handle_io_read(&ctx(0, 0xCF8, MM_IO_UINT8, 0)), Err(Status::ACCESS_DENIED));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::CheckIo(0xCF8, IoWidth::Byte, AccessType::Read)]);
    }

    #[test]
    fn test_io_write_widths() {
        for (efi_width, width) in
            [(MM_IO_UINT8, IoWidth::Byte), (MM_IO_UINT16, IoWidth::Word), (MM_IO_UINT32, IoWidth::Dword)]
        {
            let dispatcher = new_dispatcher(MockOps::default());

            assert_eq!(dispatcher.handle_io_write(&ctx(0, 0xCFC, efi_width, 0xAABB_CCDD)), Ok(0));
            assert_eq!(
                dispatcher.ops.effects(),
                vec![Effect::CheckIo(0xCFC, width, AccessType::Write), Effect::IoWrite(0xCFC, width, 0xAABB_CCDD)]
            );
        }
    }

    #[test]
    fn test_io_write_rejects_invalid_requests() {
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_io_write(&ctx(0, 0xCFC, 4, 0)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty());

        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_io_write(&ctx(0, 0x1_0000, MM_IO_UINT8, 0)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty());

        let dispatcher = new_dispatcher(MockOps { io_policy: PolicyDecision::Unavailable, ..Default::default() });
        assert_eq!(dispatcher.handle_io_write(&ctx(0, 0xCFC, MM_IO_UINT8, 0)), Err(Status::NOT_READY));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::CheckIo(0xCFC, IoWidth::Byte, AccessType::Write)]);
    }

    #[test]
    fn test_save_state_read_requires_protocol_pointer() {
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_save_state_read(&ctx(0, 0, 38, 1)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty());

        let dispatcher = new_dispatcher(MockOps { phase1_result: Ok(0), ..Default::default() });
        assert_eq!(dispatcher.handle_save_state_read(&ctx(0, 0x1000, 38, 1)), Ok(0));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::SaveStatePhase1(0x1000, 38, 1)]);
    }

    #[test]
    fn test_save_state_read2_requires_protocol_pointer() {
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_save_state_read2(&ctx(0, 0, 8, 0x2000)), Err(Status::INVALID_PARAMETER));
        assert!(dispatcher.ops.effects().is_empty());

        // Phase 2 failures are propagated to the caller rather than swallowed.
        let dispatcher = new_dispatcher(MockOps { phase2_result: Err(Status::NOT_FOUND), ..Default::default() });
        assert_eq!(dispatcher.handle_save_state_read2(&ctx(0, 0x1000, 8, 0x2000)), Err(Status::NOT_FOUND));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::SaveStatePhase2(0x1000, 8, 0x2000)]);
    }

    #[test]
    fn test_alloc_page_success() {
        let dispatcher = new_dispatcher(MockOps { allocate_result: Ok(0x8000), ..Default::default() });

        assert_eq!(
            dispatcher.handle_alloc_page(&ctx(0, u64::from(ALLOCATE_ANY_PAGES), u64::from(RUNTIME_SERVICES_DATA), 2)),
            Ok(0x8000)
        );
        assert_eq!(dispatcher.ops.effects(), vec![Effect::AllocateUserPages(2)]);
    }

    #[test]
    fn test_alloc_page_rejects_invalid_requests() {
        let any = u64::from(ALLOCATE_ANY_PAGES);
        let data = u64::from(RUNTIME_SERVICES_DATA);

        // Every rejected request must be refused with the documented status and must never
        // reach the page allocator, so each case checks the recorded effects of its own mock.
        let cases: [(&str, MockOps, u64, u64, u64, Status); 6] = [
            // Only the BSP may allocate, since allocation updates the page table.
            ("AP request", MockOps { is_bsp: false, ..Default::default() }, any, data, 1, Status::ACCESS_DENIED),
            // Only EfiRuntimeServicesData is allocatable by Ring 3.
            ("wrong memory type", MockOps::default(), any, data + 1, 1, Status::INVALID_PARAMETER),
            // Only AllocateAnyPages is supported.
            ("wrong allocate type", MockOps::default(), any + 1, data, 1, Status::UNSUPPORTED),
            // A zero page count is meaningless.
            ("zero pages", MockOps::default(), any, data, 0, Status::INVALID_PARAMETER),
            // Set high bits must not be dropped to make a valid 32-bit enum value.
            ("allocate type over 32 bits", MockOps::default(), any | (1 << 32), data, 1, Status::INVALID_PARAMETER),
            ("memory type over 32 bits", MockOps::default(), any, data | (1 << 32), 1, Status::INVALID_PARAMETER),
        ];

        for (name, ops, alloc_type, mem_type, page_count, expected) in cases {
            let dispatcher = new_dispatcher(ops);
            assert_eq!(
                dispatcher.handle_alloc_page(&ctx(0, alloc_type, mem_type, page_count)),
                Err(expected),
                "{name}"
            );
            assert!(dispatcher.ops.effects().is_empty(), "{name} reached the allocator");
        }
    }

    #[test]
    fn test_alloc_page_reports_allocator_failure() {
        let dispatcher =
            new_dispatcher(MockOps { allocate_result: Err(AllocError::OutOfMemory.into()), ..Default::default() });

        assert_eq!(
            dispatcher.handle_alloc_page(&ctx(0, u64::from(ALLOCATE_ANY_PAGES), u64::from(RUNTIME_SERVICES_DATA), 4)),
            Err(Status::OUT_OF_RESOURCES)
        );
        assert_eq!(dispatcher.ops.effects(), vec![Effect::AllocateUserPages(4)]);
    }

    #[test]
    fn test_free_page_success() {
        let dispatcher = new_dispatcher(MockOps::default());

        assert_eq!(dispatcher.handle_free_page(&ctx(0, 0x2000, 2, 0)), Ok(0));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::FreeUserPages(0x2000, 2)]);
    }

    #[test]
    fn test_free_page_rejects_invalid_requests() {
        // Every rejected request must be refused with the documented status and must never
        // reach the page allocator, so each case checks the recorded effects of its own mock.
        let cases: [(&str, MockOps, u64, u64, Status); 5] = [
            ("zero pages", MockOps::default(), 0x2000, 0, Status::INVALID_PARAMETER),
            ("unaligned address", MockOps::default(), 0x2001, 1, Status::INVALID_PARAMETER),
            // The range would end past the top of the address space.
            ("range past the end of memory", MockOps::default(), u64::MAX - 0xfff, 2, Status::INVALID_PARAMETER),
            // Ring 3 must not be able to free supervisor-owned memory.
            (
                "supervisor allocation",
                MockOps { allocation_type: Some(AllocationType::Supervisor), ..Default::default() },
                0x2000,
                1,
                Status::SECURITY_VIOLATION,
            ),
            // Address was never allocated.
            (
                "unallocated address",
                MockOps { allocation_type: None, ..Default::default() },
                0x2000,
                1,
                Status::INVALID_PARAMETER,
            ),
        ];

        for (name, ops, addr, page_count, expected) in cases {
            let dispatcher = new_dispatcher(ops);
            assert_eq!(dispatcher.handle_free_page(&ctx(0, addr, page_count, 0)), Err(expected), "{name}");
            assert!(dispatcher.ops.effects().is_empty(), "{name} reached the allocator");
        }
    }

    #[test]
    fn test_free_page_reports_allocator_failure() {
        // A page in the range belongs to someone else, so the checked free fails.
        let dispatcher =
            new_dispatcher(MockOps { free_result: Err(AllocError::NotAllocated.into()), ..Default::default() });

        assert_eq!(dispatcher.handle_free_page(&ctx(0, 0x2000, 3, 0)), Err(Status::SECURITY_VIOLATION));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::FreeUserPages(0x2000, 3)]);
    }

    #[test]
    fn test_start_ap_proc_success() {
        let dispatcher = new_dispatcher(MockOps {
            mapped: vec![(0x1000, PageOwnership::User), (0x3000, PageOwnership::User)],
            ..Default::default()
        });

        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 2, 0x3000)), Ok(0));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::StartAp(2, 0x1000, 0x3000)]);
    }

    #[test]
    fn test_start_ap_proc_allows_null_argument() {
        // A null argument is legitimate and must not be validated as a pointer.
        let dispatcher = new_dispatcher(MockOps { mapped: vec![(0x1000, PageOwnership::User)], ..Default::default() });

        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 1, 0)), Ok(0));
        assert_eq!(dispatcher.ops.effects(), vec![Effect::StartAp(1, 0x1000, 0)]);
    }

    #[test]
    fn test_start_ap_proc_rejects_invalid_pointers() {
        // Every rejected request must be refused with the documented status and must never be
        // dispatched to an AP, so each case checks the recorded effects of its own mock.
        let cases: [(&str, MockOps, u64, u64); 3] = [
            ("null procedure", MockOps::default(), 0, 0),
            ("unmapped procedure", MockOps::default(), 0x1000, 0),
            (
                "unmapped argument",
                MockOps { mapped: vec![(0x1000, PageOwnership::User)], ..Default::default() },
                0x1000,
                0x3000,
            ),
        ];

        for (name, ops, procedure, argument) in cases {
            let dispatcher = new_dispatcher(ops);
            assert_eq!(
                dispatcher.handle_start_ap_proc(&ctx(0, procedure, 1, argument)),
                Err(Status::INVALID_PARAMETER),
                "{name}"
            );
            assert!(dispatcher.ops.effects().is_empty(), "{name} was dispatched to an AP");
        }
    }

    #[test]
    fn test_start_ap_proc_refuses_a_caller_that_is_not_the_bsp() {
        // An AP reaching here is already running a procedure the BSP dispatched to it, so a
        // nested dispatch must not reach the mailbox at all.
        let dispatcher = new_dispatcher(MockOps {
            is_bsp: false,
            mapped: vec![(0x1000, PageOwnership::User)],
            ..Default::default()
        });

        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 1, 0)), Err(Status::ACCESS_DENIED));
        assert!(dispatcher.ops.effects().is_empty(), "a non-BSP caller was dispatched to an AP");
    }

    #[test]
    fn test_start_ap_proc_refuses_a_supervisor_owned_procedure() {
        // The procedure runs demoted on the target AP. Accepting a supervisor-owned address here
        // would let Ring 3 pick code for the supervisor to run.
        let dispatcher =
            new_dispatcher(MockOps { mapped: vec![(0x1000, PageOwnership::Supervisor)], ..Default::default() });

        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 1, 0)), Err(Status::SECURITY_VIOLATION));
        assert!(dispatcher.ops.effects().is_empty(), "a supervisor-owned procedure was dispatched to an AP");
    }

    #[test]
    fn test_start_ap_proc_refuses_a_supervisor_owned_argument() {
        // The argument reaches the same Ring 3 procedure, which cannot read supervisor memory.
        let dispatcher = new_dispatcher(MockOps {
            mapped: vec![(0x1000, PageOwnership::User), (0x3000, PageOwnership::Supervisor)],
            ..Default::default()
        });

        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 1, 0x3000)), Err(Status::SECURITY_VIOLATION));
        assert!(dispatcher.ops.effects().is_empty(), "a supervisor-owned argument was dispatched to an AP");
    }

    #[test]
    fn test_start_ap_proc_propagates_failures() {
        // AP startup has not been registered yet.
        let dispatcher = new_dispatcher(MockOps {
            mapped: vec![(0x1000, PageOwnership::User)],
            ap_status: None,
            ..Default::default()
        });
        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 1, 0)), Err(Status::NOT_READY));

        // A non-zero status from the platform is returned to the caller.
        let dispatcher = new_dispatcher(MockOps {
            mapped: vec![(0x1000, PageOwnership::User)],
            ap_status: Some(Status::NOT_FOUND.as_usize() as u64),
            ..Default::default()
        });
        assert_eq!(dispatcher.handle_start_ap_proc(&ctx(0, 0x1000, 1, 0)), Err(Status::NOT_FOUND));
    }

    #[test]
    fn test_mm_memory_unblocked_requires_unblocked_user_memory() {
        // Unblocked and user-owned: the only case that reports TRUE.
        let dispatcher = new_dispatcher(MockOps { mapped: vec![(0x5000, PageOwnership::User)], ..Default::default() });
        assert_eq!(dispatcher.handle_mm_memory_unblocked(&ctx(0, 0x5000, 0x100, 0)), Ok(1));

        // Not unblocked.
        let dispatcher = new_dispatcher(MockOps {
            unblocked: false,
            mapped: vec![(0x5000, PageOwnership::User)],
            ..Default::default()
        });
        assert_eq!(dispatcher.handle_mm_memory_unblocked(&ctx(0, 0x5000, 0x100, 0)), Ok(0));

        // Unblocked but supervisor-owned.
        let dispatcher =
            new_dispatcher(MockOps { mapped: vec![(0x5000, PageOwnership::Supervisor)], ..Default::default() });
        assert_eq!(dispatcher.handle_mm_memory_unblocked(&ctx(0, 0x5000, 0x100, 0)), Ok(0));

        // Unblocked but unmapped.
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_mm_memory_unblocked(&ctx(0, 0x5000, 0x100, 0)), Ok(0));

        // An empty buffer, or one that runs past the end of the address space, is not a range.
        let dispatcher = new_dispatcher(MockOps { mapped: vec![(0x5000, PageOwnership::User)], ..Default::default() });
        assert_eq!(dispatcher.handle_mm_memory_unblocked(&ctx(0, 0x5000, 0, 0)), Ok(0));
        assert_eq!(dispatcher.handle_mm_memory_unblocked(&ctx(0, u64::MAX, 0x100, 0)), Ok(0));
    }

    #[test]
    fn test_mm_is_comm_buffer_range_checks() {
        let config = CommBufferConfig {
            user: CommChannel { internal_address: 0x1_0000, size: 0x1000, ..Default::default() },
            ..Default::default()
        };
        let dispatcher = new_dispatcher(MockOps { comm_buffer: Some(config), ..Default::default() });

        // Fully inside, including the exact bounds of the buffer.
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0x1_0000, 0x1000, 0)), Ok(1));
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0x1_0800, 0x800, 0)), Ok(1));

        // Empty range, starting below the buffer, and running past its end.
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0x1_0000, 0, 0)), Ok(0));
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0xFFFF, 0x10, 0)), Ok(0));
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0x1_0800, 0x801, 0)), Ok(0));

        // A length that would overflow the end address must not wrap into the buffer.
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0x1_0000, u64::MAX, 0)), Ok(0));
    }

    #[test]
    fn test_mm_is_comm_buffer_without_configuration() {
        // Before the PassDown HOB is processed nothing can be a communication buffer.
        let dispatcher = new_dispatcher(MockOps::default());
        assert_eq!(dispatcher.handle_mm_is_comm_buffer(&ctx(0, 0x1_0000, 0x100, 0)), Ok(0));
    }
}
