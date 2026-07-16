//! Shared type definitions for MM supervisor and user cores.
//!
//! This crate provides the communication structures and enumerations that define
//! the ABI between the supervisor (ring 0) and user (ring 3) MM modules.

// GUID for gMmSupervisorHobMemoryAllocModuleGuid
// { 0x3efafe72, 0x3dbf, 0x4341, { 0xad, 0x04, 0x1c, 0xb6, 0xe8, 0xb6, 0x8e, 0x5e }}
/// GUID used in MemoryAllocationModule HOBs to identify MM Supervisor module allocations.
pub const MM_SUPERVISOR_HOB_MEMORY_ALLOC_MODULE_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("3efafe72-3dbf-4341-ad04-1cb6e8b68e5e");

// GUID for gMmSupervisorUserGuid
// { 0x30d1cc3f, 0xc1db, 0x41ed, { 0xb1, 0x13, 0xab, 0xce, 0x21, 0xb0, 0x2b, 0xce }}
/// GUID identifying the MM Supervisor User module.
pub const MM_SUPERVISOR_USER_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("30d1cc3f-c1db-41ed-b113-abce21b02bce");

/// Command types passed from the supervisor to the user core via `invoke_demoted_routine`.
///
/// Discriminant values are part of the supervisor↔user ABI and must not change.
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

/// Syscall indices for the MM Supervisor ↔ User Core syscall interface.
///
/// These match the definitions in SysCallLib.h and define the ABI used when
/// Ring 3 code issues a `syscall` instruction to the Ring 0 supervisor.
///
/// ## ABI
///
/// - RAX = call index ([`SyscallIndex`])
/// - RDX = arg1
/// - R8  = arg2
/// - R9  = arg3
///
/// On return:
/// - RAX = result value (the supervisor communicates only through RAX)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallIndex {
    /// Read MSR - Arg1: MSR index, Returns: MSR value
    RdMsr = 0x0000,
    /// Write MSR - Arg1: MSR index, Arg2: value
    WrMsr = 0x0001,
    /// CLI - Clear interrupts
    Cli = 0x0002,
    /// IO Read - Arg1: port, Arg2: width
    IoRead = 0x0003,
    /// IO Write - Arg1: port, Arg2: width, Arg3: value
    IoWrite = 0x0004,
    /// WBINVD - Write back and invalidate cache
    Wbinvd = 0x0005,
    /// HLT - Halt processor
    Hlt = 0x0006,
    /// Save State Read - Arg1: register, Arg2: CPU index
    SaveStateRead = 0x0007,
    /// Maximum value for legacy syscall indices
    LegacyMax = 0xFFFF,
    /// Allocate Pages - Arg1: alloc_type, Arg2: mem_type, Arg3: page_count
    AllocPage = 0x10004,
    /// Free Pages - Arg1: address, Arg2: page_count
    FreePage = 0x10005,
    /// Start AP Procedure - Arg1: procedure, Arg2: CPU index, Arg3: argument
    StartApProc = 0x10006,
    /// Save state read with extended support - Arg1: width, Arg2: buffer pointer
    SaveStateRead2 = 0x10021,
    /// MM memory unblocked - Arg1: address, Arg2: size
    MmMemoryUnblocked = 0x10022,
    /// MM is communication buffer - Arg1: address, Arg2: size
    MmIsCommBuffer = 0x10023,
}

impl SyscallIndex {
    /// Creates a `SyscallIndex` from a raw `u64` value.
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            0x0000 => Some(Self::RdMsr),
            0x0001 => Some(Self::WrMsr),
            0x0002 => Some(Self::Cli),
            0x0003 => Some(Self::IoRead),
            0x0004 => Some(Self::IoWrite),
            0x0005 => Some(Self::Wbinvd),
            0x0006 => Some(Self::Hlt),
            0x0007 => Some(Self::SaveStateRead),
            0xFFFF => Some(Self::LegacyMax),
            0x10004 => Some(Self::AllocPage),
            0x10005 => Some(Self::FreePage),
            0x10006 => Some(Self::StartApProc),
            0x10021 => Some(Self::SaveStateRead2),
            0x10022 => Some(Self::MmMemoryUnblocked),
            0x10023 => Some(Self::MmIsCommBuffer),
            _ => None,
        }
    }

    /// Returns the raw `u64` value of this syscall index.
    pub fn as_u64(self) -> u64 {
        self as u64
    }
}

/// Issue a raw `syscall` to the MM Supervisor from Ring 3 (user) MM.
///
/// This is the low-level primitive behind every user supervisor syscall (page
/// allocation, save-state reads, comm-buffer validation). Higher-level typed
/// wrappers should build on it.
///
/// ## ABI
///
/// - `RAX` = `call_index` ([`SyscallIndex`]), `RDX` = `arg1`, `R8` = `arg2`,
///   `R9` = `arg3`.
/// - On return, `RAX` holds the result value.
///
/// ## Safety
///
/// Transfers control to the supervisor; the arguments must be valid for the
/// given syscall index. Only meaningful in Ring 3 user MM on x86-64” on any
/// other target this is a stub reporting `EFI_UNSUPPORTED`.
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
/// Supervisor syscalls are only meaningful in Ring 3 user MM on x86-64; anywhere
/// else the operation is reported as unsupported.
///
/// ## Safety
///
/// This stub performs no operation and is always safe to call; the `unsafe`
/// marker only keeps the signature identical to the real implementation.
#[cfg(not(all(target_os = "uefi", target_arch = "x86_64")))]
pub unsafe fn raw_syscall(_call_index: u64, _arg1: u64, _arg2: u64, _arg3: u64) -> u64 {
    r_efi::efi::Status::UNSUPPORTED.as_usize() as u64
}
