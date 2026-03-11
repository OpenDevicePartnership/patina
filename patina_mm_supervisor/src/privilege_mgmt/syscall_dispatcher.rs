//! Syscall Dispatcher
//!
//! This module handles syscall requests from Ring 3 code. When Ring 3 code
//! executes the `syscall` instruction, the CPU jumps to the address in
//! MSR_IA32_LSTAR (our SyscallCenter assembly stub), which then calls into
//! this dispatcher.
//!
//! ## Syscall Interface
//!
//! The syscall uses a custom calling convention:
//! - RAX: Call index (SyscallIndex)
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
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::arch::asm;
use r_efi::efi::{ALLOCATE_ANY_PAGES, AllocateType, MemoryType, RUNTIME_SERVICES_DATA};

use crate::mm_policy::{AccessType, Instruction, IoWidth};
use patina::{base::UEFI_PAGE_SIZE, management_mode::supervisor::SyscallIndex};

use crate::{
    PageOwnership, query_address_ownership,
    state::{init_state, security_state},
};
use r_efi::efi::Status;

use super::SyscallResult;

// Firmware-only syscall-entry assembly; included only for the UEFI target so host builds
// (tests, doctests) can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("syscall_entry.asm"));

/// MM_IO_UINT8 - 8-bit I/O access width.
const MM_IO_UINT8: u64 = 0;
/// MM_IO_UINT16 - 16-bit I/O access width.
const MM_IO_UINT16: u64 = 1;
/// MM_IO_UINT32 - 32-bit I/O access width.
const MM_IO_UINT32: u64 = 2;

/// Converts an EFI_MM_IO_WIDTH enum value to our [`IoWidth`] type.
///
/// The EFI spec defines: MM_IO_UINT8=0, MM_IO_UINT16=1, MM_IO_UINT32=2.
fn efi_io_width_to_io_width(width: u64) -> Option<IoWidth> {
    match width {
        MM_IO_UINT8 => Some(IoWidth::Byte),
        MM_IO_UINT16 => Some(IoWidth::Word),
        MM_IO_UINT32 => Some(IoWidth::Dword),
        _ => None,
    }
}

/// Reads an 8-bit value from an I/O port.
///
/// ## Safety
///
/// IO port operation is unsafe and could have unexpected side effects.
/// The caller must ensure the port address is valid.
#[inline]
unsafe fn io_read_u8(port: u16) -> u8 {
    let value: u8;
    // SAFETY: IO operation is genuinely unsafe.
    // But `in al, dx` reads only the caller-specified I/O port (valid per this function's
    // contract) and accesses no memory (nomem, nostack).
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack));
    }
    value
}

/// Reads a 16-bit value from an I/O port.
///
/// ## Safety
///
/// IO port operation is unsafe and could have unexpected side effects.
/// The caller must ensure the port address is valid.
#[inline]
unsafe fn io_read_u16(port: u16) -> u16 {
    let value: u16;
    // SAFETY: IO operation is genuinely unsafe.
    // `in ax, dx` reads only the caller-specified I/O port (valid per this function's
    // contract) and accesses no memory (nomem, nostack).
    unsafe {
        asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack));
    }
    value
}

/// Reads a 32-bit value from an I/O port.
///
/// ## Safety
///
/// IO port operation is unsafe and could have unexpected side effects.
/// The caller must ensure the port address is valid.
#[inline]
unsafe fn io_read_u32(port: u16) -> u32 {
    let value: u32;
    // SAFETY: IO operation is genuinely unsafe.
    // `in eax, dx` reads only the caller-specified I/O port (valid per this function's
    // contract) and accesses no memory (nomem, nostack).
    unsafe {
        asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack));
    }
    value
}

/// Writes an 8-bit value to an I/O port.
///
/// ## Safety
///
/// IO port operation is unsafe and could have unexpected side effects.
/// The caller must ensure the port address is valid.
#[inline]
unsafe fn io_write_u8(port: u16, value: u8) {
    // SAFETY: IO operation is genuinely unsafe.
    // `out dx, al` writes only the caller-specified I/O port (valid per this function's
    // contract) and accesses no memory (nomem, nostack).
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

/// Writes a 16-bit value to an I/O port.
///
/// ## Safety
///
/// IO port operation is unsafe and could have unexpected side effects.
/// The caller must ensure the port address is valid.
#[inline]
unsafe fn io_write_u16(port: u16, value: u16) {
    // SAFETY: IO operation is genuinely unsafe.
    // `out dx, ax` writes only the caller-specified I/O port (valid per this function's
    // contract) and accesses no memory (nomem, nostack).
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack));
    }
}

/// Writes a 32-bit value to an I/O port.
///
/// ## Safety
///
/// IO port operation is unsafe and could have unexpected side effects.
/// The caller must ensure the port address is valid.
#[inline]
unsafe fn io_write_u32(port: u16, value: u32) {
    // SAFETY: IO operation is genuinely unsafe.
    // `out dx, eax` writes only the caller-specified I/O port (valid per this function's
    // contract) and accesses no memory (nomem, nostack).
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack));
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
pub struct SyscallDispatcher;

impl SyscallDispatcher {
    /// Creates a new syscall dispatcher.
    pub const fn new() -> Self {
        Self
    }

    /// Dispatches a syscall.
    ///
    /// This is the main entry point called from the assembly syscall handler.
    /// It validates the syscall index and dispatches to the appropriate handler. The result
    /// is returned to Ring 3 in RAX.
    pub fn dispatch(&self, ctx: &SyscallContext) -> SyscallResult {
        // Parse the syscall index
        let index = match SyscallIndex::from_u64(ctx.call_index) {
            Some(idx) => idx,
            None => {
                log::error!("Unknown syscall index: 0x{:x}", ctx.call_index);
                return Err(Status::UNSUPPORTED);
            }
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
            SyscallIndex::Cli => self.handle_cli(ctx),
            SyscallIndex::IoRead => self.handle_io_read(ctx),
            SyscallIndex::IoWrite => self.handle_io_write(ctx),
            SyscallIndex::Wbinvd => self.handle_wbinvd(ctx),
            SyscallIndex::Hlt => self.handle_hlt(ctx),
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
                log::error!("Syscall SaveStateRead2: {:?} returned value=0x{:x}", index, err.as_usize());
                Ok(err.as_usize() as u64) // Return error code to caller for SaveStateRead2
            }
            Err(err) => {
                panic!("Syscall: {:?} failed with error: {:?}", index, err); // Panic for other syscalls
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
        log::trace!("RDMSR: msr=0x{:x}", msr_index);

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("RDMSR: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_msr_allowed(msr_index, AccessType::Read) {
            log::error!("RDMSR: MSR 0x{:x} blocked by policy: {:?}", msr_index, e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - execute the MSR read
        // SAFETY: the policy gate authorized reading this MSR above; `read_msr` only issues
        // `rdmsr` for `msr_index` and returns its value.
        let value = unsafe { crate::cpu::read_msr(msr_index) }.unwrap_or_else(|e| {
            log::error!("RDMSR: rdmsr failed: {}", e);
            0
        });
        log::debug!("RDMSR: MSR 0x{:x} = 0x{:x}", msr_index, value);
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
        log::trace!("WRMSR: msr=0x{:x}, value=0x{:x}", msr_index, value);

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("WRMSR: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_msr_allowed(msr_index, AccessType::Write) {
            log::error!("WRMSR: MSR 0x{:x} blocked by policy: {:?}", msr_index, e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - execute the MSR write
        // SAFETY: the policy gate authorized writing this MSR above; `write_msr` only issues
        // `wrmsr` for `msr_index` with the requested value.
        if let Err(e) = unsafe { crate::cpu::write_msr(msr_index, value) } {
            log::error!("WRMSR: wrmsr failed: {}", e);
            return Err(Status::UNSUPPORTED);
        }
        log::debug!("WRMSR: MSR 0x{:x} written with 0x{:x}", msr_index, value);
        Ok(0)
    }

    /// Handles CLI (clear interrupt flag) syscall.
    ///
    /// Validates the CLI instruction against firmware policy, then executes `cli`.
    fn handle_cli(&self, _ctx: &SyscallContext) -> SyscallResult {
        log::trace!("CLI");

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("CLI: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_instruction_allowed(Instruction::Cli) {
            log::error!("CLI: Instruction blocked by policy: {:?}", e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - disable interrupts
        // SAFETY: the policy gate authorized `CLI` above; the instruction only clears the interrupt
        // flag and touches no memory (nomem, nostack).
        unsafe { asm!("cli", options(nomem, nostack)) };
        log::debug!("CLI: Interrupts disabled");
        Ok(0)
    }

    /// Handles I/O port read syscall.
    ///
    /// Validates the I/O read against firmware policy, then executes the `in` instruction.
    /// - Arg1: I/O port address
    /// - Arg2: EFI_MM_IO_WIDTH (0=UINT8, 1=UINT16, 2=UINT32)
    /// - Returns: Value read from the port in result.value
    fn handle_io_read(&self, ctx: &SyscallContext) -> SyscallResult {
        let port = ctx.arg1;
        let efi_width = ctx.arg2;
        log::trace!("IO_READ: port=0x{:x}, width={}", port, efi_width);

        // Convert EFI_MM_IO_WIDTH to IoWidth
        let io_width = match efi_io_width_to_io_width(efi_width) {
            Some(w) => w,
            None => {
                log::error!("IO_READ: Invalid IO width: {}", efi_width);
                return Err(Status::INVALID_PARAMETER);
            }
        };

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("IO_READ: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_io_allowed(port as u32, io_width, AccessType::Read) {
            log::error!("IO_READ: Port 0x{:x} width {:?} blocked by policy: {:?}", port, io_width, e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - execute the I/O read
        let port_addr = port as u16;
        // SAFETY: the policy gate authorized reading this port/width above, and `efi_width` was
        // validated to a supported width, so the matched `io_read_*` call targets a permitted port.
        let value: u64 = unsafe {
            match efi_width {
                MM_IO_UINT8 => io_read_u8(port_addr) as u64,
                MM_IO_UINT16 => io_read_u16(port_addr) as u64,
                MM_IO_UINT32 => io_read_u32(port_addr) as u64,
                _ => unreachable!(), // Already validated above
            }
        };

        log::trace!("IO_READ: port=0x{:x} => 0x{:x}", port, value);
        Ok(value)
    }

    /// Handles I/O port write syscall.
    ///
    /// Validates the I/O write against firmware policy, then executes the `out` instruction.
    /// - Arg1: I/O port address
    /// - Arg2: EFI_MM_IO_WIDTH (0=UINT8, 1=UINT16, 2=UINT32)
    /// - Arg3: Value to write
    fn handle_io_write(&self, ctx: &SyscallContext) -> SyscallResult {
        let port = ctx.arg1;
        let efi_width = ctx.arg2;
        let value = ctx.arg3;
        log::trace!("IO_WRITE: port=0x{:x}, width={}, value=0x{:x}", port, efi_width, value);

        // Convert EFI_MM_IO_WIDTH to IoWidth
        let io_width = match efi_io_width_to_io_width(efi_width) {
            Some(w) => w,
            None => {
                log::error!("IO_WRITE: Invalid IO width: {}", efi_width);
                return Err(Status::INVALID_PARAMETER);
            }
        };

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("IO_WRITE: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_io_allowed(port as u32, io_width, AccessType::Write) {
            log::error!("IO_WRITE: Port 0x{:x} width {:?} blocked by policy: {:?}", port, io_width, e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - execute the I/O write
        let port_addr = port as u16;
        // SAFETY: the policy gate authorized writing this port/width above, and `efi_width` was
        // validated to a supported width, so the matched `io_write_*` call targets a permitted port.
        unsafe {
            match efi_width {
                MM_IO_UINT8 => io_write_u8(port_addr, value as u8),
                MM_IO_UINT16 => io_write_u16(port_addr, value as u16),
                MM_IO_UINT32 => io_write_u32(port_addr, value as u32),
                _ => unreachable!(), // Already validated above
            }
        }

        log::trace!("IO_WRITE: port=0x{:x} <= 0x{:x}", port, value);
        Ok(0)
    }

    /// Handles WBINVD (write-back and invalidate cache) syscall.
    ///
    /// Validates the WBINVD instruction against firmware policy, then executes `wbinvd`.
    fn handle_wbinvd(&self, _ctx: &SyscallContext) -> SyscallResult {
        log::trace!("WBINVD");

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("WBINVD: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_instruction_allowed(Instruction::Wbinvd) {
            log::error!("WBINVD: Instruction blocked by policy: {:?}", e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - write back and invalidate cache
        // SAFETY: the policy gate authorized `WBINVD` above; the instruction only writes back and
        // invalidates caches and touches no memory (nomem, nostack).
        unsafe { asm!("wbinvd", options(nomem, nostack)) };
        log::debug!("WBINVD: Cache written back and invalidated");
        Ok(0)
    }

    /// Handles HLT (halt processor) syscall.
    ///
    /// Validates the HLT instruction against firmware policy, then executes `hlt`.
    fn handle_hlt(&self, _ctx: &SyscallContext) -> SyscallResult {
        log::trace!("HLT");

        // Validate against policy
        let gate = match security_state().policy_gate() {
            Some(g) => g,
            None => {
                log::error!("HLT: Policy gate not initialized");
                return Err(Status::NOT_READY);
            }
        };

        if let Err(e) = gate.is_instruction_allowed(Instruction::Hlt) {
            log::error!("HLT: Instruction blocked by policy: {:?}", e);
            return Err(Status::ACCESS_DENIED);
        }

        // Policy allows - halt processor (sleep until next interrupt)
        // SAFETY: the policy gate authorized `HLT` above; the instruction only halts until the next
        // interrupt and touches no memory (nomem, nostack).
        unsafe { asm!("hlt", options(nomem, nostack)) };
        log::debug!("HLT: Processor halted and resumed");
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
        crate::save_state::save_state_read_phase1(ctx.arg1, ctx.arg2, ctx.arg3)
    }

    /// Handles page allocation syscall.
    ///
    /// - Arg1: Allocate type (EFI_ALLOCATE_TYPE)
    /// - Arg2: Memory type (must be EfiRuntimeServicesData)
    /// - Arg3: Page count
    /// - Returns: Allocated physical address in result.value
    fn handle_alloc_page(&self, ctx: &SyscallContext) -> SyscallResult {
        let alloc_type = ctx.arg1 as AllocateType;
        let mem_type = ctx.arg2 as MemoryType;
        let page_count = ctx.arg3;
        log::trace!("ALLOC_PAGE: alloc_type={}, mem_type={}, count={}", alloc_type, mem_type, page_count);

        // Only BSP can allocate pages (AP allocating involves page table updates)
        if !crate::is_bsp() {
            log::error!("ALLOC_PAGE: AP cannot allocate pages");
            return Err(Status::ACCESS_DENIED);
        }

        if mem_type != RUNTIME_SERVICES_DATA {
            log::error!("ALLOC_PAGE: Invalid memory type: {}", mem_type);
            return Err(Status::INVALID_PARAMETER);
        }

        // Currently only AllocateAnyPages is supported by our page allocator
        if alloc_type != ALLOCATE_ANY_PAGES {
            log::error!("ALLOC_PAGE: Only AllocateAnyPages (0) is supported, got {}", alloc_type);
            return Err(Status::UNSUPPORTED);
        }

        if page_count == 0 {
            log::error!("ALLOC_PAGE: Zero page count");
            return Err(Status::INVALID_PARAMETER);
        }

        // Allocate pages as User type (Ring 3 driver request)
        match security_state()
            .page_allocator()
            .allocate_pages_with_type(page_count as usize, crate::mem::AllocationType::User)
        {
            Ok(addr) => {
                log::trace!("ALLOC_PAGE: Allocated {} page(s) at 0x{:x}", page_count, addr);
                Ok(addr)
            }
            Err(e) => {
                log::error!("ALLOC_PAGE: Allocation failed: {:?}", e);
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
        let addr = ctx.arg1;
        let page_count = ctx.arg2;
        log::trace!("FREE_PAGE: addr=0x{:x}, count={}", addr, page_count);

        if page_count == 0 {
            log::error!("FREE_PAGE: Zero page count");
            return Err(Status::INVALID_PARAMETER);
        }

        // Validate the address is page-aligned
        if !addr.is_multiple_of(UEFI_PAGE_SIZE as u64) {
            log::error!("FREE_PAGE: Address 0x{:x} is not page-aligned", addr);
            return Err(Status::INVALID_PARAMETER);
        }

        // Verify the range was allocated as User type (Ring 3 code should only free its own memory)
        // This prevents user code from freeing supervisor-internal allocations.
        match security_state().page_allocator().get_allocation_type(addr) {
            Some(crate::mem::AllocationType::User) => {
                // Good - this is user-owned memory
            }
            Some(crate::mem::AllocationType::Supervisor) => {
                log::error!("FREE_PAGE: Address 0x{:x} is a supervisor allocation - access denied", addr);
                return Err(Status::SECURITY_VIOLATION);
            }
            None => {
                log::error!("FREE_PAGE: Address 0x{:x} is not allocated", addr);
                return Err(Status::INVALID_PARAMETER);
            }
        }

        // Free the pages, verifying they are all User allocations
        match security_state().page_allocator().free_pages_checked(
            addr,
            page_count as usize,
            crate::mem::AllocationType::User,
        ) {
            Ok(()) => {
                log::debug!("FREE_PAGE: Freed {} page(s) at 0x{:x}", page_count, addr);
                Ok(0)
            }
            Err(e) => {
                log::error!("FREE_PAGE: Free failed: {:?}", e);
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
    /// - Procedure pointer is non-null
    /// - Procedure pointer is within user-accessible memory (unblocked region)
    /// - Argument pointer (if non-null) is within user-accessible memory
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

        log::info!("START_AP_PROC: proc=0x{:x}, cpu={}, arg=0x{:x}", procedure, cpu_index, argument);

        // 1. Validate procedure pointer is non-null
        if procedure == 0 {
            log::error!("START_AP_PROC: Null procedure pointer");
            return Err(Status::INVALID_PARAMETER);
        }

        // 2. Validate procedure pointer is within mapped memory via page table query
        if crate::query_address_ownership(procedure, core::mem::size_of::<usize>() as u64).is_none() {
            log::error!("START_AP_PROC: Procedure 0x{:x} not in mapped memory", procedure);
            return Err(Status::INVALID_PARAMETER);
        }

        // 3. Validate argument pointer (if non-null) is within mapped memory
        if argument != 0 && crate::query_address_ownership(argument, core::mem::size_of::<usize>() as u64).is_none() {
            log::error!("START_AP_PROC: Argument 0x{:x} not in mapped memory", argument);
            return Err(Status::INVALID_PARAMETER);
        }

        // 4. Delegate to the registered AP startup function
        match init_state().ap_startup_fn() {
            Some(start_fn) => {
                log::info!(
                    "START_AP_PROC: Dispatching to AP startup function at {:p} for CPU {}",
                    start_fn as *const (),
                    cpu_index
                );
                let status = start_fn(cpu_index, procedure, argument);
                if status == 0 { Ok(0) } else { Err(Status::from_usize(status as usize)) }
            }
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
        crate::save_state::save_state_read_phase2(ctx.arg1, ctx.arg2, ctx.arg3)
    }

    /// Handles MM memory unblocked check syscall.
    ///
    /// Checks if a memory range is outside MMRAM and valid (unblocked), AND
    /// is within user-owned space.
    /// - Arg1: Physical address
    /// - Arg2: Size in bytes
    /// - Returns: 1 (TRUE) if valid, 0 (FALSE) otherwise
    fn handle_mm_memory_unblocked(&self, ctx: &SyscallContext) -> SyscallResult {
        let addr = ctx.arg1;
        let size = ctx.arg2;
        log::trace!("MM_MEMORY_UNBLOCKED: addr=0x{:x}, size=0x{:x}", addr, size);

        // Check if the buffer is within an unblocked memory region
        let is_valid = security_state().unblocked_tracker().is_within_unblocked_region(addr, size);

        if !is_valid {
            log::trace!("MM_MEMORY_UNBLOCKED: addr=0x{:x} size=0x{:x} not in unblocked region", addr, size);
            return Ok(0); // FALSE
        }

        // Additional check - verify buffer is in user-owned space
        match query_address_ownership(addr, size) {
            Some(owner) => {
                if owner != PageOwnership::User {
                    log::trace!(
                        "MM_MEMORY_UNBLOCKED: addr=0x{:x} size=0x{:x} owned by {:?} - not valid",
                        addr,
                        size,
                        owner
                    );
                    return Ok(0); // FALSE
                }
            }
            None => {
                log::trace!("MM_MEMORY_UNBLOCKED: addr=0x{:x} size=0x{:x} not in mapped memory", addr, size);
                return Ok(0); // FALSE
            }
        }

        log::trace!("MM_MEMORY_UNBLOCKED: addr=0x{:x} size=0x{:x} is valid", addr, size);
        Ok(1) // TRUE
    }

    /// Handles MM is communication buffer check syscall.
    ///
    /// Verifies that a given memory range is a valid communication buffer.
    /// - Arg1: Buffer address
    /// - Arg2: Buffer size
    /// - Returns: 1 (TRUE) if valid comm buffer, 0 (FALSE) otherwise
    fn handle_mm_is_comm_buffer(&self, ctx: &SyscallContext) -> SyscallResult {
        let address = ctx.arg1;
        let size = ctx.arg2;
        log::trace!("MM_IS_COMM_BUFFER: addr=0x{:x}, size=0x{:x}", address, size);

        let config = match security_state().comm_buffer_config() {
            Some(c) => c,
            None => {
                log::error!("MM_IS_COMM_BUFFER: Comm buffer config not initialized");
                return Ok(0); // FALSE
            }
        };

        let buf_start = config.user_comm_buffer_internal;
        let buf_end = buf_start.saturating_add(config.user_comm_buffer_size);
        let range_end = address.saturating_add(size);

        // Check that the range is non-empty and falls entirely within the user comm buffer.
        let is_valid = size > 0 && address >= buf_start && range_end <= buf_end;

        log::debug!("MM_IS_COMM_BUFFER: addr=0x{:x} size=0x{:x} => {}", address, size, is_valid);
        if is_valid { Ok(1) } else { Ok(0) }
    }
}

/// C-compatible syscall dispatcher entry point.
///
/// This function is called from the assembly syscall entry stub (SyscallCenter). Its parameters
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
