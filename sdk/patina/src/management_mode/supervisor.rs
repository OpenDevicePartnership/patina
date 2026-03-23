//! Shared type definitions for MM supervisor and user cores.
//!
//! This module provides the communication structures and enumerations that define
//! the ABI between the supervisor (ring 0) and user (ring 3) MM modules.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// GUID for gMmSupervisorHobMemoryAllocModuleGuid
// { 0x3efafe72, 0x3dbf, 0x4341, { 0xad, 0x04, 0x1c, 0xb6, 0xe8, 0xb6, 0x8e, 0x5e }}
/// GUID used in `MemoryAllocationModule` HOBs to identify MM Supervisor module allocations.
pub const MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("3efafe72-3dbf-4341-ad04-1cb6e8b68e5e");

// GUID for gMmSupervisorUserGuid
// { 0x30d1cc3f, 0xc1db, 0x41ed, { 0xb1, 0x13, 0xab, 0xce, 0x21, 0xb0, 0x2b, 0xce }}
/// GUID identifying the MM Supervisor User module.
pub const MM_SUPERVISOR_USER_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("30d1cc3f-c1db-41ed-b113-abce21b02bce");

// GUID for gMmSupervisorCoreGuid
// { 0x4e4c89dc, 0xa452, 0x4b6b, { 0xb1, 0x83, 0xf1, 0x6a, 0x2a, 0x22, 0x37, 0x33 }}
/// GUID identifying the MM Supervisor Core module.
pub const MM_SUPERVISOR_CORE_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("4e4c89dc-a452-4b6b-b183-f16a2a223733");

// GUID for gMmSupervisorInitGuid
// { 0xA41AD2F6, 0x78D4, 0x47C9, { 0xAE, 0xF1, 0xB6, 0x23, 0xD0, 0x73, 0x4D, 0x63 }}
/// GUID identifying the MM Supervisor Init module.
pub const MM_SUPERVISOR_INIT_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("A41AD2F6-78D4-47C9-AEF1-B623D0734D63");

/// Command types passed from the supervisor to the user core via `invoke_demoted_routine`.
///
/// Discriminant values are part of the ABI between the supervisor and the user core and must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum UserCommandType {
    /// Initialize the user core: walk HOBs, discover drivers, dispatch.
    StartUserCore = 0,
    /// Handle a runtime MMI request: parse communication buffer and dispatch handlers.
    UserRequest = 1,
    /// Execute a procedure on an AP.
    UserApProcedure = 2,
}

impl TryFrom<u64> for UserCommandType {
    type Error = u64;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(UserCommandType::StartUserCore),
            1 => Ok(UserCommandType::UserRequest),
            2 => Ok(UserCommandType::UserApProcedure),
            other => Err(other),
        }
    }
}

/// Syscall indices for the syscall interface between the MM Supervisor and the User Core.
///
/// These match the definitions in SysCallLib.h and define the ABI used when
/// Ring 3 code issues a `syscall` instruction to the Ring 0 supervisor.
///
/// ## ABI
///
/// RAX holds the call index ([`SyscallIndex`]), and RDX, R8, and R9 hold arg1, arg2,
/// and arg3. On return, RAX holds the result value. The supervisor returns data only
/// through RAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallIndex {
    /// Reads an MSR. Arg1 is the MSR index. Returns the MSR value.
    RdMsr = 0x0000,
    /// Writes an MSR. Arg1 is the MSR index and arg2 is the value.
    WrMsr = 0x0001,
    /// Clears interrupts (CLI).
    Cli = 0x0002,
    /// Reads an I/O port. Arg1 is the port and arg2 is the width.
    IoRead = 0x0003,
    /// Writes an I/O port. Arg1 is the port, arg2 is the width, and arg3 is the value.
    IoWrite = 0x0004,
    /// Writes back and invalidates the cache (WBINVD).
    Wbinvd = 0x0005,
    /// Halts the processor (HLT).
    Hlt = 0x0006,
    /// Starts a save-state read. Arg1 is the caller's protocol pointer, arg2 is the register, and
    /// arg3 is the CPU index.
    SaveStateRead = 0x0007,
    /// Highest legacy syscall index.
    LegacyMax = 0xFFFF,
    /// Allocates pages. Arg1 is the `alloc_type`, arg2 is the `mem_type`, and arg3 is the `page_count`.
    AllocPage = 0x10004,
    /// Frees pages. Arg1 is the address and arg2 is the `page_count`.
    FreePage = 0x10005,
    /// Starts a procedure on an AP. Arg1 is the procedure, arg2 is the CPU index, and arg3 is the
    /// argument.
    StartApProc = 0x10006,
    /// Completes a save-state read started by `SaveStateRead`. Arg1 is the caller's protocol
    /// pointer, arg2 is the width, and arg3 is the output buffer pointer.
    SaveStateRead2 = 0x10021,
    /// Checks whether a memory range is unblocked for MM. Arg1 is the address and arg2 is the size.
    MmMemoryUnblocked = 0x10022,
    /// Checks whether a memory range is an MM communication buffer. Arg1 is the address and arg2
    /// is the size.
    MmIsCommBuffer = 0x10023,
}

impl TryFrom<u64> for SyscallIndex {
    type Error = u64;

    /// Converts a raw call index, returning the value back as the error if it is not a known
    /// syscall.
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0x0000 => Ok(Self::RdMsr),
            0x0001 => Ok(Self::WrMsr),
            0x0002 => Ok(Self::Cli),
            0x0003 => Ok(Self::IoRead),
            0x0004 => Ok(Self::IoWrite),
            0x0005 => Ok(Self::Wbinvd),
            0x0006 => Ok(Self::Hlt),
            0x0007 => Ok(Self::SaveStateRead),
            0xFFFF => Ok(Self::LegacyMax),
            0x10004 => Ok(Self::AllocPage),
            0x10005 => Ok(Self::FreePage),
            0x10006 => Ok(Self::StartApProc),
            0x10021 => Ok(Self::SaveStateRead2),
            0x10022 => Ok(Self::MmMemoryUnblocked),
            0x10023 => Ok(Self::MmIsCommBuffer),
            other => Err(other),
        }
    }
}

impl SyscallIndex {
    /// Returns the raw `u64` value of this syscall index.
    pub fn as_u64(self) -> u64 {
        self as u64
    }
}

/// Issues a raw `syscall` to the MM Supervisor from Ring 3 (user) MM.
///
/// This is the low-level primitive behind every user supervisor syscall, such as page
/// allocation, save-state reads, and comm-buffer validation. Higher-level typed
/// wrappers should build on it.
///
/// RAX holds `call_index` ([`SyscallIndex`]), and RDX, R8, and R9 hold `arg1`, `arg2`,
/// and `arg3`. On return, RAX holds the result value.
///
/// ## Safety
///
/// Transfers control to the supervisor. The arguments must be valid for the
/// given syscall index. Only meaningful in Ring 3 user MM on x86-64. On any
/// other target this is a stub that returns `EFI_UNSUPPORTED`.
#[cfg(all(target_os = "uefi", target_arch = "x86_64"))]
pub unsafe fn raw_syscall(call_index: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let value: u64;

    // SAFETY: A `syscall` into the MM Supervisor with the documented register ABI.
    // The listed clobbers (RCX, R11) match the `syscall` instruction.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") call_index => value,
            in("rdx") arg1,
            in("r8") arg2,
            in("r9") arg3,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }

    value
}

/// Host/non-UEFI stub so the SDK links for tests and non-x86 UEFI targets.
///
/// Supervisor syscalls are only meaningful in Ring 3 user MM on x86-64. Anywhere
/// else this returns `EFI_UNSUPPORTED`.
///
/// ## Safety
///
/// This stub does nothing and is always safe to call. The `unsafe` marker keeps the
/// signature identical to the real implementation.
#[cfg(not(all(target_os = "uefi", target_arch = "x86_64")))]
pub unsafe fn raw_syscall(_call_index: u64, _arg1: u64, _arg2: u64, _arg3: u64) -> u64 {
    r_efi::efi::Status::UNSUPPORTED.as_usize() as u64
}
