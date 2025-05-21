use crate::error::Error;
use crate::error::StResult;
use crate::pe::PE;
use core::arch::asm;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
        use crate::aarch64::runtime_function::RuntimeFunction;
    } else {
        use crate::x64::runtime_function::RuntimeFunction;
    }
}

pub struct StackTrace;
impl StackTrace {
    /// Dumps the stack trace for the given PC and SP values.
    ///
    /// # Safety
    ///
    /// This function is marked `unsafe` to indicate that the caller is
    /// responsible for validating the provided PC and SP values. Invalid values
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
    pub unsafe fn dump_with(pc: u64, sp: u64) -> StResult<()> {
        let mut pc = pc;
        let mut sp = sp;
        let mut i = 0;

        log::info!("Dumping stack trace with PC: {:#x}, SP: {:#x}", pc, sp);

        log::info!("      # Child-SP              Return Address         Call Site");

        loop {
            let no_name = "<no module>";

            let image = PE::locate_image(pc)?;

            let image_name = image.image_name.unwrap_or(no_name);

            let pc_rva = pc - image.base_address;

            let runtime_function = RuntimeFunction::find_function(&image, pc_rva as u32)?;
            let unwind_info = runtime_function.get_unwind_info()?;
            let (curr_sp, _curr_pc, prev_sp, prev_pc) = unwind_info.get_current_stack_frame(sp, pc)?;

            log::info!("      {} {:016X}      {:016X}       {}+{:X}", i, curr_sp, prev_pc, image_name, pc_rva);

            sp = prev_sp;
            pc = prev_pc;

            // We should stop when pc is zero
            if pc == 0 {
                break;
            }

            i += 1;

            // Kill switch for infinite recursive calls or for something
            // terribly bad
            if i == 20 {
                return Err(Error::StackTraceDumpFailed(image.image_name));
            }
        }

        Ok(())
    }

    /// Dumps the stack trace. This function reads the PC and SP registers and
    /// attempts to dump the call stack.
    ///
    /// # Safety
    ///
    /// It is marked `unsafe` to indicate that the caller is responsible for the
    /// validity of the PC and SP values. Invalid or corrupt machine state can
    /// result in undefined behavior, including potential page faults.
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
    pub unsafe fn dump() -> StResult<()> {
        let mut pc: u64;
        let mut sp: u64;

        cfg_if::cfg_if! {
            if #[cfg(all(target_os = "uefi", target_arch = "aarch64"))] {
                unsafe {
                    asm!(
                        "adr {pc}, .",  // Get current PC
                        "mov {sp}, sp", // Get current SP
                        pc = out(reg) pc,
                        sp = out(reg) sp,
                    );
                }
            } else {
                unsafe {
                    asm!(
                        "lea {pc}, [rip]",
                        "mov {sp}, rsp",
                        pc = out(reg) pc,
                        sp = out(reg) sp,
                    );
                }
            }
        }

        StackTrace::dump_with(pc, sp)
    }
}
