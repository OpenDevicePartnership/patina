/// Parses x64 unwind data from the `.pdata` section.
use core::fmt;

use crate::{
    byte_reader::{ByteReader, read_pointer64},
    error::{Error, StResult},
    stacktrace::StackFrame,
};

/// `UnwindInfo`
/// Source: <https://learn.microsoft.com/en-us/cpp/build/exception-handling-x64?view=msvc-170#struct-unwind_info>
#[derive(Debug)]
pub struct UnwindInfo<'a> {
    /// Byte slice pointing to the unwind info data.
    unwind_info_bytes: &'a [u8],

    /// Image name extracted from the loaded PE image.
    image_name: Option<&'static str>,

    version: u8,
    flags: u8,
    size_of_prolog: u8,
    count_of_unwind_codes: u8,
    frame_register: u8,
    frame_register_offset: u32,
    unwind_codes: &'a [u8],
}

impl fmt::Display for UnwindInfo<'_> {
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
    /// Parses the unwind info referenced by a runtime function entry.
    pub fn parse(bytes: &'a [u8], image_name: Option<&'static str>) -> StResult<UnwindInfo<'a>> {
        let mut offset = 0usize;
        let byte = bytes.read8_with(&mut offset)?;
        let version = byte & 0b111;
        let flags = byte >> 3;

        if version != 1 && version != 2 {
            return Err(Error::Malformed { module: image_name, reason: "Unsupported unwind code version" });
        }

        let size_of_prolog = bytes.read8_with(&mut offset)?;
        let count_of_unwind_codes = bytes.read8_with(&mut offset)?;
        let frame = bytes.read8_with(&mut offset)?;
        let frame_register = frame & 0xf;
        let frame_register_offset = u32::from((frame >> 4) * 16);

        // Each unwind code occupies two bytes. Ensure the count stays within
        // the available range.
        if offset + count_of_unwind_codes as usize * 2 > bytes.len() {
            return Err(Error::Malformed { module: image_name, reason: "Malformed unwind code bytes" });
        }

        // Extract the unwind codes (each unwind code is two bytes).
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

    /// Calculates the stack-pointer offset introduced by the function prolog.
    pub fn get_stack_pointer_offset(&self) -> StResult<usize> {
        UnwindCode::get_stack_pointer_offset(self.unwind_codes).map_err(|err| err.with_module(self.image_name))
    }

    /// Calculates the parameters for the previous stack frame.
    ///
    /// # Safety
    /// The provided `stack_frame` must correspond to an active x64 frame whose stack
    /// memory obeys the metadata carried by this unwind record. The implementation
    /// dereferences addresses derived from `stack_frame.sp`, so the caller must ensure
    /// those locations remain readable and contain the saved return address.
    pub fn get_previous_stack_frame(&self, stack_frame: &StackFrame) -> StResult<StackFrame> {
        let rsp = stack_frame.sp;
        let rsp_offset = self.get_stack_pointer_offset()?;
        let mut prev_rsp = rsp + rsp_offset as u64;
        // SAFETY: `prev_rsp` references the caller's stack frame and the unwind
        // metadata guarantees the return address resides at the top of that
        // frame. The pointer is 8-byte aligned, so loading a `u64` return address
        // via `read_pointer64` is well-defined.
        let prev_rip = unsafe { read_pointer64(prev_rsp)? };
        prev_rsp += 8; // pop the return address

        Ok(StackFrame { sp: prev_rsp, pc: prev_rip, ..*stack_frame })
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
    /// Parses unwind codes and calculates the stack-pointer offset produced by
    /// the function prolog.
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
                        _ => return Err(Error::Malformed { module: None, reason: "Unexpected opinfo" }),
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
                _ => panic!("Unexpected opcode"),
            };
        }
        Ok(offset)
    }

    /// Test function that parses all unwind codes.
    #[coverage(off)]
    #[cfg(all(target_os = "windows", target_arch = "x86_64", test))]
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
                        _ => return Err(Error::Malformed { module: None, reason: "Unexpected opinfo" }),
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
                _ => panic!("Unexpected opcode"),
            };

            unwind_codes.push(unwind_code);
        }
        Ok(unwind_codes)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    // Helper to build unwind-info bytes.
    fn build_unwind_bytes(
        version: u8,
        flags: u8,
        size_of_prolog: u8,
        count_of_unwind_codes: u8,
        frame_reg: u8,
        frame_reg_offset_units: u8,
        codes: &[u8],
    ) -> Vec<u8> {
        let mut v = vec![
            (flags << 3) | (version & 0b111),
            size_of_prolog,
            count_of_unwind_codes,
            (frame_reg_offset_units << 4) | (frame_reg & 0xF),
        ];
        v.extend_from_slice(codes);
        v
    }

    #[test]
    fn parse_basic_version1() {
        // Two unwind codes (each two bytes) require a length greater than
        // header (4) + codes (4).
        let codes = [0x04, 0x42, 0x02, 0x22]; // Push nonvolatile + small allocation.
        let bytes = build_unwind_bytes(1, 0, 6, 2, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("test")).unwrap();
        assert_eq!(ui.version, 1);
        assert_eq!(ui.flags, 0);
        assert_eq!(ui.size_of_prolog, 6);
        assert_eq!(ui.count_of_unwind_codes, 2);
        assert_eq!(ui.frame_register, 0);
        assert_eq!(ui.frame_register_offset, 0);
        assert_eq!(ui.unwind_codes.len(), 4);
    }

    #[test]
    fn parse_version2() {
        let codes = [0x04, 0x42];
        let bytes = build_unwind_bytes(2, 2, 4, 1, 0, 0, &codes); // flags = 2.
        let ui = UnwindInfo::parse(&bytes, None).unwrap();
        assert_eq!(ui.version, 2);
        assert_eq!(ui.flags, 2);
        assert!(ui.image_name.is_none());
    }

    #[test]
    fn parse_invalid_version() {
        let bytes = build_unwind_bytes(3, 0, 4, 0, 0, 0, &[]); // Version 3 is unsupported.
        let err = UnwindInfo::parse(&bytes, Some("bad")).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }));
    }

    #[test]
    fn parse_malformed_unwind_code_length_boundary() {
        // Header (4) + codes (4) == len; this should fail because the parser
        // expects strictly greater length.
        let codes = [0x04, 0x42]; // Count = 2 => expects four bytes of codes; boundary triggers error.
        let mut bytes = build_unwind_bytes(1, 0, 4, 2, 0, 0, &codes);
        // Trim one byte to force malformed data (also works).
        bytes.truncate(6); // Make it definitively malformed.
        let err = UnwindInfo::parse(&bytes, Some("boundary")).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }));
    }

    #[test]
    fn frame_register_offset_calculation() {
        for (unit, expected) in [(0, 0u32), (1, 16), (5, 80), (15, 240)] {
            // unit * 16
            let bytes = build_unwind_bytes(1, 0, 4, 0, 5, unit, &[]); // frame_reg = 5 (RBP).
            let ui = UnwindInfo::parse(&bytes, Some("offset")).unwrap();
            assert_eq!(ui.frame_register, 5);
            assert_eq!(ui.frame_register_offset, expected);
        }
    }

    #[test]
    fn display_includes_core_fields() {
        let codes = [0x04, 0x42];
        let bytes = build_unwind_bytes(1, 1, 8, 1, 5, 3, &codes); // flags = 1, frame_reg = 5, offset_units = 3 -> 48.
        let ui = UnwindInfo::parse(&bytes, Some("disp")).unwrap();
        let s = format!("{}", ui);
        assert!(s.contains("UnwindInfo"));
        assert!(s.contains("version: 0x01"));
        assert!(s.contains("flags: 0x01"));
        assert!(s.contains("frame_register: 0x05"));
        assert!(s.contains("frame_register_offset: 0x00000030")); // 48 decimal
    }

    #[test]
    fn stack_offset_push_nonvolatile() {
        let codes = [0x04, 0x00, 0x02, 0x00]; // Two pushes (opcode 0) -> 16 bytes.
        let bytes = build_unwind_bytes(1, 0, 6, 2, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("push")).unwrap();
        assert_eq!(ui.get_stack_pointer_offset().unwrap(), 16);
    }

    #[test]
    fn stack_offset_alloc_small() {
        let codes = [0x04, 0x22]; // Opcode 2, opinfo = 2 -> (2 * 8 + 8) = 24.
        let bytes = build_unwind_bytes(1, 0, 4, 1, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("small")).unwrap();
        assert_eq!(ui.get_stack_pointer_offset().unwrap(), 24);
    }

    #[test]
    fn stack_offset_alloc_large_scaled() {
        // AllocLarge opcode 1, opinfo = 0, next two bytes contain a count scaled by 8.
        let codes = [0x04, 0x01, 0x20, 0x00]; // Size field = 0x0020 -> 32 * 8 = 256.
        let bytes = build_unwind_bytes(1, 0, 6, 2, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("large_scaled")).unwrap();
        assert_eq!(ui.get_stack_pointer_offset().unwrap(), 256);
    }

    #[test]
    fn stack_offset_alloc_large_unscaled() {
        // AllocLarge opcode 1, opinfo = 1, next four bytes contain the raw size.
        let codes = [0x04, 0x11, 0x00, 0x01, 0x00, 0x00]; // Size = 0x0100 = 256.
        let bytes = build_unwind_bytes(1, 0, 6, 3, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("large_unscaled")).unwrap();
        assert_eq!(ui.get_stack_pointer_offset().unwrap(), 256);
    }

    #[test]
    fn stack_offset_mixed_sequence() {
        let codes_final = [
            0x10, 0x40, // Push.
            0x08, 0x40, // Push.
            0x04, 0x22, // alloc_small, opinfo = 2 -> 24.
            0x04, 0x01, // alloc_large scaled.
            0x10, 0x00, // Size value (0x0010 * 8 = 128).
        ];
        let bytes = build_unwind_bytes(1, 0, 10, 5, 0, 0, &codes_final);
        let ui = UnwindInfo::parse(&bytes, Some("mixed")).unwrap();
        assert_eq!(ui.get_stack_pointer_offset().unwrap(), 16 + 24 + 128);
    }

    #[test]
    fn stack_offset_zero_codes() {
        let bytes = build_unwind_bytes(1, 0, 0, 0, 0, 0, &[]);
        let ui = UnwindInfo::parse(&bytes, Some("empty")).unwrap();
        assert_eq!(ui.get_stack_pointer_offset().unwrap(), 0);
    }

    #[test]
    fn stack_offset_error_from_unexpected_opinfo() {
        // Opcode 1 (AllocLarge) with opinfo = 2 is invalid; the offset parser returns
        // Error::Malformed annotated with the module.
        let codes = [0x04, 0x21];
        let bytes = build_unwind_bytes(1, 0, 4, 1, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("err")).unwrap();
        let err = ui.get_stack_pointer_offset().unwrap_err();
        assert!(matches!(err, Error::Malformed { module: Some("err"), reason: "Unexpected opinfo" }));
    }

    #[test]
    fn previous_stack_frame_advances_stack_pointer_and_reads_return_address() {
        let codes = [0x04, 0x00, 0x02, 0x00];
        let bytes = build_unwind_bytes(1, 0, 6, 2, 0, 0, &codes);
        let ui = UnwindInfo::parse(&bytes, Some("frame")).expect("parsing unwind info should succeed for valid bytes");

        let offset =
            ui.get_stack_pointer_offset().expect("stack pointer offset should be computed for valid unwind codes");
        assert_eq!(offset, 16);

        let mut stack_words = Box::new([0u64; 4]);
        let return_address = 0x1122_3344_5566_7788u64;
        stack_words[2] = return_address;
        let stack_base = stack_words.as_ptr() as u64;

        let current_frame = StackFrame { sp: stack_base, pc: 0xAA_BBCC_DDEE_F012, fp: 0x1234_5678_9ABC_DEF0 };
        let previous = ui
            .get_previous_stack_frame(&current_frame)
            .expect("computing previous stack frame should succeed for well-formed stack");

        assert_eq!(previous.pc, return_address);
        let expected_sp = stack_base + offset as u64 + core::mem::size_of::<u64>() as u64;
        assert_eq!(previous.sp, expected_sp);
        assert_eq!(previous.fp, current_frame.fp);

        drop(stack_words);
    }
}
