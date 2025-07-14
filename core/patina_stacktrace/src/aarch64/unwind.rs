/// Module to parse the AArch64 unwind data from the .pdata and .xdata sections.
/// The main goal of this module is to calculate the appropriate stack pointer
/// offsets by undoing the operations performed by the prolog of a given
/// function. These offsets are then used to identify the previous stack frame's
/// stack pointer (sp) and instruction pointer (pc). Unlike x64, AArch64
/// requires more involved unwinding operations.
///
/// Unwind info in AArch64 comes in two flavors:
/// 1. Packed unwind info for canonical functions, encoded from 2-31 bits.
/// 2. .xdata-based unpacked unwind info, where the RVA of .xdata is present in
///    0-31 bits.
///
/// .pdata entry structure:
///                 .-------------------------------------------------------------------------------------------------------------------------------.
///                 | 3 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
///                 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
///                 .-------------------------------------------------------------------------------------------------------------------------------.
///                 |                                                      Function start RVA                                                       |
///                 |-------------------------------------------------------------------------------------------------------------------------------|
///                 |                                               .xdata rva/packed unwind info                                           | flag  |
///                 '-------------------------------------------------------------------------------------------------------------------------------'
///
/// .xdata structure:
///                 .-------------------------------------------------------------------------------------------------------------------------------.
///                 | 3 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
///                 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
///        .------->.-------------------------------------------------------------------------------------------------------------------------------.
///     Header 1    | Code Words        | Epilog count      | E | X | Vers  | Function Length                                                       |
///        .------->|-------------------------------------------------------------------------------------------------------------------------------|
///     Header 2    | (Reserved)                    | (Extended Code Words)         | (Extended Epilog Count)                                       |
///        .------->|-------------------------------------------------------------------------------------------------------------------------------|
///  Epilog Scopes  | Epilog Start Index                    | (reserved)    | Epilog Start Offset                                                   |
///                 |-------------------------------------------------------------------------------------------------------------------------------|
///                 | (Possibly followed by additional epilog scopes)                                                                               |
///        .------->|-------------------------------------------------------------------------------------------------------------------------------|
///  Unwind codes   | Unwind Code 3                 | Unwind Code 2                 | Unwind Code 1                 | Unwind Code 0                 |
///                 |-------------------------------------------------------------------------------------------------------------------------------|
///                 | (Possibly followed by additional words with unwind codes)                                                                     |
///                 |-------------------------------------------------------------------------------------------------------------------------------|
///                 | Exception Handler RVA (if X = 1)                                                                                              |
///                 |-------------------------------------------------------------------------------------------------------------------------------|
///                 | (Possibly followed by data needed by the exception handler)                                                                   |
///                 '-------------------------------------------------------------------------------------------------------------------------------'
///
///  - 'Header 2' only exists if the number of code words and the epilog count
///    is 0, i.e., the number of code words and epilogs is more than 31 (5
///    bits). In this case, Extended Code Words and Extended Epilog Count can be
///    used.
///  - The number of Epilog Scopes is determined either by the Epilog Count or
///    by the Extended Epilog Count.
///      - If E == 1, there will be zero epilog scopes. The Epilog Count
///        specifies the index of the first unwind code that describes the one
///        and only epilog.
///      - If E == 0, the Epilog Count specifies the total number of epilog
///        scopes.
///      - This information is needed to jump over the epilog scopes to reach
///        the unwind codes.
///  - The unwind codes describes both prolog and epilog. Each prolog is
///    terminated by `End/EndC` unwind code.
///
use core::fmt;

use crate::{
    byte_reader::ByteReader,
    error::{Error, StResult},
};

/// `UnwindInfo`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling>
#[derive(Debug)]
pub enum UnwindInfo<'a> {
    PackedUnwindInfo {
        /// image name extracted from the loaded pe image
        image_name: Option<&'static str>,

        flag: u8,
        function_length: u16,
        regf: u8,
        regl: u8,
        h: u8,
        cr: u8,
        frame_size: u16,
    },
    UnpackedUnwindInfo {
        /// image name extracted from the loaded pe image
        image_name: Option<&'static str>,

        xdata_rva: usize,

        /// Header
        function_length: u32,
        code_words: u16,
        epilog_count: u16,
        e: u8,
        x: u8,
        vers: u8,

        /// slice to .xdata bytes
        bytes: &'a [u8],
    },
}

impl fmt::Display for UnwindInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnwindInfo::PackedUnwindInfo { flag, function_length, regf, regl, h, cr, frame_size, .. } => {
                write!(
                    f,
                    "UnwindInfo::PackedUnwindInfo {{ flag: 0x{:X}, function_length: 0x{:X}, regf: 0x{:X}, regl: 0x{:X}, h: 0x{:X}, cr: 0x{:X}, frame_size: 0x{:X} }}",
                    flag, function_length, regf, regl, h, cr, frame_size
                )
            }
            UnwindInfo::UnpackedUnwindInfo {
                xdata_rva, function_length, code_words, epilog_count, e, x, vers, ..
            } => {
                write!(
                    f,
                    "UnwindInfo::UnpackedUnwindInfo {{ xdata_rva: 0x{:X}, function_length: 0x{:X}, code_words: 0x{:X}, epilog_count: 0x{:X}, e: 0x{:X}, x: 0x{:X}, vers: 0x{:X} }}",
                    xdata_rva, function_length, code_words, epilog_count, e, x, vers,
                )
            }
        }
    }
}

impl<'a> UnwindInfo<'a> {
    pub fn parse(bytes: &'a [u8], unwind_info: u32, image_name: Option<&'static str>) -> StResult<UnwindInfo<'a>> {
        let flag = (unwind_info & 0x3) as u8;
        match flag {
            // 0. packed unwind data not used; remaining bits point to an .xdata record
            0 => {
                // apparently when flag is zero then entire u32 becomes the
                // .xdata rva. The documentation is not very useful here!
                let xdata_rva = unwind_info as usize;
                let xdata_header = &bytes[xdata_rva..xdata_rva + 4];
                let xdata_header: u32 = xdata_header.read32(0).unwrap();
                let function_length = (xdata_header & 0x3FFFF) * 4;
                let mut code_words = ((xdata_header >> 27) & 0x1F) as u16;
                let mut epilog_count = ((xdata_header >> 22) & 0x1F) as u16;
                let e = ((xdata_header >> 21) & 0x1) as u8;
                let x = ((xdata_header >> 20) & 0x1) as u8;
                let vers = ((xdata_header >> 18) & 0x3) as u8;

                if vers != 0 {
                    return Err(Error::Malformed("Unsupported .xdata version. 'vers' field other than zero"));
                }

                let mut unwind_code_offset = 1usize; // In AArch64, one word = 4 bytes!

                // The second word(header 2) is only present if both the Epilog
                // Count and Code Words fields are set to 0
                if code_words == 0 && epilog_count == 0 {
                    unwind_code_offset += 1; // header 2 is present

                    let xdata_header2 = &bytes[xdata_rva + 4..xdata_rva + 8];
                    let xdata_header2: u32 = xdata_header2.read32(0).unwrap();
                    code_words = ((xdata_header2 >> 16) & 0xFF) as u16;
                    epilog_count = (xdata_header2 & 0xFFFF) as u16;
                }

                // If e == 0 then there are epilog scopes which should be
                // accounted to reach unwind codes
                if e == 0 {
                    unwind_code_offset += epilog_count as usize;
                }

                // log::info!("unwind_code_offset: {unwind_code_offset}");

                let unwind_code_rva_begin = xdata_rva + unwind_code_offset * core::mem::size_of::<u32>();
                let unwind_code_rva_end = unwind_code_rva_begin + code_words as usize * core::mem::size_of::<u32>();
                // log::info!(
                //     "unwind_code_rva_begin: {:X} unwind_code_rva_end: {:X}",
                //     unwind_code_rva_begin,
                //     unwind_code_rva_end
                // );

                // create the unwind code slice
                let bytes = &bytes[unwind_code_rva_begin..unwind_code_rva_end];

                // bytes.chunks(4).map(|ele| ele.read32(0).unwrap()).for_each(|ele| log::info!("ele: {:08X}", ele));

                Ok(UnwindInfo::UnpackedUnwindInfo {
                    image_name,
                    xdata_rva,
                    function_length,
                    code_words,
                    epilog_count,
                    e,
                    x,
                    vers,
                    bytes,
                })
            }
            // 1. packed unwind data used with a single prolog and epilog at the
            //    beginning and end of the scope
            // 2. packed unwind data used for code without any prolog and
            //    epilog. Useful for describing separated function segments
            1 | 2 => {
                let regf = ((unwind_info >> 13) & 0x7) as u8;
                let regl = ((unwind_info >> 16) & 0x7) as u8;
                let h = ((unwind_info >> 20) & 0x1) as u8;
                let cr = ((unwind_info >> 21) & 0x3) as u8;
                let frame_size = (((unwind_info >> 23) & 0x1FF) * 16) as u16;
                let function_length = (((unwind_info >> 2) & 0x7FF) * 4) as u16;

                Ok(UnwindInfo::PackedUnwindInfo { image_name, flag, function_length, regf, regl, h, cr, frame_size })
            }
            _ => {
                // 4. Reserved
                Err(Error::Malformed("Malformed unwind info bytes with flag >= 3"))
            }
        }
    }

    /// Function to calculate the stack pointer offset(s) of a function. Unlike
    /// x64, AArch64's `BL` instruction does not update the stack with the
    /// return address. Instead, the return address is stored in the `LR`
    /// register. This `LR` register gets saved to the stack in multiple ways by
    /// the callee as per the ABI. Sometimes, `LR` gets saved immediately at the
    /// beginning of the prolog using `STP FP, LR, [SP, #-0x30]!`, and other
    /// times, in the middle of the prolog using `STP FP, LR, [SP, #0x50]`,
    /// along with a couple of other methods.
    ///
    /// Not just `LR`, but to restore the previous stack frame's `SP`, we need
    /// to identify the full stack frame size created by the prolog, not just
    /// where `LR` is saved. So, all in all, it requires decoding all the prolog
    /// unwind codes!
    ///
    /// The cherry on top is that these unwind codes are encoded in two
    /// different formats: packed and unpacked.
    ///
    /// Due to above reasons this function returns two offsets.
    /// 1. `lr_offset` where the lr is saved on the stack from the current sp.
    /// 2. `sp_offset` gives the full stack frame created by prolog.
    pub fn get_stack_pointer_offset(&self) -> StResult<(usize, usize)> {
        match self {
            UnwindInfo::PackedUnwindInfo { image_name, frame_size, cr, .. } => {
                UnwindCode::get_stack_pointer_offset_packed(*frame_size, *cr)
                    .map_err(|_| Error::StackOffsetNotFound(*image_name))
            }
            UnwindInfo::UnpackedUnwindInfo { image_name, bytes, .. } => {
                UnwindCode::get_stack_pointer_offset_unpacked(bytes)
                    .map_err(|_| Error::StackOffsetNotFound(*image_name))
            }
        }
    }

    /// Function to calculate the current stack frame parameters
    pub fn get_current_stack_frame(&self, sp: u64, pc: u64) -> StResult<(u64, u64, u64, u64)> {
        let (lr_offset, sp_offset) = self.get_stack_pointer_offset()?;
        let mut prev_sp = sp + lr_offset as u64;
        let prev_pc = unsafe { *((prev_sp) as *const u64) }; // read lr
        prev_sp = sp + sp_offset as u64;
        Ok((sp, pc, prev_sp, prev_pc))
    }
}

/// `UnwindCode`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling?view=msvc-170#unwind-codes>
#[allow(dead_code)] // Enum variants are used for testing the parsed bytes. Ignore their presence in release build
#[rustfmt::skip]
#[derive(Debug, PartialEq, Eq)]
pub enum UnwindCode {
    AllocS(u8),         // 000xxxxx                                     | allocate small stack with size < 512 (2^5 * 16).
    SaveR19R20X(u8),    // 001zzzzz                                     | save <x19,x20> pair at [sp-#Z*8]!, pre-indexed offset >= -248  ex: stp   x19,x20,[sp,#-0x20]!
    SaveFpLr(u8),       // 01zzzzzz                                     | save <x29,lr> pair at [sp+#Z*8], offset <= 504.
    SaveFpLrX(u8),      // 10zzzzzz                                     | save <x29,lr> pair at [sp-(#Z+1)*8]!, pre-indexed offset >= -512
    AllocM(u16),        // 11000xxx'xxxxxxxx                            | allocate large stack with size < 32K (2^11 * 16).
    SaveRegP(u8, u8),   // 110010xx'xxzzzzzz                            | save x(19+#X) pair at [sp+#Z*8], offset <= 504
    SaveRegPX(u8, u8),  // 110011xx'xxzzzzzz                            | save pair x(19+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -512
    SaveReg(u8, u8),    // 110100xx'xxzzzzzz                            | save reg x(19+#X) at [sp+#Z*8], offset <= 504
    SaveRegX(u8, u8),   // 1101010x'xxxzzzzz                            | save reg x(19+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -256
    SaveLrPair(u8, u8), // 1101011x'xxzzzzzz                            | save pair <x(19+2*#X),lr> at [sp+#Z*8], offset <= 504
    SaveFRegP(u8, u8),  // 1101100x'xxzzzzzz                            | save pair d(8+#X) at [sp+#Z*8], offset <= 504
    SaveFRegPX(u8, u8), // 1101101x'xxzzzzzz                            | save pair d(8+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -512
    SaveFReg(u8, u8),   // 1101110x'xxzzzzzz                            | save reg d(8+#X) at [sp+#Z*8], offset <= 504
    SaveFRegX(u8, u8),  // 11011110'xxxzzzzz                            | save reg d(8+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -256
    AllocL(u32),        // 11100000'xxxxxxxx'xxxxxxxx'xxxxxxxx          | allocate large stack with size < 256M (2^24 * 16)
    SetFp,              // 11100001                                     | set up x29 with mov x29,sp
    AddFp(u8),          // 11100010'xxxxxxxx                            | set up x29 with add x29,sp,#x*8
    Nop,                // 11100011                                     | no unwind operation is required.
    End,                // 11100100                                     | end of unwind code. Implies ret in epilog.
    EndC,               // 11100101                                     | end of unwind code in current chained scope.
    SaveNext,           // 11100110                                     | save next non-volatile Int or FP register pair.
    PacSignLr,          // 11111100                                     | sign the return address in lr with pacibsp
    Reserved1,          // 11100111                                     | reserved
    // Reserved2,       // 11101xxx                                     | reserved for custom stack cases below only generated for asm routines
    Reserved3,          // 11101000                                     | Custom stack for MSFT_OP_TRAP_FRAME
    Reserved4,          // 11101001                                     | Custom stack for MSFT_OP_MACHINE_FRAME
    Reserved5,          // 11101010                                     | Custom stack for MSFT_OP_CONTEXT
    Reserved6,          // 11101011                                     | Custom stack for MSFT_OP_EC_CONTEXT
    Reserved7,          // 11101100                                     | Custom stack for MSFT_OP_CLEAR_UNWOUND_TO_CALL
    Reserved8,          // 11101101                                     | reserved
    Reserved9,          // 11101110                                     | reserved
    Reserved10,         // 11101111                                     | reserved
    // Reserved11(u8),  // 11110xxx                                     | reserved
    Reserved12(u8),     // 11111000'yyyyyyyy                            | reserved
    Reserved13(u16),    // 11111001'yyyyyyyy'yyyyyyyy                   | reserved
    Reserved14(u32),    // 11111010'yyyyyyyy'yyyyyyyy'yyyyyyyy          | reserved
    Reserved15(u32),    // 11111011'yyyyyyyy'yyyyyyyy'yyyyyyyy'yyyyyyyy | reserved
    Reserved16,         // 11111101                                     | reserved
    Reserved17,         // 11111110                                     | reserved
    Reserved18,         // 11111111                                     | reserved
}

impl fmt::Display for UnwindCode {
    // This makes life easier when debugging or calculating the offsets!
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            UnwindCode::AllocS(size) => write!(f, "AllocS({}) | sub   sp,sp,#0x{:X}", size, *size as u32 * 16u32),
            UnwindCode::SaveR19R20X(offset) => {
                write!(f, "SaveR19R20X({}) | stp   x19,x20,[sp,#-0x{:X}]!", offset, *offset as u32 * 8u32)
            }
            UnwindCode::SaveFpLr(offset) => {
                write!(f, "SaveFpLr({}) | stp   fp,lr,[sp,#0x{:X}]", offset, *offset as u32 * 8u32)
            }
            UnwindCode::SaveFpLrX(offset) => {
                write!(f, "SaveFpLrX({}) | stp   fp,lr,[sp,#-0x{:X}]!", offset, (*offset as u32 + 1u32) * 8u32)
            }
            UnwindCode::AllocM(size) => write!(f, "AllocM({}) | sub   sp,sp,#0x{:X}", size, *size as u32 * 16u32),
            UnwindCode::SaveRegP(x, z) => {
                write!(f, "SaveRegP({}, {}) | stp   x{},x{},[sp,#0x{:X}]", x, z, 19 + x, 19 + x + 1, *z as u32 * 8u32)
            }
            UnwindCode::SaveRegPX(x, z) => {
                write!(
                    f,
                    "SaveRegPX({}, {}) | stp   x{},x{},[sp,#-0x{:X}]!",
                    x,
                    z,
                    19 + x,
                    19 + x + 1,
                    (*z as u32 + 1u32) * 8u32
                )
            }
            UnwindCode::SaveReg(x, z) => {
                write!(f, "SaveReg({}, {}) | str   x{},[sp,#0x{:X}]", x, z, 19 + x, *z as u32 * 8u32)
            }
            UnwindCode::SaveRegX(x, z) => {
                write!(f, "SaveRegX({}, {}) | str   x{},[sp,#-0x{:X}]!", x, z, 19 + x, (*z as u32 + 1u32) * 8u32)
            }
            UnwindCode::SaveLrPair(x, z) => {
                write!(f, "SaveLrPair({}, {}) | stp x{},lr,[sp,#0x{:X}]", x, z, 19 + 2 * x, *z as u32 * 8u32)
            }
            UnwindCode::SaveFRegP(x, z) => {
                write!(f, "SaveFRegP({}, {}) | stp   d{},d{},[sp,#0x{:X}]", x, z, 8 + x, 8 + x + 1, *z as u32 * 8u32)
            }
            UnwindCode::SaveFRegPX(x, z) => {
                write!(
                    f,
                    "SaveFRegPX({}, {}) | stp   d{},d{},[sp,#-0x{:X}]!",
                    x,
                    z,
                    8 + x,
                    8 + x + 1,
                    (*z as u32 + 1u32) * 8u32
                )
            }
            UnwindCode::SaveFReg(x, z) => {
                write!(f, "SaveFReg({}, {}) | str   d{},[sp,#0x{:X}]", x, z, 8 + x, *z as u32 * 8u32)
            }
            UnwindCode::SaveFRegX(x, z) => {
                write!(f, "SaveFRegX({}, {}) | str   d{},[sp,#-0x{:X}]!", x, z, 8 + x, (*z as u32 + 1u32) * 8u32)
            }
            UnwindCode::AllocL(size) => write!(f, "AllocL({}) | sub   sp,sp,#0x{:X}", size, *size * 16u32),
            UnwindCode::SetFp => write!(f, "SetFp | mov  fp,sp"),
            UnwindCode::AddFp(x) => write!(f, "AddFp({}) | add fp,sp,#0x{:X}", x, *x as u32 * 8u32),
            UnwindCode::Nop => write!(f, "Nop"),
            UnwindCode::End => write!(f, "End"),
            UnwindCode::EndC => write!(f, "EndC"),
            UnwindCode::SaveNext => write!(f, "SaveNext"),
            UnwindCode::PacSignLr => write!(f, "PacSignLr"),
            UnwindCode::Reserved1 => write!(f, "Reserved1"),
            UnwindCode::Reserved3 => write!(f, "Reserved3"),
            UnwindCode::Reserved4 => write!(f, "Reserved4"),
            UnwindCode::Reserved5 => write!(f, "Reserved5"),
            UnwindCode::Reserved6 => write!(f, "Reserved6"),
            UnwindCode::Reserved7 => write!(f, "Reserved7"),
            UnwindCode::Reserved8 => write!(f, "Reserved8"),
            UnwindCode::Reserved9 => write!(f, "Reserved9"),
            UnwindCode::Reserved10 => write!(f, "Reserved10"),
            UnwindCode::Reserved12(y) => write!(f, "Reserved12({})", y),
            UnwindCode::Reserved13(y) => write!(f, "Reserved13({})", y),
            UnwindCode::Reserved14(y) => write!(f, "Reserved14({})", y),
            UnwindCode::Reserved15(y) => write!(f, "Reserved15({})", y),
            UnwindCode::Reserved16 => write!(f, "Reserved16"),
            UnwindCode::Reserved17 => write!(f, "Reserved17"),
            UnwindCode::Reserved18 => write!(f, "Reserved18"),
        }
    }
}

impl UnwindCode {
    /// Function to return correct lr offset and sp offset(stack frame size) for
    /// packed unwind codes
    pub fn get_stack_pointer_offset_packed(frame_size: u16, cr: u8) -> StResult<(usize, usize)> {
        let (lr_offset, offset) = match cr {
            // Unchained function, The <x29, lr> pair isn't saved on the stack,
            // meaning the frame_size is fully accounted for by the sp via "sub
            // sp, sp, #{frame_size}". TODO: Make it stricter to avoid using lr
            // in this case while processing stack frames. Using 0 for lr offset
            // is not the correct representation. This happens only for a
            // handful of CRT functions that use the __forceinline function
            // attribute.
            0 => (0, frame_size),
            // unchained function, <lr> is saved in stack. str
            // lr,[sp,#-{frame_size}]!
            1 => (0, frame_size),
            // Chained function with a pacibsp-signed return address. The prolog
            // begins with the `pacibsp` instruction, followed by storing the
            // signed LR onto the stack using `STP FP, LR, [SP, #-0x10]!`. SP =
            // SP - 0x10; FP is saved at SP, and the signed LR is saved at SP +
            // 8. TODO: Retrieve the signed LR and convert it to an unsigned LR.
            // Currently, the Rust UEFI toolchain does not produce "Pointer
            // Authentication Code for Instruction Address."
            2 => (0x8, frame_size),
            // Chained function, a store/load pair instruction is used in
            // prolog/epilog <x29,lr>.  stp  fp,lr,[sp,#-0x10]!. sp = sp - 0x10;
            // fp is saved at sp and lr is saved at sp + 8
            3 => (0x8, frame_size),
            _ => return Err(Error::StackOffsetNotFound(None)),
        };
        Ok((lr_offset as usize, offset as usize))
    }

    /// Function to parse the unwind codes and calculate the lr offset and sp
    /// offset after executing the function prolog
    pub fn get_stack_pointer_offset_unpacked(bytes: &[u8]) -> StResult<(usize, usize)> {
        let mut len = 0;
        let mut lr_offset = 0; // +ve offset from current sp to the offset where lr is saved
        let mut offset = 0; // +ve offset from current sp to the beginning of the function

        while len < bytes.len() {
            let byte = bytes[len];
            if (byte >> 5) & 0b111 == 0b000 {
                // AllocS(u8),         // 000xxxxx
                let x = byte & 0b00011111;
                // log::info!("{}", UnwindCode::AllocS(x));
                offset += x as u32 * 16u32;
                len += 1;
            } else if (byte >> 5) & 0b111 == 0b001 {
                // SaveR19R20X(u8),    // 001zzzzz
                let z = byte & 0b00011111;
                // log::info!("{}", UnwindCode::SaveR19R20X(z));
                offset += z as u32 * 8u32;
                len += 1;
            } else if (byte >> 6) & 0b11 == 0b01 {
                // SaveFpLr(u8),       // 01zzzzzz |  stp   fp,lr,[sp,#0x{:X}]
                let z = byte & 0b00111111;
                // log::info!("{}", UnwindCode::SaveFpLr(z));
                // From the current offset move z bytes to get to <fp,lr>
                lr_offset = offset + z as u32 * 8u32 + 8 /* step over fp */;
                len += 1;
            } else if (byte >> 6) & 0b11 == 0b10 {
                // SaveFpLrX(u8),      // 10zzzzzz | stp   fp,lr,[sp,#-0x{:X}]!
                let z = byte & 0b00111111;
                // log::info!("{}", UnwindCode::SaveFpLrX(z));
                offset += (z as u32 + 1u32) * 8u32; // pre increment the offset
                lr_offset = offset + 8 /* step over fp */;
                len += 1;
            } else if (byte >> 3) & 0b11111 == 0b11000 {
                // AllocM(u16),        // 11000xxx'xxxxxxxx
                let x = (((byte & 0b111) as u16) << 8) | bytes[len + 1] as u16;
                // log::info!("{}", UnwindCode::AllocM(x));
                offset += x as u32 * 16u32;
                len += 2;
            } else if (byte >> 2) & 0b111111 == 0b110010 {
                // SaveRegP(u8, u8),   // 110010xx'xxzzzzzz
                // let x = ((byte & 0b11) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                // let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveRegP(x, z));
                len += 2;
            } else if (byte >> 2) & 0b111111 == 0b110011 {
                // SaveRegPX(u8, u8),  // 110011xx'xxzzzzzz
                // let x = ((byte & 0b11) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveRegPX(x, z));
                offset += (z as u32 + 1u32) * 8u32;
                len += 2;
            } else if (byte >> 2) & 0b111111 == 0b110100 {
                // SaveReg(u8, u8),    // 110100xx'xxzzzzzz
                // let x = ((byte & 0b11) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                // let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveReg(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101010 {
                // SaveRegX(u8, u8),   // 1101010x'xxxzzzzz
                // let x = ((byte & 0b1) << 3) | ((bytes[len + 1] >> 5) & 0b111);
                let z = bytes[len + 1] & 0b00011111;
                // log::info!("{}", UnwindCode::SaveRegX(x, z));
                offset += (z as u32 + 1u32) * 8u32;
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101011 {
                // SaveLrPair(u8, u8), // 1101011x'xxzzzzzz
                // let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                // let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveLrPair(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101100 {
                // SaveFRegP(u8, u8),  // 1101100x'xxzzzzzz
                // let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                // let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveFRegP(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101101 {
                // SaveFRegPX(u8, u8), // 1101101x'xxzzzzzz
                // let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                // let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveFRegPX(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101110 {
                // SaveFReg(u8, u8),   // 1101110x'xxzzzzzz
                // let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                // let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveFReg(x, z));
                len += 2;
            } else if byte == 0b11011110 {
                // SaveFRegX(u8, u8),  // 11011110'xxxzzzzz
                // let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                // log::info!("{}", UnwindCode::SaveFRegX(x, z));
                offset += (z as u32 + 1u32) * 8u32;
                len += 2;
            } else if byte == 0b11100000 {
                // AllocL(u32),        // 11100000'xxxxxxxx'xxxxxxxx'xxxxxxxx
                let x = ((bytes[len + 1] as u32) << 16) | ((bytes[len + 2] as u32) << 8) | (bytes[len + 3] as u32);
                // log::info!("{}", UnwindCode::AllocL(x));
                offset += x * 16u32;
                len += 4;
            } else if byte == 0b11100001 {
                // SetFp,              // 11100001
                // log::info!("{}", UnwindCode::SetFp);
                len += 1;
            } else if byte == 0b11100010 {
                // AddFp(u8),          // 11100010'xxxxxxxx
                // let x = bytes[len + 1];
                // log::info!("{}", UnwindCode::AddFp(x));
                len += 2;
            } else if byte == 0b11100011 {
                // Nop,                // 11100011
                // log::info!("{}", UnwindCode::Nop);
                len += 1;
            } else if byte == 0b11100100 {
                // End,                // 11100100
                // log::info!("{}", UnwindCode::End);
                break; // end of prolog
            } else if byte == 0b11100101 {
                // EndC,               // 11100101
                // log::info!("{}", UnwindCode::EndC);
                break; // end of prolog
            } else if byte == 0b11100110 {
                // SaveNext,           // 11100110
                // log::info!("{}", UnwindCode::SaveNext);
                len += 1;
            } else if byte == 0b11111100 {
                // PacSignLr,          // 11111100
                // log::info!("{}", UnwindCode::PacSignLr);
                len += 1;
            } else if byte == 0b11100111 {
                // Reserved1,          // 11100111
                // log::info!("{}", UnwindCode::Reserved1);
                len += 1;
            } else if byte == 0b11101000 {
                // Reserved3,          // 11101000
                // log::info!("{}", UnwindCode::Reserved3);
                len += 1;
            } else if byte == 0b11101001 {
                // Reserved4,          // 11101001
                // log::info!("{}", UnwindCode::Reserved4);
                len += 1;
            } else if byte == 0b11101010 {
                // Reserved5,          // 11101010
                // log::info!("{}", UnwindCode::Reserved5);
                len += 1;
            } else if byte == 0b11101011 {
                // Reserved6,          // 11101011
                // log::info!("{}", UnwindCode::Reserved6);
                len += 1;
            } else if byte == 0b11101100 {
                // Reserved7,          // 11101100
                // log::info!("{}", UnwindCode::Reserved7);
                len += 1;
            } else if byte == 0b11101101 {
                // Reserved8,          // 11101101
                // log::info!("{}", UnwindCode::Reserved8);
                len += 1;
            } else if byte == 0b11101110 {
                // Reserved9,          // 11101110
                // log::info!("{}", UnwindCode::Reserved9);
                len += 1;
            } else if byte == 0b11101111 {
                // Reserved10,         // 11101111
                // log::info!("{}", UnwindCode::Reserved10);
                len += 1;
            } else if byte == 0b11111000 {
                // Reserved12(u8),     // 11111000'yyyyyyyy
                let _y = bytes[len + 1];
                // log::info!("{}", UnwindCode::Reserved12(y));
                len += 2;
            } else if byte == 0b11111001 {
                // Reserved13(u16),    // 11111001'yyyyyyyy'yyyyyyyy
                // let y = ((bytes[len + 1] as u16) << 8) | (bytes[len + 2] as u16);
                // log::info!("{}", UnwindCode::Reserved13(y));
                len += 3;
            } else if byte == 0b11111010 {
                // Reserved14(u32),    // 11111010'yyyyyyyy'yyyyyyyy'yyyyyyyy
                // let y = ((bytes[len + 1] as u32) << 16) | ((bytes[len + 2] as u32) << 8) | (bytes[len + 3] as u32);
                // log::info!("{}", UnwindCode::Reserved14(y));
                len += 4;
            } else if byte == 0b11111011 {
                // Reserved15(u32),    // 11111011'yyyyyyyy'yyyyyyyy'yyyyyyyy'yyyyyyyy
                // let y = ((bytes[len + 1] as u32) << 24)
                //     | ((bytes[len + 2] as u32) << 16)
                //     | ((bytes[len + 3] as u32) << 8)
                //     | (bytes[len + 4] as u32);
                // log::info!("{}", UnwindCode::Reserved15(y));
                len += 5;
            } else if byte == 0b11111101 {
                // Reserved16,         // 11111101
                // log::info!("{}", UnwindCode::Reserved16);
                len += 1;
            } else if byte == 0b11111110 {
                // Reserved17,         // 11111110
                // log::info!("{}", UnwindCode::Reserved17);
                len += 1;
            } else if byte == 0b11111111 {
                // Reserved18,         // 11111111
                // log::info!("{}", UnwindCode::Reserved18);
                len += 1;
            }
        }
        Ok((lr_offset as usize, offset as usize))
    }

    /// Windows only test function to parse all UnwindCodes
    #[cfg(all(target_os = "windows", target_arch = "aarch64", test))]
    pub fn parse(bytes: &[u8]) -> StResult<Vec<UnwindCode>> {
        let mut len = 0;
        let mut res = Vec::<UnwindCode>::new();
        while len < bytes.len() {
            let byte = bytes[len];
            if (byte >> 5) & 0b111 == 0b000 {
                // AllocS(u8),         // 000xxxxx
                let x = byte & 0b00011111;
                res.push(UnwindCode::AllocS(x));
                len += 1;
            } else if (byte >> 5) & 0b111 == 0b001 {
                // SaveR19R20X(u8),    // 001zzzzz
                let z = byte & 0b00011111;
                res.push(UnwindCode::SaveR19R20X(z));
                len += 1;
            } else if (byte >> 6) & 0b11 == 0b01 {
                // SaveFpLr(u8),       // 01zzzzzz
                let z = byte & 0b00111111;
                res.push(UnwindCode::SaveFpLr(z));
                len += 1;
            } else if (byte >> 6) & 0b11 == 0b10 {
                // SaveFpLrX(u8),      // 10zzzzzz
                let z = byte & 0b00111111;
                res.push(UnwindCode::SaveFpLrX(z));
                len += 1;
            } else if (byte >> 3) & 0b11111 == 0b11000 {
                // AllocM(u16),        // 11000xxx'xxxxxxxx
                let x = (((byte & 0b111) as u16) << 8) | bytes[len + 1] as u16;
                res.push(UnwindCode::AllocM(x));
                len += 2;
            } else if (byte >> 2) & 0b111111 == 0b110010 {
                // SaveRegP(u8, u8),   // 110010xx'xxzzzzzz
                let x = ((byte & 0b11) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveRegP(x, z));
                len += 2;
            } else if (byte >> 2) & 0b111111 == 0b110011 {
                // SaveRegPX(u8, u8),  // 110011xx'xxzzzzzz
                let x = ((byte & 0b11) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveRegPX(x, z));
                len += 2;
            } else if (byte >> 2) & 0b111111 == 0b110100 {
                // SaveReg(u8, u8),    // 110100xx'xxzzzzzz
                let x = ((byte & 0b11) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveReg(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101010 {
                // SaveRegX(u8, u8),   // 1101010x'xxxzzzzz
                let x = ((byte & 0b1) << 3) | ((bytes[len + 1] >> 5) & 0b111);
                let z = bytes[len + 1] & 0b00011111;
                res.push(UnwindCode::SaveRegX(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101011 {
                // SaveLrPair(u8, u8), // 1101011x'xxzzzzzz
                let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveLrPair(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101100 {
                // SaveFRegP(u8, u8),  // 1101100x'xxzzzzzz
                let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveFRegP(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101101 {
                // SaveFRegPX(u8, u8), // 1101101x'xxzzzzzz
                let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveFRegPX(x, z));
                len += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101110 {
                // SaveFReg(u8, u8),   // 1101110x'xxzzzzzz
                let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveFReg(x, z));
                len += 2;
            } else if byte == 0b11011110 {
                // SaveFRegX(u8, u8),  // 11011110'xxxzzzzz
                let x = ((byte & 0b1) << 2) | ((bytes[len + 1] >> 6) & 0b11);
                let z = bytes[len + 1] & 0b00111111;
                res.push(UnwindCode::SaveFRegX(x, z));
                len += 2;
            } else if byte == 0b11100000 {
                // AllocL(u32),        // 11100000'xxxxxxxx'xxxxxxxx'xxxxxxxx
                let x = ((bytes[len + 1] as u32) << 16) | ((bytes[len + 2] as u32) << 8) | (bytes[len + 3] as u32);
                res.push(UnwindCode::AllocL(x));
                len += 4;
            } else if byte == 0b11100001 {
                // SetFp,              // 11100001
                res.push(UnwindCode::SetFp);
                len += 1;
            } else if byte == 0b11100010 {
                // AddFp(u8),          // 11100010'xxxxxxxx
                let x = bytes[len + 1];
                res.push(UnwindCode::AddFp(x));
                len += 2;
            } else if byte == 0b11100011 {
                // Nop,                // 11100011
                res.push(UnwindCode::Nop);
                len += 1;
            } else if byte == 0b11100100 {
                // End,                // 11100100
                res.push(UnwindCode::End);
                len += 1;
            } else if byte == 0b11100101 {
                // EndC,               // 11100101
                res.push(UnwindCode::EndC);
                len += 1;
            } else if byte == 0b11100110 {
                // SaveNext,           // 11100110
                res.push(UnwindCode::SaveNext);
                len += 1;
            } else if byte == 0b11111100 {
                // PacSignLr,          // 11111100
                res.push(UnwindCode::PacSignLr);
                len += 1;
            } else if byte == 0b11100111 {
                // Reserved1,          // 11100111
                res.push(UnwindCode::Reserved1);
                len += 1;
            } else if byte == 0b11101000 {
                // Reserved3,          // 11101000
                res.push(UnwindCode::Reserved3);
                len += 1;
            } else if byte == 0b11101001 {
                // Reserved4,          // 11101001
                res.push(UnwindCode::Reserved4);
                len += 1;
            } else if byte == 0b11101010 {
                // Reserved5,          // 11101010
                res.push(UnwindCode::Reserved5);
                len += 1;
            } else if byte == 0b11101011 {
                // Reserved6,          // 11101011
                res.push(UnwindCode::Reserved6);
                len += 1;
            } else if byte == 0b11101100 {
                // Reserved7,          // 11101100
                res.push(UnwindCode::Reserved7);
                len += 1;
            } else if byte == 0b11101101 {
                // Reserved8,          // 11101101
                res.push(UnwindCode::Reserved8);
                len += 1;
            } else if byte == 0b11101110 {
                // Reserved9,          // 11101110
                res.push(UnwindCode::Reserved9);
                len += 1;
            } else if byte == 0b11101111 {
                // Reserved10,         // 11101111
                res.push(UnwindCode::Reserved10);
                len += 1;
            } else if byte == 0b11111000 {
                // Reserved12(u8),     // 11111000'yyyyyyyy
                let y = bytes[len + 1];
                res.push(UnwindCode::Reserved12(y));
                len += 2;
            } else if byte == 0b11111001 {
                // Reserved13(u16),    // 11111001'yyyyyyyy'yyyyyyyy
                let y = ((bytes[len + 1] as u16) << 8) | (bytes[len + 2] as u16);
                res.push(UnwindCode::Reserved13(y));
                len += 3;
            } else if byte == 0b11111010 {
                // Reserved14(u32),    // 11111010'yyyyyyyy'yyyyyyyy'yyyyyyyy
                let y = ((bytes[len + 1] as u32) << 16) | ((bytes[len + 2] as u32) << 8) | (bytes[len + 3] as u32);
                res.push(UnwindCode::Reserved14(y));
                len += 4;
            } else if byte == 0b11111011 {
                // Reserved15(u32),    // 11111011'yyyyyyyy'yyyyyyyy'yyyyyyyy'yyyyyyyy
                let y = ((bytes[len + 1] as u32) << 24)
                    | ((bytes[len + 2] as u32) << 16)
                    | ((bytes[len + 3] as u32) << 8)
                    | (bytes[len + 4] as u32);
                res.push(UnwindCode::Reserved15(y));
                len += 5;
            } else if byte == 0b11111101 {
                // Reserved16,         // 11111101
                res.push(UnwindCode::Reserved16);
                len += 1;
            } else if byte == 0b11111110 {
                // Reserved17,         // 11111110
                res.push(UnwindCode::Reserved17);
                len += 1;
            } else if byte == 0b11111111 {
                // Reserved18,         // 11111111
                res.push(UnwindCode::Reserved18);
                len += 1;
            }
        }
        Ok(res)
    }
}
