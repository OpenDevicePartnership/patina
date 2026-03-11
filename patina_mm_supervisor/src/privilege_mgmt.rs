//! Privilege Management for MM Supervisor Core
//!
//! This module manages the privilege level transitions between Ring 0 (supervisor)
//! and Ring 3 (user) in the MM environment. It provides:
//!
//! - One-time initialization of syscall/sysret MSRs
//! - Demotion of code execution to Ring 3 via `InvokeDemotedRoutine`
//! - Handling of syscall requests from Ring 3 code
//! - Call gate and TSS descriptor management for privilege transitions
//!
//! ## Architecture
//!
//! The privilege management follows the x86_64 syscall/sysret model:
//!
//! 1. **Initialization**: Configure MSR_IA32_STAR, MSR_IA32_LSTAR, MSR_IA32_EFER
//!    to set up syscall entry points and segment selectors.
//!
//! 2. **Demotion**: Use `InvokeDemotedRoutine` to transition from Ring 0 to Ring 3.
//!    This sets up call gates for return and prepares the Ring 3 stack.
//!
//! 3. **Syscall Entry**: When Ring 3 code executes `syscall`, the CPU jumps to
//!    the address in MSR_IA32_LSTAR (our `SyscallCenter`), which dispatches
//!    to the appropriate handler.
//!
//! 4. **Return**: Ring 3 code returns via call gate or syscall dispatcher returns
//!    via `sysret`.
//!
//! ## Segment Layout (from SmiException.nasm)
//!
//! ```text
//! PROTECTED_DS      = 0x20
//! LONG_CS_R0        = 0x38  (Ring 0 code segment)
//! LONG_DS_R0        = 0x40  (Ring 0 data segment)
//! LONG_CS_R3_PH     = 0x4B  (Ring 3 code segment placeholder)
//! LONG_DS_R3        = 0x53  (Ring 3 data segment)
//! LONG_CS_R3        = 0x5B  (Ring 3 code segment)
//! CALL_GATE_OFFSET  = 0x60  (Call gate descriptor offset)
//! TSS_SEL_OFFSET    = 0x70  (TSS selector offset)
//! TSS_DESC_OFFSET   = 0x80  (TSS descriptor offset)
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi::Status;

mod call_gate;
mod syscall_dispatcher;
pub(crate) mod syscall_setup;

pub type SyscallResult = Result<u64, Status>; // Result of a syscall: Ok(value) or Err(EFI_STATUS)

// FFI binding to the assembly `invoke_demoted_routine` routine (`call_gate_transfer.asm`).
// Only linked for the firmware (UEFI) target; host builds (unit tests, doctests, `check`)
// exclude the entry/transition assembly and use the wrapper's host paths below.
#[cfg(target_os = "uefi")]
unsafe extern "efiapi" {
    #[link_name = "invoke_demoted_routine"]
    fn invoke_demoted_routine_asm(cpu_index: usize, cpl3_routine: u64, cpl3_stack: u64, arg_count: usize, ...)
    -> usize;
}

/// Invokes a specified routine in CPL 3 (Ring 3).
///
/// This function transitions from Ring 0 to Ring 3, executes the demoted routine, and
/// returns back to Ring 0 through a call gate. The three trailing arguments are populated to
/// registers and/or CPL3 stack areas per the EFIAPI calling convention. Returns `EFI_SUCCESS`
/// if the demoted routine returned successfully, or other values for errors from ring
/// transitioning or the demoted routine.
///
/// The assembly transition only exists on the firmware (UEFI) target. On host builds it cannot
/// run, so test builds delegate to the controllable mock in the `mock` module (letting callers
/// be exercised), and other host builds (e.g. doctests) use an inert stub.
///
/// ## Safety
///
/// This function modifies privilege levels and stack pointers. Callers must ensure:
/// - Valid function pointer for `cpl3_routine`
/// - Valid stack pointer for `cpl3_stack`
/// - Correct `arg_count` matching the actual arguments
pub(crate) unsafe fn invoke_demoted_routine(
    cpu_index: usize,
    cpl3_routine: u64,
    cpl3_stack: u64,
    arg_count: usize,
    arg1: u64,
    arg2: u64,
    arg3: u64,
) -> usize {
    // Firmware (UEFI): perform the real Ring 0 -> Ring 3 transition via assembly.
    #[cfg(target_os = "uefi")]
    // SAFETY: forwards to the assembly transition; the documented preconditions are upheld by the caller.
    let ret = unsafe { invoke_demoted_routine_asm(cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3) };

    // Host test builds: delegate to the controllable mock so callers can be exercised.
    #[cfg(all(not(target_os = "uefi"), test))]
    let ret = mock::invoke_demoted_routine(cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3);

    // Other host builds (e.g. doctests): inert; the transition never runs off the firmware target.
    #[cfg(all(not(target_os = "uefi"), not(test)))]
    let ret = {
        let _ = (cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3);
        0
    };

    ret
}

/// Test-only mock backing [`invoke_demoted_routine`].
///
/// The assembly privilege transition cannot run on the host, so in test builds the wrapper
/// delegates here. Install a handler with [`set_handler`] to observe calls and control the
/// return value; with no handler installed, calls return `0` (EFI_SUCCESS).
#[cfg(test)]
pub(crate) mod mock {
    use core::cell::RefCell;

    /// Handler signature: receives the same arguments as the real routine and returns the value
    /// the supervisor should observe.
    type Handler = Box<dyn FnMut(usize, u64, u64, usize, u64, u64, u64) -> usize>;

    thread_local! {
        static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    }

    /// Installs a handler invoked in place of the real Ring 0 -> Ring 3 transition.
    pub(crate) fn set_handler<F>(handler: F)
    where
        F: FnMut(usize, u64, u64, usize, u64, u64, u64) -> usize + 'static,
    {
        HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
    }

    /// Removes any installed handler; subsequent calls return `0`.
    pub(crate) fn clear() {
        HANDLER.with(|h| *h.borrow_mut() = None);
    }

    pub(super) fn invoke_demoted_routine(
        cpu_index: usize,
        cpl3_routine: u64,
        cpl3_stack: u64,
        arg_count: usize,
        arg1: u64,
        arg2: u64,
        arg3: u64,
    ) -> usize {
        HANDLER.with(|h| match h.borrow_mut().as_mut() {
            Some(handler) => handler(cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3),
            None => 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invoke_demoted_routine_delegates_to_mock_handler() {
        // With no handler installed, the wrapper returns 0 (EFI_SUCCESS).
        mock::clear();
        // SAFETY: under `test` the wrapper delegates to the mock; no real privilege transition occurs.
        let ret = unsafe { invoke_demoted_routine(0, 0x1000, 0x2000, 3, 1, 2, 3) };
        assert_eq!(ret, 0);

        // An installed handler observes the arguments and controls the return value, so callers of
        // `invoke_demoted_routine` can be exercised on the host.
        mock::set_handler(|cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3| {
            assert_eq!(cpu_index, 7);
            assert_eq!(cpl3_routine, 0xdead_beef);
            assert_eq!(cpl3_stack, 0xcafe);
            assert_eq!(arg_count, 3);
            assert_eq!((arg1, arg2, arg3), (10, 20, 30));
            0x1234
        });
        // SAFETY: see above.
        let ret = unsafe { invoke_demoted_routine(7, 0xdead_beef, 0xcafe, 3, 10, 20, 30) };
        assert_eq!(ret, 0x1234);

        mock::clear();
    }
}
