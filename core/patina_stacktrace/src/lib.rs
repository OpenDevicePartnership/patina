//! # Stack Trace Library
//!
//! ## Introduction
//!
//! This library implements stack-walking logic. Given an instruction pointer
//! and stack pointer, the [API](#public-api) dumps the stack trace that led to
//! that machine state. It currently does not resolve symbols because PDB debug
//! info is not embedded in the PE image, unlike DWARF data in ELF images.
//! Therefore, symbol resolution must be performed offline. As a result, the
//! "Call Site" column in the output displays `module+<relative rip>` instead of
//! `module!function+<relative pc>`. Outside of this library, with PDB access,
//! those module-relative PC offsets can be resolved to function-relative
//! offsets, as shown below.
//!
//! ```cmd
//! PS C:\> .\resolve_stacktrace.ps1 -StackTrace "
//! >>     # Child-SP              Return Address         Call Site
//! >>     0 00000057261FFAE0      00007FFC9AC910E5       x64+1095
//! >>     1 00000057261FFB10      00007FFC9AC9115E       x64+10E5
//! >>     2 00000057261FFB50      00007FFC9AC911E8       x64+115E
//! >>     3 00000057261FFB90      00007FFC9AC9125F       x64+11E8
//! >>     4 00000057261FFBD0      00007FF6D3557236       x64+125F
//! >>     5 00000057261FFC10      00007FFCC4BDE8D7       patina_stacktrace-cf486b9b613e51dc+7236
//! >>     6 00000057261FFC70      00007FFCC6B7FBCC       kernel32+2E8D7
//! >>     7 00000057261FFCA0      0000000000000000       ntdll+34521
//! >>
//! >> " -PdbDirectory "C:\pdbs\"
//!
//! Output:
//! # Source Path                                                           Child-SP         Return Address   Call Site
//! 0 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   63] 00000057261FFAE0 00007FFC9AC910E5 x64!func1+25
//! 1 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   72] 00000057261FFB10 00007FFC9AC9115E x64!func2+15
//! 2 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   84] 00000057261FFB50 00007FFC9AC911E8 x64!func3+1E
//! 3 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   96] 00000057261FFB90 00007FFC9AC9125F x64!func4+28
//! 4 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @  109] 00000057261FFBD0 00007FF6D3557236 x64!StartCallStack+1F
//! 5 [C:\r\patina\core\patina_stacktrace\src\x64\tests\unwind_test_full.rs  @   98] 00000057261FFC10 00007FFCC4BDE8D7 patina_stacktrace-cf486b9b613e51dc!static unsigned int patina_stacktrace::x64::tests::unwind_test_full::call_stack_thread(union enum2$<winapi::ctypes::c_void> *)+56
//! 6 [Failed to load PDB file (HRESULT: 0x806D0005)                      ] 00000057261FFC70 00007FFCC6B7FBCC kernel32+2E8D7
//! 7 [Failed to load PDB file (HRESULT: 0x806D0005)                      ] 00000057261FFCA0 0000000000000000 ntdll+34521
//! ```
//!
//! ## Prerequisites
//!
//! This library uses the PE image `.pdata` section to calculate the stack
//! unwind information required to walk the call stack. Therefore, compile all
//! binaries with the following `rustc` flag to generate the `.pdata` sections
//! in the PE images:
//!
//! `RUSTFLAGS=-Cforce-unwind-tables`
//!
//! ## Public API
//!
//! The primary public API is the `dump()/dump_with()` function in the
//! `StackTrace` module.
//!
//! ```ignore
//!    /// Dumps the stack trace for the given PC, SP, and FP values.
//!    ///
//!    /// # Safety
//!    ///
//!    /// This function is marked `unsafe` to indicate that the caller is
//!    /// responsible for validating the provided PC, SP, and FP values. Invalid
//!    /// values can result in undefined behavior, including potential page
//!    /// faults.
//!    ///
//!    /// ```text
//!    /// # Child-SP              Return Address         Call Site
//!    /// 0 0000005E2AEFFC00      00007FFB10CB4508       aarch64+44B0
//!    /// 1 0000005E2AEFFC20      00007FFB10CB45A0       aarch64+4508
//!    /// 2 0000005E2AEFFC40      00007FFB10CB4640       aarch64+45A0
//!    /// 3 0000005E2AEFFC60      00007FFB10CB46D4       aarch64+4640
//!    /// 4 0000005E2AEFFC90      00007FF760473B98       aarch64+46D4
//!    /// 5 0000005E2AEFFCB0      00007FFB8F062310       patina_stacktrace-45f5092641a5979a+3B98
//!    /// 6 0000005E2AEFFD10      00007FFB8FF95AEC       kernel32+12310
//!    /// 7 0000005E2AEFFD50      0000000000000000       ntdll+75AEC
//!    /// ```
//!    pub unsafe fn dump_with(stack_frame: StackFrame) -> StResult<()>;
//!
//!    /// Dumps the stack trace. This function reads the PC, SP, and FP values and
//!    /// attempts to dump the call stack.
//!    ///
//!    /// # Safety
//!    ///
//!    /// It is marked `unsafe` to indicate that the caller is responsible for the
//!    /// validity of the PC, SP, and FP values. Invalid or corrupt machine state
//!    /// can result in undefined behavior, including potential page faults.
//!    ///
//!    /// ```text
//!    /// # Child-SP              Return Address         Call Site
//!    /// 0 0000005E2AEFFC00      00007FFB10CB4508       aarch64+44B0
//!    /// 1 0000005E2AEFFC20      00007FFB10CB45A0       aarch64+4508
//!    /// 2 0000005E2AEFFC40      00007FFB10CB4640       aarch64+45A0
//!    /// 3 0000005E2AEFFC60      00007FFB10CB46D4       aarch64+4640
//!    /// 4 0000005E2AEFFC90      00007FF760473B98       aarch64+46D4
//!    /// 5 0000005E2AEFFCB0      00007FFB8F062310       patina_stacktrace-45f5092641a5979a+3B98
//!    /// 6 0000005E2AEFFD10      00007FFB8FF95AEC       kernel32+12310
//!    /// 7 0000005E2AEFFD50      0000000000000000       ntdll+75AEC
//!    /// ```
//!    pub unsafe fn dump() -> StResult<()>;
//! ```
//!
//! ## API usage
//!
//! ```ignore
//!     // Inside an exception handler
//!     let stack_frame = StackFrame { pc: x64_context.rip, sp: x64_context.rsp, fp: x64_context.rbp };
//!     StackTrace::dump_with(stack_frame); // X64
//!     let stack_frame = StackFrame { pc: aarch64_context.elr, sp: aarch64_context.sp, fp: aarch64_context.fp };
//!     StackTrace::dump_with(stack_frame); // AArch64
//!
//!     // Inside a Rust panic handler and drivers
//!     StackTrace::dump();
//! ```
//!
//! ## Reference
//!
//! More reference test cases live in `src\x64\tests\*.rs`.

#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![feature(coverage_attribute)]

extern crate alloc;

mod byte_reader;
pub mod error;
mod pe;
mod stacktrace;

cfg_if::cfg_if! {
    if #[cfg(test)] {
        mod aarch64;
        mod x64;
    } else if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        mod aarch64;
    } else {
        mod x64;
    }
}

pub use stacktrace::{StackFrame, StackTrace};
