//! Monitor commands handling
//!
//! This module contains the implementation of the monitor command handling for the
//! UEFI target.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use core::{fmt::Write, str::SplitWhitespace};
use gdbstub::target::ext::{self, monitor_cmd::ConsoleOutput};

use crate::{arch::DebuggerArch, arch::SystemArch};

use super::UefiTarget;

const MONITOR_HELP: &str = "
UEFI Rust Debugger monitor commands:
    help - Display this help.
    ? - Display information about the state of the machine.
    reboot - Prepares to reboot the machine on the next continue.
    mod ... - Commands for breaking on or quering modules.
    arch ... - Architecture specific commands.
";

const MOD_HELP: &str = "
Mod commands:
    list [count] [index] - List loaded modules.
    break [module] - Set load breakpoint for a module.
    breakall - Break on all module loads.
    clear - clear all module breakpoints.
";

impl ext::monitor_cmd::MonitorCmd for UefiTarget {
    fn handle_monitor_cmd(&mut self, cmd: &[u8], mut out: ConsoleOutput<'_>) -> Result<(), Self::Error> {
        let cmd_str = core::str::from_utf8(cmd).map_err(|_| ())?;
        let mut tokens: SplitWhitespace<'_> = cmd_str.split_whitespace();
        self.monitor_buffer.reset();

        // Check for an offset modifier, and configure the monitor buffer accordingly.
        let cmd = match tokens.next() {
            Some(token) if token.starts_with("O[") && token.ends_with("]") => {
                let offset_str = &token[2..token.len() - 1];
                let offset: usize = offset_str.parse().ok().unwrap_or(0);
                self.monitor_buffer.set_start_offset(offset);
                tokens.next()
            }
            other => other,
        };

        match cmd {
            Some("help") | None => {
                let _ = self.monitor_buffer.write_str(MONITOR_HELP);
                let _ = self.monitor_buffer.write_str("External commands:\n");
                if let Some(state) = self.system_state.try_lock() {
                    for cmd in state.monitor_commands.iter() {
                        let _ = write!(self.monitor_buffer, "{}\t", cmd.command);
                    }
                };
            }
            Some("mod") => {
                self.module_cmd(&mut tokens);
            }
            Some("reboot") | Some("R") => {
                self.reboot = true;
                let _ = self.monitor_buffer.write_str("System will reboot on continue.");
            }
            Some("?") => {
                let _ = write!(
                    self.monitor_buffer,
                    "UEFI Rust Debugger.\nException Type: {:x?}",
                    self.exception_info.exception_type
                );
            }
            Some("disablechecks") => {
                self.disable_checks = true;
                let _ = self.monitor_buffer.write_str("Disabling safety checks. Good luck!");
            }
            Some("arch") => {
                SystemArch::monitor_cmd(&mut tokens, &mut self.monitor_buffer);
            }
            Some(cmd) => match self.system_state.try_lock() {
                Some(state) => {
                    if !state.handle_monitor_command(cmd, &mut tokens, &mut self.monitor_buffer) {
                        let _ = self.monitor_buffer.write_str("Unknown command. Use 'help' for a list of commands.");
                    }
                }
                None => {
                    let _ = self
                        .monitor_buffer
                        .write_str("ERROR: Failed to acquire system state lock for monitor callbacks!");
                }
            },
        }

        self.monitor_buffer.flush_to_console(&mut out);
        Ok(())
    }
}

impl UefiTarget {
    fn module_cmd(&mut self, tokens: &mut SplitWhitespace<'_>) {
        let mut state = match self.system_state.try_lock() {
            Some(state) => state,
            None => {
                let _ = self.monitor_buffer.write_str("ERROR: Failed to acquire modules lock!");
                return;
            }
        };

        match tokens.next() {
            Some("breakall") => {
                state.modules.break_on_all();
                let _ = self.monitor_buffer.write_str("Will break for all module loads.");
            }
            #[cfg(feature = "alloc")]
            Some("break") => {
                for module in tokens.by_ref() {
                    state.modules.add_module_breakpoint(module);
                }
                let _ = self.monitor_buffer.write_str("Module breakpoints:\n");
                for module in state.modules.get_module_breakpoints().iter() {
                    let _ = writeln!(self.monitor_buffer, "\t{}", module);
                }
            }
            #[cfg(not(feature = "alloc"))]
            Some("break") => {
                let _ =
                    self.monitor_buffer.write_str("Specific Module breakpoints only supported with 'alloc' feature.");
            }
            Some("clear") => {
                state.modules.clear_module_breakpoints();
                let _ = self.monitor_buffer.write_str("Cleared module breaks!");
            }
            Some("list") => {
                let count: usize = tokens.next().and_then(|token| token.parse().ok()).unwrap_or(usize::MAX);
                let start: usize = tokens.next().and_then(|token| token.parse().ok()).unwrap_or(0);
                let mut printed = 0;
                for module in state.modules.get_modules().iter().skip(start) {
                    let _ = writeln!(self.monitor_buffer, "\t{}: {:#x} : {:#x}", module.name, module.base, module.size);
                    printed += 1;
                    if printed >= count {
                        break;
                    }
                }

                if printed == 0 {
                    let _ = self.monitor_buffer.write_str("No modules.");
                }
            }
            _ => {
                let _ = self.monitor_buffer.write_str(MOD_HELP);
            }
        }
    }
}
