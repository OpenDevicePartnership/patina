//! The MM CPU component.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use patina::{
    component::component,
    error::{EfiError, Result},
    mm_services::MmServiceProvider,
};
use r_efi::efi;

use crate::protocol::{self, MM_CPU_PROTOCOL_GUID, MmCpuProtocol};

/// The static `EFI_MM_CPU_PROTOCOL` interface installed by [`MmCpuComponent`].
///
/// Function pointers are `Send`/`Sync`, so this is safe to place in a `static`
/// and hand to consumers as a stable interface pointer.
static MM_CPU_PROTOCOL: MmCpuProtocol =
    MmCpuProtocol { read_save_state: mm_cpu_read_save_state, write_save_state: mm_cpu_write_save_state };

/// Component that produces the `EFI_MM_CPU_PROTOCOL` in the MM User Core.
///
/// Replaces the C `MmSupervisorPkg/Drivers/MmSupervisedCpu` driver. It installs
/// `gEfiMmCpuProtocolGuid`; its `ReadSaveState` forwards save-state reads to the
/// MM Supervisor, which owns SMRAM and enforces the save-state security policy.
#[derive(Default)]
pub struct MmCpuComponent;

impl MmCpuComponent {
    /// Creates a new [`MmCpuComponent`].
    pub fn new() -> Self {
        Self
    }
}

#[component]
impl MmCpuComponent {
    fn entry_point(self, mm: MmServiceProvider) -> Result<()> {
        // SAFETY: `MM_CPU_PROTOCOL` is a valid, `'static` interface; a `None` handle
        // requests a freshly allocated handle for the installed interface.
        let result = unsafe {
            mm.install_protocol_interface(
                None,
                &MM_CPU_PROTOCOL_GUID,
                &MM_CPU_PROTOCOL as *const MmCpuProtocol as *mut c_void,
            )
        };

        match result {
            Ok(handle) => {
                log::info!("Installed EFI_MM_CPU_PROTOCOL on handle {handle:p}");
                Ok(())
            }
            Err(status) => {
                log::error!("Failed to install EFI_MM_CPU_PROTOCOL: {status:?}");
                EfiError::status_to_result(status)
            }
        }
    }
}

/// `EFI_MM_CPU_PROTOCOL.ReadSaveState` implementation.
///
/// Only `PROCESSOR_ID`, `RAX`, and `IO` are supported; any other register is not
/// defined for the save state and returns `EFI_NOT_FOUND` (per the PI spec). The
/// supported registers are forwarded to the MM Supervisor, which reads SMRAM,
/// enforces the save-state policy, and assembles the composite
/// `EFI_MM_SAVE_STATE_IO_INFO` for the `IO` pseudo-register.
extern "efiapi" fn mm_cpu_read_save_state(
    this: *const MmCpuProtocol,
    width: usize,
    register: u32,
    cpu_index: usize,
    buffer: *mut c_void,
) -> efi::Status {
    if this.is_null() || buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // Whitelist the supported registers. Returning early here both satisfies the
    // PI contract (`EFI_NOT_FOUND` for undefined registers) and avoids issuing a
    // syscall the supervisor would reject.
    match register {
        protocol::REGISTER_PROCESSOR_ID | protocol::REGISTER_RAX | protocol::REGISTER_IO => {}
        _ => return efi::Status::NOT_FOUND,
    }

    // SAFETY: `buffer` is a caller-provided output buffer of at least `width`
    // bytes (for `IO`, at least `size_of::<EFI_MM_SAVE_STATE_IO_INFO>()`). The
    // supervisor validates that the buffer is user-owned before writing to it.
    unsafe {
        crate::save_state::read_save_state_register(this as usize, width, register, cpu_index, buffer.cast::<u8>())
    }
}

/// `EFI_MM_CPU_PROTOCOL.WriteSaveState` implementation.
///
/// Writing the MM save state is not supported, mirroring the C `MmSupervisedCpu`
/// driver, which installs a NULL `WriteSaveState`.
extern "efiapi" fn mm_cpu_write_save_state(
    _this: *const MmCpuProtocol,
    _width: usize,
    _register: u32,
    _cpu_index: usize,
    _buffer: *const c_void,
) -> efi::Status {
    efi::Status::UNSUPPORTED
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-null protocol pointer for tests. The read path never dereferences
    /// `this`; it only forwards its address to the (stubbed) syscall.
    fn dummy_this() -> *const MmCpuProtocol {
        &MM_CPU_PROTOCOL as *const MmCpuProtocol
    }

    #[test]
    fn test_mm_cpu_read_save_state_rejects_null_this() {
        let mut buf = [0u8; 8];
        let status = mm_cpu_read_save_state(core::ptr::null(), 8, protocol::REGISTER_RAX, 0, buf.as_mut_ptr().cast());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_mm_cpu_read_save_state_rejects_null_buffer() {
        let status = mm_cpu_read_save_state(dummy_this(), 8, protocol::REGISTER_RAX, 0, core::ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_mm_cpu_read_save_state_unsupported_register_is_not_found() {
        let mut buf = [0u8; 8];
        // 39 = RBX — a valid PI register, but not one this component exposes.
        let status = mm_cpu_read_save_state(dummy_this(), 8, 39, 0, buf.as_mut_ptr().cast());
        assert_eq!(status, efi::Status::NOT_FOUND);
    }

    #[test]
    fn test_mm_cpu_read_save_state_supported_register_forwards_to_syscall() {
        let mut buf = [0u8; 8];
        // A supported register passes the whitelist and forwards to the syscall
        // wrapper, which on the host (non-UEFI) target is a stub reporting the
        // operation as unsupported. Reaching UNSUPPORTED proves the forward path.
        for register in [protocol::REGISTER_PROCESSOR_ID, protocol::REGISTER_RAX, protocol::REGISTER_IO] {
            let status = mm_cpu_read_save_state(dummy_this(), 8, register, 0, buf.as_mut_ptr().cast());
            assert_eq!(status, efi::Status::UNSUPPORTED);
        }
    }

    #[test]
    fn test_mm_cpu_write_save_state_is_unsupported() {
        let buf = [0u8; 8];
        let status = mm_cpu_write_save_state(dummy_this(), 8, protocol::REGISTER_RAX, 0, buf.as_ptr().cast());
        assert_eq!(status, efi::Status::UNSUPPORTED);
    }
}
