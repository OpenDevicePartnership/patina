//! This file contains diagnostic helpers for decoding exceptions.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

mod sealed {
    pub trait Sealed {}

    impl Sealed for crate::standard::efi::protocols::debug_support::SystemContextX64 {}
    impl Sealed for crate::standard::efi::protocols::debug_support::SystemContextAArch64 {}
}

/// This trait provides methods to decode architectural exception data. It does not touch
/// any system state or memory outside of the saved context.
///
/// # Examples
///
/// ```
/// use patina::{
///     arch::SystemContextDiagnostics,
///     standard::efi::protocols::debug_support::SystemContextX64,
/// };
///
/// # #[cfg(any(not(target_os = "uefi"), target_arch = "x86_64"))]
/// fn report_page_fault(context: &SystemContextX64) {
///     context.dump_exception(14);
/// }
/// ```
pub trait SystemContextDiagnostics: sealed::Sealed {
    /// Logs the exception classification, decoded error information, and saved registers.
    ///
    /// `exception_type` is the exception vector ID.
    fn dump_exception(&self, exception_type: usize);

    /// Logs the saved control and general-purpose registers.
    fn dump_system_context_registers(&self);

    /// Returns the saved fault address for a page fault, if the architecture marks it valid.
    ///
    /// Returns `None` for other exceptions. `exception_type` uses the same IDs as
    /// [`Self::dump_exception`]. The address is diagnostic data, not a pointer safe to dereference.
    fn page_fault_address(&self, exception_type: usize) -> Option<u64>;
}

/// This macro pretty prints registers in groups of four per line.
/// The expected input is a list of name, value pairs.
#[cfg(target_os = "uefi")]
macro_rules! log_registers {
    ( $( $name:expr, $value:expr ),+ $(,)? ) => {
        let registers = [$(($name, $value),)+];
        for chunk in registers.chunks(4) {
            match chunk {
                [c1, c2, c3, c4] => {
                    log::error!(
                        "{:>4}:  {:#018X}   {:>4}:  {:#018X}   {:>4}:  {:#018X}   {:>4}:  {:#018X}",
                        c1.0, c1.1, c2.0, c2.1, c3.0, c3.1, c4.0, c4.1
                    );
                },
                [c1, c2, c3] => {
                    log::error!(
                        "{:>4}:  {:#018X}   {:>4}:  {:#018X}   {:>4}:  {:#018X}",
                        c1.0, c1.1, c2.0, c2.1, c3.0, c3.1
                    );
                },
                [c1, c2] => {
                    log::error!(
                        "{:>4}:  {:#018X}   {:>4}:  {:#018X}",
                        c1.0, c1.1, c2.0, c2.1,
                    );
                },
                [c1] => {
                    log::error!("{:>4}:  {:#018X}", c1.0, c1.1);
                },
                _ => {
                    log::error!("");
                }
            }
        }
    };
}

#[cfg(target_os = "uefi")]
pub(super) use log_registers;
