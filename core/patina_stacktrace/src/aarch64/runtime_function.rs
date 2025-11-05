extern crate alloc;
use super::unwind::UnwindInfo;
use crate::{
    byte_reader::ByteReader,
    error::{Error, StResult},
    pe::PE,
    stacktrace::StackFrame,
};
use core::fmt;

/// `RuntimeFunction`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling>
#[derive(Debug, Clone)]
pub struct RuntimeFunction<'a> {
    /// Loaded image memory as a byte slice.
    image_base: &'a [u8],

    /// Image name extracted from the loaded PE image.
    image_name: Option<&'static str>,

    /// Start of the function RVA.
    pub func_start_rva: u32,

    /// End of the function RVA.
    pub end_rva: u32,

    /// Packed unwind info in AArch64 (the second word of the `.pdata` section).
    pub unwind_info: u32,
}

impl fmt::Display for RuntimeFunction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RuntimeFunction {{ func_start_rva: 0x{:08X}, end_rva: 0x{:08X}, unwind_info: 0x{:08X} }}",
            self.func_start_rva, self.end_rva, self.unwind_info
        )
    }
}

impl<'a> RuntimeFunction<'a> {
    pub fn new(
        image_base: &'a [u8],
        image_name: Option<&'static str>,
        func_start_rva: u32,
        end_rva: u32,
        unwind_info: u32,
    ) -> Self {
        Self { image_base, image_name, func_start_rva, end_rva, unwind_info }
    }

    /// Parses the unwind info referenced by this runtime function entry.
    pub fn get_unwind_info(&self) -> StResult<UnwindInfo<'_>> {
        UnwindInfo::parse(self.image_base, self.func_start_rva, self.unwind_info, self.image_name).map_err(|_| {
            Error::UnwindInfoNotFound {
                module: self.image_name,
                image_base: self.image_base.as_ptr() as u64,
                unwind_info: self.unwind_info,
            }
        })
    }

    /// Finds the runtime function corresponding to the given relative PC.
    ///
    /// # Safety
    /// The `stack_frame` must carry register values captured from execution within
    /// `pe`. Its program counter and stack pointer are used to index the PE image and
    /// to adjust return addresses; stale or invalid values can cause out-of-bounds
    /// reads when probing the exception directory.
    pub fn find_function(pe: &PE<'a>, stack_frame: &mut StackFrame) -> StResult<RuntimeFunction<'a>> {
        let mut pc_rva = (stack_frame.pc - pe.base_address) as u32;

        // SAFETY: `pe` owns the underlying PE image bytes and guarantees the header
        // ranges referenced by `get_exception_table()` remain readable here.
        let (exception_table_rva, exception_table_size) = unsafe { pe.get_exception_table()? };

        if (exception_table_size as usize) < (core::mem::size_of::<u32>() * 2) {
            return Err(Error::Malformed { module: pe.image_name, reason: "Invalid exception table size < 8 bytes" });
        }

        let mut retry = 2;
        while retry > 0 {
            // Jump to the `.pdata` section and parse the runtime function
            // records by breaking the section into 8-byte chunks (two u32
            // values), mapping each chunk to those two integers, filtering the
            // chunks that fall within the given RVA range, and constructing
            // `RuntimeFunction` instances from the results.
            let runtime_function = pe.bytes
                [exception_table_rva as usize..(exception_table_rva + exception_table_size) as usize]
                .chunks(core::mem::size_of::<u32>() * 2) // 2 u32
                .map(|ele| {
                    let func_start_rva = ele.read32(0).unwrap(); // unwrap() will work validated above
                    let unwind_info = ele.read32(4).unwrap(); // unwrap() will work validated above

                    let flag = unwind_info & 0x3;

                    let function_length = match flag {
                        // Packed unwind data not used; remaining bits point to an
                        // `.xdata` record. The length of the function can only be
                        // calculated by parsing the [0..=17] bits of the `.xdata`
                        // record. It indicates the total length of the function in
                        // bytes, divided by 4.
                        0 => {
                            let xdata_rva = unwind_info as usize;
                            let xdata_header = &pe.bytes[xdata_rva..xdata_rva + 4];
                            let xdata_header = xdata_header.read32(0).unwrap();
                            (xdata_header & 0x3FFFF) * 4
                        }
                        // Packed unwind data used with a single prolog and epilog
                        // at the beginning and end of the scope. The length of the
                        // function is specified directly in `.pdata[2]` bits
                        // [2..=12]. It indicates the total length of the function in
                        // bytes, divided by 4.
                        1 => ((unwind_info >> 2) & 0x7FF) * 4,
                        // Packed unwind data used for code without any prolog or
                        // epilog. This is useful for describing separated function
                        // segments. The length of the function is specified directly
                        // in `.pdata[2]` bits [2..=12]. It indicates the total length
                        // of the function in bytes, divided by 4.
                        2 => ((unwind_info >> 2) & 0x7FF) * 4,
                        // Reserved.
                        _ => 0,
                    };

                    let end_rva = func_start_rva + function_length;
                    (func_start_rva, end_rva, unwind_info)
                })
                .find(|ele| ele.0 <= pc_rva && pc_rva < ele.1)
                .map(|ele| RuntimeFunction::new(pe.bytes, pe.image_name, ele.0, ele.1, ele.2));

            retry -= 1;

            if let Some(runtime_function) = runtime_function {
                // Occasionally, the call stack may appear completely
                // nonsensical, for example, in frame 02 below, the return
                // address from frame 01 points to the start of func2(), while
                // the return address from frame 02 points to the middle of
                // func2().
                //
                // 01 0000000b`42b6f640 00007ff6`2976164c     sample!xxxx::do_panic::runtime
                // 02 0000000b`42b6f680 00007ff6`29761678     sample!sample::func2
                // 03 0000000b`42b6f770 00007ff6`297616cc     sample!sample::func2+0x2c
                //
                // This is because compilers are allowed to generate code that
                // does not unwind to its caller, such as when they can
                // determine that the code will inevitably panic.
                // For example:
                //
                // #[allow(unconditional_panic)]
                // fn func3() {
                //     let x = 10;
                //     let y = 0;
                //     println!("{}", x / y);  // <-- This will definitely panic
                // }
                //
                // fn func2() {
                //     println!("func2 called");
                //     func3();
                //     println!("func2 done");
                // }
                //
                // Due to the division-by-zero operation in func3(), the
                // compiler generated the following code directly between
                // func3() and func2(). ------------------------------------------------.
                //                                                                      |
                // 0:000> ub 00007ff6`2976164c                                          |
                // sample!sample::func3+0x90                                            |
                // 00007ff6`2976162c  ldr  x0,[sp,#0x10]                                |
                // 00007ff6`29761630  bl   sample!std::io::stdio::_print                |
                // 00007ff6`29761634  ldp  fp,lr,[sp,#0xE0]                             |
                // 00007ff6`29761638  add  sp,sp,#0xF0                                  |
                // 00007ff6`2976163c  ret                                               |
                //                                                                      |
                // 00007ff6`29761640  adrp x0,sample!_imp_SetUnhandledExceptionFilter <-+-----------------.
                // 00007ff6`29761644  add  x0,x0,#0x460                                                   |
                // 00007ff6`29761648  bl   sample!core::panicking::panic_const::panic_const_div_by_zero <-'
                //
                // 0:000> u 00007ff6`2976164c
                // sample!sample::func2
                // 00007ff6`2976164c  sub  sp,sp,#0x80
                // 00007ff6`29761650  stp  fp,lr,[sp,#0x70]
                // 00007ff6`29761654  add  fp,sp,#0x70
                // 00007ff6`29761658  add  x8,sp,#0x10
                // 00007ff6`2976165c  str  x8,[sp]
                // 00007ff6`29761660  adrp x0,sample!_imp_SetUnhandledExceptionFilter
                // 00007ff6`29761664  add  x0,x0,#0x4A8
                // 00007ff6`29761668  bl   sample!core::fmt::Arguments::new_const<1>
                //
                // Therefore, during stack walking, we need to adjust the PC so
                // that it points not to the beginning of func2(), but instead
                // to the previous func3() call. This is done by subtracting 4
                // (since ARM instructions are 32 bits wide).
                //
                // MSVC emits a special unwind code,
                // MSFT_OP_CLEAR_UNWOUND_TO_CALL, to explicitly indicate this,
                // but LLVM currently does not support it.
                //
                // In any case, if pc_rva points exactly to the start of the
                // prolog, subtract 4.
                if pc_rva == runtime_function.func_start_rva {
                    let decremented_pc_rva = pc_rva.checked_sub(4).ok_or(Error::Malformed {
                        module: pe.image_name,
                        reason: "pc_rva underflow while adjusting to previous instruction",
                    })?;
                    let decremented_pc = stack_frame.pc.checked_sub(4u64).ok_or(Error::Malformed {
                        module: pe.image_name,
                        reason: "pc underflow while adjusting to previous instruction",
                    })?;
                    log::debug!("    > Decrementing pc_rva {:X} -> {:X}", pc_rva, decremented_pc_rva); // debug
                    pc_rva = decremented_pc_rva;
                    stack_frame.pc = decremented_pc;
                } else {
                    log::debug!("    > Found Runtime function({}) for pc_rva {:X}", runtime_function, pc_rva); // debug
                    return Ok(runtime_function);
                }
            } else {
                // As per the AArch64 ABI, the return address stored by every bl
                // instruction points to the instruction immediately following
                // the call. However, this next instruction may occasionally
                // belong to a compiler-generated synthetic function that lacks
                // unwind information. In the example below, the call to
                // panic_fmt stores the address of the next instruction in LR.
                // Consequently, when no runtime function entry is found for the
                // current pc_rva, we adjust by subtracting 4 bytes from pc_rva
                // and retry the lookup. This ensures that we correctly resolve
                // unwind data even when the return address falls inside such
                // synthetic code.
                // qemu_sbsa_dxe_core!patina_samples::component::hello_world::func3:
                // 00000100`0006aebc f81f0ffe str         lr,[sp,#-0x10]!
                // 00000100`0006aec0 d100c3ff sub         sp,sp,#0x30
                // ....
                // 00000100`0006aeec 97fffb26 bl          qemu_sbsa_dxe_core!core::panicking::panic_fmt (00000100`00069b84)
                // qemu_sbsa_dxe_core!weak.default._ZN118_$LT$$u5b$core..mem..maybe_uninit..MaybeUninit$LT$T$GT$$u5d$$u20$as$u20$core..array..iter..iter_inner..PartialDrop$GT$12partial_drop17hc3d15e0976616072E:
                // 00000100`0006aef0 d37cedf0 lsl         xip0,x15,#4
                // 00000100`0006aef4 910003f1 mov         xip1,sp
                // 00000100`0006aef8 d1400631 sub         xip1,xip1,#1,lsl #0xC
                // 00000100`0006aefc f1400610 subs        xip0,xip0,#1,lsl #0xC
                // 00000100`0006af00 f940023f ldr         xzr,[xip1]

                let decremented_pc_rva = pc_rva.checked_sub(4).ok_or(Error::Malformed {
                    module: pe.image_name,
                    reason: "pc_rva underflow while retrying runtime function lookup",
                })?;
                let decremented_pc = stack_frame.pc.checked_sub(4u64).ok_or(Error::Malformed {
                    module: pe.image_name,
                    reason: "pc underflow while retrying runtime function lookup",
                })?;
                log::debug!(
                    "    > Runtime Function not found, retrying by decrementing pc_rva {:X} -> {:X}",
                    pc_rva,
                    decremented_pc_rva
                ); // debug
                pc_rva = decremented_pc_rva;
                stack_frame.pc = decremented_pc;
            }
        }

        Err(Error::RuntimeFunctionNotFound { module: pe.image_name, rip_rva: pc_rva })
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const IMAGE_SIZE: usize = 0x2000;
    const PE_POINTER_OFFSET: usize = 0x3C;
    const PE_HEADER_OFFSET: u32 = 0x80;
    const PE_SIGNATURE: u32 = 0x0000_4550;
    const PE64_MAGIC: u16 = 0x20B;
    const SIZE_OF_IMAGE_OFFSET: usize = 0x50;
    const EXCEPTION_TABLE_POINTER_OFFSET: usize = 0xA0;

    // Creates a fake PE image containing the supplied .pdata entries and additional sections.
    fn build_pe_bytes(entries: &[(u32, u32)], extra_sections: &[(u32, &[u8])]) -> Vec<u8> {
        let mut bytes = vec![0u8; IMAGE_SIZE];

        bytes[0..2].copy_from_slice(&0x5A4Du16.to_le_bytes());
        bytes[PE_POINTER_OFFSET..PE_POINTER_OFFSET + 4].copy_from_slice(&PE_HEADER_OFFSET.to_le_bytes());

        let header = PE_HEADER_OFFSET as usize;
        bytes[header..header + 4].copy_from_slice(&PE_SIGNATURE.to_le_bytes());
        bytes[header + 0x18..header + 0x1A].copy_from_slice(&PE64_MAGIC.to_le_bytes());
        bytes[header + SIZE_OF_IMAGE_OFFSET..header + SIZE_OF_IMAGE_OFFSET + 4]
            .copy_from_slice(&(IMAGE_SIZE as u32).to_le_bytes());

        let exception_rva = 0x400u32;
        let exception_size = (entries.len() * core::mem::size_of::<u32>() * 2) as u32;
        bytes[header + EXCEPTION_TABLE_POINTER_OFFSET..header + EXCEPTION_TABLE_POINTER_OFFSET + 4]
            .copy_from_slice(&exception_rva.to_le_bytes());
        bytes[header + EXCEPTION_TABLE_POINTER_OFFSET + 4..header + EXCEPTION_TABLE_POINTER_OFFSET + 8]
            .copy_from_slice(&exception_size.to_le_bytes());

        for (index, &(start, unwind)) in entries.iter().enumerate() {
            let offset = exception_rva as usize + index * 8;
            bytes[offset..offset + 4].copy_from_slice(&start.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&unwind.to_le_bytes());
        }

        for &(rva, data) in extra_sections {
            let offset = rva as usize;
            let end = offset + data.len();
            assert!(end <= bytes.len(), "section exceeds image bounds");
            bytes[offset..end].copy_from_slice(data);
        }

        bytes
    }

    fn make_packed_unwind_info(function_length: u32, frame_size: u32, flag: u32) -> u32 {
        let units_len = (function_length / 4) & 0x7FF;
        let frame_units = (frame_size / 16) & 0x1FF;
        let reg_f = 3u32;
        let reg_i = 2u32;
        let h = 1u32;
        let cr = 2u32;
        flag | (units_len << 2) | (reg_f << 13) | (reg_i << 16) | (h << 20) | (cr << 21) | (frame_units << 23)
    }

    #[test]
    fn get_unwind_info_returns_packed_variant() {
        let image = vec![0u8; 0x100];
        let unwind = make_packed_unwind_info(0x40, 0x80, 1);
        let runtime = RuntimeFunction::new(&image, Some("image"), 0x20, 0x60, unwind);

        match runtime.get_unwind_info().unwrap() {
            UnwindInfo::PackedUnwindInfo { flag, function_length, frame_size, .. } => {
                assert_eq!(flag, 1);
                assert_eq!(function_length, 0x40);
                assert_eq!(frame_size, 0x80);
            }
            other => panic!("unexpected unwind info variant: {other:?}"),
        }
    }

    #[test]
    fn get_unwind_info_translates_xdata_error() {
        let mut image = vec![0u8; 0x200];
        let xdata_rva = 0x80u32;
        let function_units = 0x10u32;
        let header = function_units | (1 << 18); // vers=1 -> unsupported
        image[xdata_rva as usize..xdata_rva as usize + 4].copy_from_slice(&header.to_le_bytes());

        let runtime = RuntimeFunction::new(&image, Some("image"), 0x20, 0x40, xdata_rva);
        let err = runtime.get_unwind_info().unwrap_err();
        assert!(matches!(
            err,
            Error::UnwindInfoNotFound {
                module: Some("image"),
                unwind_info,
                ..
            } if unwind_info == xdata_rva
        ));
    }

    #[test]
    fn find_function_returns_packed_entry() {
        let function_length = 0x80u32;
        let unwind = make_packed_unwind_info(function_length, 0x40, 1);
        let entries = [(0x100u32, unwind)];
        let image = build_pe_bytes(&entries, &[]);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x100 + 0x20, ..StackFrame::default() };

        let runtime = RuntimeFunction::find_function(&pe, &mut frame).expect("runtime function");
        assert_eq!(runtime.func_start_rva, 0x100);
        assert_eq!(runtime.end_rva, 0x100 + function_length);
        assert_eq!(runtime.unwind_info, unwind);
        assert_eq!(frame.pc, 0x120);
    }

    #[test]
    fn find_function_adjusts_when_pc_points_to_prolog() {
        let prev_unwind = make_packed_unwind_info(0x40, 0x40, 1);
        let curr_unwind = make_packed_unwind_info(0x40, 0x40, 1);
        let entries = [(0x0E0u32, prev_unwind), (0x120u32, curr_unwind)];
        let image = build_pe_bytes(&entries, &[]);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x120, ..StackFrame::default() };

        let runtime = RuntimeFunction::find_function(&pe, &mut frame).expect("adjusted runtime function");
        assert_eq!(runtime.func_start_rva, 0x0E0);
        assert_eq!(runtime.end_rva, 0x0E0 + 0x40);
        assert_eq!(frame.pc, 0x120 - 4);
    }

    #[test]
    fn find_function_returns_error_after_retries() {
        let unwind = make_packed_unwind_info(0x40, 0x40, 1);
        let entries = [(0x100u32, unwind)];
        let image = build_pe_bytes(&entries, &[]);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x200, ..StackFrame::default() };

        let err = RuntimeFunction::find_function(&pe, &mut frame).unwrap_err();
        assert!(matches!(err, Error::RuntimeFunctionNotFound { module: Some("image"), .. }));
        assert_eq!(frame.pc, 0x200 - 8);
    }

    #[test]
    fn find_function_reads_xdata_length() {
        let function_length = 0x60u32;
        let function_units = function_length / 4;
        let xdata_rva = 0x600u32;
        let header = function_units | (1 << 21) | (1 << 27); // e=1, one unwind code word
        let mut xdata = vec![0u8; 8];
        xdata[0..4].copy_from_slice(&header.to_le_bytes());
        xdata[4..8].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        let entries = [(0x180u32, xdata_rva)];
        let image = build_pe_bytes(&entries, &[(xdata_rva, &xdata)]);
        let pe = PE { base_address: 0, _size_of_image: image.len() as u32, image_name: Some("image"), bytes: &image };
        let mut frame = StackFrame { pc: 0x180 + 0x10, ..StackFrame::default() };

        let runtime = RuntimeFunction::find_function(&pe, &mut frame).expect("runtime function with xdata");
        assert_eq!(runtime.func_start_rva, 0x180);
        assert_eq!(runtime.end_rva, 0x180 + function_length);

        match runtime.get_unwind_info().unwrap() {
            UnwindInfo::UnpackedUnwindInfo { function_length: parsed_len, unwind_codes, .. } => {
                assert_eq!(parsed_len, function_length);
                assert_eq!(unwind_codes, &[0xAA, 0xBB, 0xCC, 0xDD]);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
