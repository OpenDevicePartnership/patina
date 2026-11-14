//! MM save-state read syscalls from Ring 3 user MM to the MM Supervisor.
//!
//! The MM save state lives in supervisor-only SMRAM, so the CPU protocol's
//! `ReadSaveState` cannot read it directly. It issues these syscalls to have the
//! MM Supervisor perform the read (and its security-policy check) in Ring 0.
//!
//! ## ABI
//!
//! The read is a two-phase syscall, mirroring the historical `SysCallLib.h`
//! interface used by the C `MmSupervisedCpu` driver:
//!
//! 1. [`SyscallIndex::SaveStateRead`] takes the `this` token in arg1, the
//!    register in arg2, and the CPU index in arg3. The supervisor validates the
//!    request and records it.
//! 2. [`SyscallIndex::SaveStateRead2`] takes the `this` token in arg1, the width
//!    in arg2, and the output buffer in arg3. The supervisor applies the MM
//!    security policy, reads the register (building the composite
//!    `EFI_MM_SAVE_STATE_IO_INFO` for the IO pseudo-register), and copies the
//!    result into the caller's buffer.
//!
//! The supervisor returns the `EFI_STATUS` for each phase in RAX. The `this`
//! token is passed in both phases so the supervisor can check they match.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    management_mode::supervisor::{SyscallIndex, raw_syscall},
    standard::efi,
};

/// Reads `width` bytes of save-state `register` for CPU `cpu_index` into
/// `buffer` through the MM Supervisor.
///
/// `register` is a raw `EFI_MM_SAVE_STATE_REGISTER` value. `this` is an opaque
/// token passed in both syscall phases so the supervisor can check they match.
/// The CPU protocol producer passes its protocol interface pointer.
///
/// Returns `EFI_SUCCESS`, or the supervisor's error status such as
/// `EFI_NOT_FOUND`, `EFI_ACCESS_DENIED`, or `EFI_INVALID_PARAMETER`.
///
/// ## Safety
///
/// `buffer` must point to at least `width` bytes of user-owned, writable memory.
/// The supervisor validates that the buffer is user-owned before writing, but
/// the caller must still guarantee the pointer and length are valid.
pub(crate) unsafe fn read_save_state_register(
    this: usize,
    width: usize,
    register: u32,
    cpu_index: usize,
    buffer: *mut u8,
) -> efi::Status {
    // Phase 1: record the register and CPU index.
    // SAFETY: `SaveStateRead` takes the `this` token, register, and CPU index as
    // scalar arguments; no memory is dereferenced by the supervisor in this phase.
    let phase1 = unsafe {
        raw_syscall(SyscallIndex::SaveStateRead.as_u64(), this as u64, u64::from(register), cpu_index as u64)
    };
    let phase1 = efi::Status::from_usize(phase1 as usize);
    if phase1 != efi::Status::SUCCESS {
        return phase1;
    }

    // Phase 2: apply policy, read, and copy into the caller's buffer.
    // SAFETY: `SaveStateRead2` writes up to `width` bytes into `buffer`, which the
    // caller guarantees is valid for `width` bytes. The supervisor also checks user
    // ownership before writing.
    let phase2 = unsafe {
        raw_syscall(SyscallIndex::SaveStateRead2.as_u64(), this as u64, width as u64, buffer.expose_provenance() as u64)
    };
    efi::Status::from_usize(phase2 as usize)
}
