use core::{
    arch::asm,
    num::NonZeroUsize,
    sync::atomic::{AtomicBool, Ordering},
};

use gdbstub::{
    arch::{RegId, Registers},
    target::ext::breakpoints::WatchKind,
};
use patina_internal_cpu::interrupts::ExceptionContext;
use patina_paging::PagingType;

use super::{DebuggerArch, UefiArchRegs};
use crate::{ExceptionInfo, ExceptionType, memory};

/// The "int 3" instruction.
const INT_3: u8 = 0xCC;

static POKE_TEST_MARKER: AtomicBool = AtomicBool::new(false);

/// The uninhabitable type for implementing X64 architecture.
pub enum X64Arch {}

impl gdbstub::arch::Arch for X64Arch {
    type Usize = u64;
    type BreakpointKind = usize;
    type Registers = X64CoreRegs;
    type RegId = X64CoreRegId;
}

impl DebuggerArch for X64Arch {
    const DEFAULT_EXCEPTION_TYPES: &'static [usize] = &[0, 1, 3, 4, 5, 6, 8, 11, 12, 13, 14, 17];
    const BREAKPOINT_INSTRUCTION: &'static [u8] = &[INT_3];
    const GDB_TARGET_XML: &'static str = r#"<?xml version="1.0"?><!DOCTYPE target SYSTEM "gdb-target.dtd"><target><architecture>i386:x86-64</architecture><xi:include href="registers.xml"/></target>"#;
    const GDB_REGISTERS_XML: &'static str = include_str!("xml/x64_registers.xml");

    type PageTable = patina_paging::x64::X64PageTable<memory::DebugPageAllocator>;

    #[inline(always)]
    fn breakpoint() {
        unsafe { asm!("int 3") };
    }

    fn process_entry(exception_type: u64, context: &mut ExceptionContext) -> ExceptionInfo {
        ExceptionInfo {
            exception_type: match exception_type {
                1 => {
                    context.rflags &= !0x100; // Clear the trap flag.
                    ExceptionType::Step
                }
                3 => {
                    // The "int 3" will still move the RIP forward. Step it back
                    // so the debugger shows the correct instruction.
                    context.rip -= 1;
                    ExceptionType::Breakpoint
                }
                13 => ExceptionType::GeneralProtectionFault(context.exception_data),
                14 => ExceptionType::AccessViolation(context.cr2 as usize),
                _ => ExceptionType::Other(exception_type),
            },
            instruction_pointer: context.rip,
            context: *context,
        }
    }

    fn process_exit(exception_info: &mut ExceptionInfo) {
        if exception_info.exception_type == ExceptionType::Breakpoint {
            // If the instruction is a hard-coded "int 3", then step past it on return.
            // SAFETY: Given the exception type, the RIP should be valid.
            if unsafe { *((exception_info.context.rip) as *const u8) == INT_3 } {
                exception_info.context.rip += 1;
            }
        }

        // Always invalidate the TLB in case mappings were changed. The instruction
        // cache doesn't need to be flushed since it should already be invalidated
        // by the write according to the Intel SDM Vol 3 section 11.6. The CR3
        // write is also serializing so no barriers are needed.
        unsafe {
            asm!("mov {0}, cr3", "mov cr3, {0}", out(reg) _);
        }
    }

    fn set_single_step(exception_info: &mut ExceptionInfo) {
        exception_info.context.rflags |= 0x100; // Set the trap flag.
    }

    fn initialize() {
        // Clear the hardware breakpoints.
        let mut hw_breakpoints = X64HardwareBreakpoints::read();
        hw_breakpoints.clear_all();
        hw_breakpoints.flush();
    }

    fn add_watchpoint(address: u64, length: u64, access_type: WatchKind) -> bool {
        let mut hw_breakpoints = X64HardwareBreakpoints::read();

        // First check for duplicate watchpoints.
        for i in 0..=X64HardwareBreakpoints::MAX_INDEX {
            if hw_breakpoints.get_enabled(i) && hw_breakpoints.get_address(i) == address {
                return true;
            }
        }

        for i in 0..=X64HardwareBreakpoints::MAX_INDEX {
            if !hw_breakpoints.get_enabled(i) {
                hw_breakpoints.set_address(i, address);
                hw_breakpoints.set_len(i, length);
                hw_breakpoints.set_rw(i, access_type);
                hw_breakpoints.set_enabled(i, true);
                hw_breakpoints.flush();
                return true;
            }
        }
        false
    }

    fn remove_watchpoint(address: u64, _length: u64, _access_type: WatchKind) -> bool {
        let mut hw_breakpoints = X64HardwareBreakpoints::read();
        for i in 0..=X64HardwareBreakpoints::MAX_INDEX {
            if hw_breakpoints.get_enabled(i) && hw_breakpoints.get_address(i) == address {
                hw_breakpoints.set_enabled(i, false);
                hw_breakpoints.flush();
                return true;
            }
        }
        false
    }

    fn reboot() {
        // Reset the system through the keyboard controller IO port.
        unsafe {
            asm!("cli", "out dx, al", in("dx") 0x64, in("al") 0xFE_u8);

            // this is kept in a separate loop because we don't anticipate returning from this
            loop {
                asm!("hlt");
            }
        }
    }

    fn get_page_table() -> Result<Self::PageTable, ()> {
        let cr3: u64;
        unsafe { asm!("mov {}, cr3", out(reg) cr3) };
        let cr4: u64;
        unsafe { asm!("mov {}, cr4", out(reg) cr4) };

        // Check CR4 to determine if we are using 4-level or 5-level paging.
        let paging_type = { if cr4 & (1 << 12) != 0 { PagingType::Paging5Level } else { PagingType::Paging4Level } };

        // SAFETY: The CR3 is currently being should be identity mapped and so
        // should point to a valid page table.
        unsafe {
            patina_paging::x64::X64PageTable::from_existing(cr3, memory::DebugPageAllocator {}, paging_type)
                .map_err(|_| ())
        }
    }

    fn monitor_cmd(tokens: &mut core::str::SplitWhitespace, out: &mut dyn core::fmt::Write) {
        match tokens.next() {
            Some("regs") => {
                let mut gdtr: u64 = 0;
                unsafe {
                    asm!(
                        "sgdt [{}]",
                        in(reg) &mut gdtr,
                        options(nostack, preserves_flags)
                    );
                }
                let _ = write!(out, "GDT: {gdtr:#x?}");
            }
            _ => {
                let _ = out.write_str("Unknown X64 monitor command. Supported commands: regs");
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
        unsafe { asm!("mov {}, [{}]", out(reg) _value, in(reg) address, options(nostack)) };

        // Check if the marker was cleared, indicating a page fault. Reset either way.
        if POKE_TEST_MARKER.swap(false, Ordering::SeqCst) { Ok(()) } else { Err(()) }
    }

    fn check_memory_poke_test(context: &mut ExceptionContext) -> bool {
        let poke_test = POKE_TEST_MARKER.swap(false, Ordering::SeqCst);
        if poke_test {
            // We need to increment the instruction pointer to step past the load
            context.rip += 3;
        }

        poke_test
    }
}

/// X64 core registers
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct X64CoreRegs {
    /// RAX, RBX, RCX, RDX, RSI, RDI, RBP, RSP, r8-r15
    pub regs: [u64; 16],
    /// Instruction pointer
    pub rip: u64,
    /// Status register
    pub eflags: u64,
    /// Segment registers: CS, SS, DS, ES, FS, GS
    pub segments: [u32; 6],
    /// Control registers: CR0, CR2, CR3, CR4
    pub control: [u64; 4],
    /// FPU internal registers
    pub fpu: [u32; 7],
    /// FPU registers: FOP +  ST0 through ST7
    pub st: [[u8; 10]; 9],
}

impl Registers for X64CoreRegs {
    type ProgramCounter = u64;

    fn pc(&self) -> Self::ProgramCounter {
        self.rip
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

        write_bytes!(&self.rip.to_le_bytes());
        write_bytes!(&self.eflags.to_le_bytes());

        for &segment in &self.segments {
            write_bytes!(&segment.to_le_bytes());
        }

        for &cr in &self.control {
            write_bytes!(&cr.to_le_bytes());
        }

        for &fpu_reg in &self.fpu {
            write_bytes!(&fpu_reg.to_le_bytes());
        }

        for st_reg in &self.st {
            write_bytes!(st_reg);
        }
    }

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

        for reg in &mut self.regs {
            *reg = read!(u64);
        }

        self.rip = read!(u64);
        self.eflags = read!(u64);

        for segment in &mut self.segments {
            *segment = read!(u32);
        }

        for cr in &mut self.control {
            *cr = read!(u64);
        }

        // Just skip the FPU registers, will not be written back anyways.

        Ok(())
    }
}

impl UefiArchRegs for X64CoreRegs {
    fn from_context(context: &ExceptionContext) -> Self {
        X64CoreRegs {
            regs: [
                context.rax,
                context.rbx,
                context.rcx,
                context.rdx,
                context.rsi,
                context.rdi,
                context.rbp,
                context.rsp,
                context.r8,
                context.r9,
                context.r10,
                context.r11,
                context.r12,
                context.r13,
                context.r14,
                context.r15,
            ],
            rip: context.rip,
            eflags: context.rflags,
            segments: [
                context.cs as u32,
                context.ss as u32,
                context.ds as u32,
                context.es as u32,
                context.fs as u32,
                context.gs as u32,
            ],
            control: [context.cr0, context.cr2, context.cr3, context.cr4],
            fpu: [0; 7],
            st: [[0; 10]; 9],
        }
    }

    fn write_to_context(&self, context: &mut ExceptionContext) {
        context.rax = self.regs[0];
        context.rbx = self.regs[1];
        context.rcx = self.regs[2];
        context.rdx = self.regs[3];
        context.rsi = self.regs[4];
        context.rdi = self.regs[5];
        context.rbp = self.regs[6];
        context.rsp = self.regs[7];
        context.r8 = self.regs[8];
        context.r9 = self.regs[9];
        context.r10 = self.regs[10];
        context.r11 = self.regs[11];
        context.r12 = self.regs[12];
        context.r13 = self.regs[13];
        context.r14 = self.regs[14];
        context.r15 = self.regs[15];

        context.rip = self.rip;
        context.rflags = self.eflags;

        context.cs = self.segments[0] as u64;
        context.ss = self.segments[1] as u64;
        context.ds = self.segments[2] as u64;
        context.es = self.segments[3] as u64;
        context.fs = self.segments[4] as u64;
        context.gs = self.segments[5] as u64;

        context.cr0 = self.control[0];
        context.cr2 = self.control[1];
        context.cr3 = self.control[2];
        context.cr4 = self.control[3];
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum X64CoreRegId {
    Gpr(u8),
    Rip,
    Eflags,
    Segment(u8),
    Control(u8),
    Fpu(u8),
    St(u8),
}

impl RegId for X64CoreRegId {
    fn from_raw_id(id: usize) -> Option<(Self, Option<core::num::NonZeroUsize>)> {
        let (reg_id, size) = match id {
            0..=15 => (Self::Gpr(id as u8), 8),
            16 => (Self::Rip, 8),
            17 => (Self::Eflags, 8),
            18..=23 => (Self::Segment((id - 18) as u8), 4),
            24..=28 => (Self::Control((id - 24) as u8), 8),
            29..=35 => (Self::Fpu((id - 24) as u8), 4),
            36..=44 => (Self::St((id - 31) as u8), 10),
            _ => return None,
        };

        Some((reg_id, Some(NonZeroUsize::new(size)?)))
    }
}

/// Structure for abstracting the x64 debug registers for hardware breakpoints.
struct X64HardwareBreakpoints {
    dr7: u64,
}

impl X64HardwareBreakpoints {
    pub const MAX_INDEX: usize = 3;

    // The DR7 register is as follows for relevent bits.
    //
    // 64   32     30    28     26    24     22    20     18    16     8    7    6    5    4    3    2    1    0
    // |-----|------|-----|------|-----|------|-----|------|-----|-----|----|----|----|----|----|----|----|----|
    // | ... | LEN3 | RW3 | LEN2 | RW2 | LEN1 | RW1 | LEN0 | RW0 | ... | G3 | L3 | G2 | L2 | G1 | L1 | G0 | L0 |
    // |-----|------|-----|------|-----|------|-----|------|-----|-----|----|----|----|----|----|----|----|----|
    //

    /// The first 8 bits of DR7 consist of the global and local enable bits for
    /// the 4 hardware breakpoints.
    const DR7_ENABLE_MASK: u64 = 0xFF;
    /// The Local Enable bit is every other bit starting from bit 0 for each breakpoint.
    const DR7_LOCAL_ENABLE_STRIDE: usize = 2;
    /// RW is 2 bits long
    const DR7_RW_MASK: u64 = 0x3;
    /// RW starts at bit 16
    const DR7_RW_OFFSET: usize = 16;
    /// Each RW value is 4 bits appart.
    const DR7_RW_STRIDE: usize = 4;
    /// LEN is 2 bits long
    const DR7_LEN_MASK: u64 = 0x3;
    /// LEN starts at bit 18
    const DR7_LEN_OFFSET: usize = 18;
    /// Each LEN value is 4 bits appart.
    const DR7_LEN_STRIDE: usize = 4;

    pub fn read() -> Self {
        let dr7: u64;
        unsafe { asm!("mov {}, dr7", out(reg) dr7) };
        X64HardwareBreakpoints { dr7 }
    }

    pub fn flush(&mut self) {
        unsafe { asm!("mov dr7, {}", in(reg) self.dr7) };
    }

    pub fn clear_all(&mut self) {
        self.dr7 &= !Self::DR7_ENABLE_MASK;
    }

    pub fn get_enabled(&self, index: usize) -> bool {
        (self.dr7 >> (index * Self::DR7_LOCAL_ENABLE_STRIDE)) & 0x1 != 0
    }

    pub fn set_enabled(&mut self, index: usize, enabled: bool) {
        if enabled {
            self.dr7 |= 1 << (index * Self::DR7_LOCAL_ENABLE_STRIDE);
        } else {
            self.dr7 &= !(1 << (index * Self::DR7_LOCAL_ENABLE_STRIDE));
        }
    }

    pub fn set_rw(&mut self, index: usize, kind: WatchKind) {
        self.dr7 &= !(Self::DR7_RW_MASK << (index * Self::DR7_RW_STRIDE + Self::DR7_RW_OFFSET));
        match kind {
            WatchKind::Read | WatchKind::ReadWrite => {
                self.dr7 |= 3 << (index * Self::DR7_RW_STRIDE + Self::DR7_RW_OFFSET);
            }
            WatchKind::Write => {
                self.dr7 |= 1 << (index * Self::DR7_RW_STRIDE + Self::DR7_RW_OFFSET);
            }
        }
    }

    pub fn set_len(&mut self, index: usize, len: u64) {
        let len = match len {
            1 => 0,
            2 => 1,
            4 => 2,
            _ => 3,
        };

        self.dr7 &= !(Self::DR7_LEN_MASK << (index * Self::DR7_LEN_STRIDE + Self::DR7_LEN_OFFSET));
        self.dr7 |= (len as u64) << (index * Self::DR7_LEN_STRIDE + Self::DR7_LEN_OFFSET);
    }

    pub fn get_address(&self, index: usize) -> u64 {
        let mut addr = 0;
        unsafe {
            match index {
                0 => asm!("mov {}, dr0", out(reg) addr),
                1 => asm!("mov {}, dr1", out(reg) addr),
                2 => asm!("mov {}, dr2", out(reg) addr),
                3 => asm!("mov {}, dr3", out(reg) addr),
                _ => debug_assert!(false, "Invalid x64 hardware breakpoint index."),
            }
        }
        addr
    }

    pub fn set_address(&mut self, index: usize, addr: u64) {
        unsafe {
            match index {
                0 => asm!("mov dr0, {}", in(reg) addr),
                1 => asm!("mov dr1, {}", in(reg) addr),
                2 => asm!("mov dr2, {}", in(reg) addr),
                3 => asm!("mov dr3, {}", in(reg) addr),
                _ => debug_assert!(false, "Invalid x64 hardware breakpoint index."),
            }
        }
    }
}
