//! No-alloc system state implementation providing stub functionality
//! when the `alloc` feature is not available.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::SystemStateTrait;

pub(crate) struct SystemState {
    /// If true, break on all module loads regardless of name.
    break_all: bool,
}

impl SystemState {
    /// Create a new system state.
    pub const fn new() -> Self {
        SystemState { break_all: false }
    }
}

impl SystemStateTrait for SystemState {
    fn dump_monitor_commands(&self, out: &mut dyn core::fmt::Write) {
        let _ = writeln!(out, "    External monitor commands require the 'alloc' feature.");
    }

    fn handle_monitor_command(
        &self,
        _command: &str,
        _args: &mut core::str::SplitWhitespace<'_>,
        _out: &mut dyn core::fmt::Write,
    ) -> bool {
        false
    }

    fn add_module(&mut self, _name: &str, _base: usize, _size: usize) {}

    fn check_module_breakpoints(&self, _name: &str) -> bool {
        self.break_all
    }

    fn add_module_breakpoint(&mut self, _name: &str) {}

    fn set_module_breakpoint_all(&mut self) {
        self.break_all = true;
    }

    fn clear_module_breakpoints(&mut self) {
        self.break_all = false;
    }

    fn dump_modules(&self, out: &mut dyn core::fmt::Write, _start: usize, _count: usize) -> usize {
        let _ = writeln!(out, "Module tracking requires the 'alloc' feature.");
        0
    }

    fn dump_module_breakpoints(&self, out: &mut dyn core::fmt::Write) {
        let _ = writeln!(out, "Module breakpoints require the 'alloc' feature.");
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate alloc;
    use alloc::string::String;

    use super::*;

    #[test]
    fn test_add_module_is_noop() {
        let mut state = SystemState::new();
        state.add_module("test_module", 0x1000, 0x2000);
        // Without alloc, modules aren't tracked so dump returns 0.
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 0, usize::MAX);
        assert_eq!(printed, 0);
    }

    #[test]
    fn test_add_module_breakpoint_is_noop() {
        let mut state = SystemState::new();
        state.add_module_breakpoint("test_module");
        // Without alloc, specific breakpoints are not tracked.
        assert!(!state.check_module_breakpoints("test_module"));
    }

    #[test]
    fn test_module_breakpoint_all() {
        let mut state = SystemState::new();
        assert!(!state.check_module_breakpoints("any_module"));
        state.set_module_breakpoint_all();
        assert!(state.check_module_breakpoints("any_module"));
        state.clear_module_breakpoints();
        assert!(!state.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_handle_monitor_command_always_false() {
        let state = SystemState::new();
        let mut out = String::new();
        let args = &mut "arg1 arg2".split_whitespace();
        assert!(!state.handle_monitor_command("any_command", args, &mut out));
    }

    #[test]
    fn test_dump_monitor_commands() {
        let state = SystemState::new();
        let mut out = String::new();
        state.dump_monitor_commands(&mut out);
    }

    #[test]
    fn test_dump_modules() {
        let state = SystemState::new();
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 0, usize::MAX);
        assert_eq!(printed, 0);
    }

    #[test]
    fn test_dump_module_breakpoints() {
        let state = SystemState::new();
        let mut out = String::new();
        state.dump_module_breakpoints(&mut out);
    }
}
