//! Alloc-based system state implementation providing full module tracking
//! and monitor command support.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{boxed::Box, string::String, vec::Vec};

use super::SystemStateTrait;
use crate::MonitorCommandFn;

pub(crate) struct SystemState {
    /// Tracks external monitor commands.
    monitor_commands: Vec<MonitorCallback>,
    /// Tracks loaded modules.
    modules: Vec<ModuleInfo>,
    /// Tracks module breakpoints set by the user.
    module_breakpoints: Vec<String>,
    /// If true, break on all module loads regardless of name.
    break_all: bool,
}

impl SystemState {
    /// Create a new system state.
    pub const fn new() -> Self {
        SystemState {
            modules: Vec::new(),
            monitor_commands: Vec::new(),
            module_breakpoints: Vec::new(),
            break_all: false,
        }
    }

    pub fn add_monitor_command(
        &mut self,
        command: &'static str,
        description: &'static str,
        callback: Box<MonitorCommandFn>,
    ) {
        let monitor = MonitorCallback { command, description, callback };
        self.monitor_commands.push(monitor);
    }
}

impl SystemStateTrait for SystemState {
    fn dump_monitor_commands(&self, out: &mut dyn core::fmt::Write) {
        for cmd in &self.monitor_commands {
            let _ = writeln!(out, "    {} - {}", cmd.command, cmd.description);
        }
    }

    fn handle_monitor_command(
        &self,
        command: &str,
        args: &mut core::str::SplitWhitespace<'_>,
        out: &mut dyn core::fmt::Write,
    ) -> bool {
        for monitor_cmd in &self.monitor_commands {
            if monitor_cmd.command == command {
                (monitor_cmd.callback)(args, out);
                return true;
            }
        }

        false
    }

    fn add_module(&mut self, name: &str, base: usize, size: usize) {
        self.modules.push(ModuleInfo { name: String::from(name), base, size });
    }

    fn check_module_breakpoints(&self, name: &str) -> bool {
        if self.break_all {
            return true;
        }

        let trimmed = name.trim_end_matches(".efi");
        for module in &self.module_breakpoints {
            if module.eq_ignore_ascii_case(trimmed) {
                return true;
            }
        }

        false
    }

    fn add_module_breakpoint(&mut self, name: &str) {
        let trimmed = name.trim().trim_end_matches(".efi");
        if !trimmed.is_empty() {
            self.module_breakpoints.push(String::from(trimmed));
        }
    }

    fn set_module_breakpoint_all(&mut self) {
        self.break_all = true;
    }

    fn clear_module_breakpoints(&mut self) {
        self.module_breakpoints.clear();
        self.break_all = false;
    }

    fn dump_modules(&self, out: &mut dyn core::fmt::Write, start: usize, count: usize) -> usize {
        let mut printed = 0;
        for module in self.modules.iter().skip(start) {
            let _ = writeln!(out, "\t{}: {:#x} : {:#x}", module.name, module.base, module.size);
            printed += 1;
            if printed >= count {
                break;
            }
        }
        printed
    }

    fn dump_module_breakpoints(&self, out: &mut dyn core::fmt::Write) {
        for module in &self.module_breakpoints {
            let _ = writeln!(out, "\t{module}");
        }
    }
}

/// Information about a loaded module.
struct ModuleInfo {
    name: String,
    base: usize,
    size: usize,
}

/// Stores the command and its associated callback function for monitor commands.
struct MonitorCallback {
    /// The monitor command string that triggers the callback.
    command: &'static str,
    /// The description of the monitor command.
    description: &'static str,
    /// The callback function that will be invoked when the command is executed.
    /// See [MonitorCommandFn] for more details on the function signature.
    callback: Box<MonitorCommandFn>,
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_add_module() {
        let mut state = SystemState::new();
        state.add_module("test_module", 0x1000, 0x2000);
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 0, usize::MAX);
        assert_eq!(printed, 1);
        assert!(out.contains("test_module"));
        assert!(out.contains("0x1000"));
        assert!(out.contains("0x2000"));
    }

    #[test]
    fn test_check_module_breakpoints() {
        let mut state = SystemState::new();
        state.add_module_breakpoint("test_module");
        assert!(state.check_module_breakpoints("test_module"));
        assert!(!state.check_module_breakpoints("other_module"));
    }

    #[test]
    fn test_break_on_all() {
        let mut state = SystemState::new();
        state.set_module_breakpoint_all();
        assert!(state.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_clear_module_breakpoints() {
        let mut state = SystemState::new();
        state.add_module_breakpoint("test_module");
        state.set_module_breakpoint_all();
        state.clear_module_breakpoints();
        assert!(!state.check_module_breakpoints("test_module"));
        assert!(!state.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_add_module_breakpoint() {
        let mut state = SystemState::new();
        state.add_module_breakpoint("test_module");
        let mut out = String::new();
        state.dump_module_breakpoints(&mut out);
        assert!(out.contains("test_module"));
    }

    #[test]
    fn test_handle_monitor_command() {
        let mut system_state = SystemState::new();
        let command = "test_command";
        let description = "This is a test command";
        let callback: Box<MonitorCommandFn> = Box::new(|args, out| {
            let _ = writeln!(out, "Executed with args: {:?}", args.collect::<Vec<_>>());
        });
        system_state.add_monitor_command(command, description, callback);

        let mut out = String::new();
        let args = &mut "arg1 arg2".split_whitespace();
        assert!(system_state.handle_monitor_command(command, args, &mut out));
        assert_eq!(out, "Executed with args: [\"arg1\", \"arg2\"]\n");

        assert!(!system_state.handle_monitor_command("invalid", args, &mut out));
    }

    #[test]
    fn test_add_monitor_command_with_captured_data() {
        let mut system_state = SystemState::new();
        let command = "test_command";
        let description = "This is a test command";

        let x = 5;
        let callback: Box<MonitorCommandFn> = Box::new(move |_args, out| {
            let _ = writeln!(out, "Captured state: {}", x);
        });
        system_state.add_monitor_command(command, description, callback);

        let mut out = String::new();
        let args = &mut "arg1 arg2".split_whitespace();
        assert!(system_state.handle_monitor_command(command, args, &mut out));
        assert_eq!(out, "Captured state: 5\n");

        assert!(!system_state.handle_monitor_command("invalid", args, &mut out));
    }

    #[test]
    fn test_dump_monitor_commands() {
        let mut state = SystemState::new();
        state.add_monitor_command("cmd_a", "Description A", Box::new(|_, _| {}));
        state.add_monitor_command("cmd_b", "Description B", Box::new(|_, _| {}));

        let mut out = String::new();
        state.dump_monitor_commands(&mut out);
        assert!(out.contains("cmd_a - Description A"));
        assert!(out.contains("cmd_b - Description B"));
    }

    #[test]
    fn test_dump_modules_with_start_and_count() {
        let mut state = SystemState::new();
        state.add_module("mod_a", 0x1000, 0x100);
        state.add_module("mod_b", 0x2000, 0x200);
        state.add_module("mod_c", 0x3000, 0x300);
        state.add_module("mod_d", 0x4000, 0x400);

        // Dump all modules.
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 0, usize::MAX);
        assert_eq!(printed, 4);
        assert!(out.contains("mod_a"));
        assert!(out.contains("mod_d"));

        // Dump with a count limit.
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 0, 2);
        assert_eq!(printed, 2);
        assert!(out.contains("mod_a"));
        assert!(out.contains("mod_b"));
        assert!(!out.contains("mod_c"));

        // Dump with a start offset.
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 2, usize::MAX);
        assert_eq!(printed, 2);
        assert!(!out.contains("mod_a"));
        assert!(!out.contains("mod_b"));
        assert!(out.contains("mod_c"));
        assert!(out.contains("mod_d"));

        // Dump with start and count.
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 1, 2);
        assert_eq!(printed, 2);
        assert!(!out.contains("mod_a"));
        assert!(out.contains("mod_b"));
        assert!(out.contains("mod_c"));
        assert!(!out.contains("mod_d"));

        // Start past end returns 0.
        let mut out = String::new();
        let printed = state.dump_modules(&mut out, 10, usize::MAX);
        assert_eq!(printed, 0);
        assert!(out.is_empty());
    }
}
