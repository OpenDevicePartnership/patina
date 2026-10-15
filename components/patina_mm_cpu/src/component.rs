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
    management_mode::mm_services::MmServiceProvider,
    standard::efi,
};

use crate::protocol::{MM_CPU_PROTOCOL_GUID, MmCpuProtocol};

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
                core::ptr::from_ref(&MM_CPU_PROTOCOL).cast_mut().cast::<c_void>(),
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
/// Every register is forwarded to the MM Supervisor, which reads SMRAM,
/// enforces the save-state policy, and assembles the composite
/// `EFI_MM_SAVE_STATE_IO_INFO` for the IO pseudo-register.
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

    // SAFETY: `buffer` is a caller-provided output buffer of at least `width`
    // bytes (for IO, at least `size_of::<EFI_MM_SAVE_STATE_IO_INFO>()`). The
    // supervisor validates that the buffer is user-owned before writing to it.
    unsafe { crate::save_state::read_save_state_register(this.addr(), width, register, cpu_index, buffer.cast::<u8>()) }
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
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::protocol;

    /// A non-null protocol pointer for tests. The read path never dereferences
    /// `this`; it only forwards its address to the (stubbed) syscall.
    fn dummy_this() -> *const MmCpuProtocol {
        core::ptr::from_ref(&MM_CPU_PROTOCOL)
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
    fn test_mm_cpu_read_save_state_forwards_every_register_to_syscall() {
        let mut buf = [0u8; 8];
        // Every register reaches the syscall wrapper, which on the host (non-UEFI)
        // target is a stub reporting the operation as unsupported. Reaching
        // UNSUPPORTED proves the forward path; the supervisor applies the policy.
        // `EFI_MM_SAVE_STATE_REGISTER_RBX`. 999 is not a defined `EFI_MM_SAVE_STATE_REGISTER` value.
        const REGISTER_RBX: u32 = 39;
        for register in
            [protocol::REGISTER_PROCESSOR_ID, protocol::REGISTER_RAX, protocol::REGISTER_IO, REGISTER_RBX, 999]
        {
            let status = mm_cpu_read_save_state(dummy_this(), 8, register, 0, buf.as_mut_ptr().cast());
            assert_eq!(status, efi::Status::UNSUPPORTED, "register {register}");
        }
    }

    #[test]
    fn test_mm_cpu_write_save_state_is_unsupported() {
        let buf = [0u8; 8];
        let status = mm_cpu_write_save_state(dummy_this(), 8, protocol::REGISTER_RAX, 0, buf.as_ptr().cast());
        assert_eq!(status, efi::Status::UNSUPPORTED);
    }
}
