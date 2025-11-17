use crate::{error::StResult, pe::PE};
use core::{
    arch::asm,
    fmt::{self, Display, Formatter},
};

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        use crate::aarch64::runtime_function::RuntimeFunction;
    } else {
        use crate::x64::runtime_function::RuntimeFunction;
    }
}

/// Represents the CPU register state for a single stack frame.
/// This structure captures the key registers used for stack
/// unwinding.
#[derive(Default, Clone, Copy)]
pub struct StackFrame {
    /// The program counter (PC) at the time of the stack frame capture.
    pub pc: u64,

    /// The stack pointer (SP) at the time of the stack frame capture.
    pub sp: u64,

    /// The frame pointer (FP) at the time of the stack frame capture.
    pub fp: u64,
}

impl Display for StackFrame {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "PC: {:016X}, SP: {:016X}, FP: {:016X}", self.pc, self.sp, self.fp)
    }
}

impl StackFrame {
    fn end_of_stack(&self) -> bool {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "aarch64")] {
                self.pc == 0 || self.fp == 0
            } else {
                self.pc == 0
            }
        }
    }
}

/// A structure representing a stack trace.
pub struct StackTrace;

impl StackTrace {
    /// Dumps the stack trace for the given PC, SP, and FP values.
    ///
    /// # Safety
    ///
    /// This function is marked `unsafe` to indicate that the caller is
    /// responsible for validating the provided PC, SP, and FP values. Invalid
    /// values can result in undefined behavior, including potential page
    /// faults.
    ///
    /// ```text
    /// # Child-SP              Return Address         Call Site
    /// 0 0000005E2AEFFC00      00007FFB10CB4508       aarch64+44B0
    /// 1 0000005E2AEFFC20      00007FFB10CB45A0       aarch64+4508
    /// 2 0000005E2AEFFC40      00007FFB10CB4640       aarch64+45A0
    /// 3 0000005E2AEFFC60      00007FFB10CB46D4       aarch64+4640
    /// 4 0000005E2AEFFC90      00007FF760473B98       aarch64+46D4
    /// 5 0000005E2AEFFCB0      00007FFB8F062310       patina_stacktrace-45f5092641a5979a+3B98
    /// 6 0000005E2AEFFD10      00007FFB8FF95AEC       kernel32+12310
    /// 7 0000005E2AEFFD50      0000000000000000       ntdll+75AEC
    /// ```
    #[coverage(off)]
    #[inline(never)]
    pub unsafe fn dump_with(mut stack_frame: StackFrame) -> StResult<()> {
        let mut i = 0;

        log::warn!("Dumping stack trace with {}", stack_frame);

        log::warn!("      # Child-SP              Return Address         Call Site");

        loop {
            let no_name = "<no module>";

            // SAFETY: The caller of `dump_with` supplies a valid PC captured from
            // a live stack frame. We rely on that guarantee to probe memory for the
            // surrounding PE image without triggering undefined behavior.
            let image = unsafe { PE::locate_image(stack_frame.pc) }?;
            log::debug!("{image}");

            let runtime_function = RuntimeFunction::find_function(&image, &mut stack_frame)?;
            let unwind_info = runtime_function.get_unwind_info()?;
            let prev_stack_frame = unwind_info.get_previous_stack_frame(&stack_frame)?;

            let pc_rva = stack_frame.pc - image.base_address;
            let image_name = image.image_name.unwrap_or(no_name);
            log::warn!(
                "     {i:>2} {:016X}      {:016X}       {image_name}+{pc_rva:X}",
                stack_frame.sp,
                prev_stack_frame.pc
            );
            log::debug!("======================================================================="); // debug

            if prev_stack_frame.pc == stack_frame.pc {
                log::error!("PC didn't change. Possible stack corruption detected. Stopping stack trace.");
                break;
            }

            stack_frame = prev_stack_frame;

            if stack_frame.end_of_stack() {
                log::warn!("Finished dumping stack trace");
                break;
            }

            i += 1;
        }

        Ok(())
    }

    /// Dumps the stack trace. This function reads the PC, SP, and FP values and
    /// attempts to dump the call stack.
    ///
    /// # Safety
    ///
    /// It is marked `unsafe` to indicate that the caller is responsible for the
    /// validity of the PC, SP, and FP values. Invalid or corrupt machine state
    /// can result in undefined behavior, including potential page faults.
    ///
    /// ```text
    /// # Child-SP              Return Address         Call Site
    /// 0 0000005E2AEFFC00      00007FFB10CB4508       aarch64+44B0
    /// 1 0000005E2AEFFC20      00007FFB10CB45A0       aarch64+4508
    /// 2 0000005E2AEFFC40      00007FFB10CB4640       aarch64+45A0
    /// 3 0000005E2AEFFC60      00007FFB10CB46D4       aarch64+4640
    /// 4 0000005E2AEFFC90      00007FF760473B98       aarch64+46D4
    /// 5 0000005E2AEFFCB0      00007FFB8F062310       patina_stacktrace-45f5092641a5979a+3B98
    /// 6 0000005E2AEFFD10      00007FFB8FF95AEC       kernel32+12310
    /// 7 0000005E2AEFFD50      0000000000000000       ntdll+75AEC
    /// ```
    #[coverage(off)]
    #[inline(never)]
    pub unsafe fn dump() -> StResult<()> {
        let mut stack_frame = StackFrame::default();

        cfg_if::cfg_if! {
            if #[cfg(target_arch = "aarch64")] {
                // SAFETY: Inline assembly reads the current program counter
                // (PC), stack pointer (SP), and frame pointer (FP). It does not
                // modify memory or violate Rust safety invariants. The caller
                // must ensure that using these register values is safe.
                // SAFETY: Reading PC/SP/FP does not mutate memory and the hardware
                // guarantees those registers exist on aarch64.
                unsafe {
                    asm!(
                        "adr {pc}, .",   // Get current PC (program counter)
                        "mov {sp}, sp",  // Get current SP (stack pointer)
                        "mov {fp}, x29", // Get current FP (frame pointer)
                        pc = out(reg) stack_frame.pc,
                        sp = out(reg) stack_frame.sp,
                        fp = out(reg) stack_frame.fp,
                    );
                }
            } else {
                // SAFETY: Inline assembly reads the current program counter
                // (PC), stack pointer (SP), and frame pointer (FP) on x86_64.
                // It does not modify the memory or violate Rust safety
                // invariants. The caller must ensure that using these register
                // values is safe.
                // SAFETY: Reading PC/SP/FP does not mutate memory and the hardware
                // guarantees those registers exist on x86_64.
                unsafe {
                    asm!(
                        "lea {pc}, [rip]", // Get current PC (program counter)
                        "mov {sp}, rsp",   // Get current SP (stack pointer)
                        "mov {fp}, rbp",   // Get current FP (frame pointer) - Not used
                        pc = out(reg) stack_frame.pc,
                        sp = out(reg) stack_frame.sp,
                        fp = out(reg) stack_frame.fp,
                    );
                }
            }
        }

        // SAFETY: `stack_frame` originates from trusted register snapshots; all
        // invariants for `dump_with` are upheld locally before forwarding.
        unsafe { StackTrace::dump_with(stack_frame) }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::StackFrame;

    #[test]
    fn display_formats_hex_values() {
        let frame = StackFrame { pc: 0x1234, sp: 0xABCD, fp: 0xFEDC };
        assert_eq!(format!("{frame}"), "PC: 0000000000001234, SP: 000000000000ABCD, FP: 000000000000FEDC");
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn end_of_stack_aarch64_checks_pc_or_fp() {
        let mut frame = StackFrame { pc: 0x1, sp: 0, fp: 0x1 };
        assert!(!frame.end_of_stack());

        frame.pc = 0;
        assert!(frame.end_of_stack());

        frame.pc = 0x1;
        frame.fp = 0;
        assert!(frame.end_of_stack());
    }

    #[cfg(not(target_arch = "aarch64"))]
    #[test]
    fn end_of_stack_non_aarch64_checks_pc_only() {
        let mut frame = StackFrame { pc: 0x1, sp: 0, fp: 0 };
        assert!(!frame.end_of_stack());

        frame.pc = 0;
        assert!(frame.end_of_stack());
    }
}
