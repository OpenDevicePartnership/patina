//! Syscall Dispatcher
//!
//! This module handles syscall requests from Ring 3 code. When Ring 3 code
//! executes the `syscall` instruction, the CPU jumps to the address in
//! `MSR_IA32_LSTAR` (our `SyscallCenter` assembly stub), which then calls into
//! this dispatcher.
//!
//! ## Syscall Interface
//!
//! The syscall uses a custom calling convention:
//! - RAX: Call index (`SyscallIndex`)
//! - RDX: Argument 1
//! - R8:  Argument 2
//! - R9:  Argument 3
//! - RCX: Caller return address (set by syscall instruction)
//! - R11: RFLAGS (set by syscall instruction)
//!
//! The dispatcher validates the request and dispatches to the appropriate handler.
//!
//! ## Security
//!
//! All syscall handlers must validate their arguments and check that any
//! memory pointers are within valid user-accessible regions.
//!
//! ## Side Effects
//!
//! Handlers never touch hardware or supervisor-global state directly: once a request has been
//! validated they delegate to the [`SyscallOps`] implementation the dispatcher was built with.
//! The supervisor uses [`FirmwareOps`]; tests substitute their own implementation.
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

// Firmware-only syscall-entry assembly; included only for the UEFI target so host builds
// (tests, doctests) can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("syscall_entry.asm"));

/// `MM_IO_UINT8` - 8-bit I/O access width.
const MM_IO_UINT8: u64 = 0;
/// `MM_IO_UINT16` - 16-bit I/O access width.
const MM_IO_UINT16: u64 = 1;
/// `MM_IO_UINT32` - 32-bit I/O access width.
const MM_IO_UINT32: u64 = 2;

/// Converts an `EFI_MM_IO_WIDTH` enum value to our [`IoWidth`] type.
///
/// The EFI spec defines: `MM_IO_UINT8=0`, `MM_IO_UINT16=1`, `MM_IO_UINT32=2`.
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
/// The x86 I/O space is 16 bits wide, so anything larger is a malformed request. Rejecting it
/// here keeps a truncated port from being validated by the policy gate.
fn io_port_from_arg(port: u64) -> Option<u16> {
    u16::try_from(port).ok()
}

/// Converts a firmware policy decision into a syscall result, logging why a request was refused.
///
/// `operation` names the syscall for the log message.
fn check_policy(operation: &str, decision: PolicyDecision) -> Result<(), Status> {
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

/// Returns the syscall name used in log messages for a privileged instruction.
fn instruction_name(instruction: Instruction) -> &'static str {
    match instruction {
        Instruction::Cli => "CLI",
        Instruction::Wbinvd => "WBINVD",
        Instruction::Hlt => "HLT",
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
    /// Caller return address (from RCX, set by syscall instruction).
    pub caller_addr: u64,
    /// Ring 3 stack pointer at syscall entry.
    pub ring3_stack_ptr: u64,
}

/// The syscall dispatcher handles incoming syscalls from Ring 3.
///
/// The dispatcher owns the [`SyscallOps`] implementation used to carry out validated requests.
/// The supervisor uses [`FirmwareOps`]; unit tests substitute a test implementation to exercise
/// the validation logic without executing privileged instructions.
pub struct SyscallDispatcher<O: SyscallOps = FirmwareOps> {
    /// Performs the privileged work a validated syscall asks for.
    ops: O,
}

impl SyscallDispatcher<FirmwareOps> {
    /// Creates a new syscall dispatcher that operates on real hardware and supervisor state.
    pub const fn new() -> Self {
        Self::with_ops(FirmwareOps)
    }
}

impl<O: SyscallOps> SyscallDispatcher<O> {
    /// Creates a syscall dispatcher backed by the given [`SyscallOps`] implementation.
    pub const fn with_ops(ops: O) -> Self {
        Self { ops }
    }

    /// Dispatches a syscall.
    ///
    /// This is the main entry point called from the assembly syscall handler.
    /// It validates the syscall index and dispatches to the appropriate handler. The result
    /// is returned to Ring 3 in RAX.
    pub fn dispatch(&self, ctx: &SyscallContext) -> SyscallResult {
        // Parse the syscall index
        let index = if let Some(index) = SyscallIndex::from_u64(ctx.call_index) {
            index
        } else {
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

        // Dispatch to the appropriate handler
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
            Err(err) if index == SyscallIndex::SaveStateRead2 => {
                log::trace!("Syscall SaveStateRead2: {:?} returned value=0x{:x}", index, err.as_usize());
                Ok(err.as_usize() as u64) // Return error code to caller for SaveStateRead2
            }
            Err(err) => {
                panic!("Syscall: {index:?} failed with error: {err:?}"); // Panic for other syscalls
            }
            _ => result,
        }
    }

    /// Handles MSR read syscall.
    ///
    /// Validates the MSR read against firmware policy, then executes `rdmsr`.
    /// - Arg1: MSR index
    /// - Returns: MSR value in result.value
    fn handle_rdmsr(&self, ctx: &SyscallContext) -> SyscallResult {
        let msr_index = ctx.arg1 as u32;
        log::trace!("RDMSR: msr=0x{msr_index:x}");

        check_policy("RDMSR", self.ops.check_msr(msr_index, AccessType::Read))?;

        // SAFETY: the policy gate authorized reading this MSR above, which is the contract of
        // `SyscallOps::read_msr`.
        let value = unsafe { self.ops.read_msr(msr_index) };
        log::debug!("RDMSR: MSR 0x{msr_index:x} = 0x{value:x}");
        Ok(value)
    }

    /// Handles MSR write syscall.
    ///
    /// Validates the MSR write against firmware policy, then executes `wrmsr`.
    /// - Arg1: MSR index
    /// - Arg2: Value to write
    fn handle_wrmsr(&self, ctx: &SyscallContext) -> SyscallResult {
        let msr_index = ctx.arg1 as u32;
        let value = ctx.arg2;
        log::trace!("WRMSR: msr=0x{msr_index:x}, value=0x{value:x}");

        check_policy("WRMSR", self.ops.check_msr(msr_index, AccessType::Write))?;

        // SAFETY: the policy gate authorized writing this MSR above, which is the contract of
        // `SyscallOps::write_msr`.
        unsafe { self.ops.write_msr(msr_index, value) };
        log::debug!("WRMSR: MSR 0x{msr_index:x} written with 0x{value:x}");
        Ok(0)
    }

    /// Handles the syscalls that ask the supervisor to execute a privileged instruction
    /// (`CLI`, `WBINVD` or `HLT`) on behalf of Ring 3.
    ///
    /// Validates the instruction against firmware policy before executing it.
    fn handle_instruction(&self, instruction: Instruction) -> SyscallResult {
        log::trace!("{instruction:?}");

        check_policy(instruction_name(instruction), self.ops.check_instruction(instruction))?;

        // SAFETY: the policy gate authorized this instruction above, which is the contract of
        // `SyscallOps::execute_instruction`.
        unsafe { self.ops.execute_instruction(instruction) };
        log::debug!("{instruction:?}: Executed");
        Ok(0)
    }

    /// Handles I/O port read syscall.
    ///
    /// Validates the I/O read against firmware policy, then executes the `in` instruction.
    /// - Arg1: I/O port address
    /// - Arg2: `EFI_MM_IO_WIDTH` (0=UINT8, 1=UINT16, 2=UINT32)
    /// - Returns: Value read from the port in result.value
    ///
    /// A port outside the 16-bit I/O space is rejected with `EFI_INVALID_PARAMETER` rather than
    /// truncated, so the supervisor never accesses a port the caller did not ask for.
    fn handle_io_read(&self, ctx: &SyscallContext) -> SyscallResult {
        let port = ctx.arg1;
        let efi_width = ctx.arg2;
        log::trace!("IO_READ: port=0x{port:x}, width={efi_width}");

        // Convert EFI_MM_IO_WIDTH to IoWidth
        let io_width = if let Some(io_width) = efi_io_width_to_io_width(efi_width) {
            io_width
        } else {
            log::error!("IO_READ: Invalid IO width: {efi_width}");
            return Err(Status::INVALID_PARAMETER);
        };

        // Reject ports outside the 16-bit I/O space before the policy gate sees them.
        let port_addr = if let Some(port_addr) = io_port_from_arg(port) {
            port_addr
        } else {
            log::error!("IO_READ: Port 0x{port:x} is outside the 16-bit I/O space");
            return Err(Status::INVALID_PARAMETER);
        };

        check_policy("IO_READ", self.ops.check_io(port_addr, io_width, AccessType::Read))?;

        // SAFETY: the policy gate authorized reading this port and width above, which is the
        // contract of `SyscallOps::io_read`.
        let value = unsafe { self.ops.io_read(port_addr, io_width) };

        log::trace!("IO_READ: port=0x{port:x} => 0x{value:x}");
        Ok(value)
    }

    /// Handles I/O port write syscall.
    ///
    /// Validates the I/O write against firmware policy, then executes the `out` instruction.
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

        // Convert EFI_MM_IO_WIDTH to IoWidth
        let io_width = if let Some(io_width) = efi_io_width_to_io_width(efi_width) {
            io_width
        } else {
            log::error!("IO_WRITE: Invalid IO width: {efi_width}");
            return Err(Status::INVALID_PARAMETER);
        };

        // Reject ports outside the 16-bit I/O space before the policy gate sees them.
        let port_addr = if let Some(port_addr) = io_port_from_arg(port) {
            port_addr
        } else {
            log::error!("IO_WRITE: Port 0x{port:x} is outside the 16-bit I/O space");
            return Err(Status::INVALID_PARAMETER);
        };

        check_policy("IO_WRITE", self.ops.check_io(port_addr, io_width, AccessType::Write))?;

        // SAFETY: the policy gate authorized writing this port and width above, which is the
        // contract of `SyscallOps::io_write`.
        unsafe { self.ops.io_write(port_addr, io_width, value) };

        log::trace!("IO_WRITE: port=0x{port:x} <= 0x{value:x}");
        Ok(0)
    }

    /// Handles save state read syscall (legacy).
    ///
    /// - Arg1: User MM CPU protocol pointer
    /// - Arg2: Register to be read (`EFI_MM_SAVE_STATE_REGISTER`)
    /// - Arg3: CPU index to read from
    fn handle_save_state_read(&self, ctx: &SyscallContext) -> SyscallResult {
        log::trace!("SAVE_STATE_READ: protocol=0x{:x}, register={}, cpu={}", ctx.arg1, ctx.arg2, ctx.arg3);

        // Validate parameters
        if ctx.arg1 == 0 {
            log::error!("SAVE_STATE_READ: Null protocol pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        // Delegate to save state module Phase 1
        self.ops.save_state_read_phase1(ctx.arg1, ctx.arg2, ctx.arg3)
    }

    /// Handles page allocation syscall.
    ///
    /// - Arg1: Allocate type (`EFI_ALLOCATE_TYPE`)
    /// - Arg2: Memory type (must be `EfiRuntimeServicesData`)
    /// - Arg3: Page count
    /// - Returns: Allocated physical address in result.value
    fn handle_alloc_page(&self, ctx: &SyscallContext) -> SyscallResult {
        let alloc_type = ctx.arg1 as AllocateType;
        let mem_type = ctx.arg2 as MemoryType;
        let page_count = ctx.arg3;
        log::trace!("ALLOC_PAGE: alloc_type={alloc_type}, mem_type={mem_type}, count={page_count}");

        // Only BSP can allocate pages (AP allocating involves page table updates)
        if !self.ops.is_bsp() {
            log::error!("ALLOC_PAGE: AP cannot allocate pages");
            return Err(Status::ACCESS_DENIED);
        }

        if mem_type != RUNTIME_SERVICES_DATA {
            log::error!("ALLOC_PAGE: Invalid memory type: {mem_type}");
            return Err(Status::INVALID_PARAMETER);
        }

        // Currently only AllocateAnyPages is supported by our page allocator
        if alloc_type != ALLOCATE_ANY_PAGES {
            log::error!("ALLOC_PAGE: Only AllocateAnyPages (0) is supported, got {alloc_type}");
            return Err(Status::UNSUPPORTED);
        }

        if page_count == 0 {
            log::error!("ALLOC_PAGE: Zero page count");
            return Err(Status::INVALID_PARAMETER);
        }

        // Allocate pages as User type (Ring 3 driver request)
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

    /// Handles page free syscall.
    ///
    /// Mirrors the C implementation's `SMM_FREE_PAGE` case.
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

        // Validate the address is page-aligned
        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("FREE_PAGE: Address 0x{addr:x} is not page-aligned");
            return Err(Status::INVALID_PARAMETER);
        }

        // Verify the range was allocated as User type (Ring 3 code should only free its own memory)
        // This prevents user code from freeing supervisor-internal allocations.
        match self.ops.allocation_type(addr) {
            Some(crate::memory::AllocationType::User) => {
                // Good - this is user-owned memory
            }
            Some(crate::memory::AllocationType::Supervisor) => {
                log::error!("FREE_PAGE: Address 0x{addr:x} is a supervisor allocation - access denied");
                return Err(Status::SECURITY_VIOLATION);
            }
            None => {
                log::error!("FREE_PAGE: Address 0x{addr:x} is not allocated");
                return Err(Status::INVALID_PARAMETER);
            }
        }

        // Free the pages, verifying they are all User allocations
        match self.ops.free_user_pages(addr, page_count as usize) {
            Ok(()) => {
                log::debug!("FREE_PAGE: Freed {range}");
                Ok(0)
            }
            Err(err) => {
                log::error!("FREE_PAGE: Free failed: {err:?}");
                Err(Status::SECURITY_VIOLATION)
            }
        }
    }

    /// Handles start AP procedure syscall.
    ///
    /// Validates the request and delegates to the platform-specific AP startup
    /// function registered during [`MmSupervisorCore`] initialization.
    ///
    /// Checks performed before dispatch:
    /// - Caller is the BSP
    /// - Procedure pointer is non-null and user-owned
    /// - Argument pointer (if non-null) is user-owned
    ///
    /// The remaining validation (CPU index range, BSP check, AP busy check) and
    /// the actual dispatch are handled by the registered AP startup function,
    /// which has access to the CPU manager and mailbox manager.
    ///
    /// - Arg1: Procedure function pointer
    /// - Arg2: CPU index
    /// - Arg3: Argument pointer
    fn handle_start_ap_proc(&self, ctx: &SyscallContext) -> SyscallResult {
        let procedure = ctx.arg1;
        let cpu_index = ctx.arg2;
        let argument = ctx.arg3;

        log::debug!("START_AP_PROC: proc=0x{procedure:x}, cpu={cpu_index}, arg=0x{argument:x}");

        // 1. Caller is the BSP. APs poll their mailbox for work the BSP dispatches, so an AP
        //    reaching here is running a procedure already dispatched to it. Letting it dispatch
        //    in turn would nest the MP state machine: it could contend for a mailbox the BSP is
        //    filling, target itself and then spin the full AP timeout waiting for a response it
        //    cannot post, or leave a second AP busy while the BSP believes every AP is penned.
        if !self.ops.is_bsp() {
            log::error!("START_AP_PROC: only the BSP may dispatch a procedure to an AP");
            return Err(Status::ACCESS_DENIED);
        }

        // 2. Validate procedure pointer is non-null
        if procedure == 0 {
            log::error!("START_AP_PROC: Null procedure pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        // 3. The procedure runs demoted on the target AP, so it has to be user-owned. Accepting
        //    any mapped address would let Ring 3 name a supervisor address to be run for it.
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

        // 5. Delegate to the registered AP startup function
        match self.ops.start_ap_procedure(cpu_index, procedure, argument) {
            Some(0) => Ok(0),
            Some(status) => Err(Status::from_usize(status as usize)),
            None => {
                log::error!("START_AP_PROC: AP startup not initialized");
                Err(Status::NOT_READY)
            }
        }
    }

    /// Handles extended save state read syscall.
    ///
    /// - Arg1: User MM CPU protocol pointer
    /// - Arg2: Width of buffer to read in bytes
    /// - Arg3: User buffer to hold return data
    fn handle_save_state_read2(&self, ctx: &SyscallContext) -> SyscallResult {
        // Validate parameters
        if ctx.arg1 == 0 {
            log::error!("SAVE_STATE_READ2: Null protocol pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        // Delegate to save state module Phase 2
        self.ops.save_state_read_phase2(ctx.arg1, ctx.arg2, ctx.arg3)
    }

    /// Handles MM memory unblocked check syscall.
    ///
    /// Checks if a memory range is outside MMRAM and valid (unblocked), AND
    /// is within user-owned space.
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

        // Check if the buffer is within an unblocked memory region
        let is_valid = self.ops.is_within_unblocked_region(&range);

        if !is_valid {
            log::trace!("MM_MEMORY_UNBLOCKED: {range} not in unblocked region");
            return Ok(0); // FALSE
        }

        // Additional check - verify buffer is in user-owned space
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

    /// Handles MM is communication buffer check syscall.
    ///
    /// Verifies that a given memory range is a valid communication buffer.
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

        let config = if let Some(comm_buffer_config) = self.ops.comm_buffer_config() {
            comm_buffer_config
        } else {
            log::error!("MM_IS_COMM_BUFFER: Comm buffer config not initialized");
            return Ok(0); // FALSE
        };

        // The range must lie entirely within the user comm buffer.
        let is_valid =
            MemoryRange::new(config.user.internal, config.user.size).is_ok_and(|buffer| buffer.contains_range(&range));

        log::debug!("MM_IS_COMM_BUFFER: {range} => {is_valid}");
        if is_valid { Ok(1) } else { Ok(0) }
    }
}

/// C-compatible syscall dispatcher entry point.
///
/// This function is called from the assembly syscall entry stub (`SyscallCenter`). Its parameters
/// carry the syscall registers described by the module-level calling convention, and its return
/// value is the result placed in RAX for the Ring 3 caller.
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

    // Unwrap is safe here because the dispatch() will always return a u64
    // result, and panic on failure.
    SyscallDispatcher::new().dispatch(&ctx).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syscall_index_roundtrip() {
        for idx in
            [SyscallIndex::RdMsr, SyscallIndex::WrMsr, SyscallIndex::Cli, SyscallIndex::IoRead, SyscallIndex::IoWrite]
        {
            assert_eq!(SyscallIndex::from_u64(idx.as_u64()), Some(idx));
        }
    }

    #[test]
    fn test_unknown_syscall_index() {
        // Values that fall in the gaps between defined indices map to `None`.
        assert_eq!(SyscallIndex::from_u64(0x0008), None);
        assert_eq!(SyscallIndex::from_u64(0x10000), None);
        assert_eq!(SyscallIndex::from_u64(0xDEAD_BEEF), None);
    }
}
