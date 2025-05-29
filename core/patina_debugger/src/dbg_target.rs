//! GDB Target Implementation
//!
//! This module contains the implementation of the GDB target for accessing the
//! system state and invoking debugging functionality.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

mod breakpoint;
mod monitor;

use gdbstub::target::{
    ext::{
        self,
        base::singlethread::{SingleThreadBase, SingleThreadResume, SingleThreadResumeOps},
        breakpoints::{self, BreakpointsOps},
    },
    Target, TargetError, TargetResult,
};
use spin::Mutex;

use crate::{
    arch::{DebuggerArch, SystemArch, UefiArchRegs},
    memory,
    system::SystemState,
    transport::BufferWriter,
    ExceptionInfo,
};

/// Addresses that windbg will attempt to read in a loop, reads from these addresses
/// will just return 0 to avoid long retry delays.
#[cfg(feature = "windbg_workarounds")]
const WINDBG_MOCK_ADDRESSES: [u64; 3] = [0xfffff78000000268, 0, 0x34c00];

/// UEFI target for GDB.
pub struct UefiTarget {
    /// Exception information for exception context.
    exception_info: ExceptionInfo,
    /// Flag to indicate if the target has been resumed.
    resume: bool,
    /// Flag to indicate if the target should reboot.
    reboot: bool,
    /// Disables safety checks for the target.
    disable_checks: bool,
    /// Tracks external system state.
    system_state: &'static Mutex<SystemState>,
    /// Buffer used for monitor calls.
    monitor_buffer: BufferWriter<'static>,
}

impl UefiTarget {
    /// Create a new UEFI target.
    pub fn new(
        exception_info: ExceptionInfo,
        system_state: &'static Mutex<SystemState>,
        monitor_buffer: &'static mut [u8],
    ) -> Self {
        UefiTarget {
            exception_info,
            resume: false,
            reboot: false,
            disable_checks: false,
            system_state,
            monitor_buffer: BufferWriter::new(monitor_buffer),
        }
    }

    /// Checks if the target has been resumed.
    pub fn is_resumed(&self) -> bool {
        self.resume
    }

    /// Checks if the target should reboot.
    pub fn reboot_on_resume(&self) -> bool {
        self.reboot
    }

    /// Consumes the target and returns the updated exception information.
    pub fn into_exception_info(self) -> ExceptionInfo {
        self.exception_info
    }
}

impl Target for UefiTarget {
    type Arch = SystemArch;
    type Error = ();

    fn base_ops(&mut self) -> gdbstub::target::ext::base::BaseOps<Self::Arch, Self::Error> {
        gdbstub::target::ext::base::BaseOps::SingleThread(self)
    }

    #[cfg(feature = "windbg_workarounds")]
    #[inline(always)]
    fn use_no_ack_mode(&self) -> bool {
        false
    }

    #[cfg(feature = "windbg_workarounds")]
    #[inline(always)]
    fn use_rle(&self) -> bool {
        false
    }

    #[cfg(feature = "windbg_workarounds")]
    #[inline(always)]
    fn use_x_upcase_packet(&self) -> bool {
        false
    }

    #[inline(always)]
    fn support_breakpoints(&mut self) -> Option<BreakpointsOps<'_, Self>> {
        Some(self)
    }

    #[inline(always)]
    fn support_monitor_cmd(&mut self) -> Option<ext::monitor_cmd::MonitorCmdOps<'_, Self>> {
        Some(self)
    }

    #[inline(always)]
    fn support_target_description_xml_override(
        &mut self,
    ) -> Option<ext::target_description_xml_override::TargetDescriptionXmlOverrideOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadBase for UefiTarget {
    fn read_registers(&mut self, regs: &mut <Self::Arch as gdbstub::arch::Arch>::Registers) -> TargetResult<(), Self> {
        regs.read_from_context(&self.exception_info.context);
        Ok(())
    }

    fn write_registers(&mut self, regs: &<Self::Arch as gdbstub::arch::Arch>::Registers) -> TargetResult<(), Self> {
        regs.write_to_context(&mut self.exception_info.context);
        Ok(())
    }

    fn read_addrs(
        &mut self,
        start_addr: <Self::Arch as gdbstub::arch::Arch>::Usize,
        data: &mut [u8],
    ) -> TargetResult<usize, Self> {
        // Windbg will try to find some well known windows structures. Return
        // 0s instead of failing to prevent long retry delays.
        #[cfg(feature = "windbg_workarounds")]
        if WINDBG_MOCK_ADDRESSES.contains(&start_addr) {
            data.fill(0);
            return Ok(data.len());
        }

        match memory::read_memory::<SystemArch>(start_addr, data, self.disable_checks) {
            Ok(bytes_read) => Ok(bytes_read),
            Err(_) => {
                log::info!("Failed to read memory at 0x{:x} : 0x{:x}", start_addr, data.len());
                Err(gdbstub::target::TargetError::NonFatal)
            }
        }
    }

    fn write_addrs(
        &mut self,
        start_addr: <Self::Arch as gdbstub::arch::Arch>::Usize,
        data: &[u8],
    ) -> TargetResult<(), Self> {
        match memory::write_memory::<SystemArch>(start_addr, data) {
            Ok(_) => Ok(()),
            Err(_) => {
                log::info!("Failed to write memory at 0x{:x} : 0x{:x}", start_addr, data.len());
                Err(gdbstub::target::TargetError::NonFatal)
            }
        }
    }

    #[inline(always)]
    fn support_resume(&mut self) -> Option<SingleThreadResumeOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadResume for UefiTarget {
    fn resume(&mut self, _signal: Option<gdbstub::common::Signal>) -> Result<(), Self::Error> {
        // The resume will happen at the top of the loop in the debugger.
        self.resume = true;
        Ok(())
    }

    #[inline(always)]
    fn support_single_step(&mut self) -> Option<ext::base::singlethread::SingleThreadSingleStepOps<'_, Self>> {
        Some(self)
    }
}

impl ext::base::singlethread::SingleThreadSingleStep for UefiTarget {
    fn step(&mut self, _signal: Option<gdbstub::common::Signal>) -> Result<(), Self::Error> {
        SystemArch::set_single_step(&mut self.exception_info);
        self.resume = true;
        Ok(())
    }
}

impl breakpoints::Breakpoints for UefiTarget {
    #[inline(always)]
    fn support_sw_breakpoint(&mut self) -> Option<breakpoints::SwBreakpointOps<'_, Self>> {
        Some(self)
    }

    #[inline(always)]
    fn support_hw_watchpoint(&mut self) -> Option<breakpoints::HwWatchpointOps<'_, Self>> {
        Some(self)
    }
}

impl ext::target_description_xml_override::TargetDescriptionXmlOverride for UefiTarget {
    fn target_description_xml(
        &self,
        annex: &[u8],
        offset: u64,
        length: usize,
        buf: &mut [u8],
    ) -> TargetResult<usize, Self> {
        let offset = offset as usize;
        let xml = match annex {
            b"target.xml" => SystemArch::GDB_TARGET_XML,
            b"registers.xml" => SystemArch::GDB_REGISTERS_XML,
            _ => return Err(TargetError::NonFatal),
        };

        let bytes = xml.trim().as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }

        let start = offset;
        let end = (start + length).min(bytes.len());
        let copy_bytes: &[u8] = &bytes[start..end];

        let copy_len = copy_bytes.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&copy_bytes[..copy_len]);
        Ok(copy_len)
    }
}
