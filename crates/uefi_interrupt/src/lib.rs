//! Crate for managing interrupts and exceptions.
//!
//! This crate contains definitions, interfaces, and implementations for managing
//! interrupts and exceptions on different architectures. The primary use case
//! this crate is through the `InterruptManager` trait and the implementations
//! of the interrupt managers.
//!
//! ## Examples and Usage
//!
//! ```
//! let interrupt_manager = InterruptManagerX64::new();
//! let handler: UefiExceptionHandler = |exception_type: u64, context: EfiSystemContext| {
//!     // Do something.
//! }
//!
//! // Initialize interrupts and exceptions.
//! interrupt_manager.initialize();
//!
//! // Set an exception handler.
//! interrupt_manager.RegisterExceptionHandler(0, handler).expect("Failed to setup exception handler!");
//!
//! // Remove interrupt handler.
//! interrupt_manager.UnregisterExceptionHandler(0).expect("Failed to remove exception handler!");
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![feature(abi_x86_interrupt)]

use efi_system_context::EfiSystemContext;
use uefi_core::error::EfiError;

pub mod efi_system_context;
mod exception_handling;

/// Exception Handler Routine.
///
/// u64 - Exception Type
/// EfiSystemContext - Pointer to the exception state.
///
pub type UefiExceptionHandler = extern "efiapi" fn(u64, EfiSystemContext);

/// Trait for structs that implement and manage interrupts.
///
/// Generic trait that can be used to abstract the architecture and platform
/// specifics for handling interrupts and exceptions. The interrupt manage will
/// configure the hardware to take interrupts, manage the entry point for interrupts,
/// and provide a callback mechanism for callers to handle exceptions.
///
pub trait InterruptManager {
    /// Initializes the hardware and software structures for interrupts and exceptions.
    ///
    /// This routine will initialize the architecture and platforms specific mechanisms
    /// for interrupts and exceptions to be taken. This routine may install some
    /// architecture specific default handlers for exceptions.
    ///
    fn initialize(&mut self) -> Result<(), EfiError>;

    /// Registers a callback for the given exception type.
    fn register_exception_handler(
        &mut self,
        exception_type: usize,
        handler: UefiExceptionHandler,
    ) -> Result<(), EfiError> {
        exception_handling::register_exception_handler(exception_type, handler)
    }

    /// Removes the registered exception handlers for the given exception type.
    fn unregister_exception_handler(&mut self, exception_type: usize) -> Result<(), EfiError> {
        exception_handling::unregister_exception_handler(exception_type)
    }
}

uefi_core::if_x64! {
    mod x64;
    pub use x64::InterruptManagerX64 as InterruptManagerX64;
}

uefi_core::if_aarch64! {
    mod aarch64;
    pub use aarch64::InterruptManagerAarch64 as InterruptManagerAarch64;
}

/// A no-op version of an interrupt manager for testing or bring-up.
#[derive(Default, Copy, Clone)]
pub struct InterruptManagerNull {}
impl InterruptManager for InterruptManagerNull {
    fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
}
