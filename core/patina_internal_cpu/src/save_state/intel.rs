//! Intel x64 SMRAM Save State Map
//!
//! Register-to-offset lookup table for the Intel 64-bit SMRAM save state
//! layout (`SMRAM_SAVE_STATE_MAP64`).  The save state area starts at
//! `SMBASE + 0x7C00` (the address stored in `CpuSaveState[CpuIndex]`).
//!
//! Reference: Intel SDM Vol 3C, Table 31-3; MdePkg `SmramSaveStateMap.h`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{
    IO_TYPE_INPUT, IO_TYPE_OUTPUT, IO_WIDTH_UINT8, IO_WIDTH_UINT16, IO_WIDTH_UINT32, MmSaveStateRegister, ParsedIoInfo,
    RegisterInfo, VendorConstants,
};

/// IOMisc Type field value: OUT instruction (port in DX).
const IO_MISC_TYPE_OUT: u32 = 0;
/// IOMisc Type field value: IN instruction (port in DX).
const IO_MISC_TYPE_IN: u32 = 1;
/// IOMisc Type field value: OUT instruction (immediate port operand).
const IO_MISC_TYPE_OUT_IMMEDIATE: u32 = 8;
/// IOMisc Type field value: IN instruction (immediate port operand).
const IO_MISC_TYPE_IN_IMMEDIATE: u32 = 9;

/// Intel-specific offsets and behaviour constants.
pub static VENDOR_CONSTANTS: VendorConstants = VendorConstants {
    smmrevid_offset: 0x02FC,
    io_info_offset: 0x03A4,
    efer_offset: 0x03E0,
    rax_offset: 0x035C,
    min_rev_id_io: 0x30004,
    lma_always_64: false,
};

/// Looks up the Intel x64 save state register info for a PI register.
///
/// Returns `None` for pseudo-registers (IO, LMA, ProcessorId) — those are
/// handled separately — and for unsupported registers (limits, LdtInfo).
pub fn register_info(reg: MmSaveStateRegister) -> Option<RegisterInfo> {
    match reg {
        // Descriptor table bases (split hi/lo u32 — non-contiguous on Intel)
        MmSaveStateRegister::GdtBase => Some(RegisterInfo { lo_offset: 0x028C, hi_offset: 0x01D0, native_width: 8 }),
        MmSaveStateRegister::IdtBase => Some(RegisterInfo { lo_offset: 0x0294, hi_offset: 0x01D8, native_width: 8 }),
        MmSaveStateRegister::LdtBase => Some(RegisterInfo { lo_offset: 0x029C, hi_offset: 0x01D4, native_width: 8 }),

        // Limits / LdtInfo — not supported in Intel 64-bit save state map.
        MmSaveStateRegister::GdtLimit
        | MmSaveStateRegister::IdtLimit
        | MmSaveStateRegister::LdtLimit
        | MmSaveStateRegister::LdtInfo => None,

        // Segment selectors (4-byte fields on Intel)
        MmSaveStateRegister::Es => Some(RegisterInfo { lo_offset: 0x03A8, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::Cs => Some(RegisterInfo { lo_offset: 0x03AC, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::Ss => Some(RegisterInfo { lo_offset: 0x03B0, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::Ds => Some(RegisterInfo { lo_offset: 0x03B4, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::Fs => Some(RegisterInfo { lo_offset: 0x03B8, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::Gs => Some(RegisterInfo { lo_offset: 0x03BC, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::LdtrSel => Some(RegisterInfo { lo_offset: 0x03C0, hi_offset: 0, native_width: 4 }),
        MmSaveStateRegister::TrSel => Some(RegisterInfo { lo_offset: 0x03C4, hi_offset: 0, native_width: 4 }),

        // Debug registers (8-byte, contiguous)
        MmSaveStateRegister::Dr7 => Some(RegisterInfo { lo_offset: 0x03C8, hi_offset: 0x03CC, native_width: 8 }),
        MmSaveStateRegister::Dr6 => Some(RegisterInfo { lo_offset: 0x03D0, hi_offset: 0x03D4, native_width: 8 }),

        // Extended registers R8–R15 (8-byte, contiguous, descending addresses)
        MmSaveStateRegister::R8 => Some(RegisterInfo { lo_offset: 0x0354, hi_offset: 0x0358, native_width: 8 }),
        MmSaveStateRegister::R9 => Some(RegisterInfo { lo_offset: 0x034C, hi_offset: 0x0350, native_width: 8 }),
        MmSaveStateRegister::R10 => Some(RegisterInfo { lo_offset: 0x0344, hi_offset: 0x0348, native_width: 8 }),
        MmSaveStateRegister::R11 => Some(RegisterInfo { lo_offset: 0x033C, hi_offset: 0x0340, native_width: 8 }),
        MmSaveStateRegister::R12 => Some(RegisterInfo { lo_offset: 0x0334, hi_offset: 0x0338, native_width: 8 }),
        MmSaveStateRegister::R13 => Some(RegisterInfo { lo_offset: 0x032C, hi_offset: 0x0330, native_width: 8 }),
        MmSaveStateRegister::R14 => Some(RegisterInfo { lo_offset: 0x0324, hi_offset: 0x0328, native_width: 8 }),
        MmSaveStateRegister::R15 => Some(RegisterInfo { lo_offset: 0x031C, hi_offset: 0x0320, native_width: 8 }),

        // General-purpose registers (8-byte, contiguous)
        MmSaveStateRegister::Rax => Some(RegisterInfo { lo_offset: 0x035C, hi_offset: 0x0360, native_width: 8 }),
        MmSaveStateRegister::Rbx => Some(RegisterInfo { lo_offset: 0x0374, hi_offset: 0x0378, native_width: 8 }),
        MmSaveStateRegister::Rcx => Some(RegisterInfo { lo_offset: 0x0364, hi_offset: 0x0368, native_width: 8 }),
        MmSaveStateRegister::Rdx => Some(RegisterInfo { lo_offset: 0x036C, hi_offset: 0x0370, native_width: 8 }),
        MmSaveStateRegister::Rsp => Some(RegisterInfo { lo_offset: 0x037C, hi_offset: 0x0380, native_width: 8 }),
        MmSaveStateRegister::Rbp => Some(RegisterInfo { lo_offset: 0x0384, hi_offset: 0x0388, native_width: 8 }),
        MmSaveStateRegister::Rsi => Some(RegisterInfo { lo_offset: 0x038C, hi_offset: 0x0390, native_width: 8 }),
        MmSaveStateRegister::Rdi => Some(RegisterInfo { lo_offset: 0x0394, hi_offset: 0x0398, native_width: 8 }),
        MmSaveStateRegister::Rip => Some(RegisterInfo { lo_offset: 0x03D8, hi_offset: 0x03DC, native_width: 8 }),

        // Flags and control registers
        MmSaveStateRegister::Rflags => Some(RegisterInfo { lo_offset: 0x03E8, hi_offset: 0x03EC, native_width: 8 }),
        MmSaveStateRegister::Cr0 => Some(RegisterInfo { lo_offset: 0x03F8, hi_offset: 0x03FC, native_width: 8 }),
        MmSaveStateRegister::Cr3 => Some(RegisterInfo { lo_offset: 0x03F0, hi_offset: 0x03F4, native_width: 8 }),
        // CR4 is only 4 bytes in the Intel x64 save state map.
        MmSaveStateRegister::Cr4 => Some(RegisterInfo { lo_offset: 0x0240, hi_offset: 0, native_width: 4 }),

        // Pseudo-registers are not in the architectural register map.
        MmSaveStateRegister::Io | MmSaveStateRegister::Lma | MmSaveStateRegister::ProcessorId => None,
    }
}

/// Parses Intel's `IOMisc` field from the SMRAM save state.
///
/// Intel IOMisc bit layout:
/// - Bit 0:      `SmiFlag` — 1 if the SMI was caused by an I/O instruction.
/// - Bits \[3:1\]:  `Length` — I/O width in bytes (1, 2, or 4).
/// - Bits \[7:4\]:  `Type` — 0/8 = OUT (DX/immediate), 1/9 = IN (DX/immediate).
/// - Bits \[31:16\]: `Port` — I/O port address.
///
/// Returns `None` if `SmiFlag` is 0 (SMI was not caused by I/O) or the I/O
/// type is not a simple IN or OUT (e.g. string / REP I/O).
pub fn parse_io_field(io_field: u32) -> Option<ParsedIoInfo> {
    log::trace!("Parsing IOMisc field: 0x{:08x}", io_field);

    // Check SmiFlag.
    let smi_flag = io_field & 1;
    if smi_flag == 0 {
        return None;
    }

    let length = (io_field >> 1) & 0x7;
    let io_type_raw = (io_field >> 4) & 0xF;
    let port = ((io_field >> 16) & 0xFFFF) as u16;

    // Simple IN/OUT in both DX and immediate-port forms are supported.
    let io_type = match io_type_raw {
        IO_MISC_TYPE_OUT | IO_MISC_TYPE_OUT_IMMEDIATE => IO_TYPE_OUTPUT,
        IO_MISC_TYPE_IN | IO_MISC_TYPE_IN_IMMEDIATE => IO_TYPE_INPUT,
        _ => return None,
    };

    // Map length to IO width enum and byte count.
    let (io_width, byte_count) = match length {
        1 => (IO_WIDTH_UINT8, 1usize),
        2 => (IO_WIDTH_UINT16, 2usize),
        4 => (IO_WIDTH_UINT32, 4usize),
        _ => return None,
    };

    log::trace!(
        "Parsed IO field: io_type = {}, io_width = {}, byte_count = {}, io_port = 0x{:x}",
        io_type,
        io_width,
        byte_count,
        port
    );
    Some(ParsedIoInfo { io_type, io_width, byte_count, io_port: port })
}

/// Returns whether the Intel SMRAM save state map exposes I/O trap information
/// for a save state with the given raw `SMMRevId`.
/// Must be above 0x30004
///
/// older revisions do not carry I/O trap information.
pub fn io_info_supported(smm_rev_id: u32) -> bool {
    smm_rev_id >= VENDOR_CONSTANTS.min_rev_id_io
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpr_offsets() {
        let rax = register_info(MmSaveStateRegister::Rax).unwrap();
        assert_eq!(rax.lo_offset, 0x035C);
        assert_eq!(rax.hi_offset, 0x0360);
        assert_eq!(rax.native_width, 8);

        let r15 = register_info(MmSaveStateRegister::R15).unwrap();
        assert_eq!(r15.lo_offset, 0x031C);
        assert_eq!(r15.hi_offset, 0x0320);
        assert_eq!(r15.native_width, 8);
    }

    #[test]
    fn test_segment_selectors() {
        let es = register_info(MmSaveStateRegister::Es).unwrap();
        assert_eq!(es.lo_offset, 0x03A8);
        assert_eq!(es.hi_offset, 0);
        assert_eq!(es.native_width, 4);
    }

    #[test]
    fn test_descriptor_table_bases_are_split() {
        let gdt = register_info(MmSaveStateRegister::GdtBase).unwrap();
        assert_eq!(gdt.lo_offset, 0x028C);
        assert_eq!(gdt.hi_offset, 0x01D0);
        assert_eq!(gdt.native_width, 8);
        // Verify they are non-contiguous on Intel.
        assert_ne!(gdt.hi_offset, gdt.lo_offset + 4);
    }

    #[test]
    fn test_limits_unsupported_on_intel() {
        assert!(register_info(MmSaveStateRegister::GdtLimit).is_none());
        assert!(register_info(MmSaveStateRegister::IdtLimit).is_none());
        assert!(register_info(MmSaveStateRegister::LdtLimit).is_none());
        assert!(register_info(MmSaveStateRegister::LdtInfo).is_none());
    }

    #[test]
    fn test_cr4_is_4_bytes_on_intel() {
        let cr4 = register_info(MmSaveStateRegister::Cr4).unwrap();
        assert_eq!(cr4.native_width, 4);
        assert_eq!(cr4.hi_offset, 0);
    }

    #[test]
    fn test_pseudo_registers_return_none() {
        assert!(register_info(MmSaveStateRegister::Io).is_none());
        assert!(register_info(MmSaveStateRegister::Lma).is_none());
        assert!(register_info(MmSaveStateRegister::ProcessorId).is_none());
    }

    #[test]
    fn test_parse_io_field_in() {
        // SmiFlag=1, Length=1 (byte), Type=1 (IN), Port=0x80
        // Bits: Port(31:16)=0x0080, Type(7:4)=1, Length(3:1)=1, SmiFlag(0)=1
        let io_field: u32 = (0x0080 << 16) | (1 << 4) | (1 << 1) | 1;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_INPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT8);
        assert_eq!(parsed.byte_count, 1);
        assert_eq!(parsed.io_port, 0x80);
    }

    #[test]
    fn test_parse_io_field_out() {
        // SmiFlag=1, Length=4 (dword), Type=0 (OUT), Port=0xCF8
        let io_field: u32 = (0x0CF8 << 16) | (4 << 1) | 1;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_OUTPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT32);
        assert_eq!(parsed.byte_count, 4);
        assert_eq!(parsed.io_port, 0x0CF8);
    }

    #[test]
    fn test_parse_io_field_out_immediate() {
        // SmiFlag=1, Length=1 (byte), Type=8 (OUT_IMMEDIATE), Port=0xB2
        let io_field: u32 = (0x00B2 << 16) | (8 << 4) | (1 << 1) | 1;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_OUTPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT8);
        assert_eq!(parsed.byte_count, 1);
        assert_eq!(parsed.io_port, 0xB2);
    }

    #[test]
    fn test_parse_io_field_in_immediate() {
        // SmiFlag=1, Length=1 (byte), Type=9 (IN_IMMEDIATE), Port=0x80
        let io_field: u32 = (0x0080 << 16) | (9 << 4) | (1 << 1) | 1;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_INPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT8);
        assert_eq!(parsed.byte_count, 1);
        assert_eq!(parsed.io_port, 0x80);
    }

    #[test]
    fn test_parse_io_field_no_smi_flag() {
        // SmiFlag=0 → should return None
        let io_field: u32 = (0x0080 << 16) | (1 << 4) | (1 << 1);
        assert!(parse_io_field(io_field).is_none());
    }

    #[test]
    fn test_parse_io_field_string_io() {
        // SmiFlag=1, Length=1, Type=4 (string, not IN/OUT) → None
        let io_field: u32 = (0x0080 << 16) | (4 << 4) | (1 << 1) | 1;
        assert!(parse_io_field(io_field).is_none());
    }

    #[test]
    fn test_io_info_supported_gated_on_rev_id() {
        // IOMisc is only present once SMMRevId >= min_rev_id_io.
        assert!(!io_info_supported(VENDOR_CONSTANTS.min_rev_id_io - 1));
        assert!(io_info_supported(VENDOR_CONSTANTS.min_rev_id_io));
        assert!(io_info_supported(VENDOR_CONSTANTS.min_rev_id_io + 1));
    }

    #[test]
    fn test_register_coverage() {
        let architectural_regs = [
            MmSaveStateRegister::GdtBase,
            MmSaveStateRegister::IdtBase,
            MmSaveStateRegister::LdtBase,
            MmSaveStateRegister::Es,
            MmSaveStateRegister::Cs,
            MmSaveStateRegister::Ss,
            MmSaveStateRegister::Ds,
            MmSaveStateRegister::Fs,
            MmSaveStateRegister::Gs,
            MmSaveStateRegister::LdtrSel,
            MmSaveStateRegister::TrSel,
            MmSaveStateRegister::Dr7,
            MmSaveStateRegister::Dr6,
            MmSaveStateRegister::R8,
            MmSaveStateRegister::R9,
            MmSaveStateRegister::R10,
            MmSaveStateRegister::R11,
            MmSaveStateRegister::R12,
            MmSaveStateRegister::R13,
            MmSaveStateRegister::R14,
            MmSaveStateRegister::R15,
            MmSaveStateRegister::Rax,
            MmSaveStateRegister::Rbx,
            MmSaveStateRegister::Rcx,
            MmSaveStateRegister::Rdx,
            MmSaveStateRegister::Rsp,
            MmSaveStateRegister::Rbp,
            MmSaveStateRegister::Rsi,
            MmSaveStateRegister::Rdi,
            MmSaveStateRegister::Rip,
            MmSaveStateRegister::Rflags,
            MmSaveStateRegister::Cr0,
            MmSaveStateRegister::Cr3,
            MmSaveStateRegister::Cr4,
        ];

        for reg in &architectural_regs {
            assert!(register_info(*reg).is_some(), "Missing Intel lookup for {:?}", reg);
        }
    }
}
