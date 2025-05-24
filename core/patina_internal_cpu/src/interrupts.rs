//! UEFI Interrupt Module
//!
//! This module provides implementation for handling interrupts.
//!
//! This module provides implementation for [InterruptManager]. The [Interrupts] struct is the only accessible struct
//! when using this module. The other structs are architecture specific implementations and replace the [Interrupts]
//! struct at compile time based on the target architecture.
//!
//! If compiling for AARCH64, the `gic_manager` module is also available.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use mu_pi::protocols::cpu_arch::EfiSystemContext;
use patina_sdk::error::EfiError;

mod exception_handling;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "x86_64"))] {
        mod x64;
        pub type Interrupts = x64::InterruptsX64;
    } else if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        mod aarch64;
        pub type Interrupts = aarch64::InterruptsAarch64;
        pub use aarch64::gic_manager;
    } else if #[cfg(feature = "doc")] {
        mod x64;
        mod aarch64;
        mod null;
        pub use x64::InterruptsX64;
        pub use aarch64::InterruptsAarch64;
        pub use null::InterruptsNull;

        /// Type alias whose implementation is [InterruptsX64], [InterruptsAarch64], or
        /// [InterruptsNull] depending on the compilation target.
        ///
        /// This struct is for documentation purposes only. Please refer to the individual implementations for specific
        /// details.
        pub type Interrupts = InterruptsNull;

    } else {
        mod x64;
        mod aarch64;
        mod null;
        pub type Interrupts = null::InterruptsNull;
    }
}

// Declare the architecture specific context structure.
cfg_if::cfg_if! {
    if #[cfg(test)] {
        pub type ExceptionContext = null::ExceptionContextNull;
    } else if #[cfg(target_arch = "x86_64")] {
        pub type ExceptionContext = x64::ExceptionContextX64;
    } else if #[cfg(target_arch = "aarch64")] {
        pub type ExceptionContext = aarch64::ExceptionContextAArch64;
    } else  {
        pub type ExceptionContext = null::ExceptionContextNull;
    }
}

/// Type for storing the exception type. This should correspond to the architecture
/// specific interrupt type ID.
pub type ExceptionType = usize;

/// Trait for converting the architecture specific context structures into the
/// UEFI System Context structure.
pub(crate) trait EfiSystemContextFactory {
    /// Creates a EfiSystemContext wrapper pointing to the architecture specific context.
    fn create_efi_system_context(&mut self) -> EfiSystemContext;
}

/// Trait for dumping stack trace for architecture specific context.
pub(crate) trait EfiExceptionStackTrace {
    /// Dump the stack trace for architecture specific context.
    fn dump_stack_trace(&self);
}

/// Trait for structs that implement and manage interrupts.
///
/// Generic trait that can be used to abstract the architecture and platform
/// specifics for handling interrupts and exceptions. The interrupt manage will
/// configure the hardware to take interrupts, manage the entry point for interrupts,
/// and provide a callback mechanism for callers to handle exceptions.
///
pub trait InterruptManager {
    /// Registers a callback for the given exception type.
    fn register_exception_handler(&self, exception_type: ExceptionType, handler: HandlerType) -> Result<(), EfiError> {
        exception_handling::register_exception_handler(exception_type, handler)
    }

    /// Removes the registered exception handlers for the given exception type.
    fn unregister_exception_handler(&self, exception_type: ExceptionType) -> Result<(), EfiError> {
        exception_handling::unregister_exception_handler(exception_type)
    }
}

/// Type for storing the handler for a given exception.
pub enum HandlerType {
    /// No handler is registered.
    None,
    /// Handler is a UEFI compliant routine.
    UefiRoutine(mu_pi::protocols::cpu_arch::InterruptHandler),
    /// Handler is a implementation of the interrupt handler trait.
    Handler(&'static dyn InterruptHandler),
}

impl HandlerType {
    /// Returns true if the handler is None.
    fn is_none(&self) -> bool {
        matches!(self, HandlerType::None)
    }
}

/// Trait for structs to handle interrupts.
///
/// Interrupt handlers are expected to be static and are called from the exception
/// handler. Because exceptions can be reentrant, any mutable state within the
/// handler is expected to leverage internal locking.
///
pub trait InterruptHandler: Sync {
    /// Invoked when the registered interrupt is triggered.
    ///
    /// Upon return, the processor will be resumed from the exception with any
    /// changes made to the provided exception context. If it is not safe to resume,
    /// then the handler should panic or otherwise halt the system.
    ///
    fn handle_interrupt(&'static self, exception_type: ExceptionType, context: &mut ExceptionContext);
}

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "x86_64"))] {
        pub use x64::enable_interrupts;
        pub use x64::disable_interrupts;
        pub use x64::get_interrupt_state;
    } else if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        pub use aarch64::enable_interrupts;
        pub use aarch64::disable_interrupts;
        pub use aarch64::get_interrupt_state;
    } else  {
        pub use null::enable_interrupts;
        pub use null::disable_interrupts;
        pub use null::get_interrupt_state;
    }
}
