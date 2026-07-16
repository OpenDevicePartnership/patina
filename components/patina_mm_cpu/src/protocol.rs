//! `EFI_MM_CPU_PROTOCOL` ABI definitions.
//!
//! Rust definitions of the PI 1.5 `EFI_MM_CPU_PROTOCOL` (`gEfiMmCpuProtocolGuid`)
//! and the subset of `EFI_MM_SAVE_STATE_REGISTER` values this component supports.
//! These mirror `MdePkg/Include/Protocol/MmCpu.h`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use r_efi::efi;

/// `gEfiMmCpuProtocolGuid` — `eb346b97-975f-4a9f-8b22-f8e92bb3d569`.
pub const MM_CPU_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xeb34_6b97, 0x975f, 0x4a9f, 0x8b, 0x22, &[0xf8, 0xe9, 0x2b, 0xb3, 0xd5, 0x69]);

/// `EFI_MM_SAVE_STATE_REGISTER_RAX` — the general-purpose RAX register.
pub const REGISTER_RAX: u32 = 38;

/// `EFI_MM_SAVE_STATE_REGISTER_IO` — pseudo-register describing the I/O
/// instruction that was in progress when the MMI was triggered. Reading it
/// yields an `EFI_MM_SAVE_STATE_IO_INFO`.
pub const REGISTER_IO: u32 = 512;

/// `EFI_MM_SAVE_STATE_REGISTER_PROCESSOR_ID` — pseudo-register for the CPU's ID.
pub const REGISTER_PROCESSOR_ID: u32 = 514;

/// `EFI_MM_READ_SAVE_STATE` — read a register from a CPU's MM save state.
pub type MmReadSaveState = extern "efiapi" fn(
    this: *const MmCpuProtocol,
    width: usize,
    register: u32,
    cpu_index: usize,
    buffer: *mut c_void,
) -> efi::Status;

/// `EFI_MM_WRITE_SAVE_STATE` — write a register to a CPU's MM save state.
pub type MmWriteSaveState = extern "efiapi" fn(
    this: *const MmCpuProtocol,
    width: usize,
    register: u32,
    cpu_index: usize,
    buffer: *const c_void,
) -> efi::Status;

/// `EFI_MM_CPU_PROTOCOL` — access to CPU save-state registers while in MM.
#[repr(C)]
pub struct MmCpuProtocol {
    /// Reads a register from the specified CPU's MM save state.
    pub read_save_state: MmReadSaveState,
    /// Writes a register to the specified CPU's MM save state.
    pub write_save_state: MmWriteSaveState,
}
