/// Parses AArch64 unwind data from the `.pdata` and `.xdata` sections.
/// The main goal of this module is to calculate the appropriate stack-pointer
/// offsets by undoing the operations performed by the prolog of a given
/// function. These offsets are then used to identify the previous stack frame's
/// stack pointer (SP) and instruction pointer (PC). Unlike x64, AArch64
/// requires more involved unwinding operations.
///
/// Unwind info in AArch64 comes in two flavors:
/// 1. Packed unwind info for canonical functions, encoded from 2-31 bits.
/// 2. .xdata-based unpacked unwind info, where the RVA of .xdata is present in
///    0-31 bits.
///
/// .pdata entry structure:
///                      .-------------------------------------------------------------------------------------------------------------------------------.
///                      | 3 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
///                      | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
///                      .-------------------------------------------------------------------------------------------------------------------------------.
///                      |                                                      Function start RVA                                                       |
///                      |-------------------------------------------------------------------------------------------------------------------------------|
///                      |                                               .xdata rva/packed unwind info                                           | flag  |
///                      '-------------------------------------------------------------------------------------------------------------------------------'
///
/// .xdata structure:
///                      .-------------------------------------------------------------------------------------------------------------------------------.
///                      | 3 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
///                      | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 | 9 | 8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
///        .------------>.-------------------------------------------------------------------------------------------------------------------------------.
///     Header 1         | Code Words        | Epilog count      | E | X | Vers  | Function Length                                                       |
///        .------------>|-------------------------------------------------------------------------------------------------------------------------------|
///   Header 2(opt)      | (Reserved)                    | (Extended Code Words)         | (Extended Epilog Count)                                       |
///        .------------>|-------------------------------------------------------------------------------------------------------------------------------|
///  Epilog Scope 1(opt) | Epilog Start Index                    | (reserved)    | Epilog Start Offset                                                   |
///        .------------>|-------------------------------------------------------------------------------------------------------------------------------|
///  Epilog Scope 2(opt) | (Possibly followed by additional epilog scopes)                                                                               |
///        .------------>|-------------------------------------------------------------------------------------------------------------------------------|
///  Unwind codes        | Unwind Code 3                 | Unwind Code 2                 | Unwind Code 1                 | Unwind Code 0                 |
///                      |-------------------------------------------------------------------------------------------------------------------------------|
///                      | (Possibly followed by additional words with unwind codes)                                                                     |
///                      |-------------------------------------------------------------------------------------------------------------------------------|
///                      | Exception Handler RVA (if X = 1)                                                                                              |
///                      |-------------------------------------------------------------------------------------------------------------------------------|
///                      | (Possibly followed by data needed by the exception handler)                                                                   |
///                      '-------------------------------------------------------------------------------------------------------------------------------'
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
///  - The unwind codes describe both the prolog and epilog. Each prolog is
///    terminated by `End/EndC` unwind code.
///
use core::fmt;

use crate::{
    byte_reader::{ByteReader, read_pointer64},
    error::{Error, StResult},
    stacktrace::StackFrame,
};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameChainMode {
    Unchained = 0,
    UnchainedSavedLr = 1,
    ChainedWithPac = 2,
    Chained = 3,
}

impl FrameChainMode {
    fn from_bits(bits: u8, module: Option<&'static str>) -> StResult<Self> {
        match bits {
            0 => Ok(Self::Unchained),
            1 => Ok(Self::UnchainedSavedLr),
            2 => Ok(Self::ChainedWithPac),
            3 => Ok(Self::Chained),
            _ => Err(Error::Malformed { module, reason: "Unsupported chain register encoding" }),
        }
    }
}

/// `UnwindInfo`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling>
#[derive(Debug)]
pub enum UnwindInfo<'a> {
    PackedUnwindInfo {
        /// Image name extracted from the loaded PE image.
        image_name: Option<&'static str>,
        func_start_rva: u32,

        flag: u8,
        function_length: u16,
        reg_f: u8,
        reg_i: u8,
        h: u8,
        cr: FrameChainMode,
        frame_size: u16,
    },
    UnpackedUnwindInfo {
        /// Image name extracted from the loaded PE image.
        image_name: Option<&'static str>,
        func_start_rva: u32,

        xdata_rva: usize,

        /// Header fields.
        function_length: u32,
        unwind_code_words: u16,
        epilog_count: u16,
        e: u8,
        x: u8,
        vers: u8,

        /// Slice containing the `.xdata` unwind bytes.
        unwind_codes: &'a [u8],
    },
}

impl fmt::Display for UnwindInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnwindInfo::PackedUnwindInfo {
                func_start_rva,
                flag,
                function_length,
                reg_f,
                reg_i,
                h,
                cr,
                frame_size,
                ..
            } => {
                let cr_value = *cr as u8;
                write!(
                    f,
                    "UnwindInfo::PackedUnwindInfo {{ flag: 0x{:X}, func_start_rva: 0x{:X}, function_length: 0x{:X}, reg_f: 0x{:X}, reg_i: 0x{:X}, h: 0x{:X}, cr: 0x{:X}, frame_size: 0x{:X} }}",
                    flag, func_start_rva, function_length, reg_f, reg_i, h, cr_value, frame_size
                )
            }
            UnwindInfo::UnpackedUnwindInfo {
                xdata_rva,
                func_start_rva,
                function_length,
                unwind_code_words,
                epilog_count,
                e,
                x,
                vers,
                unwind_codes,
                ..
            } => {
                write!(
                    f,
                    "UnwindInfo::UnpackedUnwindInfo {{ xdata_rva: 0x{:X}, func_start_rva: 0x{:X}, function_length: 0x{:X}, vers: 0x{:X}, x: 0x{:X}, e: 0x{:X}, epilog_count: 0x{:X}, unwind_code_words: 0x{:X}, unwind_codes: ",
                    xdata_rva, func_start_rva, function_length, vers, x, e, epilog_count, unwind_code_words,
                )?;
                for byte in *unwind_codes {
                    write!(f, "{:02X} ", byte)?;
                }
                write!(f, "}}")
            }
        }
    }
}

impl<'a> UnwindInfo<'a> {
    pub fn parse(
        bytes: &'a [u8],
        func_start_rva: u32,
        unwind_info: u32,
        image_name: Option<&'static str>,
    ) -> StResult<UnwindInfo<'a>> {
        let flag = (unwind_info & 0x3) as u8;
        match flag {
            // 0. Packed unwind data not used; remaining bits point to an `.xdata` record.
            0 => {
                let xdata_rva = unwind_info as usize;
                let xdata = &bytes[xdata_rva..];
                let xdata_header: u32 = xdata.read32(0)?;
                let function_length = (xdata_header & 0x3FFFF) * 4;
                let mut unwind_code_words = ((xdata_header >> 27) & 0x1F) as u16;
                let mut epilog_count = ((xdata_header >> 22) & 0x1F) as u16;
                let e = ((xdata_header >> 21) & 0x1) as u8;
                let x = ((xdata_header >> 20) & 0x1) as u8;
                let vers = ((xdata_header >> 18) & 0x3) as u8;

                if vers != 0 {
                    return Err(Error::Malformed {
                        module: image_name,
                        reason: "Unsupported .xdata version. 'vers' field other than zero",
                    });
                }

                let mut byte_offset = 4_usize; // skip over header 1

                // The second word(header 2) is only present if both the Epilog
                // Count and Code Words fields are set to 0
                if unwind_code_words == 0 && epilog_count == 0 {
                    let xdata_header2: u32 = xdata.read32(4)?;
                    epilog_count = (xdata_header2 & 0xFFFF) as u16; // extended epilog count
                    unwind_code_words = ((xdata_header2 >> 16) & 0xFF) as u16; // extended code words
                    byte_offset += 4; // header 2 is present, skip over it
                }

                let mut unwind_code_offset = byte_offset;
                // If e == 0, then there are epilog scopes that should be
                // accounted for to reach the unwind codes.
                if e == 0 {
                    unwind_code_offset += 4 * epilog_count as usize; // skip over epilog scopes
                } else {
                    // When e == 1, there is exactly one epilog. It has no
                    // associated scope, and the epilog count contains the index
                    // into the epilog unwind code byte.
                    unwind_code_offset += epilog_count as usize;
                }

                let unwind_code_rva_begin = xdata_rva + unwind_code_offset;
                let unwind_code_rva_end = unwind_code_rva_begin + unwind_code_words as usize * 4;

                // create the unwind code slice
                let unwind_codes = &bytes[unwind_code_rva_begin..unwind_code_rva_end];

                if e == 1 {
                    epilog_count = 1;
                }

                Ok(UnwindInfo::UnpackedUnwindInfo {
                    image_name,
                    func_start_rva,
                    xdata_rva,
                    function_length,
                    unwind_code_words,
                    epilog_count,
                    e,
                    x,
                    vers,
                    unwind_codes,
                })
            }
            // 1. Packed unwind data used with a single prolog and epilog at the
            // beginning and end of the scope.
            // 2. Packed unwind data used for code without any prolog or epilog,
            // useful for describing separate function segments.
            1 | 2 => {
                let reg_f = ((unwind_info >> 13) & 0x7) as u8;
                let reg_i = ((unwind_info >> 16) & 0x7) as u8;
                let h = ((unwind_info >> 20) & 0x1) as u8;
                let cr_bits = ((unwind_info >> 21) & 0x3) as u8;
                let cr = FrameChainMode::from_bits(cr_bits, image_name)?;
                let frame_size = (((unwind_info >> 23) & 0x1FF) * 16) as u16;
                let function_length = (((unwind_info >> 2) & 0x7FF) * 4) as u16;

                Ok(UnwindInfo::PackedUnwindInfo {
                    image_name,
                    func_start_rva,
                    flag,
                    function_length,
                    reg_f,
                    reg_i,
                    h,
                    cr,
                    frame_size,
                })
            }
            _ => {
                // 3. Reserved for future use.
                Err(Error::Malformed { module: image_name, reason: "Malformed unwind info bytes with flag >= 3" })
            }
        }
    }

    /// Calculates the parameters for the previous stack frame.
    ///
    /// # Safety
    /// The supplied `stack_frame` must originate from a real AArch64 stack frame whose
    /// recorded SP/FP/PC still identify readable memory governed by the current unwind
    /// metadata. Supplying incorrect register snapshots can lead to invalid pointer
    /// dereferences while decoding the caller state.
    pub fn get_previous_stack_frame(&self, stack_frame: &StackFrame) -> StResult<StackFrame> {
        log::debug!("    > {}", &self); // debug
        match self {
            UnwindInfo::PackedUnwindInfo { image_name, frame_size, cr, reg_f, reg_i, h, .. } => {
                UnwindCode::get_previous_stack_frame_packed(*frame_size, *cr, *reg_i, *reg_f, *h, stack_frame)
                    .map_err(|err| err.with_module(*image_name))
            }
            UnwindInfo::UnpackedUnwindInfo { image_name, unwind_codes, .. } => {
                UnwindCode::get_previous_stack_frame_unpacked(unwind_codes, stack_frame)
                    .map_err(|err| err.with_module(*image_name))
            }
        }
    }
}

/// `UnwindCode`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling?view=msvc-170#unwind-codes>
#[allow(dead_code)] // Enum variants are used for testing the parsed bytes. Ignore their presence in release builds.
#[rustfmt::skip]
#[derive(Debug, PartialEq, Eq)]
pub enum UnwindCode {
    AllocS(u8),               // 000xxxxx                                     | allocate small stack with size < 512 (2^5 * 16).
    SaveR19R20X(u8),          // 001zzzzz                                     | save <x19,x20> pair at [sp-#Z*8]!, pre-indexed offset >= -248  ex: stp   x19,x20,[sp,#-0x20]!
    SaveFpLr(u8),             // 01zzzzzz                                     | save <x29,lr> pair at [sp+#Z*8], offset <= 504.
    SaveFpLrX(u8),            // 10zzzzzz                                     | save <x29,lr> pair at [sp-(#Z+1)*8]!, pre-indexed offset >= -512
    AllocM(u16),              // 11000xxx'xxxxxxxx                            | allocate large stack with size < 32K (2^11 * 16).
    SaveRegP(u8, u8),         // 110010xx'xxzzzzzz                            | save x(19+#X) pair at [sp+#Z*8], offset <= 504
    SaveRegPX(u8, u8),        // 110011xx'xxzzzzzz                            | save pair x(19+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -512
    SaveReg(u8, u8),          // 110100xx'xxzzzzzz                            | save reg x(19+#X) at [sp+#Z*8], offset <= 504
    SaveRegX(u8, u8),         // 1101010x'xxxzzzzz                            | save reg x(19+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -256
    SaveLrPair(u8, u8),       // 1101011x'xxzzzzzz                            | save pair <x(19+2*#X),lr> at [sp+#Z*8], offset <= 504
    SaveFRegP(u8, u8),        // 1101100x'xxzzzzzz                            | save pair d(8+#X) at [sp+#Z*8], offset <= 504
    SaveFRegPX(u8, u8),       // 1101101x'xxzzzzzz                            | save pair d(8+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -512
    SaveFReg(u8, u8),         // 1101110x'xxzzzzzz                            | save reg d(8+#X) at [sp+#Z*8], offset <= 504
    SaveFRegX(u8, u8),        // 11011110'xxxzzzzz                            | save reg d(8+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -256
    AllocZ(u32),              // 11011111'zzzzzzzz                            | allocate stack with size z * SVE-VL
    AllocL(u32),              // 11100000'xxxxxxxx'xxxxxxxx'xxxxxxxx          | allocate large stack with size < 256M (2^24 * 16)
    SetFp,                    // 11100001                                     | set up x29 with mov x29,sp
    AddFp(u8),                // 11100010'xxxxxxxx                            | set up x29 with add x29,sp,#x*8
    Nop,                      // 11100011                                     | no unwind operation is required.
    End,                      // 11100100                                     | end of unwind code. Implies ret in epilog.
    EndC,                     // 11100101                                     | end of unwind code in previous chained scope.
    SaveNext,                 // 11100110                                     | save next non-volatile Int or FP register pair.
    PacSignLr,                // 11111100                                     | sign the return address in lr with pacibsp
    Reserved1,                // 11100111                                     | reserved
    // Reserved2,             // 11101xxx                                     | reserved for custom stack cases below only generated for asm routines
    MsftOpTrapFrame,          // 11101000                                     | Custom stack for MSFT_OP_TRAP_FRAME
    MsftOpMachineFrame,       // 11101001                                     | Custom stack for MSFT_OP_MACHINE_FRAME
    MsftOpContext,            // 11101010                                     | Custom stack for MSFT_OP_CONTEXT
    MsftOpEcContext,          // 11101011                                     | Custom stack for MSFT_OP_EC_CONTEXT
    MsftOpClearUnwoundToCall, // 11101100                                     | Custom stack for MSFT_OP_CLEAR_UNWOUND_TO_CALL
    Reserved8,                // 11101101                                     | reserved
    Reserved9,                // 11101110                                     | reserved
    Reserved10,               // 11101111                                     | reserved
    // Reserved11(u8),        // 11110xxx                                     | reserved
    Reserved12(u8),           // 11111000'yyyyyyyy                            | reserved
    Reserved13(u16),          // 11111001'yyyyyyyy'yyyyyyyy                   | reserved
    Reserved14(u32),          // 11111010'yyyyyyyy'yyyyyyyy'yyyyyyyy          | reserved
    Reserved15(u32),          // 11111011'yyyyyyyy'yyyyyyyy'yyyyyyyy'yyyyyyyy | reserved
    Reserved16,               // 11111101                                     | reserved
    Reserved17,               // 11111110                                     | reserved
    Reserved18,               // 11111111                                     | reserved
}

impl fmt::Display for UnwindCode {
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
            UnwindCode::AllocZ(size) => write!(f, "AllocZ({})", size),
            UnwindCode::AllocL(size) => {
                write!(f, "AllocL({}) | sub   sp,sp,#0x{:X}", size, *size * 16u32)
            }
            UnwindCode::SetFp => write!(f, "SetFp | mov  fp,sp"),
            UnwindCode::AddFp(x) => write!(f, "AddFp({}) | add fp,sp,#0x{:X}", x, *x as u32 * 8u32),
            UnwindCode::Nop => write!(f, "Nop"),
            UnwindCode::End => write!(f, "End"),
            UnwindCode::EndC => write!(f, "EndC"),
            UnwindCode::SaveNext => write!(f, "SaveNext"),
            UnwindCode::PacSignLr => write!(f, "PacSignLr"),
            UnwindCode::Reserved1 => write!(f, "Reserved1"),
            UnwindCode::MsftOpTrapFrame => write!(f, "MsftOpTrapFrame"),
            UnwindCode::MsftOpMachineFrame => write!(f, "MsftOpMachineFrame"),
            UnwindCode::MsftOpContext => write!(f, "MsftOpContext"),
            UnwindCode::MsftOpEcContext => write!(f, "MsftOpEcContext"),
            UnwindCode::MsftOpClearUnwoundToCall => write!(f, "MsftOpClearUnwoundToCall"),
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
    #[inline]
    fn ensure_in_bounds(unwind_codes: &[u8], index: usize) -> StResult<()> {
        if index < unwind_codes.len() {
            Ok(())
        } else {
            Err(Error::UnwindCodeOutOfBounds { module: None, requested: index, available: unwind_codes.len() })
        }
    }

    /// Returns the previous stack frame for packed unwind codes.
    ///
    /// # Safety
    /// `stack_frame` must point at an in-flight frame whose stack memory matches the
    /// layout described by the packed unwind record. The routine dereferences locations
    /// derived from `stack_frame.sp`, so the caller must guarantee those addresses remain
    /// accessible and correctly initialized.
    pub fn get_previous_stack_frame_packed(
        frame_size: u16,
        cr: FrameChainMode,
        reg_i: u8,
        reg_f: u8,
        h: u8,
        stack_frame: &StackFrame,
    ) -> StResult<StackFrame> {
        let mut prev_sp = stack_frame.sp;
        let mut prev_pc = stack_frame.pc;

        let mut integer_save_size = reg_i;
        if cr == FrameChainMode::UnchainedSavedLr {
            integer_save_size += 1; // unchained function, <lr> is saved in stack
        }

        let mut floating_point_save_size = reg_f;
        if reg_f != 0 {
            floating_point_save_size += 1;
        }

        let mut reg_save_size = integer_save_size + floating_point_save_size;
        if reg_save_size > 0 {
            reg_save_size += h * 8;
        }

        reg_save_size = (reg_save_size + 1) & !1; // ALIGN_UP_BY 2

        let location_size = frame_size / 8 - reg_save_size as u16;

        log::debug!("    > integer_save_size: 0x{:X}", integer_save_size); // debug
        log::debug!("    > floating_point_save_size: 0x{:X}", floating_point_save_size); // debug
        log::debug!("    > reg_save_size: 0x{:X}", reg_save_size); // debug
        log::debug!("    > location_size: 0x{:X}", location_size); // debug

        log::debug!("    > IN(packed): {}", stack_frame); // debug

        if cr == FrameChainMode::ChainedWithPac {
            log::error!("   > PAC-sign return address encountered");
            return Err(Error::UnexpectedUnwindCode { module: None });
        }

        let mut sav_slot = 0;
        let mut save_predec_done = false; // This account for pre-decrement stack save operation

        // Save integer registers
        if reg_i != 0 {
            // Special case for only x19 + LR, for which an _x option is not
            // available, so do the SP decrement by itself first.
            if reg_i == 1 && cr == FrameChainMode::UnchainedSavedLr {
                log::debug!("    > alloc_s (0x{:X})", reg_save_size * 8);
                // prev_pc = read_pointer64(prev_sp + location_size as u64 * 8); // dereference lr
                prev_sp += reg_save_size as u64 * 8;
                save_predec_done = true;
            }

            // Issue save-pair instructions as long as there are even number
            // or registers to lave left.
            let mut intreg = 0;
            while intreg < (reg_i / 2) * 2 {
                if !save_predec_done {
                    log::debug!("    > save_regp_x (x{}, x{}, -0x{:X})", intreg, intreg + 1, reg_save_size * 8);
                    sav_slot += 2;
                    save_predec_done = true;
                } else {
                    log::debug!("    > save_regp (x{}, x{}, 0x{:X})", intreg, intreg + 1, sav_slot * 8);
                    sav_slot += 2;
                }
                intreg += 2
            }

            // Address the remaining possible cases:
            //    - Last remaining odd register
            //    - LR, when CR=1 (saving LR needed but no FP chain)
            //    - Both, as a pair
            if (reg_i % 2) == 1 {
                if cr == FrameChainMode::UnchainedSavedLr {
                    // special case at the top of the function makes sure
                    // !save_predec_done can't even happen.
                    log::debug!("    > save_lrpair (x{}, 0x{:X})", intreg, sav_slot * 8);
                    // sav_slot += 2;
                } else if !save_predec_done {
                    log::debug!("    > save_reg_x (x{}, -0x{:X})", intreg, reg_save_size * 8);
                    // sav_slot += 1;
                    // save_predec_done = true;
                } else {
                    log::debug!("    > save_reg (x{}, 0x{:X})", intreg, sav_slot * 8);
                    // sav_slot += 1;
                }
            } else if cr == FrameChainMode::UnchainedSavedLr {
                if !save_predec_done {
                    log::debug!("    > save_reg_x (x{}, -0x{:X})", 11, reg_save_size * 8);
                    // sav_slot += 1;
                    // save_predec_done = true;
                } else {
                    log::debug!("    > save_reg (x{}, 0x{:X})", 11, sav_slot * 8);
                    // sav_slot += 1;
                }
            }
        }

        // Skipping floating point registers save handling

        // Reserve space for locals and fp,lr chain.
        if location_size > 0 {
            if cr == FrameChainMode::ChainedWithPac || cr == FrameChainMode::Chained {
                if location_size <= (512 / 8) {
                    log::debug!("    > save_fplr_x (0x{:X})", -(location_size as i32 * 8));
                } else {
                    log::debug!("    > alloc  (0x{:X})", location_size * 8);
                    log::debug!("    > save_fplr (0x{:X})", 0);
                }

                log::debug!("    > set_fp");
                // fp_set = true;
            } else {
                // +0028 add  sp,sp,#20           ; Actual=add   sp,sp,#0x20
                // +002C ldr  lr,[sp,#0x0]        ; Actual=ldr   lr,[sp],#0x10
                log::debug!("    > alloc  (0x{:X})", location_size * 8); // 0x20
                // SAFETY: `prev_sp` points to the active stack frame supplied by the
                // caller, and the unwind metadata guarantees the return address lives
                // `location_size * 8` bytes above it. The computed address therefore
                // targets properly aligned stack memory containing the saved LR.
                prev_pc = unsafe { read_pointer64(prev_sp + location_size as u64 * 8)? }; // dereference lr
            }
        }

        prev_sp += frame_size as u64;

        let prev_stack_frame = StackFrame { sp: prev_sp, pc: prev_pc, ..*stack_frame };
        log::debug!("    > OUT(packed): {}", prev_stack_frame); // debug
        Ok(prev_stack_frame)
    }

    /// Returns the previous stack frame for unpacked unwind codes.
    ///
    /// # Safety
    /// The `stack_frame` argument must represent a valid frame in the target thread and
    /// its stack pointer must provide read access to the slots touched by the unwind
    /// bytecode. Otherwise the pointer arithmetic performed here can observe
    /// uninitialized or unmapped memory.
    pub fn get_previous_stack_frame_unpacked(unwind_codes: &[u8], stack_frame: &StackFrame) -> StResult<StackFrame> {
        let mut i = 0;
        let mut prev_sp = stack_frame.sp;
        let mut prev_pc = stack_frame.pc;
        let mut prev_fp = stack_frame.fp;

        log::debug!("    > IN(unpacked): {}", stack_frame); // debug

        // The main unwind decode logic
        while i < unwind_codes.len() {
            let byte = unwind_codes[i];
            if (byte >> 5) & 0b111 == 0b000 {
                // AllocS(u8) -> 000xxxxx
                // 000xxxxx: allocate small stack with size < 512 (2^5 * 16).
                // example: sub   sp,sp,#0xA0

                let x = byte & 0b00011111;
                log::debug!("    > {}", UnwindCode::AllocS(x)); // debug

                prev_sp += x as u64 * 16; // deallocate space on the stack

                i += 1;
            } else if (byte >> 5) & 0b111 == 0b001 {
                // SaveR19R20X(u8) -> 001zzzzz
                // 001zzzzz: save <x19,x20> pair at [sp-#Z*8]!, pre-indexed offset >= -248

                let z = byte & 0b00011111;
                log::debug!("    > {}", UnwindCode::SaveR19R20X(z)); // debug

                prev_sp += z as u64 * 8; // pre increment the offset
                // ignore r19 r20 values

                i += 1;
            } else if (byte >> 6) & 0b11 == 0b01 {
                // SaveFpLr(u8) -> 01zzzzzz |  stp   fp,lr,[sp,#0x{:X}]
                // 01zzzzzz: save <x29,lr> pair at [sp+#Z*8], offset <= 504.

                let z = byte & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveFpLr(z)); // debug

                // no pre decrement of sp
                // SAFETY: `prev_sp` originates from the in-flight stack frame and the
                // unwind opcode guarantees that FP and LR were stored contiguously at
                // `z * 8` bytes from SP. Reading both slots stays within the recorded
                // stack allocation and observes aligned 64-bit values.
                prev_fp = unsafe { read_pointer64(prev_sp + z as u64 * 8)? }; // dereference fp
                // SAFETY: Similar to the FP read above, the LR sits immediately after
                // the FP in memory (8 bytes further), within the stack's allocated range.
                prev_pc = unsafe {
                    read_pointer64(prev_sp + z as u64 * 8 + 8 /* step over fp */)?
                }; // dereference lr

                i += 1;
            } else if (byte >> 6) & 0b11 == 0b10 {
                // SaveFpLrX(u8) -> 10zzzzzz | stp   fp,lr,[sp,#-0x{:X}]!
                // 10zzzzzz: save <x29,lr> pair at [sp-(#Z+1)*8]!, pre-indexed offset >= -512

                let z = byte & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveFpLrX(z)); // debug

                // SAFETY: The pre-decrement semantics of the opcode ensure SP already
                // points at the saved FP/LR pair. The current stack pointer is trusted
                // to reference live stack memory, so loading both values is sound.
                prev_fp = unsafe { read_pointer64(prev_sp)? }; // dereference fp
                // SAFETY: Similar to the FP read above, the LR sits immediately after
                // the FP in memory (8 bytes further), within the stack's allocated range.
                prev_pc = unsafe {
                    read_pointer64(prev_sp + 8 /* step over fp */)?
                }; // dereference lr
                prev_sp += (z as u64 + 1) * 8; // pre increment the offset

                i += 1;
            } else if (byte >> 3) & 0b11111 == 0b11000 {
                // AllocM(u16) -> 11000xxx'xxxxxxxx
                // 11000xxx'xxxxxxxx: allocate large stack with size < 32K (2^11 * 16).

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = (((byte & 0b111) as u16) << 8) | unwind_codes[i + 1] as u16;
                log::debug!("    > {}", UnwindCode::AllocM(x)); // debug

                prev_sp += x as u64 * 16; // deallocate space on the stack

                i += 2;
            } else if (byte >> 2) & 0b111111 == 0b110010 {
                // SaveRegP(u8, u8) -> 110010xx'xxzzzzzz
                // 110010xx'xxzzzzzz: save x(19+#X) pair at [sp+#Z*8], offset <= 504
                // example: stp   x19,x20,[sp,#0x80]

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b11) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveRegP(x, z)); // debug

                i += 2;
            } else if (byte >> 2) & 0b111111 == 0b110011 {
                // SaveRegPX(u8, u8) -> 110011xx'xxzzzzzz
                // 110011xx'xxzzzzzz: save pair x(19+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -512

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b11) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveRegPX(x, z)); // debug

                prev_sp += (z as u64 + 1) * 8; // pre increment the offset

                i += 2;
            } else if (byte >> 2) & 0b111111 == 0b110100 {
                // SaveReg(u8, u8) -> 110100xx'xxzzzzzz
                // 110100xx'xxzzzzzz: save reg x(19+#X) at [sp+#Z*8], offset <= 504
                // example: str   lr,[sp,#0x90] or str   x30,[sp,#0x90]

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b11) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveReg(x, z)); // debug

                // Sometimes the LR alone can be saved using SaveReg unwind code.
                // If so, we need to update the prev_pc accordingly.
                // str   lr,[sp,#0x8]
                if x + 19 == 30 {
                    // SAFETY: When LR is stored via `SaveReg`, the unwind metadata
                    // ensures the slot resides within the current stack allocation.
                    // The offset is expressed in units of 8 bytes, so the computed
                    // address is aligned and references initialized stack memory.
                    prev_pc = unsafe { read_pointer64(prev_sp + z as u64 * 8)? }; // dereference lr
                    log::debug!("    > LR is saved using SaveReg"); // debug
                }

                i += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101010 {
                // SaveRegX(u8, u8) -> 1101010x'xxxzzzzz
                // 1101010x'xxxzzzzz: save reg x(19+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -256

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b1) << 3) | ((unwind_codes[i + 1] >> 5) & 0b111);
                let z = unwind_codes[i + 1] & 0b00011111;
                log::debug!("    > {}", UnwindCode::SaveRegX(x, z)); // debug

                // Sometimes the LR alone can be saved using SaveRegX unwind code.
                // If so, we need to update the prev_pc accordingly.
                // str   lr,[sp,#-0x10]!
                if x + 19 == 30 {
                    // SAFETY: The opcode pre-decrements SP to the saved LR slot. That
                    // slot lies within the active stack frame described by the unwind
                    // metadata, so dereferencing it observes a valid 64-bit return
                    // address.
                    prev_pc = unsafe { read_pointer64(prev_sp)? }; // dereference lr
                    log::debug!("    > LR is saved using SaveRegX"); // debug
                }

                prev_sp += (z as u64 + 1) * 8; // pre increment the offset

                i += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101011 {
                // SaveLrPair(u8, u8) -> 1101011x'xxzzzzzz
                // 1101011x'xxzzzzzz: save pair <x(19+2*#X),lr> at [sp+#Z*8], offset <= 504
                // example: stp x19, lr, [sp,#0x90]

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b1) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveLrPair(x, z)); // debug

                // no pre decrement of sp
                // SAFETY: `SaveLrPair` stores the LR immediately after the
                // companion general-purpose register at the provided offset.
                // The unwind info guarantees the memory is part of the live
                // stack frame, so loading the return address is well-defined.
                prev_pc = unsafe {
                    read_pointer64(prev_sp + z as u64 * 8 + 8 /* step over fp */)?
                }; // dereference lr

                i += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101100 {
                // SaveFRegP(u8, u8) -> 1101100x'xxzzzzzz
                // 1101100x'xxzzzzzz: save pair d(8+#X) at [sp+#Z*8], offset <= 504

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b1) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveFRegP(x, z)); // debug

                i += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101101 {
                // SaveFRegPX(u8, u8) -> 1101101x'xxzzzzzz
                // 1101101x'xxzzzzzz: save pair d(8+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -512

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b1) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveFRegPX(x, z)); // debug

                i += 2;
            } else if (byte >> 1) & 0b1111111 == 0b1101110 {
                // SaveFReg(u8, u8) -> 1101110x'xxzzzzzz
                // 1101110x'xxzzzzzz: save reg d(8+#X) at [sp+#Z*8], offset <= 504

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = ((byte & 0b1) << 2) | ((unwind_codes[i + 1] >> 6) & 0b11);
                let z = unwind_codes[i + 1] & 0b00111111;
                log::debug!("    > {}", UnwindCode::SaveFReg(x, z)); // debug

                i += 2;
            } else if byte == 0b11011110 {
                // SaveFRegX(u8, u8) -> 11011110'xxxzzzzz
                // 11011110'xxxzzzzz: save reg d(8+#X) at [sp-(#Z+1)*8]!, pre-indexed offset >= -256

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = (unwind_codes[i + 1] >> 5) & 0b111;
                let z = unwind_codes[i + 1] & 0b0011111;
                log::debug!("    > {}", UnwindCode::SaveFRegX(x, z)); // debug

                prev_sp += (z as u64 + 1) * 8; // pre increment the offset

                i += 2;
            } else if byte == 0b11011111 {
                // AllocZ(u32) -> 11011111'zzzzzzzz
                // 11011111'zzzzzzzz: allocate stack with size z * SVE-VL

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = unwind_codes[i + 1] as u32;
                log::debug!("    > {}", UnwindCode::AllocZ(x)); // debug

                i += 2;
            } else if byte == 0b11100000 {
                // AllocL(u32) -> 11100000'xxxxxxxx'xxxxxxxx'xxxxxxxx
                // 11100000'xxxxxxxx'xxxxxxxx'xxxxxxxx: allocate large stack with size < 256M (2^24 * 16)

                Self::ensure_in_bounds(unwind_codes, i + 3)?;
                let x = ((unwind_codes[i + 1] as u32) << 16)
                    | ((unwind_codes[i + 2] as u32) << 8)
                    | (unwind_codes[i + 3] as u32);
                log::debug!("    > {}", UnwindCode::AllocL(x)); // debug

                prev_sp += x as u64 * 16; // pre increment the offset

                i += 4;
            } else if byte == 0b11100001 {
                // SetFp -> 11100001
                // 11100001: set up x29 with mov x29,sp

                prev_sp = prev_fp; // restore sp from fp

                log::debug!("    > {}", UnwindCode::SetFp); // debug
                i += 1;
            } else if byte == 0b11100010 {
                // AddFp(u8) -> 11100010'xxxxxxxx
                // 11100010'xxxxxxxx: set up x29 with add x29,sp,#x*8

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let x = unwind_codes[i + 1];
                log::debug!("    > {}", UnwindCode::AddFp(x)); // debug

                prev_sp = prev_fp - (x as u64 * 8); // restore sp from fp

                i += 2;
            } else if byte == 0b11100011 {
                // Nop -> 11100011

                log::debug!("    > {}", UnwindCode::Nop); // debug

                i += 1;
            } else if byte == 0b11100100 {
                // End -> 11100100

                log::debug!("    > {}", UnwindCode::End); // debug

                break; // end of prolog
            } else if byte == 0b11100101 {
                // EndC -> 11100101

                log::debug!("    > {}", UnwindCode::EndC); // debug

                break; // end of prolog
            } else if byte == 0b11100110 {
                // SaveNext -> 11100110
                // 11100110: save next register pair.
                // Example:
                // stp x19,x20,[sp,#-0x60]!   => SaveR19R20X(12)
                // stp x21,x22,[sp,#0x10]     => SaveNext
                // stp x23,x24,[sp,#0x20]     => SaveNext
                // stp x25,x26,[sp,#0x30]     => SaveNext
                // stp x27,x28,[sp,#0x40]     => SaveNext

                log::debug!("    > {}", UnwindCode::SaveNext); // debug

                i += 1;
            } else if byte == 0b11111100 {
                // PacSignLr -> 11111100

                log::debug!("    > {}", UnwindCode::PacSignLr); // debug

                i += 1;
            } else if byte == 0b11100111 {
                // Reserved1 -> 11100111

                log::debug!("    > {}", UnwindCode::Reserved1); // debug

                i += 1;
            } else if byte == 0b11101000 {
                // MsftOpTrapFrame -> 11101000

                log::debug!("    > {}", UnwindCode::MsftOpTrapFrame); // debug

                i += 1;
            } else if byte == 0b11101001 {
                // MsftOpMachineFrame -> 11101001

                log::debug!("    > {}", UnwindCode::MsftOpMachineFrame); // debug

                i += 1;
            } else if byte == 0b11101010 {
                // MsftOpContext -> 11101010

                log::debug!("    > {}", UnwindCode::MsftOpContext); // debug

                i += 1;
            } else if byte == 0b11101011 {
                // MsftOpEcContext -> 11101011

                log::debug!("    > {}", UnwindCode::MsftOpEcContext); // debug

                i += 1;
            } else if byte == 0b11101100 {
                // MsftOpClearUnwoundToCall -> 11101100

                log::debug!("    > {}", UnwindCode::MsftOpClearUnwoundToCall); // debug

                i += 1;
            } else if byte == 0b11101101 {
                // Reserved8 -> 11101101

                log::debug!("    > {}", UnwindCode::Reserved8); // debug

                i += 1;
            } else if byte == 0b11101110 {
                // Reserved9 -> 11101110

                log::debug!("    > {}", UnwindCode::Reserved9); // debug

                i += 1;
            } else if byte == 0b11101111 {
                // Reserved10 -> 11101111

                log::debug!("    > {}", UnwindCode::Reserved10); // debug

                i += 1;
            } else if byte == 0b11111000 {
                // Reserved12(u8) -> 11111000'yyyyyyyy

                Self::ensure_in_bounds(unwind_codes, i + 1)?;
                let y = unwind_codes[i + 1];
                log::debug!("    > {}", UnwindCode::Reserved12(y)); // debug

                i += 2;
            } else if byte == 0b11111001 {
                // Reserved13(u16) -> 11111001'yyyyyyyy'yyyyyyyy

                Self::ensure_in_bounds(unwind_codes, i + 2)?;
                let y = ((unwind_codes[i + 1] as u16) << 8) | (unwind_codes[i + 2] as u16);
                log::debug!("    > {}", UnwindCode::Reserved13(y)); // debug

                i += 3;
            } else if byte == 0b11111010 {
                // Reserved14(u32) -> 11111010'yyyyyyyy'yyyyyyyy'yyyyyyyy

                Self::ensure_in_bounds(unwind_codes, i + 3)?;
                let y = ((unwind_codes[i + 1] as u32) << 16)
                    | ((unwind_codes[i + 2] as u32) << 8)
                    | (unwind_codes[i + 3] as u32);
                log::debug!("    > {}", UnwindCode::Reserved14(y)); // debug

                i += 4;
            } else if byte == 0b11111011 {
                // Reserved15(u32) -> 11111011'yyyyyyyy'yyyyyyyy'yyyyyyyy'yyyyyyyy

                Self::ensure_in_bounds(unwind_codes, i + 4)?;
                let y = ((unwind_codes[i + 1] as u32) << 24)
                    | ((unwind_codes[i + 2] as u32) << 16)
                    | ((unwind_codes[i + 3] as u32) << 8)
                    | (unwind_codes[i + 4] as u32);
                log::debug!("    > {}", UnwindCode::Reserved15(y)); // debug

                i += 5;
            } else if byte == 0b11111101 {
                // Reserved16 -> 11111101

                log::debug!("    > {}", UnwindCode::Reserved16); // debug

                i += 1;
            } else if byte == 0b11111110 {
                // Reserved17 -> 11111110

                log::debug!("    > {}", UnwindCode::Reserved17); // debug

                i += 1;
            } else if byte == 0b11111111 {
                // Reserved18 -> 11111111

                log::debug!("    > {}", UnwindCode::Reserved18); // debug

                i += 1;
            }

            log::debug!("    > prev_pc: {prev_pc:016X} prev_sp: {prev_sp:016X} prev_fp: {prev_fp:016X}"); // debug
        }

        let prev_stack_frame = StackFrame { sp: prev_sp, pc: prev_pc, fp: prev_fp };
        log::debug!("    > OUT(unpacked): {}", prev_stack_frame); // debug
        Ok(prev_stack_frame)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    const IMAGE_SIZE: usize = 0x400;

    #[test]
    fn parse_packed_flag_one() {
        let bytes = vec![0u8; IMAGE_SIZE];
        let unwind = (1u32) // flag 1 (packed)
            | ((0x20u32) << 2) // function length units
            | ((3u32) << 13) // reg_f
            | ((2u32) << 16) // reg_i
            | ((1u32) << 20) // h
            | ((2u32) << 21) // cr
            | ((5u32) << 23); // frame size units

        match UnwindInfo::parse(&bytes, 0x100, unwind, Some("image")) {
            Ok(UnwindInfo::PackedUnwindInfo { flag, function_length, reg_f, reg_i, h, cr, frame_size, .. }) => {
                assert_eq!(flag, 1);
                assert_eq!(function_length, 0x80);
                assert_eq!(reg_f, 3);
                assert_eq!(reg_i, 2);
                assert_eq!(h, 1);
                assert_eq!(cr, FrameChainMode::ChainedWithPac);
                assert_eq!(frame_size, 0x50);
            }
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn parse_unpacked_with_extended_header() {
        let mut bytes = vec![0u8; IMAGE_SIZE];
        let xdata_rva = 0x80usize;
        let function_units = 0x12u32;
        let mut header1 = function_units; // function length bits
        header1 |= (0u32) << 18; // vers
        header1 |= (0u32) << 20; // x
        header1 |= (0u32) << 21; // e -> triggers epilog scopes
        header1 |= (0u32) << 22; // epilog count (initially 0)
        header1 |= (0u32) << 27; // code words (initially 0)
        bytes[xdata_rva..xdata_rva + 4].copy_from_slice(&header1.to_le_bytes());

        let extended_epilog_count = 3u32;
        let extended_code_words = 4u32;
        let header2 = (extended_code_words << 16) | extended_epilog_count;
        bytes[xdata_rva + 4..xdata_rva + 8].copy_from_slice(&header2.to_le_bytes());

        let epilog_scopes_offset = xdata_rva + 8;
        for i in 0..extended_epilog_count as usize {
            let scope_bytes = (0xA0u32 + i as u32).to_le_bytes();
            bytes[epilog_scopes_offset + i * 4..epilog_scopes_offset + (i + 1) * 4].copy_from_slice(&scope_bytes);
        }

        let unwind_codes_offset = epilog_scopes_offset + extended_epilog_count as usize * 4;
        let unwind_codes: [u8; 16] =
            [0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC];
        bytes[unwind_codes_offset..unwind_codes_offset + unwind_codes.len()].copy_from_slice(&unwind_codes);

        match UnwindInfo::parse(&bytes, 0x200, xdata_rva as u32, Some("image")) {
            Ok(UnwindInfo::UnpackedUnwindInfo {
                function_length,
                unwind_code_words,
                epilog_count,
                e,
                x,
                vers,
                unwind_codes: parsed_codes,
                ..
            }) => {
                assert_eq!(function_length, function_units * 4);
                assert_eq!(unwind_code_words, extended_code_words as u16);
                assert_eq!(epilog_count, extended_epilog_count as u16);
                assert_eq!(e, 0);
                assert_eq!(x, 0);
                assert_eq!(vers, 0);
                assert_eq!(parsed_codes, &unwind_codes);
            }
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn parse_with_reserved_flag_returns_error() {
        let bytes = vec![0u8; IMAGE_SIZE];
        let err = UnwindInfo::parse(&bytes, 0x100, 3, None).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }));
    }

    #[test]
    fn parse_unpacked_invalid_version_errors() {
        let mut bytes = vec![0u8; IMAGE_SIZE];
        let xdata_rva = 0x40usize;
        let mut header1 = 0x10u32; // function length units
        header1 |= 1 << 18; // vers = 1 -> unsupported
        bytes[xdata_rva..xdata_rva + 4].copy_from_slice(&header1.to_le_bytes());

        let err = UnwindInfo::parse(&bytes, 0x80, xdata_rva as u32, Some("image")).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }));
    }

    #[test]
    fn get_previous_stack_frame_packed_basic() {
        let mut stack = vec![0u64; 8];
        let return_address = 0xDEADBEEFu64;
        let base_ptr = stack.as_mut_ptr();
        // SAFETY: The stack vector owns `base_ptr`, and writing at index 4 stays
        // within its allocated eight-element buffer.
        unsafe {
            *base_ptr.add(4) = return_address;
        }

        let frame = StackFrame { sp: base_ptr as u64, pc: 0xAAAABBBBCCCCDDDD, fp: 0 };
        let prev = UnwindCode::get_previous_stack_frame_packed(32, FrameChainMode::Unchained, 0, 0, 0, &frame).unwrap();
        assert_eq!(prev.pc, return_address);
        assert_eq!(prev.sp, frame.sp + 32);
        assert_eq!(prev.fp, frame.fp);
    }

    #[test]
    fn get_previous_stack_frame_packed_cr_two_errors() {
        let stack = [0u64; 1];
        let frame = StackFrame { sp: stack.as_ptr() as u64, pc: 0x1234, fp: 0 };
        match UnwindCode::get_previous_stack_frame_packed(16, FrameChainMode::ChainedWithPac, 0, 0, 0, &frame) {
            Err(err) => assert_eq!(err, Error::UnexpectedUnwindCode { module: None }),
            Ok(_) => panic!("expected error for PAC-signed return address"),
        }
    }

    #[test]
    fn frame_chain_mode_from_bits_parses_all_variants() {
        assert_eq!(FrameChainMode::from_bits(0, Some("test")).unwrap(), FrameChainMode::Unchained);
        assert_eq!(FrameChainMode::from_bits(1, Some("test")).unwrap(), FrameChainMode::UnchainedSavedLr);
        assert_eq!(FrameChainMode::from_bits(2, Some("test")).unwrap(), FrameChainMode::ChainedWithPac);
        assert_eq!(FrameChainMode::from_bits(3, Some("test")).unwrap(), FrameChainMode::Chained);

        let err = FrameChainMode::from_bits(4, Some("mod")).unwrap_err();
        assert_eq!(err, Error::Malformed { module: Some("mod"), reason: "Unsupported chain register encoding" });
    }

    #[test]
    fn get_previous_stack_frame_unpacked_save_fp_lr() {
        let mut stack = vec![0u64; 4];
        let saved_fp = 0xCAFEBABECAFEBABEu64;
        let return_address = 0x1122334455667788u64;
        let base_ptr = stack.as_mut_ptr();
        // SAFETY: `base_ptr` refers to the four-element vector above, so indices 0
        // and 1 are in-bounds for writing the saved FP and LR.
        unsafe {
            *base_ptr.add(0) = saved_fp;
            *base_ptr.add(1) = return_address;
        }

        let frame = StackFrame { sp: base_ptr as u64, pc: 0x7777, fp: 0 };
        let codes = [0x40, 0xE4]; // SaveFpLr(0), End
        let prev = UnwindCode::get_previous_stack_frame_unpacked(&codes, &frame).unwrap();

        assert_eq!(prev.pc, return_address);
        assert_eq!(prev.fp, saved_fp);
        assert_eq!(prev.sp, frame.sp);
    }

    #[test]
    fn get_previous_stack_frame_unpacked_save_reg_lr() {
        let mut stack = vec![0u64; 8];
        let return_address = 0xABCDEF0123456789u64;
        let base_ptr = stack.as_mut_ptr();
        // SAFETY: The eight-element vector backs `base_ptr`; writing to index 1
        // remains within the allocated slice.
        unsafe {
            *base_ptr.add(1) = return_address;
        }

        let frame = StackFrame { sp: base_ptr as u64, pc: 0x2222, fp: 0 };
        let codes = [0xD2, 0xC1, 0xE4]; // SaveReg storing LR at offset 1, End
        let prev = UnwindCode::get_previous_stack_frame_unpacked(&codes, &frame).unwrap();
        assert_eq!(prev.pc, return_address);
        assert_eq!(prev.sp, frame.sp);
    }

    #[test]
    fn get_previous_stack_frame_unpacked_allocs_adjusts_sp() {
        let stack = [0u64; 1];
        let base_ptr = stack.as_ptr();
        let frame = StackFrame { sp: base_ptr as u64, pc: 0xBEEF, fp: 0 };
        let codes = [0x02, 0xE4]; // AllocS(2) -> +32 bytes, End
        let prev = UnwindCode::get_previous_stack_frame_unpacked(&codes, &frame).unwrap();
        assert_eq!(prev.sp, frame.sp + 32);
        assert_eq!(prev.pc, frame.pc);
    }

    #[test]
    fn unwind_info_packed_get_previous_stack_frame_reads_saved_lr() {
        let mut stack_words = Box::new([0u64; 8]);
        let return_address = 0xFEED_FACE_CAFE_F00Du64;
        stack_words[4] = return_address;
        let stack_base = stack_words.as_ptr() as u64;

        let frame = StackFrame { sp: stack_base, pc: 0xAA55, fp: 0xBB66 };
        let info = UnwindInfo::PackedUnwindInfo {
            image_name: Some("packed"),
            func_start_rva: 0,
            flag: 1,
            function_length: 0,
            reg_f: 0,
            reg_i: 0,
            h: 0,
            cr: FrameChainMode::Unchained,
            frame_size: 32,
        };

        let previous = info.get_previous_stack_frame(&frame).expect("packed unwind should succeed");
        assert_eq!(previous.pc, return_address);
        assert_eq!(previous.sp, frame.sp + 32);
        assert_eq!(previous.fp, frame.fp);

        drop(stack_words);
    }

    #[test]
    fn unwind_info_unpacked_get_previous_stack_frame_reads_saved_fp_and_lr() {
        let mut stack_words = Box::new([0u64; 2]);
        let saved_fp = 0x1111_2222_3333_4444u64;
        let return_address = 0x5555_6666_7777_8888u64;
        stack_words[0] = saved_fp;
        stack_words[1] = return_address;
        let stack_base = stack_words.as_ptr() as u64;

        let codes = [0x40, 0xE4]; // SaveFpLr(0), End
        let info = UnwindInfo::UnpackedUnwindInfo {
            image_name: Some("unpacked"),
            func_start_rva: 0,
            xdata_rva: 0,
            function_length: 0,
            unwind_code_words: 1,
            epilog_count: 0,
            e: 0,
            x: 0,
            vers: 0,
            unwind_codes: &codes,
        };

        let frame = StackFrame { sp: stack_base, pc: 0x9999, fp: 0 };
        let previous = info.get_previous_stack_frame(&frame).expect("unpacked unwind should succeed");

        assert_eq!(previous.pc, return_address);
        assert_eq!(previous.fp, saved_fp);
        assert_eq!(previous.sp, stack_base);

        drop(stack_words);
    }

    #[test]
    fn get_previous_stack_frame_unpacked_exercises_all_opcodes() {
        let mut stack_words = vec![0u64; 128].into_boxed_slice();
        let stack_base = stack_words.as_ptr() as u64;

        let saved_fp_from_fp_lr_x_index = 60usize;
        let saved_fp_from_fp_lr_x = stack_base + saved_fp_from_fp_lr_x_index as u64 * 8;
        stack_words[3] = saved_fp_from_fp_lr_x;
        let lr_from_fp_lr_x = 0x4444_4444_4444_4444u64;
        stack_words[4] = lr_from_fp_lr_x;

        let saved_fp_regular_index = 50usize;
        let saved_fp_regular = stack_base + saved_fp_regular_index as u64 * 8;
        stack_words[5] = saved_fp_regular;
        let lr_from_fplr = 0x5555_5555_5555_5555u64;
        stack_words[6] = lr_from_fplr;

        let lr_from_save_reg = 0x6666_6666_6666_6666u64;
        stack_words[9] = lr_from_save_reg;

        let lr_from_save_regx = 0x7777_7777_7777_7777u64;
        stack_words[8] = lr_from_save_regx;

        let lr_from_save_lr_pair = 0x8888_8888_8888_8888u64;
        stack_words[11] = lr_from_save_lr_pair;

        stack_words[saved_fp_regular_index] = 0;
        stack_words[saved_fp_from_fp_lr_x_index] = 0;

        let codes: Vec<u8> = vec![
            0x01, // AllocS(1)
            0x21, // SaveR19R20X(z=1)
            0x80, // SaveFpLrX(z=0)
            0x41, // SaveFpLr(z=1)
            0xC0, 0x01, // AllocM(x=1)
            0xC8, 0x00, // SaveRegP
            0xCC, 0x01, // SaveRegPX(z=1)
            0xD2, 0xC1, // SaveReg (stores LR)
            0xD5, 0x60, // SaveRegX (stores LR)
            0xD6, 0x01, // SaveLrPair(z=1)
            0xD8, 0x00, // SaveFRegP
            0xDA, 0x00, // SaveFRegPX
            0xDC, 0x00, // SaveFReg
            0xDE, 0x00, // SaveFRegX(z=0)
            0xDF, 0x02, // AllocZ
            0xE0, 0x00, 0x00, 0x01, // AllocL(x=1)
            0xE1, // SetFp
            0xE2, 0x02, // AddFp(x=2)
            0xE3, // Nop
            0xE6, // SaveNext
            0xFC, // PacSignLr
            0xE7, // Reserved1
            0xE8, // MsftOpTrapFrame
            0xE9, // MsftOpMachineFrame
            0xEA, // MsftOpContext
            0xEB, // MsftOpEcContext
            0xEC, // MsftOpClearUnwoundToCall
            0xED, // Reserved8
            0xEE, // Reserved9
            0xEF, // Reserved10
            0xF8, 0x00, // Reserved12
            0xF9, 0x00, 0x01, // Reserved13
            0xFA, 0x00, 0x00, 0x01, // Reserved14
            0xFB, 0x00, 0x00, 0x00, 0x01, // Reserved15
            0xFD, // Reserved16
            0xFE, // Reserved17
            0xFF, // Reserved18
            0xE4, // End
        ];

        let frame = StackFrame { sp: stack_base, pc: 0xAAAABBBBCCCCDDDD, fp: 0x1111_2222_3333_4444 };

        let previous =
            UnwindCode::get_previous_stack_frame_unpacked(&codes, &frame).expect("unpacked unwind should succeed");

        assert_eq!(previous.pc, lr_from_save_lr_pair);
        assert_eq!(previous.fp, saved_fp_regular);
        assert_eq!(previous.sp, saved_fp_regular - 16);
    }

    #[test]
    fn get_previous_stack_frame_packed_handles_saved_lr_and_registers() {
        let frame_size = 128u16;
        let reg_i = 1u8;
        let reg_f = 0u8;
        let h = 0u8;
        let chain_mode = FrameChainMode::UnchainedSavedLr;

        let mut stack = vec![0u64; 64];
        let base_ptr = stack.as_mut_ptr() as u64;

        let mut integer_save_size = reg_i;
        if chain_mode == FrameChainMode::UnchainedSavedLr {
            integer_save_size += 1;
        }
        let mut floating_point_save_size = reg_f;
        if reg_f != 0 {
            floating_point_save_size += 1;
        }
        let mut reg_save_size = integer_save_size + floating_point_save_size;
        if reg_save_size > 0 {
            reg_save_size += h * 8;
        }
        reg_save_size = (reg_save_size + 1) & !1;

        let mut lr_slot_base = base_ptr;
        if reg_i == 1 && chain_mode == FrameChainMode::UnchainedSavedLr {
            lr_slot_base += reg_save_size as u64 * 8;
        }
        let location_size = frame_size / 8 - reg_save_size as u16;
        lr_slot_base += location_size as u64 * 8;

        let lr_index = ((lr_slot_base - base_ptr) / 8) as usize;
        let lr_value = 0xAAAA_BBBB_CCCC_DDDDu64;
        stack[lr_index] = lr_value;

        let frame = StackFrame { sp: base_ptr, pc: 0x1111_2222_3333_4444, fp: 0 };
        let prev = UnwindCode::get_previous_stack_frame_packed(frame_size, chain_mode, reg_i, reg_f, h, &frame)
            .expect("packed unwind should succeed");

        let expected_sp = frame.sp
            + frame_size as u64
            + if reg_i == 1 && chain_mode == FrameChainMode::UnchainedSavedLr { reg_save_size as u64 * 8 } else { 0 };

        assert_eq!(prev.pc, lr_value);
        assert_eq!(prev.sp, expected_sp);
    }

    #[test]
    fn get_previous_stack_frame_packed_handles_register_saves_and_h_alignment() {
        let frame_size = 160; // 0xA0
        let mut stack = vec![0u64; 64];
        let base_ptr = stack.as_mut_ptr() as u64;
        let integer_save_size = 3u8;
        let mut floating_point_save_size = 2u8;
        if floating_point_save_size != 0 {
            floating_point_save_size += 1;
        }

        let mut reg_save_size = integer_save_size + floating_point_save_size;
        if reg_save_size > 0 {
            reg_save_size += 8; // h value supplied to the unwind call
        }

        reg_save_size = (reg_save_size + 1) & !1;
        let location_index = (frame_size / 8) as usize - reg_save_size as usize;

        let saved_lr = 0xBBBB_CCCC_DDDD_EEEE;
        stack[location_index] = saved_lr;

        let frame = StackFrame { sp: base_ptr, pc: 0x5555_6666_7777_8888, fp: 0 };
        let prev = UnwindCode::get_previous_stack_frame_packed(frame_size, FrameChainMode::Unchained, 3, 2, 1, &frame)
            .expect("packed unwind should succeed");

        assert_eq!(prev.sp, frame.sp + frame_size as u64);
        assert_eq!(prev.pc, saved_lr);
    }

    #[test]
    fn get_previous_stack_frame_packed_chain_with_fp_store() {
        let frame_size = 96; // 0x60
        let mut stack = vec![0u64; 64];
        let base_ptr = stack.as_mut_ptr() as u64;
        stack[6] = 0x1111_2222_3333_4444;
        stack[7] = 0x9999_AAAA_BBBB_CCCC;

        let frame = StackFrame { sp: base_ptr, pc: 0x1010_2020_3030_4040, fp: 0 };
        let prev = UnwindCode::get_previous_stack_frame_packed(frame_size, FrameChainMode::Chained, 0, 0, 0, &frame)
            .expect("packed unwind should succeed");

        assert_eq!(prev.pc, frame.pc);
        assert_eq!(prev.sp, frame.sp + frame_size as u64);
    }

    #[test]
    fn get_previous_stack_frame_unpacked_handles_endc() {
        let codes = [0xE5];
        let frame = StackFrame { sp: 0x1000, pc: 0x2000, fp: 0x3000 };
        let previous = UnwindCode::get_previous_stack_frame_unpacked(&codes, &frame).unwrap();
        assert_eq!(previous.sp, frame.sp);
        assert_eq!(previous.pc, frame.pc);
        assert_eq!(previous.fp, frame.fp);
    }

    #[test]
    fn unwind_info_display_for_packed_variant() {
        let info = UnwindInfo::PackedUnwindInfo {
            image_name: Some("image"),
            func_start_rva: 0x1234,
            flag: 1,
            function_length: 0x40,
            reg_f: 2,
            reg_i: 3,
            h: 1,
            cr: FrameChainMode::Chained,
            frame_size: 0x80,
        };

        let rendered = format!("{info}");
        assert!(rendered.contains("UnwindInfo::PackedUnwindInfo"));
        assert!(rendered.contains("func_start_rva: 0x1234"));
        assert!(rendered.contains("function_length: 0x40"));
        assert!(rendered.contains("reg_f: 0x2"));
        assert!(rendered.contains("reg_i: 0x3"));
        assert!(rendered.contains("cr: 0x3"));
        assert!(rendered.contains("frame_size: 0x80"));
    }

    #[test]
    fn unwind_info_display_for_unpacked_variant() {
        let codes = [0xAA, 0xBB, 0xCC];
        let info = UnwindInfo::UnpackedUnwindInfo {
            image_name: None,
            func_start_rva: 0x5678,
            xdata_rva: 0x200,
            function_length: 0x100,
            unwind_code_words: 2,
            epilog_count: 1,
            e: 1,
            x: 0,
            vers: 0,
            unwind_codes: &codes,
        };

        let rendered = format!("{info}");
        assert!(rendered.contains("UnwindInfo::UnpackedUnwindInfo"));
        assert!(rendered.contains("xdata_rva: 0x200"));
        assert!(rendered.contains("func_start_rva: 0x5678"));
        assert!(rendered.contains("function_length: 0x100"));
        assert!(rendered.contains("epilog_count: 0x1"));
        assert!(rendered.contains("unwind_code_words: 0x2"));
        assert!(rendered.contains("AA"));
        assert!(rendered.contains("BB"));
        assert!(rendered.contains("CC"));
    }

    #[test]
    fn unwind_code_display_formats_all_variants() {
        let cases = [
            (UnwindCode::AllocS(3), "AllocS(3) | sub   sp,sp,#0x30"),
            (UnwindCode::SaveR19R20X(4), "SaveR19R20X(4) | stp   x19,x20,[sp,#-0x20]!"),
            (UnwindCode::SaveFpLr(2), "SaveFpLr(2) | stp   fp,lr,[sp,#0x10]"),
            (UnwindCode::SaveFpLrX(1), "SaveFpLrX(1) | stp   fp,lr,[sp,#-0x10]!"),
            (UnwindCode::AllocM(5), "AllocM(5) | sub   sp,sp,#0x50"),
            (UnwindCode::SaveRegP(1, 2), "SaveRegP(1, 2) | stp   x20,x21,[sp,#0x10]"),
            (UnwindCode::SaveRegPX(1, 2), "SaveRegPX(1, 2) | stp   x20,x21,[sp,#-0x18]!"),
            (UnwindCode::SaveReg(2, 3), "SaveReg(2, 3) | str   x21,[sp,#0x18]"),
            (UnwindCode::SaveRegX(1, 3), "SaveRegX(1, 3) | str   x20,[sp,#-0x20]!"),
            (UnwindCode::SaveLrPair(1, 2), "SaveLrPair(1, 2) | stp x21,lr,[sp,#0x10]"),
            (UnwindCode::SaveFRegP(2, 1), "SaveFRegP(2, 1) | stp   d10,d11,[sp,#0x8]"),
            (UnwindCode::SaveFRegPX(2, 1), "SaveFRegPX(2, 1) | stp   d10,d11,[sp,#-0x10]!"),
            (UnwindCode::SaveFReg(0, 4), "SaveFReg(0, 4) | str   d8,[sp,#0x20]"),
            (UnwindCode::SaveFRegX(1, 2), "SaveFRegX(1, 2) | str   d9,[sp,#-0x18]!"),
            (UnwindCode::AllocZ(3), "AllocZ(3)"),
            (UnwindCode::AllocL(2), "AllocL(2) | sub   sp,sp,#0x20"),
            (UnwindCode::SetFp, "SetFp | mov  fp,sp"),
            (UnwindCode::AddFp(3), "AddFp(3) | add fp,sp,#0x18"),
            (UnwindCode::Nop, "Nop"),
            (UnwindCode::End, "End"),
            (UnwindCode::EndC, "EndC"),
            (UnwindCode::SaveNext, "SaveNext"),
            (UnwindCode::PacSignLr, "PacSignLr"),
            (UnwindCode::Reserved1, "Reserved1"),
            (UnwindCode::MsftOpTrapFrame, "MsftOpTrapFrame"),
            (UnwindCode::MsftOpMachineFrame, "MsftOpMachineFrame"),
            (UnwindCode::MsftOpContext, "MsftOpContext"),
            (UnwindCode::MsftOpEcContext, "MsftOpEcContext"),
            (UnwindCode::MsftOpClearUnwoundToCall, "MsftOpClearUnwoundToCall"),
            (UnwindCode::Reserved8, "Reserved8"),
            (UnwindCode::Reserved9, "Reserved9"),
            (UnwindCode::Reserved10, "Reserved10"),
            (UnwindCode::Reserved12(0x12), "Reserved12(18)"),
            (UnwindCode::Reserved13(0x1234), "Reserved13(4660)"),
            (UnwindCode::Reserved14(0x12345), "Reserved14(74565)"),
            (UnwindCode::Reserved15(0x12345678), "Reserved15(305419896)"),
            (UnwindCode::Reserved16, "Reserved16"),
            (UnwindCode::Reserved17, "Reserved17"),
            (UnwindCode::Reserved18, "Reserved18"),
        ];

        for (code, expected) in cases {
            assert_eq!(format!("{code}"), expected);
        }
    }
}
