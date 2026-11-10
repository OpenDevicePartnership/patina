//! Privilege management for the MM Supervisor.
//!
//! Moves execution between Ring 0 (supervisor) and Ring 3 (user) code in MM:
//!
//! - Programming of the syscall/sysret MSRs for each demotion
//! - Demotion of a routine to Ring 3 with `invoke_demoted_routine`
//! - Dispatch of syscalls made by Ring 3 code
//! - Call gate and TSS descriptor management
//!
//! ## Architecture
//!
//! The design follows the `x86_64` syscall/sysret model:
//!
//! 1. MSR setup: before each demotion, `invoke_demoted_routine` programs `MSR_IA32_STAR`,
//!    `MSR_IA32_LSTAR`, `MSR_IA32_FMASK` and `MSR_IA32_EFER` with the syscall entry point,
//!    the segment selectors, and the RFLAGS bits the CPU clears when Ring 3 enters Ring 0. It
//!    restores the previous values on return.
//!
//! 2. Demotion: `invoke_demoted_routine` drops from Ring 0 to Ring 3. It sets up the
//!    call gate used for the return and prepares the Ring 3 stack.
//!
//! 3. Syscall entry: when Ring 3 code executes `syscall`, the CPU jumps to the
//!    address in `MSR_IA32_LSTAR` (`syscall_center`), which dispatches to the handler.
//!
//! 4. Return: the demoted routine returns to Ring 0 through the call gate, and the
//!    syscall dispatcher returns to Ring 3 with `sysret`.
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
//! CALL_GATE_OFFSET  = 0x60  (Call gate descriptor offset; the asm uses selector 0x63, RPL 3)
//! TSS_SEL_OFFSET    = 0x70  (TSS descriptor offset in the GDT)
//! TSS_DESC_OFFSET   = 0x80  (Offset of the TSS that follows the GDT)
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::standard::efi::Status;

pub(crate) mod call_gate;
pub(crate) mod save_state;
mod syscall_dispatcher;
mod syscall_ops;
pub(crate) mod syscall_setup;

/// Result of a syscall: the value to hand back to Ring 3, or the status to report.
pub type SyscallResult = Result<u64, Status>;

// FFI binding to the assembly `invoke_demoted_routine` routine (`call_gate_transfer.asm`).
// Only linked for the firmware (UEFI) target; host builds (unit tests, doctests, `check`)
// exclude the entry/transition assembly and use the wrapper's host paths below.
#[cfg(target_os = "uefi")]
unsafe extern "efiapi" {
    /// Performs the Ring 0 to Ring 3 transition and the return through the call gate.
    ///
    /// Declared here and defined in `call_gate_transfer.asm`. Callers go through
    /// [`invoke_demoted_routine`], which is the only wrapper that upholds its contract.
    #[link_name = "invoke_demoted_routine"]
    fn invoke_demoted_routine_asm(cpu_index: usize, cpl3_routine: u64, cpl3_stack: u64, arg_count: usize, ...)
    -> usize;
}

/// Number of arguments [`invoke_demoted_routine`] passes to the demoted routine.
pub(crate) const DEMOTED_ROUTINE_ARG_COUNT: usize = 3;

/// Runs a routine at CPL 3 (Ring 3) and returns to Ring 0 through the call gate.
///
/// The three trailing arguments are placed in registers or on the CPL 3 stack per the
/// EFIAPI calling convention. Returns `EFI_SUCCESS` if the demoted routine succeeded, or
/// the error from the ring transition or the routine.
///
/// The assembly transition exists only on the firmware (UEFI) target. Host test builds call
/// the controllable `mock` module instead, and other host builds (e.g. doctests) use an
/// inert stub that returns 0.
///
/// ## Safety
///
/// This function changes privilege level and stack pointer. Callers must ensure:
/// - `cpl3_routine` is a valid function pointer
/// - `cpl3_stack` is a valid stack pointer
/// - `arg_count` matches the arguments actually passed
pub(crate) unsafe fn invoke_demoted_routine(
    cpu_index: usize,
    cpl3_routine: u64,
    cpl3_stack: u64,
    arg_count: usize,
    arg1: u64,
    arg2: u64,
    arg3: u64,
) -> usize {
    #[cfg(target_os = "uefi")]
    // SAFETY: the caller upholds the documented preconditions of the assembly transition.
    let ret = unsafe { invoke_demoted_routine_asm(cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3) };

    #[cfg(all(not(target_os = "uefi"), test))]
    let ret = mock::invoke_demoted_routine(cpu_index, cpl3_routine, cpl3_stack, arg_count, arg1, arg2, arg3);

    // Other host builds (e.g. doctests) never run the transition.
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
/// return value; with no handler installed, calls return `0` (`EFI_SUCCESS`).
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
        HANDLER.with(|handler_cell| *handler_cell.borrow_mut() = Some(Box::new(handler)));
    }

    /// Removes any installed handler; subsequent calls return `0`.
    pub(crate) fn clear() {
        HANDLER.with(|handler_cell| *handler_cell.borrow_mut() = None);
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
        HANDLER.with(|handler_cell| match handler_cell.borrow_mut().as_mut() {
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
