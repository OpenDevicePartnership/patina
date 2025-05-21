//! UEFI Debugger
//!
//! This crate provides a debugger implementation that will install itself in the
//! exception handlers and communicate with debugger software using the GDB Remote
//! protocol. The debugger is intended to be used in the boot phase cores.
//!
//! This crate is under construction and may be missing functionality, documentation,
//! and testing.
//!
//! ## Getting Started
//!
//! For more details on using the debugger on a device, see the [readme](./Readme.md).
//!
//! ## Examples and Usage
//!
//! The debugger consists of the static access routines and the underlying debugger
//! struct. The top level platform code should initialize the statis `UefiDebugger`
//! struct with the appropriate serial transport and default configuration. The
//! platform has the option of setting static configuration, or enabling the
//! debugger in runtime code based on platform policy. During entry, the platform
//! should use the `set_debugger` routine to set the global instance of the debugger.
//!
//! Core code should use the static routines to interact with the debugger. If the
//! debugger is either not set or not enabled, the static routines will be no-ops.
//!
//! ```rust
//! extern crate patina_sdk;
//! extern crate patina_internal_cpu;
//!
//! use patina_internal_cpu::interrupts::{Interrupts, InterruptManager};
//!
//! static DEBUGGER: patina_debugger::UefiDebugger<patina_sdk::serial::uart::UartNull> =
//!     patina_debugger::UefiDebugger::new(patina_sdk::serial::uart::UartNull{});
//!
//! fn entry() {
//!
//!     // Configure the debugger. This is used for dynamic configuration of the debugger.
//!     // For static configuration use the with_config method on construction.
//!     DEBUGGER.configure(true, true, 0);
//!
//!     // Set the global debugger instance. This can only be done once.
//!     patina_debugger::set_debugger(&DEBUGGER);
//!
//!     // Call the core entry. The core can then initialize and access the debugger
//!     // through the static routines.
//!     start();
//!
//! }
//!
//! fn start() {
//!     let mut interrupt_manager = Interrupts::default();
//!
//!     // Initialize the debugger. This will cause a debug break because of the
//!     // initial break configuration set above.
//!     patina_debugger::initialize(&mut interrupt_manager);
//!
//!     // Notify the debugger of a module load.
//!     patina_debugger::notify_module_load("module.efi", 0x420000, 0x10000);
//!
//!     // Poll the debugger for any pending interrupts.
//!     patina_debugger::poll_debugger();
//!
//!     // Break into the debugger if the debugger is enabled.
//!     if patina_debugger::enabled() {
//!         patina_debugger::breakpoint();
//!     }
//! }
//!
//! ```
//!
//! The debugger can be further configured by using various functions on the
//! initialization of the debugger struct. See the definition for [debugger::UefiDebugger]
//! for more details. Notably, if the device is using the same transport for
//! logging and debugger, it is advisable to use `.without_log_init()`.
//!
//! ## Features
//!
//! `windbg_workarounds` - (Default) Enables workarounds for Windbg compatibility.
//!
//! `alloc` - Uses allocated buffers rather than static buffers for all memory. This provides additional functionality
//! but prevents debugging prior to allocations being available.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(test), no_std)]

mod arch;
mod dbg_target;
mod debugger;
mod memory;
mod modules;
mod transport;

extern crate alloc;

pub use debugger::UefiDebugger;

use arch::{DebuggerArch, SystemArch};
use patina_internal_cpu::interrupts::{ExceptionContext, InterruptManager};
use patina_sdk::serial::SerialIO;

/// Global instance of the debugger.
///
/// This is only expected to be set once, and will be accessed through the static
/// routines after that point. Because the debugger is expected to install itself
/// in exception handlers and will have access to other static state for things
/// like breakpoints, it is not safe to remove or replace it. For this reason,
/// this uses the Once lock to provide these properties.
///
static DEBUGGER: spin::Once<&dyn Debugger> = spin::Once::new();

/// Trait for debugger interaction. This is required to allow for a global to the
/// platform specific debugger implementation. For safety, these routines should
/// only be invoked on the global instance of the debugger.
trait Debugger: Sync {
    /// Initializes the debugger.
    fn initialize(&'static self, interrupt_manager: &mut dyn InterruptManager);

    /// Checks if the debugger is enabled.
    fn enabled(&'static self) -> bool;

    /// Notifies the debugger of a module load.
    fn notify_module_load(&'static self, module_name: &str, _address: usize, _length: usize);

    /// Polls the debugger for any pending interrupts.
    fn poll_debugger(&'static self);
}

#[derive(Debug)]
#[allow(dead_code)]
enum DebugError {
    /// The debugger lock could not be acquired. Usually indicating the debugger faulted.
    Reentry,
    /// The debugger configuration is locked. This indicates a failure during debugger configuration.
    ConfigLocked,
    /// The debugger was invoked without being fuly initialized.
    NotInitialized,
    /// Failure from the GDB stub initialization.
    GdbStubInit,
    /// Failure from the GDB stub.
    GdbStubError(gdbstub::stub::GdbStubError<(), patina_sdk::error::EfiError>),
    /// Failure to reboot the system.
    RebootFailure,
}

/// Sets the global instance of the debugger.
pub fn set_debugger<T: SerialIO>(debugger: &'static UefiDebugger<T>) {
    DEBUGGER.call_once(|| debugger);
}

/// Initializes the debugger. This will install the debugger into the exception
/// handlers using the provided interrupt manager. This routine may invoke a debug
/// break depending on configuration.
pub fn initialize(interrupt_manager: &mut dyn InterruptManager) {
    if let Some(debugger) = DEBUGGER.get() {
        debugger.initialize(interrupt_manager);
    }
}

/// Invokes a debug break instruction.
pub fn breakpoint() {
    SystemArch::breakpoint();
}

/// Notifies the debugger of a module load at the provided address and length.
/// This should be invoked before the module has begun execution.
pub fn notify_module_load(module_name: &str, address: usize, length: usize) {
    if let Some(debugger) = DEBUGGER.get() {
        debugger.notify_module_load(module_name, address, length);
    }
}

/// Polls the debugger for any pending interrupts. The routine may cause a debug
/// break.
pub fn poll_debugger() {
    if let Some(debugger) = DEBUGGER.get() {
        debugger.poll_debugger();
    }
}

/// Checks if the debugger is enabled.
pub fn enabled() -> bool {
    match DEBUGGER.get() {
        Some(debugger) => debugger.enabled(),
        None => false,
    }
}

/// Exception information for the debugger.
#[derive(Debug)]
#[allow(dead_code)]
struct ExceptionInfo {
    /// The type of exception that occurred.
    pub exception_type: ExceptionType,
    /// The system context at the time of the exception.
    pub context: ExceptionContext,
}

/// Exception type information.
#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum ExceptionType {
    /// A break due to a completed instruction step.
    Step,
    /// A break due to a breakpoint instruction.
    Breakpoint,
    /// A break due to an invalid memory access. The accessed address is provided.
    AccessViolation(usize),
    /// A general protection fault. Exception data is provided.
    GeneralProtectionFault(u64),
    /// A break due to an exception type not handled by the debugger. The exception type is provided.
    Other(u64),
}
