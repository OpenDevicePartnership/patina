use core::{
    arch::asm,
    num::NonZeroUsize,
    ops::Shr,
    sync::atomic::{AtomicBool, Ordering},
};

use gdbstub::arch::{RegId, Registers};
use patina::{read_sysreg, write_sysreg};
use patina_internal_cpu::interrupts::ExceptionContext;

use crate::{ExceptionInfo, ExceptionType};

use super::{DebuggerArch, UefiArchRegs};
use bitfield_struct::bitfield;

pub enum Aarch64Arch {}

const NUM_WATCHPOINTS: usize = 4;

const EC_INST_ABORT_LOWER_EL: u64 = 0x20;
const EC_INST_ABORT_CURRENT_EL: u64 = 0x21;
const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;
const EC_DATA_ABORT_CURRENT_EL: u64 = 0x25;
const EC_BREAKPOINT_LOWER_EL: u64 = 0x30;
const EC_BREAKPOINT_CURRENT_EL: u64 = 0x31;
const EC_SW_STEP_CURRENT_EL: u64 = 0x32;
const EC_SW_STEP_LOWER_EL: u64 = 0x33;
const EC_WATCHPOINT_LOWER_EL: u64 = 0x34;
const EC_WATCHPOINT_CURRENT_EL: u64 = 0x35;
const EC_BRK_INSTRUCTION: u64 = 0x3C;

const SPSR_DEBUG_MASK: u64 = 0x200;
const SPSR_SOFTWARE_STEP: u64 = 0x200000;

const MDSCR_SOFTWARE_STEP: u64 = 0x1;
const MDSCR_MDE: u64 = 0x8000;
const MDSCR_KDE: u64 = 0x2000;

const OS_LOCK_STATUS_LOCKED: u64 = 0x2;

const DAIF_DEBUG_MASK: u64 = 0x200;

static POKE_TEST_MARKER: AtomicBool = AtomicBool::new(false);

impl gdbstub::arch::Arch for Aarch64Arch {
    type Usize = u64;
    type Registers = Aarch64CoreRegs;
    type BreakpointKind = usize;
    type RegId = Aarch64CoreRegId;
}

impl DebuggerArch for Aarch64Arch {
    const DEFAULT_EXCEPTION_TYPES: &'static [usize] = &[0]; // Synchronous exception
    const BREAKPOINT_INSTRUCTION: &'static [u8] = &[0x00, 0x00, 0x20, 0xD4]; // BRK #0
    const GDB_TARGET_XML: &'static str = r#"<?xml version="1.0"?><!DOCTYPE target SYSTEM "gdb-target.dtd"><target><architecture>aarch64</architecture><xi:include href="registers.xml"/></target>"#;
    const GDB_REGISTERS_XML: &'static str = include_str!("xml/aarch64_registers.xml");

    type PageTable = patina_paging::aarch64::AArch64PageTable<patina_paging::page_allocator::PageAllocatorStub>;

    #[inline(always)]
    fn breakpoint() {
        unsafe {
            asm!("brk 0", options(nostack));
        }
    }

    fn process_entry(_exception_type: u64, context: &mut ExceptionContext) -> ExceptionInfo {
        let exception_class = (context.esr >> 26) & 0x3F;
        ExceptionInfo {
            context: *context,
            exception_type: match exception_class {
                EC_SW_STEP_CURRENT_EL | EC_SW_STEP_LOWER_EL => {
                    // Clear the step bit in the MDSCR
                    let mut mdscr_el1_reg = read_sysreg!(mdscr_el1);
                    mdscr_el1_reg &= !MDSCR_SOFTWARE_STEP;
                    write_sysreg!(reg mdscr_el1, mdscr_el1_reg);

                    ExceptionType::Step
                }
                EC_BREAKPOINT_LOWER_EL
                | EC_BREAKPOINT_CURRENT_EL
                | EC_WATCHPOINT_LOWER_EL
                | EC_WATCHPOINT_CURRENT_EL
                | EC_BRK_INSTRUCTION => ExceptionType::Breakpoint,
                EC_INST_ABORT_LOWER_EL
                | EC_INST_ABORT_CURRENT_EL
                | EC_DATA_ABORT_LOWER_EL
                | EC_DATA_ABORT_CURRENT_EL => ExceptionType::AccessViolation(context.far as usize),
                _ => ExceptionType::Other(exception_class),
            },
            instruction_pointer: context.elr,
        }
    }

    fn process_exit(exception_info: &mut ExceptionInfo) {
        if exception_info.exception_type == ExceptionType::Breakpoint {
            let elr = exception_info.context.elr as *const u8;
            let breakpoint_instruction = Self::BREAKPOINT_INSTRUCTION;
            let instruction_size = breakpoint_instruction.len();

            // If the instruction is a hard-coded "brk 0", then step past it on return.
            // SAFETY: Given the exception type, the RIP should be valid.
            if unsafe { core::slice::from_raw_parts(elr, instruction_size) } == breakpoint_instruction {
                exception_info.context.elr += instruction_size as u64;
            }

            // Always clear the ICache since the debugger may have altered instructions.
            // SAFETY: This is an architecturally defined mechanism to clear the ICache.
            unsafe {
                asm!("ic iallu", "isb sy");
            }
        }
    }

    fn set_single_step(exception_info: &mut ExceptionInfo) {
        // Clear the DEBUG bit if set. This could be set because the debug exception are
        // originally enabled from a outside of an exception. If this bit is set though
        // then the SS bit will not be respected.
        exception_info.context.spsr &= !SPSR_DEBUG_MASK;
        // Set the Software Step bit in the SPSR.
        exception_info.context.spsr |= SPSR_SOFTWARE_STEP;
        // Set the Software Step bit in the MDSCR. making sure MDE and KDE are set.
        let mut mdscr_el1_reg = read_sysreg!(mdscr_el1);
        mdscr_el1_reg |= MDSCR_SOFTWARE_STEP | MDSCR_MDE | MDSCR_KDE;
        write_sysreg!(reg mdscr_el1, mdscr_el1_reg);
    }

    fn initialize() {
        // Disable debug exceptions in DAIF while configuring
        let mut daif_reg = read_sysreg!(daif);
        daif_reg |= DAIF_DEBUG_MASK;
        write_sysreg!(reg daif, daif_reg, "isb sy");

        // Clear the OS lock if needed
        let oslsr_el1_reg = read_sysreg!(oslsr_el1);
        if oslsr_el1_reg & OS_LOCK_STATUS_LOCKED != 0 {
            write_sysreg!(reg oslar_el1, reg xzr, "isb sy");
        }

        // Enable kernel and monitor debug bits
        let mut mdscr_el1_reg = read_sysreg!(mdscr_el1);
        mdscr_el1_reg |= MDSCR_MDE | MDSCR_KDE;
        write_sysreg!(reg mdscr_el1, mdscr_el1_reg);

        // Clear watchpoints
        for i in 0..NUM_WATCHPOINTS {
            write_dbg_wcr(i, Wcr::from(0));
        }

        // Enable debug exceptions in DAIF
        daif_reg = read_sysreg!(daif);
        daif_reg &= !DAIF_DEBUG_MASK;
        write_sysreg!(reg daif, daif_reg, "isb sy");
    }

    fn add_watchpoint(address: u64, length: u64, access_type: gdbstub::target::ext::breakpoints::WatchKind) -> bool {
        let bas = Wcr::calculate_bas(length);
        let lsc = Wcr::calculate_lsc(access_type);

        // Check for duplicates
        for i in 0..NUM_WATCHPOINTS {
            let wcr = read_dbg_wcr(i);
            if wcr.enable() && wcr.bas() == bas && wcr.lsc() == lsc && read_dbg_wvr(i) == address {
                return true;
            }
        }

        // Find an empty slot
        for i in 0..NUM_WATCHPOINTS {
            let wcr = read_dbg_wcr(i);
            if !wcr.enable() {
                let mut wcr = Wcr::from(0);
                wcr.set_enable(true);
                wcr.set_bas(bas);
                wcr.set_lsc(lsc);

                // These are required to trap at all level in the normal world. Refer to
                // table D2-13 in the ARM A profile reference manual.
                wcr.set_hmc(true);
                wcr.set_ssc(0b01);
                wcr.set_pac(0b11);
                write_dbg_wvr(i, address);
                write_dbg_wcr(i, wcr);
                return true;
            }
        }

        false
    }

    fn remove_watchpoint(address: u64, length: u64, access_type: gdbstub::target::ext::breakpoints::WatchKind) -> bool {
        let bas = Wcr::calculate_bas(length);
        let lsc = Wcr::calculate_lsc(access_type);

        for i in 0..NUM_WATCHPOINTS {
            let wcr = read_dbg_wcr(i);
            if wcr.enable() && wcr.bas() == bas && wcr.lsc() == lsc && read_dbg_wvr(i) == address {
                write_dbg_wcr(i, Wcr::from(0));
                return true;
            }
        }

        false
    }

    fn reboot() {
        // reboot through PSCI SYSTEM_RESET
        // this directly loads a value into x0, but this is safe here because we are rebooting anyway
        // so this doesn't matter if we clobber x0
        unsafe {
            asm!("ldr x0, =0x84000009", "smc 0");
        }
    }

    fn get_page_table() -> Result<Self::PageTable, ()> {
        // TODO: Check for EL1?
        let ttbr0_el2 = read_sysreg!(ttbr0_el2);
        unsafe {
            patina_paging::aarch64::AArch64PageTable::from_existing(
                ttbr0_el2,
                patina_paging::page_allocator::PageAllocatorStub,
                patina_paging::PagingType::Paging4Level,
            )
            .map_err(|_| ())
        }
    }

    fn monitor_cmd(tokens: &mut core::str::SplitWhitespace, out: &mut dyn core::fmt::Write) {
        macro_rules! print_sysreg {
            ($reg:ident, $out:expr) => {{
                let value = read_sysreg!($reg);
                let _ = writeln!($out, "{}: {:#x}", stringify!($reg), value);
            }};
        }

        match tokens.next() {
            Some("regs") => {
                print_sysreg!(ttbr0_el2, out);
                print_sysreg!(mair_el2, out);
                print_sysreg!(esr_el2, out);
                print_sysreg!(far_el2, out);
                print_sysreg!(tcr_el2, out);
                print_sysreg!(sctlr_el2, out);
                print_sysreg!(spsr_el2, out);
                print_sysreg!(daif, out);
                print_sysreg!(hcr_el2, out);
            }
            Some("flush_tlb") => {
                // SAFETY: This is the architecturally defined way to flush the TLB
                unsafe {
                    asm!("tlbi alle2", "dsb sy", "isb sy", options(nostack, nomem));
                }
            }
            _ => {
                let _ = out.write_str("Unknown AArch64 monitor command. Supported commands: regs, flush_tlb");
            }
        }
    }

    #[inline(never)]
    fn memory_poke_test(address: u64) -> Result<(), ()> {
        POKE_TEST_MARKER.store(true, Ordering::SeqCst);

        // Attempt to read the address to check if it is accessible.
        // This will raise a page fault if the address is not accessible.

        let _value: u64;
        // SAFETY: The safety of this is dubious and may cause a page fault, but
        // the exception handler will catch it and resolve it by stepping beyond
        // the exception.
        unsafe { asm!("ldr {}, [{}]", out(reg) _value, in(reg) address, options(nostack)) };

        // Check if the marker was cleared, indicating a page fault. Reset either way.
        if POKE_TEST_MARKER.swap(false, Ordering::SeqCst) { Ok(()) } else { Err(()) }
    }

    fn check_memory_poke_test(context: &mut ExceptionContext) -> bool {
        let poke_test = POKE_TEST_MARKER.swap(false, Ordering::SeqCst);
        if poke_test {
            // We need to increment the instruction pointer to step past the load
            context.elr += 4;
        }

        poke_test
    }
}

/// AArch64 core registers
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Aarch64CoreRegs {
    /// X0-X30 general purpose registers
    pub regs: [u64; 31],
    /// Stack pointer
    pub sp: u64,
    /// Instruction pointer
    pub pc: u64,
    /// Floating point status
    pub fpsr: u64,
    /// PE status
    pub cpsr: u32,
}

impl Registers for Aarch64CoreRegs {
    type ProgramCounter = u64;

    fn pc(&self) -> Self::ProgramCounter {
        self.pc
    }

    fn gdb_serialize(&self, mut write_byte: impl FnMut(Option<u8>)) {
        macro_rules! write_bytes {
            ($bytes:expr) => {
                for b in $bytes {
                    write_byte(Some(*b))
                }
            };
        }

        for &reg in &self.regs {
            write_bytes!(&reg.to_le_bytes());
        }

        write_bytes!(&self.sp.to_le_bytes());
        write_bytes!(&self.pc.to_le_bytes());
        write_bytes!(&self.fpsr.to_le_bytes());
        write_bytes!(&self.cpsr.to_le_bytes());
    }

    #[allow(unused_assignments)]
    fn gdb_deserialize(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let mut offset = 0;

        macro_rules! read {
            ($t:ty) => {{
                if offset + core::mem::size_of::<$t>() > bytes.len() {
                    return Err(());
                }
                let mut array = [0u8; core::mem::size_of::<$t>()];
                array.copy_from_slice(&bytes[offset..offset + core::mem::size_of::<$t>()]);
                offset += 8;
                <$t>::from_le_bytes(array)
            }};
        }

        for reg in self.regs.iter_mut() {
            *reg = read!(u64);
        }

        self.sp = read!(u64);
        self.pc = read!(u64);
        self.fpsr = read!(u64);
        self.cpsr = read!(u32);
        Ok(())
    }
}

impl UefiArchRegs for Aarch64CoreRegs {
    fn from_context(context: &ExceptionContext) -> Self {
        Aarch64CoreRegs {
            regs: [
                context.x0,
                context.x1,
                context.x2,
                context.x3,
                context.x4,
                context.x5,
                context.x6,
                context.x7,
                context.x8,
                context.x9,
                context.x10,
                context.x11,
                context.x12,
                context.x13,
                context.x14,
                context.x15,
                context.x16,
                context.x17,
                context.x18,
                context.x19,
                context.x20,
                context.x21,
                context.x22,
                context.x23,
                context.x24,
                context.x25,
                context.x26,
                context.x27,
                context.x28,
                context.fp,
                context.lr,
            ],
            sp: context.sp,
            pc: context.elr,
            fpsr: context.fpsr,
            cpsr: context.spsr as u32,
        }
    }

    fn write_to_context(&self, context: &mut ExceptionContext) {
        context.x0 = self.regs[0];
        context.x1 = self.regs[1];
        context.x2 = self.regs[2];
        context.x3 = self.regs[3];
        context.x4 = self.regs[4];
        context.x5 = self.regs[5];
        context.x6 = self.regs[6];
        context.x7 = self.regs[7];
        context.x8 = self.regs[8];
        context.x9 = self.regs[9];
        context.x10 = self.regs[10];
        context.x11 = self.regs[11];
        context.x12 = self.regs[12];
        context.x13 = self.regs[13];
        context.x14 = self.regs[14];
        context.x15 = self.regs[15];
        context.x16 = self.regs[16];
        context.x17 = self.regs[17];
        context.x18 = self.regs[18];
        context.x19 = self.regs[19];
        context.x20 = self.regs[20];
        context.x21 = self.regs[21];
        context.x22 = self.regs[22];
        context.x23 = self.regs[23];
        context.x24 = self.regs[24];
        context.x25 = self.regs[25];
        context.x26 = self.regs[26];
        context.x27 = self.regs[27];
        context.x28 = self.regs[28];
        context.fp = self.regs[29];
        context.lr = self.regs[30];
        context.sp = self.sp;
        context.elr = self.pc;
        context.fpsr = self.fpsr;
        context.spsr = self.cpsr as u64;
    }

    fn read_register_from_context(
        context: &ExceptionContext,
        reg_id: <super::SystemArch as gdbstub::arch::Arch>::RegId,
        buf: &mut [u8],
    ) -> Result<usize, ()> {
        macro_rules! read_field {
            ($value:expr) => {{
                let size = core::mem::size_of_val(&$value);
                let bytes = $value.to_le_bytes();
                buf.get_mut(0..size).ok_or(())?.copy_from_slice(&bytes);
                Ok(bytes.len())
            }};
        }

        match reg_id {
            Aarch64CoreRegId::Gpr(n) => match n {
                0 => read_field!(context.x0),
                1 => read_field!(context.x1),
                2 => read_field!(context.x2),
                3 => read_field!(context.x3),
                4 => read_field!(context.x4),
                5 => read_field!(context.x5),
                6 => read_field!(context.x6),
                7 => read_field!(context.x7),
                8 => read_field!(context.x8),
                9 => read_field!(context.x9),
                10 => read_field!(context.x10),
                11 => read_field!(context.x11),
                12 => read_field!(context.x12),
                13 => read_field!(context.x13),
                14 => read_field!(context.x14),
                15 => read_field!(context.x15),
                16 => read_field!(context.x16),
                17 => read_field!(context.x17),
                18 => read_field!(context.x18),
                19 => read_field!(context.x19),
                20 => read_field!(context.x20),
                21 => read_field!(context.x21),
                22 => read_field!(context.x22),
                23 => read_field!(context.x23),
                24 => read_field!(context.x24),
                25 => read_field!(context.x25),
                26 => read_field!(context.x26),
                27 => read_field!(context.x27),
                28 => read_field!(context.x28),
                _ => Err(()),
            },
            Aarch64CoreRegId::Fp => {
                read_field!(context.fp)
            }
            Aarch64CoreRegId::Lr => {
                read_field!(context.lr)
            }
            Aarch64CoreRegId::Sp => {
                read_field!(context.sp)
            }
            Aarch64CoreRegId::Elr => {
                read_field!(context.elr)
            }
            Aarch64CoreRegId::Fpsr => {
                read_field!(context.fpsr)
            }
            Aarch64CoreRegId::Spsr => {
                read_field!(context.spsr as u32)
            }
        }
    }

    fn write_register_to_context(
        context: &mut ExceptionContext,
        reg_id: <super::SystemArch as gdbstub::arch::Arch>::RegId,
        buf: &[u8],
    ) -> Result<(), ()> {
        macro_rules! write_field {
            ($field:expr, $field_type:ty) => {{
                let size = core::mem::size_of::<$field_type>();
                let value = <$field_type>::from_le_bytes(buf.get(0..size).ok_or(())?.try_into().map_err(|_| ())?);
                $field = value;
            }};
        }

        match reg_id {
            Aarch64CoreRegId::Gpr(n) => match n {
                0 => write_field!(context.x0, u64),
                1 => write_field!(context.x1, u64),
                2 => write_field!(context.x2, u64),
                3 => write_field!(context.x3, u64),
                4 => write_field!(context.x4, u64),
                5 => write_field!(context.x5, u64),
                6 => write_field!(context.x6, u64),
                7 => write_field!(context.x7, u64),
                8 => write_field!(context.x8, u64),
                9 => write_field!(context.x9, u64),
                10 => write_field!(context.x10, u64),
                11 => write_field!(context.x11, u64),
                12 => write_field!(context.x12, u64),
                13 => write_field!(context.x13, u64),
                14 => write_field!(context.x14, u64),
                15 => write_field!(context.x15, u64),
                16 => write_field!(context.x16, u64),
                17 => write_field!(context.x17, u64),
                18 => write_field!(context.x18, u64),
                19 => write_field!(context.x19, u64),
                20 => write_field!(context.x20, u64),
                21 => write_field!(context.x21, u64),
                22 => write_field!(context.x22, u64),
                23 => write_field!(context.x23, u64),
                24 => write_field!(context.x24, u64),
                25 => write_field!(context.x25, u64),
                26 => write_field!(context.x26, u64),
                27 => write_field!(context.x27, u64),
                28 => write_field!(context.x28, u64),
                _ => return Err(()),
            },
            Aarch64CoreRegId::Fp => {
                write_field!(context.fp, u64);
            }
            Aarch64CoreRegId::Lr => {
                write_field!(context.lr, u64);
            }
            Aarch64CoreRegId::Sp => {
                write_field!(context.sp, u64);
            }
            Aarch64CoreRegId::Elr => {
                write_field!(context.elr, u64);
            }
            Aarch64CoreRegId::Fpsr => {
                write_field!(context.fpsr, u64);
            }
            Aarch64CoreRegId::Spsr => {
                context.spsr = u32::from_le_bytes(buf.try_into().map_err(|_| ())?) as u64;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum Aarch64CoreRegId {
    Gpr(u8),
    Fp,
    Lr,
    Sp,
    Elr,
    Fpsr,
    Spsr,
}

impl RegId for Aarch64CoreRegId {
    fn from_raw_id(id: usize) -> Option<(Self, Option<NonZeroUsize>)> {
        let (regi_id, size) = match id {
            0..=28 => (Aarch64CoreRegId::Gpr(id as u8), 8),
            29 => (Aarch64CoreRegId::Fp, 8),
            30 => (Aarch64CoreRegId::Lr, 8),
            31 => (Aarch64CoreRegId::Sp, 8),
            32 => (Aarch64CoreRegId::Elr, 8),
            33 => (Aarch64CoreRegId::Fpsr, 8),
            34 => (Aarch64CoreRegId::Spsr, 4),
            _ => return None,
        };

        Some((regi_id, Some(NonZeroUsize::new(size)?)))
    }
}

#[bitfield(u64)]
pub struct Wcr {
    pub enable: bool,
    #[bits(2)]
    pub pac: u8,
    #[bits(2)]
    pub lsc: u8,
    #[bits(8)]
    pub bas: u8,
    pub hmc: bool,
    #[bits(2)]
    pub ssc: u8,
    #[bits(4)]
    pub lbn: u8,
    pub wt: bool,
    #[bits(3)]
    pub reserved_0: u8,
    #[bits(5)]
    pub mask: u8,
    pub ssce: bool,
    #[bits(34)]
    pub reserved_1: u64,
}

impl Wcr {
    pub fn calculate_bas(length: u64) -> u8 {
        // Byte Address Select is a bitmap where each bit in Address + N up to +7.
        // shift away full 8 by (8 - count) to get this.
        0xFF_u64.shr(8 - 8_u64.min(length)) as u8
    }

    pub fn calculate_lsc(access_type: gdbstub::target::ext::breakpoints::WatchKind) -> u8 {
        match access_type {
            gdbstub::target::ext::breakpoints::WatchKind::Write => 0b10,
            gdbstub::target::ext::breakpoints::WatchKind::Read => 0b01,
            gdbstub::target::ext::breakpoints::WatchKind::ReadWrite => 0b11,
        }
    }
}

fn read_dbg_wcr(index: usize) -> Wcr {
    let value = match index {
        0 => read_sysreg!(dbgwcr0_el1),
        1 => read_sysreg!(dbgwcr1_el1),
        2 => read_sysreg!(dbgwcr2_el1),
        3 => read_sysreg!(dbgwcr3_el1),
        _ => 0,
    };
    Wcr::from(value)
}

fn write_dbg_wcr(index: usize, wcr: Wcr) {
    let value: u64 = wcr.into();
    match index {
        0 => write_sysreg!(reg dbgwcr0_el1, value, "isb sy"),
        1 => write_sysreg!(reg dbgwcr1_el1, value, "isb sy"),
        2 => write_sysreg!(reg dbgwcr2_el1, value, "isb sy"),
        3 => write_sysreg!(reg dbgwcr3_el1, value, "isb sy"),
        _ => {}
    }
}

fn read_dbg_wvr(index: usize) -> u64 {
    match index {
        0 => read_sysreg!(dbgwvr0_el1),
        1 => read_sysreg!(dbgwvr1_el1),
        2 => read_sysreg!(dbgwvr2_el1),
        3 => read_sysreg!(dbgwvr3_el1),
        _ => 0,
    }
}

fn write_dbg_wvr(index: usize, value: u64) {
    match index {
        0 => write_sysreg!(reg dbgwvr0_el1, value),
        1 => write_sysreg!(reg dbgwvr1_el1, value),
        2 => write_sysreg!(reg dbgwvr2_el1, value),
        3 => write_sysreg!(reg dbgwvr3_el1, value),
        _ => {}
    }
}
