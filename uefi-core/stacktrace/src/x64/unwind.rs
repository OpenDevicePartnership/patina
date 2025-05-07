use crate::alloc::string::ToString;
/// Module to parse the x64 unwind data from .pdata section.
use core::fmt;

use crate::{
    byte_reader::ByteReader,
    error::{Error, StResult},
};

/// `RuntimeFunction`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170#struct-runtime_function>
#[derive(Debug, Clone)]
pub struct RuntimeFunction<'a> {
    /// loaded image memory as a byte slice
    image_base: &'a [u8],

    /// image name extracted from the loaded pe image
    image_name: Option<&'a str>,

    /// start of the function rva
    pub start_rva: u32,

    /// end of the function rva
    pub end_rva: u32,

    /// rva for unwind info
    pub unwind_info: u32,
}

impl<'a> fmt::Display for RuntimeFunction<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RuntimeFunction {{ image_base:0x{:p}, start_rva: 0x{:08X}, end_rva: 0x{:08X}, unwind_info: 0x{:08X} }}",
            self.image_base.as_ptr(),
            self.start_rva,
            self.end_rva,
            self.unwind_info
        )
    }
}

impl<'a> RuntimeFunction<'a> {
    pub fn new(
        image_base: &'a [u8],
        image_name: Option<&'a str>,
        start_rva: u32,
        end_rva: u32,
        unwind_info: u32,
    ) -> Self {
        Self { image_base, image_name, start_rva, end_rva, unwind_info }
    }

    /// Parse the Unwind Info data pointed by RuntimeFunction
    pub fn get_unwind_info(&self) -> StResult<UnwindInfo> {
        UnwindInfo::parse(&self.image_base[self.unwind_info as usize..], self.image_name).map_err(|_| {
            Error::UnwindInfoNotFound(
                self.image_name.map(|s| s.to_string()),
                self.image_base.as_ptr() as u64,
                self.unwind_info,
            )
        })
    }
}

/// `UnwindInfo`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170#struct-unwind_info>
#[derive(Debug)]
pub struct UnwindInfo<'a> {
    /// byte slice pointing to unwind info data
    unwind_info_bytes: &'a [u8],

    /// image name extracted from the loaded pe image
    image_name: Option<&'a str>,

    version: u8,
    flags: u8,
    size_of_prolog: u8,
    count_of_unwind_codes: u8,
    frame_register: u8,
    frame_register_offset: u32,
    unwind_codes: &'a [u8],
}

impl<'a> fmt::Display for UnwindInfo<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UnwindInfo {{ unwind_info_bytes:0x{:p}, version: 0x{:02X}, flags: 0x{:02X}, size_of_prolog: 0x{:02X}, count_of_unwind_codes: 0x{:02X}, frame_register: 0x{:02X}, frame_register_offset: 0x{:08X} }}",
            self.unwind_info_bytes.as_ptr(),
            self.version,
            self.flags,
            self.size_of_prolog,
            self.count_of_unwind_codes,
            self.frame_register,
            self.frame_register_offset
        )
    }
}

impl<'a> UnwindInfo<'a> {
    /// Function to Parse the Unwind Info data pointed by RuntimeFunction
    pub fn parse(bytes: &'a [u8], image_name: Option<&'a str>) -> StResult<UnwindInfo<'a>> {
        let mut offset = 0usize;
        let byte = bytes.read8_with(&mut offset)?;
        let version = byte & 0b111;
        let flags = byte >> 3;

        if version != 1 && version != 2 {
            let msg = format!("unsupported unwind code version ({})", version);
            return Err(Error::Malformed(msg));
        }

        let size_of_prolog = bytes.read8_with(&mut offset)?;
        let count_of_unwind_codes = bytes.read8_with(&mut offset)?;
        let frame = bytes.read8_with(&mut offset)?;
        let frame_register = frame & 0xf;
        let frame_register_offset = u32::from((frame >> 4) * 16);

        // Each unwind code is a 2 byte struct. Validate if we are well with in
        // the range
        if offset + count_of_unwind_codes as usize * 2 >= bytes.len() {
            let msg = "Malformed unwind code bytes".to_string();
            return Err(Error::Malformed(msg));
        }

        // Extract unwind codes(each unwind code is a 2 byte struct)
        let unwind_codes: &[u8] = &bytes[offset..offset + count_of_unwind_codes as usize * 2];
        Ok(Self {
            unwind_info_bytes: bytes,
            image_name,
            version,
            flags,
            size_of_prolog,
            count_of_unwind_codes,
            frame_register,
            frame_register_offset,
            unwind_codes,
        })
    }

    /// Function to calculate the stack pointer offset in the function
    pub fn get_stack_pointer_offset(&self) -> StResult<usize> {
        UnwindCode::get_stack_pointer_offset(self.unwind_codes)
            .map_err(|_| Error::StackOffsetNotFound(self.image_name.map(|s| s.to_string())))
    }
}

/// `UnwindCode`
/// Source: https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170#struct-unwind_code
#[allow(dead_code)] // Enum variants are used for testing the parsed bytes. Ignore their presence in release build
#[derive(Debug)]
enum UnwindCode {
    PushNonVolatile {
        // push <non volatile reg>
        prolog_offset: u8,
        reg: u8,
    },
    AllocLarge {
        // sub rsp, 0xE8
        prolog_offset: u8,
        size: u32,
    },
    AllocSmall {
        // sub rsp, 0x20
        prolog_offset: u8,
        size: u32,
    },
    SetFP {
        prolog_offset: u8,
        offset: u32,
    },
    SaveNonVolatile {
        // mov rax, rsp; mov [rax + 10h] <non volatile reg>
        prolog_offset: u8,
        reg: u8,
        offset: u32,
    },
    SaveNonVolatileFar {
        // mov rax, rsp; mov [rax + 10h] <non volatile reg>
        prolog_offset: u8,
        reg: u8,
        offset: u32,
    },
    SaveXMM(u8, u8),
    SaveXMMFar(u8, u8),
    SaveXMM128(u8, u8),
    SaveXMM128Far(u8, u8),
    PushMachFrame(u8, u8),
}

impl UnwindCode {
    /// Function to parse the UnwindCode and calculate the stack pointer offset
    /// from the function prolog
    pub fn get_stack_pointer_offset(bytes: &[u8]) -> StResult<usize> {
        let mut offset = 0usize;
        let byte_count = bytes.len();
        let mut index = 0;
        while index < byte_count {
            let _prolog_offset = bytes.read8_with(&mut index)?;
            let opcode_opinfo = bytes.read8_with(&mut index)?;
            let opcode = opcode_opinfo & 0xF;
            let opinfo = opcode_opinfo >> 4;

            match opcode {
                0 => offset += 8, // PushNonVolatile
                1 => {
                    // AllocLarge
                    let size = match opinfo {
                        // If the operation info equals 0, then the size of the
                        // allocation divided by 8 is recorded in the next slot,
                        // allowing an allocation up to 512K - 8.
                        0 => u32::from(bytes.read16_with(&mut index)?) * 8,
                        // If the operation info equals 1, then the unscaled
                        // size of the allocation is recorded in the next two
                        // slots in little-endian format, allowing allocations
                        // up to 4GB - 8
                        1 => bytes.read32_with(&mut index)?,
                        i => return Err(Error::Malformed(format!("unexpected opinfo {}", i))),
                    };

                    offset += size as usize;
                }
                2 => offset += opinfo as usize * 8 + 8, // AllocSmall
                3 => (),                                // SetFP
                4 => {
                    // SaveNonVolatile - do not contribute to rsp but still we should consume the bytes
                    bytes.read16_with(&mut index)?;
                }
                5 => {
                    // SaveNonVolatileFar - do not contribute to rsp but still we should consume the bytes
                    bytes.read32_with(&mut index)?;
                }
                6..=10 => (), // These opcodes do not contribute to rsp offset
                _ => panic!("unexpected opcode"),
            };
        }
        Ok(offset)
    }

    /// Test function to parse all UnwindCodes
    #[cfg(test)]
    pub(crate) fn _parse(bytes: &[u8], frame_register_offset: u32) -> StResult<Vec<UnwindCode>> {
        let byte_count = bytes.len();
        let mut offset = 0;
        let mut unwind_codes = Vec::new();
        while offset < byte_count {
            let prolog_offset = bytes.read8_with(&mut offset)?;
            let opcode_opinfo = bytes.read8_with(&mut offset)?;
            let opcode = opcode_opinfo & 0xF;
            let opinfo = opcode_opinfo >> 4;

            let unwind_code = match opcode {
                0 => UnwindCode::PushNonVolatile { prolog_offset, reg: opinfo },
                1 => {
                    let size = match opinfo {
                        // If the operation info equals 0, then the size of the
                        // allocation divided by 8 is recorded in the next slot,
                        // allowing an allocation up to 512K - 8.
                        0 => u32::from(bytes.read16_with(&mut offset)?) * 8,
                        // If the operation info equals 1, then the unscaled
                        // size of the allocation is recorded in the next two
                        // slots in little-endian format, allowing allocations
                        // up to 4GB - 8
                        1 => bytes.read32_with(&mut offset)?,
                        i => return Err(Error::Malformed(format!("unexpected opinfo {}", i))),
                    };
                    UnwindCode::AllocLarge { prolog_offset, size }
                }
                2 => UnwindCode::AllocSmall { prolog_offset, size: opinfo as u32 * 8 + 8 },
                3 => UnwindCode::SetFP { prolog_offset, offset: frame_register_offset },
                4 => {
                    let reg_offset = u32::from(bytes.read16_with(&mut offset)?) * 8;
                    UnwindCode::SaveNonVolatile { prolog_offset, reg: opcode, offset: reg_offset }
                }
                5 => {
                    let reg_offset = bytes.read32_with(&mut offset)?;
                    UnwindCode::SaveNonVolatileFar { prolog_offset, reg: opcode, offset: reg_offset }
                }
                6 => UnwindCode::SaveXMM(prolog_offset, opinfo),
                7 => UnwindCode::SaveXMMFar(prolog_offset, opinfo),
                8 => UnwindCode::SaveXMM128(prolog_offset, opinfo),
                9 => UnwindCode::SaveXMM128Far(prolog_offset, opinfo),
                10 => UnwindCode::PushMachFrame(prolog_offset, opinfo),
                _ => panic!("unexpected opcode"),
            };

            unwind_codes.push(unwind_code);
        }
        Ok(unwind_codes)
    }
}
