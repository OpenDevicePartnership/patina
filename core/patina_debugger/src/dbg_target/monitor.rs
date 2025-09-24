//! Monitor commands handling
//!
//! This module contains the implementation of the monitor command handling for the
//! Patina target.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{fmt::Write, str::SplitWhitespace};
use gdbstub::target::ext::{self, monitor_cmd::ConsoleOutput};

use crate::{arch::DebuggerArch, arch::SystemArch};

use super::PatinaTarget;

const MONITOR_HELP: &str = "
Patina Rust Debugger monitor commands:
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

impl ext::monitor_cmd::MonitorCmd for PatinaTarget {
    fn handle_monitor_cmd(&mut self, cmd: &[u8], out: ConsoleOutput<'_>) -> Result<(), Self::Error> {
        let cmd_str = core::str::from_utf8(cmd).map_err(|_| ())?;
        let mut tokens: SplitWhitespace<'_> = cmd_str.split_whitespace();

        // Wrap the output in a buffer to reduce the number of packets sent. Without
        // this formated string may send a packet for each character.
        let mut buf = MonitorBuffer::<'_, 128>::new(out);

        // Check for an offset modifier, and configure the monitor buffer accordingly.
        let cmd = match tokens.next() {
            Some(token) if token.starts_with("O[") && token.ends_with("]") => {
                let offset_str = &token[2..token.len() - 1];
                let offset: usize = offset_str.parse().ok().unwrap_or(0);
                buf.set_start_offset(offset);
                tokens.next()
            }
            other => other,
        };

        match cmd {
            Some("help") | None => {
                let _ = buf.write_str(MONITOR_HELP);
                let _ = buf.write_str("External commands:\n");
                if let Some(state) = self.system_state.try_lock() {
                    for cmd in state.monitor_commands.iter() {
                        let _ = writeln!(buf, "    {} - {}", cmd.command, cmd.description);
                    }
                };
            }
            Some("mod") => {
                self.module_cmd(&mut tokens, &mut buf);
            }
            Some("reboot") | Some("R") => {
                self.reboot = true;
                let _ = buf.write_str("System will reboot on continue.");
            }
            Some("?") => {
                let _ = write!(
                    buf,
                    concat!(
                        "Patina Rust Debugger ",
                        env!("CARGO_PKG_VERSION"),
                        "\n",
                        "Instruction Pointer: {:#X}\n",
                        "Exception Type: {}\n"
                    ),
                    self.exception_info.instruction_pointer, self.exception_info.exception_type
                );
            }
            Some("disablechecks") => {
                self.disable_checks = true;
                let _ = buf.write_str("Disabling safety checks. Good luck!");
            }
            Some("arch") => {
                SystemArch::monitor_cmd(&mut tokens, &mut buf);
            }
            Some(cmd) => match self.system_state.try_lock() {
                Some(state) => {
                    if !state.handle_monitor_command(cmd, &mut tokens, &mut buf) {
                        let _ = buf.write_str("Unknown command. Use 'help' for a list of commands.");
                    }
                }
                None => {
                    let _ = buf.write_str("ERROR: Failed to acquire system state lock for monitor callbacks!");
                }
            },
        }

        Ok(())
    }
}

impl PatinaTarget {
    fn module_cmd(&mut self, tokens: &mut SplitWhitespace<'_>, out: &mut dyn Write) {
        let mut state = match self.system_state.try_lock() {
            Some(state) => state,
            None => {
                let _ = out.write_str("ERROR: Failed to acquire modules lock!");
                return;
            }
        };

        match tokens.next() {
            Some("breakall") => {
                state.modules.break_on_all();
                let _ = out.write_str("Will break for all module loads.");
            }
            #[cfg(feature = "alloc")]
            Some("break") => {
                for module in tokens.by_ref() {
                    state.modules.add_module_breakpoint(module);
                }
                let _ = out.write_str("Module breakpoints:\n");
                for module in state.modules.get_module_breakpoints().iter() {
                    let _ = writeln!(out, "\t{module}");
                }
            }
            #[cfg(not(feature = "alloc"))]
            Some("break") => {
                let _ = out.write_str("Specific Module breakpoints only supported with 'alloc' feature.");
            }
            Some("clear") => {
                state.modules.clear_module_breakpoints();
                let _ = out.write_str("Cleared module breaks!");
            }
            Some("list") => {
                let count: usize = tokens.next().and_then(|token| token.parse().ok()).unwrap_or(usize::MAX);
                let start: usize = tokens.next().and_then(|token| token.parse().ok()).unwrap_or(0);
                let mut printed = 0;
                for module in state.modules.get_modules().iter().skip(start) {
                    let _ = writeln!(out, "\t{}: {:#x} : {:#x}", module.name, module.base, module.size);
                    printed += 1;
                    if printed >= count {
                        break;
                    }
                }

                if printed == 0 {
                    let _ = out.write_str("No modules.");
                }
            }
            _ => {
                let _ = out.write_str(MOD_HELP);
            }
        }
    }
}

/// A wrapper that batches writes. This is to reduce the number of packets
/// for a monitor transaction.
struct MonitorBuffer<'a, const N: usize> {
    buffer: [u8; N],
    pos: usize,
    start_offset: usize,
    out: ConsoleOutput<'a>,
}

impl<'a, const N: usize> MonitorBuffer<'a, N> {
    /// Creates a new BufferedWriter with the specified log level and writer.
    const fn new(out: ConsoleOutput<'a>) -> Self {
        MonitorBuffer { buffer: [0; N], pos: 0, start_offset: 0, out }
    }

    /// Sets the start offset for the buffer.
    fn set_start_offset(&mut self, offset: usize) {
        self.start_offset = offset;
    }

    /// Flushes the current buffer to the underlying writer.
    fn flush(&mut self) {
        if self.pos > 0 {
            let data = &self.buffer[0..self.pos];
            self.out.write_raw(data);
            self.pos = 0;
        }
    }
}

impl<const N: usize> Write for MonitorBuffer<'_, N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut data = s.as_bytes();
        let mut len = data.len();

        // Ignore the start offset number of characters.
        if self.start_offset > 0 {
            if self.start_offset >= len {
                self.start_offset -= len;
                return Ok(());
            } else {
                // Adjust the data to skip the start offset.
                data = &data[self.start_offset..];
                len = data.len();
                self.start_offset = 0; // Reset start offset after using it.
            }
        }

        // buffer the message if it will fit.
        if len < N {
            // If it will not fit with the current data, flush the current data.
            if len > N - self.pos {
                self.flush();
            }
            self.buffer[self.pos..self.pos + len].copy_from_slice(data);
            self.pos += len;
        } else {
            // this message is too big to buffer, flush then write the message.
            self.flush();
            self.out.write_raw(data);
        }

        Ok(())
    }
}

impl<const N: usize> Drop for MonitorBuffer<'_, N> {
    fn drop(&mut self) {
        self.flush();
    }
}
