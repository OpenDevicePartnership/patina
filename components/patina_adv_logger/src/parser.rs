//! Parsing logic for the Advanced Logger to be used in the standard environment.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use crate::memory_log::{AdvLoggerInfo, AdvLoggerMessageEntry};
use alloc::format;
use core::str;

/// Parser for the Advanced Logger buffer.
pub struct Parser<'a> {
    data: &'a [u8],
    entry_meta: bool,
}

impl<'a> Parser<'a> {
    /// Creates a new `Parser` instance with the provided data slice from an advanced
    /// logger buffer.
    pub const fn new(data: &'a [u8]) -> Self {
        Parser { data, entry_meta: true }
    }

    /// Sets whether to print entry metadata (level, phase, timestamp) in the log output.
    pub const fn with_entry_metadata(mut self, with_meta: bool) -> Self {
        self.entry_meta = with_meta;
        self
    }

    fn get_log(&self) -> Result<&AdvLoggerInfo, &'static str> {
        if self.data.is_empty() {
            return Err("Data is empty");
        }

        if self.data.len() < size_of::<AdvLoggerInfo>() {
            return Err("Data is too small to contain a valid memory log");
        }

        // SAFETY: We confirmed the byte array is at least large enough to hold
        //         the `AdvLoggerInfo` struct, we will confirm the total size
        //         afterwards.
        let log = unsafe {
            let address = self.data.as_ptr() as u64;
            match AdvLoggerInfo::adopt_memory_log(address) {
                Some(log) => log,
                None => return Err("Failed to parse memory log"),
            }
        };

        // Check that the entire log described is present to make sure we don't
        // read past the end of the buffer.
        if self.data.len() < log.get_log_buffer_size() {
            return Err("Buffer size is smaller than the log size");
        }

        Ok(log)
    }

    /// Writes the log header information to the provided output stream.
    pub fn write_header<W: std::io::Write>(&self, out: &mut W) -> Result<(), &'static str> {
        let log = self.get_log()?;
        let header = &format!("{:#x?}\n", log);
        out.write(header.as_bytes()).map_err(|_| "Failed to write to output.")?;
        Ok(())
    }

    /// Writes the log entries to the provided output stream.
    pub fn write_log<W: std::io::Write>(&self, out: &mut W) -> Result<(), &'static str> {
        let log = self.get_log()?;
        let frequency = log.get_frequency();

        let mut carry_entry: Option<&AdvLoggerMessageEntry> = None;
        for entry in log.iter() {
            if let Some(carry) = carry_entry {
                // If the carry entry is not the same boot phase, drop it. This
                // means messages from different environments are interleaved.
                if carry.boot_phase != entry.boot_phase {
                    carry_entry = None;
                }
            }

            if self.entry_meta && carry_entry.is_none() {
                let timestamp = entry.timestamp;
                let meta_data = &format!(
                    "{:<5}|{:<8}|{}| ",
                    level_name(entry.level),
                    phase_name(entry.boot_phase),
                    get_time_str(timestamp, frequency)
                );
                out.write(meta_data.as_bytes()).map_err(|_| "Failed to write to output.")?;
            }

            let msg = entry.get_message();
            out.write(msg).map_err(|_| "Failed to write to output.")?;
            carry_entry = if !msg.is_empty() && msg[msg.len() - 1] == b'\n' { None } else { Some(entry) };
        }

        Ok(())
    }
}

fn get_time_str(timestamp: u64, frequency: u64) -> String {
    // If there is no frequency, return the raw timestamp.
    if frequency == 0 {
        return format!("{}", timestamp);
    }

    // Convert the timestamp to a human-readable format
    let mut time_ms = timestamp / (frequency / 1000);

    let milliseconds = time_ms % 1000;
    time_ms /= 1000;
    let seconds = time_ms % 60;
    time_ms /= 60;
    let minutes = time_ms % 60;
    time_ms /= 60;
    let hours = time_ms % 24;
    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, milliseconds)
}

fn phase_name(phase: u16) -> &'static str {
    match phase {
        0 => "UNSPEC",
        1 => "SEC",
        2 => "PEI",
        3 => "PEI64",
        4 => "DXE",
        5 => "RUNTIME",
        6 => "MM_CORE",
        7 => "MM",
        8 => "SMM_CORE",
        9 => "SMM",
        10 => "TFA",
        11 => "CNT",
        _ => "UNKNOWN",
    }
}

fn level_name(level: u32) -> &'static str {
    if level & crate::memory_log::DEBUG_LEVEL_ERROR != 0 {
        "ERR"
    } else if level & crate::memory_log::DEBUG_LEVEL_WARNING != 0 {
        "WARN"
    } else if level & crate::memory_log::DEBUG_LEVEL_INFO != 0 {
        "INFO"
    } else if level & crate::memory_log::DEBUG_LEVEL_VERBOSE != 0 {
        "VERB"
    } else {
        "UNKN"
    }
}
