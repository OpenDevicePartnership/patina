//! Implementation related to external system state such as module tracking
//! and monitor callbacks.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(any(feature = "alloc", test))]
mod alloc;
#[cfg(any(not(feature = "alloc"), test))]
mod no_alloc;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        pub(crate) use self::alloc::SystemState;
    } else {
        pub(crate) use no_alloc::SystemState;
    }
}

/// Trait defining the common interface for system state management.
///
/// This trait ensures consistent API between the alloc and no-alloc
/// implementations of `SystemState`.
pub(crate) trait SystemStateTrait {
    /// Writes the list of external monitor commands to the provided writer.
    fn dump_monitor_commands(&self, out: &mut dyn core::fmt::Write);

    /// Attempts to handle an external monitor command. Returns `true` if the command
    /// was recognized, and `false` if it was not found.
    fn handle_monitor_command(
        &self,
        command: &str,
        args: &mut core::str::SplitWhitespace<'_>,
        out: &mut dyn core::fmt::Write,
    ) -> bool;

    /// Adds a module to the tracked list.
    fn add_module(&mut self, name: &str, base: usize, size: usize);

    /// Checks if a module breakpoint matches the given name.
    fn check_module_breakpoints(&self, name: &str) -> bool;

    /// Adds a module breakpoint by name.
    fn add_module_breakpoint(&mut self, name: &str);

    /// Enables breaking on all module loads.
    fn set_module_breakpoint_all(&mut self);

    /// Clears all module breakpoints and disables break-all.
    fn clear_module_breakpoints(&mut self);

    /// Writes the list of loaded modules to the provided writer.
    /// Starts at `start` and writes at most `count` entries.
    /// Returns the number of modules written.
    fn dump_modules(&self, out: &mut dyn core::fmt::Write, start: usize, count: usize) -> usize;

    /// Writes the list of module breakpoints to the provided writer.
    fn dump_module_breakpoints(&self, out: &mut dyn core::fmt::Write);
}
