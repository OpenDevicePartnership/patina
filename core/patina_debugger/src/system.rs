//! Implementation related to external system state such as module tracking
//! and monitor callbacks.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::{string::String, vec::Vec};

use crate::MonitorCommandFn;

pub(crate) struct SystemState {
    /// Tracks modules state.
    pub modules: Modules,
    /// Tracks external monitor commands.
    pub monitor_commands: Vec<MonitorCallback>,
}

impl SystemState {
    /// Create a new system state.
    pub const fn new() -> Self {
        SystemState { modules: Modules::new(), monitor_commands: Vec::new() }
    }

    pub fn add_monitor_command(
        &mut self,
        command: &'static str,
        description: &'static str,
        callback: MonitorCommandFn,
    ) {
        let _monitor = MonitorCallback { command, description, callback };
        cfg_if::cfg_if! {
            if #[cfg(feature = "alloc")] {
                self.monitor_commands.push(_monitor);
                log::info!("Added debugger monitor command: {}", command);
            }
            else {
                log::warn!("Monitor commands are only supported with the 'alloc' feature enabled. Will not add command: {}", command);
            }
        }
    }

    /// Add a monitor command to the system state. Returns `true` if the command
    /// was recognized, and `false` if it was not found.
    pub fn handle_monitor_command(
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
}

/// Information about a loaded module.
pub(crate) struct ModuleInfo {
    pub name: String,
    pub base: usize,
    pub size: usize,
}

/// Manages loaded modules and module breakpoints.
pub(crate) struct Modules {
    modules: Vec<ModuleInfo>,
    module_breakpoints: Vec<String>,
    break_all: bool,
}

impl Modules {
    pub const fn new() -> Self {
        Modules { modules: Vec::new(), module_breakpoints: Vec::new(), break_all: false }
    }

    pub fn add_module(&mut self, name: &str, base: usize, size: usize) {
        self.modules.push(ModuleInfo { name: String::from(name), base, size });
    }

    pub fn check_module_breakpoints(&self, name: &str) -> bool {
        if self.break_all {
            return true;
        }

        for module in &self.module_breakpoints {
            let trimmed = name.trim_end_matches(".efi");
            if module.eq_ignore_ascii_case(trimmed) {
                return true;
            }
        }

        false
    }

    #[cfg(feature = "alloc")]
    pub fn add_module_breakpoint(&mut self, name: &str) {
        let trimmed = name.trim().trim_end_matches(".efi");
        if !trimmed.is_empty() {
            self.module_breakpoints.push(String::from(trimmed));
        }
    }

    pub fn break_on_all(&mut self) {
        self.break_all = true;
    }

    pub fn clear_module_breakpoints(&mut self) {
        self.module_breakpoints.clear();
        self.break_all = false;
    }

    pub fn get_modules(&self) -> &Vec<ModuleInfo> {
        &self.modules
    }

    #[cfg(feature = "alloc")]
    pub fn get_module_breakpoints(&self) -> &Vec<String> {
        &self.module_breakpoints
    }
}

/// Stores the command and its associated callback function for monitor commands.
pub(crate) struct MonitorCallback {
    /// The monitor command string that triggers the callback.
    pub command: &'static str,
    /// The description of the monitor command.
    pub description: &'static str,
    /// The callback function that will be invoked when the command is executed.
    /// See [MonitorCommandFn] for more details on the function signature.
    pub callback: MonitorCommandFn,
}

#[cfg(feature = "alloc")]
#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_add_module() {
        let mut modules = Modules::new();
        modules.add_module("test_module", 0x1000, 0x2000);
        assert_eq!(modules.get_modules().len(), 1);
        assert_eq!(modules.get_modules()[0].name, "test_module");
        assert_eq!(modules.get_modules()[0].base, 0x1000);
        assert_eq!(modules.get_modules()[0].size, 0x2000);
    }

    #[test]
    fn test_check_module_breakpoints() {
        let mut modules = Modules::new();
        modules.add_module_breakpoint("test_module");
        assert!(modules.check_module_breakpoints("test_module"));
        assert!(!modules.check_module_breakpoints("other_module"));
    }

    #[test]
    fn test_break_on_all() {
        let mut modules = Modules::new();
        modules.break_on_all();
        assert!(modules.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_clear_module_breakpoints() {
        let mut modules = Modules::new();
        modules.add_module_breakpoint("test_module");
        modules.break_on_all();
        modules.clear_module_breakpoints();
        assert!(!modules.check_module_breakpoints("test_module"));
        assert!(!modules.check_module_breakpoints("any_module"));
    }

    #[test]
    fn test_add_module_breakpoint() {
        let mut modules = Modules::new();
        modules.add_module_breakpoint("test_module");
        assert_eq!(modules.get_module_breakpoints().len(), 1);
        assert_eq!(modules.get_module_breakpoints()[0], "test_module");
    }

    #[test]
    fn test_handle_monitor_command() {
        let mut system_state = SystemState::new();
        let command = "test_command";
        let description = "This is a test command";
        let callback: MonitorCommandFn = |args, out| {
            let _ = writeln!(out, "Executed with args: {:?}", args.collect::<Vec<_>>());
        };
        system_state.add_monitor_command(command, description, callback);

        let mut out = String::new();
        let args = &mut "arg1 arg2".split_whitespace();
        assert!(system_state.handle_monitor_command(command, args, &mut out));
        assert_eq!(out, "Executed with args: [\"arg1\", \"arg2\"]\n");

        assert!(!system_state.handle_monitor_command("invalid", args, &mut out));
    }
}
