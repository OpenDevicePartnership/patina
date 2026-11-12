//! This file contains diagnostic helpers for decoding exceptions.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use crate::{UEFI_PAGE_MASK, UEFI_PAGE_SIZE, bit, standard::efi::protocols::debug_support};

/// This trait provides methods to decode architectural exception data. It does not touch
/// any system state or memory outside of the saved context.
///
/// # Examples
///
/// ```
/// use patina::{
///     arch::SystemContextDiagnostics,
///     standard::efi::protocols::debug_support::SystemContextX64,
/// };
///
/// fn report_page_fault(context: &SystemContextX64) {
///     context.dump_exception(14);
/// }
/// ```
pub trait SystemContextDiagnostics {
    /// Logs the exception classification, decoded error information, and saved registers.
    ///
    /// `exception_type` is the exception vector ID.
    fn dump_exception(&self, exception_type: usize);

    /// Logs the saved control and general-purpose registers.
    fn dump_system_context_registers(&self);

    /// Returns the saved fault address for a page fault, if the architecture marks it valid.
    ///
    /// Returns `None` for other exceptions. `exception_type` uses the same IDs as
    /// [`Self::dump_exception`]. The address is diagnostic data, not a pointer safe to dereference.
    fn page_fault_address(&self, exception_type: usize) -> Option<u64>;
}

/// This macro pretty prints registers in groups of four per line.
/// The expected input is a list of name, value pairs.
macro_rules! log_registers {
    ( $( $name:expr, $value:expr ),+ $(,)? ) => {
        let registers = [$(($name, $value),)+];
        for chunk in registers.chunks(4) {
            match chunk {
                [c1, c2, c3, c4] => {
                    log::error!(
                        "{:>4}:  {:#018X}   {:>4}:  {:#018X}   {:>4}:  {:#018X}   {:>4}:  {:#018X}",
                        c1.0, c1.1, c2.0, c2.1, c3.0, c3.1, c4.0, c4.1
                    );
                },
                [c1, c2, c3] => {
                    log::error!(
                        "{:>4}:  {:#018X}   {:>4}:  {:#018X}   {:>4}:  {:#018X}",
                        c1.0, c1.1, c2.0, c2.1, c3.0, c3.1
                    );
                },
                [c1, c2] => {
                    log::error!(
                        "{:>4}:  {:#018X}   {:>4}:  {:#018X}",
                        c1.0, c1.1, c2.0, c2.1,
                    );
                },
                [c1] => {
                    log::error!("{:>4}:  {:#018X}", c1.0, c1.1);
                },
                _ => {
                    log::error!("");
                }
            }
        }
    };
}

impl SystemContextDiagnostics for debug_support::SystemContextX64 {
    fn dump_exception(&self, exception_type: usize) {
        match exception_type {
            8 => {
                log::error!("EXCEPTION: DOUBLE FAULT");
                log::error!("Instruction Pointer: {:#X?}", self.rip);
                log::error!("Code Segment: {:#X?}", self.cs);
                log::error!("RFLAGS: {:#X?}", self.rflags);
                log::error!("Stack Segment: {:#X?}", self.ss);
                log::error!("Stack Pointer: {:#X?}", self.rsp);
                log::error!("Data Segment: {:#X?}", self.ds);
                log::error!("Paging Enable: {}", self.cr0 & 0x80000000 != 0);
                log::error!("Protection Enable: {}", self.cr0 & 0x00000001 != 0);
                log::error!("Page Directory Base: {:#X?}", self.cr3);
                log::error!("Control Flags (cr4): {:#X?}", self.cr4);
                log::error!("");
                self.dump_system_context_registers();
            }
            13 => {
                log::error!("EXCEPTION: GP FAULT");
                log::error!("Instruction Pointer: {:#X?}", self.rip);
                log::error!("Code Segment: {:#X?}", self.cs);
                log::error!("RFLAGS: {:#X?}", self.rflags);
                log::error!("Stack Segment: {:#X?}", self.ss);
                log::error!("Stack Pointer: {:#X?}", self.rsp);
                log::error!("Data Segment: {:#X?}", self.ds);
                log::error!("Paging Enable: {}", self.cr0 & 0x80000000 != 0);
                log::error!("Protection Enable: {}", self.cr0 & 0x00000001 != 0);
                log::error!("Page Directory Base: {:#X?}", self.cr3);
                log::error!("Control Flags (cr4): {:#X?}", self.cr4);
                interpret_gp_fault_exception_data(self.exception_data);
                log::error!("");
                self.dump_system_context_registers();
            }
            14 => {
                let rsp_page = self.rsp & !UEFI_PAGE_MASK as u64;
                let cr2_page = self.cr2 & !UEFI_PAGE_MASK as u64;
                let same_or_below =
                    (cr2_page == rsp_page) || (cr2_page == (rsp_page.wrapping_sub(UEFI_PAGE_SIZE as u64)));
                let data_access = (self.exception_data & bit!(4)) == 0;

                // Stack overflows cause page faults, let's give a hint if it looks like one.
                // The heuristic we are using is that stack overflow will either have CR2 on the same page
                // as RSP or directly below it. Sometimes RSP is decremented first, sometimes second. Stack overflows
                // are always data access, so ensure that is the case as well.
                if same_or_below && data_access {
                    log::error!("EXCEPTION: PAGE FAULT STACK OVERFLOW");
                } else {
                    log::error!("EXCEPTION: PAGE FAULT");
                }
                log::error!("Accessed Address: {:#X?}", self.cr2);
                log::error!("Paging Enabled: {}", self.cr0 & 0x80000000 != 0);
                log::error!("Instruction Pointer: {:#X?}", self.rip);
                log::error!("Code Segment: {:#X?}", self.cs);
                log::error!("RFLAGS: {:#X?}", self.rflags);
                log::error!("Stack Segment: {:#X?}", self.ss);
                log::error!("Data Segment: {:#X?}", self.ds);
                log::error!("Stack Pointer: {:#X?}", self.rsp);
                log::error!("Page Directory Base: {:#X?}", self.cr3);
                log::error!("Paging Features (cr4): {:#X?}", self.cr4);
                interpret_page_fault_exception_data(self.exception_data);
                log::error!("");
                self.dump_system_context_registers();
            }
            _ => {
                log::error!("EXCEPTION: {exception_type}");
                self.dump_system_context_registers();
            }
        }
    }

    fn dump_system_context_registers(&self) {
        log::error!("Control Registers:");
        log_registers!(
            "CR0",
            self.cr0,
            "CR2",
            self.cr2,
            "CR3",
            self.cr3,
            "CR4",
            self.cr4,
            "RIP",
            self.rip,
            "CS",
            self.cs,
            "SS",
            self.ss,
            "DS",
            self.ds,
            "RSP",
            self.rsp,
            "RFLAGS",
            self.rflags
        );
        log::error!("");
        log::error!("General-Purpose Registers:");
        log_registers!(
            "RAX", self.rax, "RBX", self.rbx, "RCX", self.rcx, "RDX", self.rdx, "RSI", self.rsi, "RDI", self.rdi,
            "RBP", self.rbp, "R8", self.r8, "R9", self.r9, "R10", self.r10, "R11", self.r11, "R12", self.r12, "R13",
            self.r13, "R14", self.r14, "R15", self.r15
        );
        log::debug!("Full Context: {self:#X?}");
    }

    fn page_fault_address(&self, exception_type: usize) -> Option<u64> {
        (exception_type == 14).then_some(self.cr2)
    }
}

impl SystemContextDiagnostics for debug_support::SystemContextAArch64 {
    fn dump_exception(&self, exception_type: usize) {
        if exception_type == 0 {
            log::error!("");
            log::error!("EXCEPTION: Synchronous Exception");
            log::error!("");

            // determine if this was a page fault
            let ec = (self.esr >> 26) & 0x3F;
            let iss = self.esr & 0xFFFFFF;
            let far_valid = (iss & bit!(10)) == 0;

            let sp_page = self.sp & !UEFI_PAGE_MASK as u64;
            let far_page = self.far & !UEFI_PAGE_MASK as u64;
            let same_or_below = (far_page == sp_page) || (far_page == (sp_page.wrapping_sub(UEFI_PAGE_SIZE as u64)));

            if ec == 0x20 || ec == 0x21 {
                // Instruction Abort from a lower EL or same EL.
                log::error!("Page Fault (Instruction Abort)");
            } else if ec == 0x24 || ec == 0x25 {
                // Data Abort from a lower EL or same EL.
                // Stack overflows cause page faults, let's give a hint if it looks like one.
                // The heuristic we are using is that stack overflow will either have FAR on the same page
                // as SP or directly below it. Sometimes SP is decremented first, sometimes second.
                if far_valid && same_or_below {
                    log::error!("Page Fault (Stack Overflow)");
                } else {
                    log::error!("Page Fault (Data Abort)");
                }
            }
            log::error!("");

            if !far_valid {
                log::error!("FAR not valid, page table won't be dumped");
            }
        } else {
            log::error!("EXCEPTION: {exception_type}");
        }
        self.dump_system_context_registers();
        log::error!("");
    }

    fn dump_system_context_registers(&self) {
        log::error!("Exception Registers:");
        log_registers!("ESR", self.esr, "ELR", self.elr, "SPSR", self.spsr, "FAR", self.far,);
        log::error!("");
        log::error!("General-Purpose Registers:");
        log_registers!(
            "x0", self.x0, "x1", self.x1, "x2", self.x2, "x3", self.x3, "x4", self.x4, "x5", self.x5, "x6", self.x6,
            "x7", self.x7, "x8", self.x8, "x9", self.x9, "x10", self.x10, "x11", self.x11, "x12", self.x12, "x13",
            self.x13, "x14", self.x14, "x15", self.x15, "x16", self.x16, "x17", self.x17, "x18", self.x18, "x19",
            self.x19, "x20", self.x20, "x21", self.x21, "x22", self.x22, "x23", self.x23, "x24", self.x24, "x25",
            self.x25, "x26", self.x26, "x27", self.x27, "x28", self.x28, "fp", self.fp, "lr", self.lr, "sp", self.sp
        );
        log::debug!("Full Context: {self:#X?}");
    }

    fn page_fault_address(&self, exception_type: usize) -> Option<u64> {
        let ec = (self.esr >> 26) & 0x3F;
        let iss = self.esr & 0xFFFFFF;
        let page_fault = ec == 0x20 || ec == 0x21 || ec == 0x24 || ec == 0x25;
        let far_valid = (iss & bit!(10)) == 0;
        (exception_type == 0 && page_fault && far_valid).then_some(self.far)
    }
}

// see Intel SDM Vol 3A section 7.15
fn interpret_page_fault_exception_data(exception_data: u64) {
    log::error!("Error Code: {exception_data:#X?}");
    if (exception_data & bit!(0)) == 0 {
        log::error!("Page not present");
    } else {
        log::error!("Page-level protection violation");
    }

    if (exception_data & bit!(1)) == 0 {
        log::error!("R/W: Read");
    } else {
        log::error!("R/W: Write");
    }

    if (exception_data & bit!(2)) != 0 {
        log::error!("User-mode access violation");
    } else {
        log::error!("Supervisor-mode access violation");
    }

    if (exception_data & bit!(3)) != 0 {
        log::error!("Reserved bit violation");
    }

    if (exception_data & bit!(4)) == 0 {
        log::error!("Data access");
    } else {
        log::error!("Instruction fetch access");
    }
}

// see Intel SDM Vol 3A section 7.15
fn interpret_gp_fault_exception_data(exception_data: u64) {
    if exception_data != 0 {
        log::error!("Segment descriptor or IDT vector number: {exception_data:#X?}");
    } else {
        log::error!("Not from loading a segment descriptor");
    }
}
