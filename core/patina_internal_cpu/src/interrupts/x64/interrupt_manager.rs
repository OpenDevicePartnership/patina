//! X64 Interrupt manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    bit,
    error::EfiError,
    pi::protocol::cpu_arch::EfiSystemContext,
    {UEFI_PAGE_MASK, UEFI_PAGE_SIZE},
};
#[cfg(target_arch = "x86_64")]
use patina_mtrr::{Mtrr, error::MtrrError};
use patina_paging::PageTable;
use patina_stacktrace::{StackFrame, StackTrace};

use crate::interrupts::{
    EfiExceptionInfoDump, ExceptionType, HandlerType, InterruptHandler, InterruptManager, x64::ExceptionContextX64,
};

/// X64 Implementation of the `InterruptManager`.
///
/// An x64 version of the `InterruptManager` for managing IDT based interrupts.
///
#[derive(Default, Copy, Clone)]
pub struct InterruptsX64 {}

#[allow(dead_code)]
impl InterruptsX64 {
    /// Creates a new instance of the x64 implementation of the `InterruptManager`.
    pub const fn new() -> Self {
        Self {}
    }

    /// Initializes the hardware and software structures for interrupts and exceptions.
    ///
    /// This routine will initialize the architecture and platforms specific mechanisms
    /// for interrupts and exceptions to be taken. This routine may install some
    /// architecture specific default handlers for exceptions.
    ///
    /// Default and platform handlers are registered before activating the GDT and IDT.
    /// Virtual-wire mode is then programmed on non-CC guests.
    pub fn initialize(
        &mut self,
        exception_handlers: &[(ExceptionType, &'static dyn InterruptHandler)],
    ) -> Result<(), EfiError> {
        self.register_initial_handlers(exception_handlers)?;

        #[cfg(target_os = "uefi")]
        {
            patina::arch::disable_interrupts();
            Self::install_gdt_and_idt();
            patina::arch::enable_interrupts();
            Self::initialize_virtual_wire_mode()?;
        }

        Ok(())
    }

    /// Registers exception handlers and loads the IDT without touching the GDT, TSS, FPU, or APIC.
    ///
    /// Intended for environments such as the MM supervisor that program their own GDT and TSS.
    /// The default #DF, #GP, and #PF handlers and the supplied platform handlers are registered
    /// before the IDT is loaded, so no vector is live without a handler. The interrupt-enable state
    /// on entry is preserved.
    ///
    /// Every gate uses `CODE_SELECTOR` (0x38), and the #DF and #PF gates use IST 1. The active GDT
    /// must define a 64-bit ring-0 code segment at that selector, and the TSS loaded in TR must
    /// provide a valid IST 1 stack.
    ///
    /// # Errors
    ///
    /// Returns any error produced while registering a platform handler.
    pub fn initialize_idt(
        &mut self,
        exception_handlers: &[(ExceptionType, &'static dyn InterruptHandler)],
    ) -> Result<(), EfiError> {
        self.register_initial_handlers(exception_handlers)?;

        #[cfg(target_os = "uefi")]
        {
            let interrupts_were_enabled = patina::arch::interrupts_enabled();
            patina::arch::disable_interrupts();
            Self::install_idt();
            if interrupts_were_enabled {
                patina::arch::enable_interrupts();
            }
        }

        Ok(())
    }

    /// Registers the default #DF, #GP, and #PF handlers followed by the supplied platform handlers.
    fn register_initial_handlers(
        &mut self,
        exception_handlers: &[(ExceptionType, &'static dyn InterruptHandler)],
    ) -> Result<(), EfiError> {
        self.register_exception_handler(8, HandlerType::UefiRoutine(double_fault_handler))
            .expect("Failed to install default exception handler!");
        self.register_exception_handler(13, HandlerType::UefiRoutine(general_protection_fault_handler))
            .expect("Failed to install default exception handler!");
        self.register_exception_handler(14, HandlerType::UefiRoutine(page_fault_handler))
            .expect("Failed to install default exception handler!");

        for &(exception_type, handler) in exception_handlers {
            self.register_exception_handler(exception_type, HandlerType::Handler(handler))?;
        }

        Ok(())
    }

    /// Programs virtual-wire mode on non-CC guests after platform exception handlers are installed. CC guests do not
    /// require this and may fail if it is attempted. If this function is called from a CC guest it will simply return
    /// `Ok()`.
    #[cfg(target_os = "uefi")]
    fn initialize_virtual_wire_mode() -> Result<(), EfiError> {
        if patina::arch::is_cc_guest() {
            return Ok(());
        }
        super::apic::initialize_virtual_wire_mode()
    }

    #[cfg_attr(coverage, coverage(off))]
    #[cfg(target_os = "uefi")]
    /// Prepares both descriptor tables, loads the GDT, reloads segments and the TSS, then loads the IDT.
    fn install_gdt_and_idt() {
        let gdtr = crate::gdt::descriptor();
        let gdt_ptr = gdtr.base as *const u64;
        assert!((gdt_ptr as usize) < patina::SIZE_4GB, "GDT above 4GB, MP services will fail");

        let idtr = super::idt::prepare_idt();
        let idt_ptr = idtr.base as *const u64;
        assert!((idt_ptr as usize) < patina::SIZE_4GB, "IDT above 4GB, MP services will fail");

        // SAFETY: Both descriptors reference fully initialized static tables. The GDT defines
        // the code, data, and TSS selectors used below and by the IDT. Initialization is exclusive.
        unsafe {
            core::arch::asm!(
                "lgdt [{gdtr}]",
                "push {sel}",
                "lea {tmp}, [rip + 2f]",
                "push {tmp}",
                "retfq",
                "2:",
                "mov ss, {data:x}",
                "mov ds, {data:x}",
                "mov es, {data:x}",
                "mov fs, {data:x}",
                "mov gs, {data:x}",
                "ltr {tss:x}",
                "lidt [{idtr}]",
                gdtr = in(reg) &raw const gdtr,
                idtr = in(reg) &raw const idtr,
                sel = in(reg) u64::from(crate::gdt::CODE_SELECTOR),
                data = in(reg) crate::gdt::DATA_SELECTOR,
                tss = in(reg) crate::gdt::TSS_SELECTOR,
                tmp = out(reg) _,
                options(preserves_flags),
            );
        }

        log::info!("Loaded GDT @ {gdt_ptr:p}");
        log::info!("Loaded IDT @ {idt_ptr:p}");
    }

    #[cfg_attr(coverage, coverage(off))]
    #[cfg(target_os = "uefi")]
    /// Prepares and loads only the IDT. The active GDT and TSS must define the selector and IST it uses.
    fn install_idt() {
        let idtr = super::idt::prepare_idt();

        // SAFETY: `idtr` describes the fully initialized static IDT. The active GDT must define a 64-bit
        // code segment at `CODE_SELECTOR`, and the TSS in TR must provide a valid IST 1 stack.
        unsafe {
            core::arch::asm!("lidt [{idtr}]", idtr = in(reg) &raw const idtr, options(readonly, nostack, preserves_flags));
        }

        let idt_ptr = idtr.base as *const u64;
        log::info!("Loaded IDT @ {idt_ptr:p}");
    }
}

impl InterruptManager for InterruptsX64 {}

#[cfg_attr(coverage, coverage(off))]
/// Default handler for double faults.
extern "efiapi" fn double_fault_handler(_exception_type: isize, context: EfiSystemContext) {
    // SAFETY: We don't have any choice here, we are in an exception and have to do our best
    // to report. The system is dead anyway.
    let x64_context = unsafe { context.system_context_x64.as_ref().unwrap() };
    log::error!("EXCEPTION: DOUBLE FAULT");
    log::error!("Instruction Pointer: {:#X?}", x64_context.rip);
    log::error!("Code Segment: {:#X?}", x64_context.cs);
    log::error!("RFLAGS: {:#X?}", x64_context.rflags);
    log::error!("Stack Segment: {:#X?}", x64_context.ss);
    log::error!("Stack Pointer: {:#X?}", x64_context.rsp);
    log::error!("Data Segment: {:#X?}", x64_context.ds);
    log::error!("Paging Enable: {}", x64_context.cr0 & 0x80000000 != 0);
    log::error!("Protection Enable: {}", x64_context.cr0 & 0x00000001 != 0);
    log::error!("Page Directory Base: {:#X?}", x64_context.cr3);
    log::error!("Control Flags (cr4): {:#X?}", x64_context.cr4);

    log::error!("");

    (x64_context as &ExceptionContextX64).dump_system_context_registers();

    panic!("EXCEPTION: Double Fault");
}

#[cfg_attr(coverage, coverage(off))]
/// Default handler for GP faults.
extern "efiapi" fn general_protection_fault_handler(_exception_type: isize, context: EfiSystemContext) {
    // SAFETY: We don't have any choice here, we are in an exception and have to do our best
    // to report. The system is dead anyway.
    let x64_context = unsafe { context.system_context_x64.as_ref().unwrap() };
    log::error!("EXCEPTION: GP FAULT");
    log::error!("Instruction Pointer: {:#X?}", x64_context.rip);
    log::error!("Code Segment: {:#X?}", x64_context.cs);
    log::error!("RFLAGS: {:#X?}", x64_context.rflags);
    log::error!("Stack Segment: {:#X?}", x64_context.ss);
    log::error!("Stack Pointer: {:#X?}", x64_context.rsp);
    log::error!("Data Segment: {:#X?}", x64_context.ds);
    log::error!("Paging Enable: {}", x64_context.cr0 & 0x80000000 != 0);
    log::error!("Protection Enable: {}", x64_context.cr0 & 0x00000001 != 0);
    log::error!("Page Directory Base: {:#X?}", x64_context.cr3);
    log::error!("Control Flags (cr4): {:#X?}", x64_context.cr4);
    interpret_gp_fault_exception_data(x64_context.exception_data);

    log::error!("");

    (x64_context as &ExceptionContextX64).dump_system_context_registers();

    log::error!("Dumping Exception Stack Trace:");
    let stack_frame = StackFrame { pc: x64_context.rip, sp: x64_context.rsp, fp: x64_context.rbp };
    // SAFETY: Called during exception handling with CPU context registers. The exception context
    // is considered valid to dump at this time.
    if let Err(err) = unsafe { StackTrace::dump_with(stack_frame) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: GP FAULT");
}

#[cfg_attr(coverage, coverage(off))]
/// Default handler for page faults.
extern "efiapi" fn page_fault_handler(_exception_type: isize, context: EfiSystemContext) {
    // SAFETY: We don't have any choice here, we are in an exception and have to do our best
    // to report. The system is dead anyway.
    let x64_context = unsafe { context.system_context_x64.as_ref().unwrap() };

    let rsp_page = x64_context.rsp & !UEFI_PAGE_MASK as u64;
    let cr2_page = x64_context.cr2 & !UEFI_PAGE_MASK as u64;
    let same_or_below = (cr2_page == rsp_page) || (cr2_page == (rsp_page.wrapping_sub(UEFI_PAGE_SIZE as u64)));
    let data_access = (x64_context.exception_data & bit!(4)) == 0;

    // Stack overflows cause page faults, let's give a hint if it looks like one.
    // The heuristic we are using is that stack overflow will either have CR2 on the same page
    // as RSP or directly below it. Sometimes RSP is decremented first, sometimes second. Stack overflows
    // are always data access, so ensure that is the case as well.
    if same_or_below && data_access {
        log::error!("EXCEPTION: PAGE FAULT STACK OVERFLOW");
    } else {
        log::error!("EXCEPTION: PAGE FAULT");
    }

    log::error!("Accessed Address: {:#X?}", x64_context.cr2);
    log::error!("Paging Enabled: {}", x64_context.cr0 & 0x80000000 != 0);
    log::error!("Instruction Pointer: {:#X?}", x64_context.rip);
    log::error!("Code Segment: {:#X?}", x64_context.cs);
    log::error!("RFLAGS: {:#X?}", x64_context.rflags);
    log::error!("Stack Segment: {:#X?}", x64_context.ss);
    log::error!("Data Segment: {:#X?}", x64_context.ds);
    log::error!("Stack Pointer: {:#X?}", x64_context.rsp);
    log::error!("Page Directory Base: {:#X?}", x64_context.cr3);
    log::error!("Paging Features (cr4): {:#X?}", x64_context.cr4);
    interpret_page_fault_exception_data(x64_context.exception_data);

    log::error!("");

    (x64_context as &ExceptionContextX64).dump_system_context_registers();

    dump_pte(x64_context.cr2);

    log::error!("Dumping Exception Stack Trace:");
    let stack_frame = StackFrame { pc: x64_context.rip, sp: x64_context.rsp, fp: x64_context.rbp };
    // SAFETY: Called during page fault exception handling with CPU context registers. The exception context
    // is considered valid to dump at this time.
    if let Err(err) = unsafe { StackTrace::dump_with(stack_frame) } {
        log::error!("StackTrace: {err}");
    }

    panic!("EXCEPTION: PAGE FAULT");
}

#[cfg_attr(coverage, coverage(off))]
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

#[cfg_attr(coverage, coverage(off))]
// see Intel SDM Vol 3A section 7.15
fn interpret_gp_fault_exception_data(exception_data: u64) {
    if exception_data != 0 {
        log::error!("Segment descriptor or IDT vector number: {exception_data:#X?}");
    } else {
        log::error!("Not from loading a segment descriptor");
    }
}

// There is no value in coverage for this function.
#[cfg_attr(coverage, coverage(off))]
/// Dumps the page table entries for the given CR2. This uses the active page tables as they should be the same as the
/// ones at the time of the fault.
///
fn dump_pte(cr2: u64) {
    if let Ok(pt) =
        // SAFETY: We are in an exception handler and want to dump the page tables, there is no other active code
        // modifying the page tables.
        unsafe {
            patina_paging::x64::X64PageTable::open_active(patina_paging::page_allocator::PageAllocatorStub)
        }
    {
        let _ = pt.dump_page_tables(cr2 & !(UEFI_PAGE_MASK as u64), UEFI_PAGE_SIZE as u64);
    }

    // we don't carry the caching attributes in the page table, so get them from the MTRRs
    #[cfg(target_arch = "x86_64")]
    {
        let mtrr = patina_mtrr::create_mtrr_lib(0);

        log::error!("");
        match mtrr.get_memory_attribute(cr2) {
            Ok(cache_attribute) => {
                log::error!("MTRR Cache Attribute: {cache_attribute}");
            }
            Err(MtrrError::MtrrNotSupported) => {
                log::error!("MTRRs Unsupported");
            }
            Err(_) => {
                log::error!("MTRR query failed");
            }
        }
        log::error!("");
    }
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod test {
    extern crate std;

    use serial_test::serial;

    use super::*;

    #[test]
    #[serial(exception_handlers)]
    fn test_interrupts_x64() {
        let mut interrupts = InterruptsX64::new();
        assert!(interrupts.initialize(&[]).is_ok());
        assert!(interrupts.unregister_exception_handler(8).is_ok());
        assert!(interrupts.unregister_exception_handler(13).is_ok());
        assert!(interrupts.unregister_exception_handler(14).is_ok());
    }

    #[test]
    #[serial(exception_handlers)]
    fn test_interrupts_x64_platform_handlers() {
        struct PlatformHandler;

        impl InterruptHandler for PlatformHandler {
            fn handle_interrupt(
                &'static self,
                _exception_type: ExceptionType,
                _context: &mut crate::interrupts::ExceptionContext,
            ) {
            }
        }

        static HANDLER: PlatformHandler = PlatformHandler;
        let mut interrupts = InterruptsX64::new();
        assert!(interrupts.initialize(&[(20, &HANDLER)]).is_ok());
        assert!(interrupts.unregister_exception_handler(20).is_ok());
        for exception_type in [8, 13, 14] {
            assert!(interrupts.unregister_exception_handler(exception_type).is_ok());
        }
    }

    #[test]
    #[serial(exception_handlers)]
    fn test_interrupts_x64_initialize_idt() {
        struct PlatformHandler;

        impl InterruptHandler for PlatformHandler {
            fn handle_interrupt(
                &'static self,
                _exception_type: ExceptionType,
                _context: &mut crate::interrupts::ExceptionContext,
            ) {
            }
        }

        static HANDLER: PlatformHandler = PlatformHandler;
        let mut interrupts = InterruptsX64::new();
        assert!(interrupts.initialize_idt(&[(20, &HANDLER)]).is_ok());
        for exception_type in [8, 13, 14, 20] {
            assert!(interrupts.unregister_exception_handler(exception_type).is_ok());
        }
    }

    #[test]
    #[serial(exception_handlers)]
    fn test_interrupts_x64_initialize_idt_duplicate_platform_handler() {
        struct PlatformHandler;

        impl InterruptHandler for PlatformHandler {
            fn handle_interrupt(
                &'static self,
                _exception_type: ExceptionType,
                _context: &mut crate::interrupts::ExceptionContext,
            ) {
            }
        }

        static HANDLER: PlatformHandler = PlatformHandler;
        let mut interrupts = InterruptsX64::new();
        assert!(matches!(interrupts.initialize_idt(&[(20, &HANDLER), (20, &HANDLER)]), Err(EfiError::AlreadyStarted)));
        for exception_type in [8, 13, 14, 20] {
            assert!(interrupts.unregister_exception_handler(exception_type).is_ok());
        }
    }
}
