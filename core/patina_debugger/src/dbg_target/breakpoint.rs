//! Breakpoint implementations
//!
//! This module contains the implementation for setting/removing software and
//! hardware execution breakpoints and data watchpoints.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use gdbstub::{
    arch::Arch,
    target::{ext::breakpoints, TargetResult},
};

use crate::{
    arch::{DebuggerArch, SystemArch},
    memory,
};

use super::PatinaTarget;

const MAX_BREAKPOINTS: usize = 25;
const BREAKPOINT_LENGTH: usize = SystemArch::BREAKPOINT_INSTRUCTION.len();

static BREAKPOINTS: spin::Mutex<[Breakpoint; MAX_BREAKPOINTS]> =
    spin::Mutex::new([Breakpoint::empty(); MAX_BREAKPOINTS]);

#[derive(Copy, Clone)]
struct Breakpoint {
    set: bool,
    addr: u64,
    original: [u8; BREAKPOINT_LENGTH],
}

impl Breakpoint {
    const fn empty() -> Self {
        Self { set: false, addr: 0, original: [0; BREAKPOINT_LENGTH] }
    }
}

impl breakpoints::SwBreakpoint for PatinaTarget {
    fn add_sw_breakpoint(
        &mut self,
        addr: u64,
        _kind: <Self::Arch as Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        let mut breakpoints = BREAKPOINTS.lock();
        for bp in breakpoints.iter_mut() {
            if !bp.set {
                // Save the original memory and write the breakpoint instruction.
                memory::read_memory::<SystemArch>(addr, &mut bp.original, self.disable_checks)?;
                memory::write_memory::<SystemArch>(addr, SystemArch::BREAKPOINT_INSTRUCTION)?;

                bp.addr = addr;
                bp.set = true;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn remove_sw_breakpoint(&mut self, addr: u64, _kind: usize) -> TargetResult<bool, Self> {
        let mut breakpoints = BREAKPOINTS.lock();
        for bp in breakpoints.iter_mut() {
            if bp.set && bp.addr == addr {
                // Restore the original memory.
                memory::write_memory::<SystemArch>(addr, &bp.original)?;

                bp.set = false;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl breakpoints::HwWatchpoint for PatinaTarget {
    fn add_hw_watchpoint(
        &mut self,
        addr: <Self::Arch as Arch>::Usize,
        len: <Self::Arch as Arch>::Usize,
        kind: breakpoints::WatchKind,
    ) -> TargetResult<bool, Self> {
        Ok(SystemArch::add_watchpoint(addr, len, kind))
    }

    fn remove_hw_watchpoint(
        &mut self,
        addr: <Self::Arch as Arch>::Usize,
        len: <Self::Arch as Arch>::Usize,
        kind: breakpoints::WatchKind,
    ) -> TargetResult<bool, Self> {
        Ok(SystemArch::remove_watchpoint(addr, len, kind))
    }
}
