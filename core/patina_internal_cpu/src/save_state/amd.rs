//! AMD64 SMRAM Save State Map
//!
//! Register-to-offset lookup table for the AMD 64-bit SMRAM save state
//! layout (`AMD_SMRAM_SAVE_STATE_MAP64`).  The save state area starts at
//! `SMBASE + 0xFC00` (the address stored in `CpuSaveState[CpuIndex]`).
//! All offsets are relative to that base.
//!
//! Reference: AMD64 Architecture Programmer's Manual Vol 2, Table 10-2;
//! `MdePkg` `AmdSmramSaveStateMap.h`.
//!
//! ## Key Differences from Intel
//!
//! - Segment selectors are 2 bytes (UINT16) vs Intel's 4-byte fields.
//! - GDT, IDT, and LDT limits **are** supported (Intel returns `None`).
//! - CR4 is 8 bytes (Intel stores only 4 bytes).
//! - All 8-byte registers are stored contiguously (Intel splits DT bases).
//! - IO information uses `IO_DWord` at offset 0x2C0 with a different bit
//!   layout from Intel's `IOMisc`.
//! - AMD64 always operates in 64-bit mode during SMM, so the LMA
//!   pseudo-register always returns 64-bit without checking EFER.LMA.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::bit;

use super::{
    IO_TYPE_INPUT, IO_TYPE_OUTPUT, IO_WIDTH_UINT8, IO_WIDTH_UINT16, IO_WIDTH_UINT32, MmSaveStateRegister, ParsedIoInfo,
    RegisterInfo, VendorConstants,
};

/// `IO_DWord` bit 0: direction (0 = WRITE/OUT, 1 = READ/IN).
const IO_DIRECTION_IN: u32 = bit!(0);

/// `IO_DWord` bit 1: set when the field holds a valid I/O trap record.
const IO_TRAP_VALID: u32 = bit!(1);

/// `IO_DWord` bit 4: SZ8 - 8-bit (byte) access.
const IO_SIZE_BYTE: u32 = bit!(4);
/// `IO_DWord` bit 5: SZ16 - 16-bit (word) access.
const IO_SIZE_WORD: u32 = bit!(5);
// IO_DWord bit 6 (SZ32, 32-bit) is the default when neither SZ8 nor SZ16 is
// set, so it does not need a named constant in the decode path.

/// Name of this save state layout, for logs.
pub(super) const VENDOR_NAME: &str = "AMD";

/// AMD-specific offsets and behaviour constants.
pub(super) const VENDOR_CONSTANTS: VendorConstants = VendorConstants {
    smmrevid_offset: 0x02FC,
    io_info_offset: 0x02C0,
    efer_offset: 0x02D0,
    rax_offset: 0x03F8,
    min_rev_id_io: 0x30064,
    lma_always_64: true,
};

// ---------------------------------------------------------------------------
// `AMD_SMRAM_SAVE_STATE_MAP64` field offsets, relative to `SMBASE + 0xFC00`.
//
// `_LO` / `_HI` name the two halves of a 64-bit field. AMD stores both halves
// contiguously, so `_HI` is always `_LO + 4`.
// ---------------------------------------------------------------------------

// System descriptor bases.
const GDTR_BASE_LO: u16 = 0x0268;
const GDTR_BASE_HI: u16 = 0x026C;
const IDTR_BASE_LO: u16 = 0x0278;
const IDTR_BASE_HI: u16 = 0x027C;
const LDTR_BASE_LO: u16 = 0x0288;
const LDTR_BASE_HI: u16 = 0x028C;

// System descriptor limits. GDTR/IDTR limits are architecturally 16-bit but
// occupy UINT32 fields; the LDTR limit is a full system-segment limit.
const GDTR_LIMIT: u16 = 0x0264;
const IDTR_LIMIT: u16 = 0x0274;
const LDTR_LIMIT: u16 = 0x0284;

// Segment selectors: the 2-byte `Selector` at the start of each 16-byte
// segment descriptor.
const ES_SELECTOR: u16 = 0x0200;
const CS_SELECTOR: u16 = 0x0210;
const SS_SELECTOR: u16 = 0x0220;
const DS_SELECTOR: u16 = 0x0230;
const FS_SELECTOR: u16 = 0x0240;
const GS_SELECTOR: u16 = 0x0250;
const LDTR_SELECTOR: u16 = 0x0280;
const TR_SELECTOR: u16 = 0x0290;

// Debug registers.
const DR7_LO: u16 = 0x0338;
const DR7_HI: u16 = 0x033C;
const DR6_LO: u16 = 0x0340;
const DR6_HI: u16 = 0x0344;

// Extended general-purpose registers (stored at descending addresses).
const R8_LO: u16 = 0x03B8;
const R8_HI: u16 = 0x03BC;
const R9_LO: u16 = 0x03B0;
const R9_HI: u16 = 0x03B4;
const R10_LO: u16 = 0x03A8;
const R10_HI: u16 = 0x03AC;
const R11_LO: u16 = 0x03A0;
const R11_HI: u16 = 0x03A4;
const R12_LO: u16 = 0x0398;
const R12_HI: u16 = 0x039C;
const R13_LO: u16 = 0x0390;
const R13_HI: u16 = 0x0394;
const R14_LO: u16 = 0x0388;
const R14_HI: u16 = 0x038C;
const R15_LO: u16 = 0x0380;
const R15_HI: u16 = 0x0384;

// General-purpose registers.
const RAX_LO: u16 = 0x03F8;
const RAX_HI: u16 = 0x03FC;
const RBX_LO: u16 = 0x03D0;
const RBX_HI: u16 = 0x03D4;
const RCX_LO: u16 = 0x03F0;
const RCX_HI: u16 = 0x03F4;
const RDX_LO: u16 = 0x03E8;
const RDX_HI: u16 = 0x03EC;
const RSP_LO: u16 = 0x03C8;
const RSP_HI: u16 = 0x03CC;
const RBP_LO: u16 = 0x03C0;
const RBP_HI: u16 = 0x03C4;
const RSI_LO: u16 = 0x03E0;
const RSI_HI: u16 = 0x03E4;
const RDI_LO: u16 = 0x03D8;
const RDI_HI: u16 = 0x03DC;
const RIP_LO: u16 = 0x0378;
const RIP_HI: u16 = 0x037C;

// Flags and control registers. CR4 is 8 bytes on AMD (4 bytes on Intel).
const RFLAGS_LO: u16 = 0x0370;
const RFLAGS_HI: u16 = 0x0374;
const CR0_LO: u16 = 0x0358;
const CR0_HI: u16 = 0x035C;
const CR3_LO: u16 = 0x0350;
const CR3_HI: u16 = 0x0354;
const CR4_LO: u16 = 0x0348;
const CR4_HI: u16 = 0x034C;

///
/// AMD save state struct layout (from SMBASE + 0xFC00):
///
///   +0x000 - 0x1FF: Reserved (padding).
///   +0x200 - 0x25F: Segment descriptors (ES, CS, SS, DS, FS, GS) - 16 bytes each.
///   +0x260 - 0x29F: System descriptors (GDTR, IDTR, LDTR, TR) - 16 bytes each.
///   +0x2A0 - 0x2BF: MSRs (`KernelGsBase`, STAR, LSTAR, CSTAR).
///   +0x2C0:         `IO_DWord` (4 bytes).
///   +0x2D0:         EFER (8 bytes).
///   +0x2FC:         `SMMRevId` (4 bytes).
///   +0x338 - 0x3FF: Registers (DR7, DR6, CR4, CR3, CR0, RFLAGS, RIP, R15..R8,
///                               RBP, RSP, RBX, RDI, RSI, RDX, RCX, RAX).
///
/// Each segment descriptor is 16 bytes:
///   +0: Selector (UINT16)
///   +2: Attributes (UINT16)
///   +4: Limit (UINT32)
///   +8: `BaseLoDword` (UINT32)
///   +C: `BaseHiDword` (UINT32)
///
/// Looks up the AMD64 save state register info for a PI register.
///
/// Returns `None` for pseudo-registers (IO, LMA, `ProcessorId`) and for
/// `LdtInfo` (not supported in AMD's save state map).
pub(super) fn register_info(reg: MmSaveStateRegister) -> Option<RegisterInfo> {
    match reg {
        MmSaveStateRegister::GdtBase => {
            Some(RegisterInfo { lo_offset: GDTR_BASE_LO, hi_offset: GDTR_BASE_HI, native_width: 8 })
        }
        MmSaveStateRegister::IdtBase => {
            Some(RegisterInfo { lo_offset: IDTR_BASE_LO, hi_offset: IDTR_BASE_HI, native_width: 8 })
        }
        // NOTE: The C reference code has a copy-paste bug where both lo and
        // hi point to `_LDTRBaseLoDword` (0x288).  The correct hi offset is
        // `_LDTRBaseHiDword` (0x28C).
        MmSaveStateRegister::LdtBase => {
            Some(RegisterInfo { lo_offset: LDTR_BASE_LO, hi_offset: LDTR_BASE_HI, native_width: 8 })
        }

        // GDT and IDT limits are architecturally 16-bit, stored in UINT32
        // fields.  Only the lower 2 bytes are meaningful.
        MmSaveStateRegister::GdtLimit => Some(RegisterInfo { lo_offset: GDTR_LIMIT, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::IdtLimit => Some(RegisterInfo { lo_offset: IDTR_LIMIT, hi_offset: 0, native_width: 2 }),
        // LDT limit is a system-segment limit (up to 32 bits in long mode).
        MmSaveStateRegister::LdtLimit => Some(RegisterInfo { lo_offset: LDTR_LIMIT, hi_offset: 0, native_width: 4 }),

        // LdtInfo is not supported, and pseudo-registers are not in the architectural register map.
        MmSaveStateRegister::LdtInfo
        | MmSaveStateRegister::Io
        | MmSaveStateRegister::Lma
        | MmSaveStateRegister::ProcessorId => None,

        MmSaveStateRegister::Es => Some(RegisterInfo { lo_offset: ES_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::Cs => Some(RegisterInfo { lo_offset: CS_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::Ss => Some(RegisterInfo { lo_offset: SS_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::Ds => Some(RegisterInfo { lo_offset: DS_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::Fs => Some(RegisterInfo { lo_offset: FS_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::Gs => Some(RegisterInfo { lo_offset: GS_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::LdtrSel => Some(RegisterInfo { lo_offset: LDTR_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::TrSel => Some(RegisterInfo { lo_offset: TR_SELECTOR, hi_offset: 0, native_width: 2 }),
        MmSaveStateRegister::Dr7 => Some(RegisterInfo { lo_offset: DR7_LO, hi_offset: DR7_HI, native_width: 8 }),
        MmSaveStateRegister::Dr6 => Some(RegisterInfo { lo_offset: DR6_LO, hi_offset: DR6_HI, native_width: 8 }),
        MmSaveStateRegister::R8 => Some(RegisterInfo { lo_offset: R8_LO, hi_offset: R8_HI, native_width: 8 }),
        MmSaveStateRegister::R9 => Some(RegisterInfo { lo_offset: R9_LO, hi_offset: R9_HI, native_width: 8 }),
        MmSaveStateRegister::R10 => Some(RegisterInfo { lo_offset: R10_LO, hi_offset: R10_HI, native_width: 8 }),
        MmSaveStateRegister::R11 => Some(RegisterInfo { lo_offset: R11_LO, hi_offset: R11_HI, native_width: 8 }),
        MmSaveStateRegister::R12 => Some(RegisterInfo { lo_offset: R12_LO, hi_offset: R12_HI, native_width: 8 }),
        MmSaveStateRegister::R13 => Some(RegisterInfo { lo_offset: R13_LO, hi_offset: R13_HI, native_width: 8 }),
        MmSaveStateRegister::R14 => Some(RegisterInfo { lo_offset: R14_LO, hi_offset: R14_HI, native_width: 8 }),
        MmSaveStateRegister::R15 => Some(RegisterInfo { lo_offset: R15_LO, hi_offset: R15_HI, native_width: 8 }),
        MmSaveStateRegister::Rax => Some(RegisterInfo { lo_offset: RAX_LO, hi_offset: RAX_HI, native_width: 8 }),
        MmSaveStateRegister::Rbx => Some(RegisterInfo { lo_offset: RBX_LO, hi_offset: RBX_HI, native_width: 8 }),
        MmSaveStateRegister::Rcx => Some(RegisterInfo { lo_offset: RCX_LO, hi_offset: RCX_HI, native_width: 8 }),
        MmSaveStateRegister::Rdx => Some(RegisterInfo { lo_offset: RDX_LO, hi_offset: RDX_HI, native_width: 8 }),
        MmSaveStateRegister::Rsp => Some(RegisterInfo { lo_offset: RSP_LO, hi_offset: RSP_HI, native_width: 8 }),
        MmSaveStateRegister::Rbp => Some(RegisterInfo { lo_offset: RBP_LO, hi_offset: RBP_HI, native_width: 8 }),
        MmSaveStateRegister::Rsi => Some(RegisterInfo { lo_offset: RSI_LO, hi_offset: RSI_HI, native_width: 8 }),
        MmSaveStateRegister::Rdi => Some(RegisterInfo { lo_offset: RDI_LO, hi_offset: RDI_HI, native_width: 8 }),
        MmSaveStateRegister::Rip => Some(RegisterInfo { lo_offset: RIP_LO, hi_offset: RIP_HI, native_width: 8 }),
        // Flags and Control Registers
        MmSaveStateRegister::Rflags => {
            Some(RegisterInfo { lo_offset: RFLAGS_LO, hi_offset: RFLAGS_HI, native_width: 8 })
        }
        MmSaveStateRegister::Cr0 => Some(RegisterInfo { lo_offset: CR0_LO, hi_offset: CR0_HI, native_width: 8 }),
        MmSaveStateRegister::Cr3 => Some(RegisterInfo { lo_offset: CR3_LO, hi_offset: CR3_HI, native_width: 8 }),
        // CR4 is 8 bytes on AMD (vs 4 bytes on Intel).
        MmSaveStateRegister::Cr4 => Some(RegisterInfo { lo_offset: CR4_LO, hi_offset: CR4_HI, native_width: 8 }),
    }
}

/// Parses AMD's `IO_DWord` field from the SMRAM save state.
///
/// AMD `IO_DWord` bit layout:
/// - Bit 0:      Direction - 0 = WRITE (OUT), 1 = READ (IN).
/// - Bit 1:      Valid - set when the field holds a valid I/O trap record.
/// - Bits \[3:2\]:  Reserved.
/// - Bit 4:      SZ8 - 8-bit (byte) access.
/// - Bit 5:      SZ16 - 16-bit (word) access.
/// - Bit 6:      SZ32 - 32-bit (dword) access.
/// - Bits \[15:7\]: Reserved.
/// - Bits \[31:16\]: I/O port address.
///
/// Returns `None` if the data-size encoding is invalid (value 2 is reserved).
pub(super) fn parse_io_field(io_field: u32) -> Option<ParsedIoInfo> {
    if io_field & IO_TRAP_VALID == 0 {
        return None;
    }

    let io_type = if io_field & IO_DIRECTION_IN != 0 { IO_TYPE_INPUT } else { IO_TYPE_OUTPUT };
    let port = ((io_field >> 16) & 0xFFFF) as u16;

    let (io_width, byte_count) = if io_field & IO_SIZE_BYTE != 0 {
        (IO_WIDTH_UINT8, 1usize)
    } else if io_field & IO_SIZE_WORD != 0 {
        (IO_WIDTH_UINT16, 2usize)
    } else {
        (IO_WIDTH_UINT32, 4usize)
    };

    Some(ParsedIoInfo { io_type, io_width, byte_count, io_port: port })
}

/// Returns whether the AMD SMRAM save state map exposes I/O trap information
/// for a save state with the given raw `SMMRevId`.
/// Always return true because this is not really used.
///
pub(super) fn io_info_supported(_smm_rev_id: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vendor_name() {
        assert_eq!(VENDOR_NAME, "AMD");
    }

    // ----------------------------------------------------------------
    // Register map tests
    // ----------------------------------------------------------------

    #[test]
    fn test_gpr_offsets() {
        let rax = register_info(MmSaveStateRegister::Rax).unwrap();
        assert_eq!(rax.lo_offset, 0x03F8);
        assert_eq!(rax.hi_offset, 0x03FC);
        assert_eq!(rax.native_width, 8);

        let rcx = register_info(MmSaveStateRegister::Rcx).unwrap();
        assert_eq!(rcx.lo_offset, 0x03F0);
        assert_eq!(rcx.hi_offset, 0x03F4);
    }

    #[test]
    fn test_segment_selectors_are_2_bytes() {
        let cs = register_info(MmSaveStateRegister::Cs).unwrap();
        assert_eq!(cs.lo_offset, 0x0210);
        assert_eq!(cs.native_width, 2);
        assert_eq!(cs.hi_offset, 0);
    }

    #[test]
    fn test_descriptor_table_bases_contiguous() {
        let gdt = register_info(MmSaveStateRegister::GdtBase).unwrap();
        assert_eq!(gdt.lo_offset, 0x0268);
        assert_eq!(gdt.hi_offset, 0x026C);
        assert_eq!(gdt.native_width, 8);
        // AMD uses contiguous lo/hi (unlike Intel's split layout).
        assert_eq!(gdt.hi_offset, gdt.lo_offset + 4);
    }

    #[test]
    fn test_limits_supported_on_amd() {
        let gdt_limit = register_info(MmSaveStateRegister::GdtLimit).unwrap();
        assert_eq!(gdt_limit.native_width, 2);
        assert_eq!(gdt_limit.lo_offset, 0x0264);

        let idt_limit = register_info(MmSaveStateRegister::IdtLimit).unwrap();
        assert_eq!(idt_limit.native_width, 2);
        assert_eq!(idt_limit.lo_offset, 0x0274);

        let ldt_limit = register_info(MmSaveStateRegister::LdtLimit).unwrap();
        assert_eq!(ldt_limit.native_width, 4);
        assert_eq!(ldt_limit.lo_offset, 0x0284);
    }

    #[test]
    fn test_cr4_is_8_bytes_on_amd() {
        let cr4 = register_info(MmSaveStateRegister::Cr4).unwrap();
        assert_eq!(cr4.native_width, 8);
        assert_eq!(cr4.lo_offset, 0x0348);
        assert_eq!(cr4.hi_offset, 0x034C);
    }

    #[test]
    fn test_ldt_info_unsupported() {
        assert!(register_info(MmSaveStateRegister::LdtInfo).is_none());
    }

    #[test]
    fn test_pseudo_registers_return_none() {
        assert!(register_info(MmSaveStateRegister::Io).is_none());
        assert!(register_info(MmSaveStateRegister::Lma).is_none());
        assert!(register_info(MmSaveStateRegister::ProcessorId).is_none());
    }

    #[test]
    fn test_register_coverage() {
        // All architectural registers except LdtInfo should be supported.
        let supported_regs = [
            MmSaveStateRegister::GdtBase,
            MmSaveStateRegister::IdtBase,
            MmSaveStateRegister::LdtBase,
            MmSaveStateRegister::GdtLimit,
            MmSaveStateRegister::IdtLimit,
            MmSaveStateRegister::LdtLimit,
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

        for reg in &supported_regs {
            assert!(register_info(*reg).is_some(), "Missing AMD lookup for {reg:?}");
        }
    }

    // ----------------------------------------------------------------
    // IO_DWord parsing tests
    // ----------------------------------------------------------------

    #[test]
    fn test_parse_io_field_in_byte() {
        // Direction=1 (IN), SZ8 (bit 4), Port=0x80
        let io_field: u32 = (0x0080 << 16) | IO_SIZE_BYTE | IO_DIRECTION_IN | IO_TRAP_VALID;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_INPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT8);
        assert_eq!(parsed.byte_count, 1);
        assert_eq!(parsed.io_port, 0x80);
    }

    #[test]
    fn test_parse_io_field_out_dword() {
        // Direction=0 (OUT), SZ32 (bit 6), Port=0xCF8
        let io_field: u32 = (0x0CF8 << 16) | (1 << 6) | IO_TRAP_VALID;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_OUTPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT32);
        assert_eq!(parsed.byte_count, 4);
        assert_eq!(parsed.io_port, 0x0CF8);
    }

    #[test]
    fn test_parse_io_field_in_word() {
        // Direction=1 (IN), SZ16 (bit 5), Port=0x3F8
        let io_field: u32 = (0x03F8 << 16) | IO_SIZE_WORD | IO_DIRECTION_IN | IO_TRAP_VALID;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_type, IO_TYPE_INPUT);
        assert_eq!(parsed.io_width, IO_WIDTH_UINT16);
        assert_eq!(parsed.byte_count, 2);
        assert_eq!(parsed.io_port, 0x03F8);
    }

    #[test]
    fn test_parse_io_field_defaults_to_dword() {
        // No size bit set → dword (matches the AMD reference's `else` branch).
        let io_field: u32 = (0x0CF8 << 16) | IO_TRAP_VALID;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_width, IO_WIDTH_UINT32);
        assert_eq!(parsed.byte_count, 4);
    }

    #[test]
    fn test_parse_io_field_byte_takes_priority_over_word() {
        // Both SZ8 and SZ16 set → byte wins (bit 4 checked first), matching the
        // AMD reference if/else order.
        let io_field: u32 = (0x0080 << 16) | IO_SIZE_BYTE | IO_SIZE_WORD | IO_TRAP_VALID;
        let parsed = parse_io_field(io_field).unwrap();
        assert_eq!(parsed.io_width, IO_WIDTH_UINT8);
        assert_eq!(parsed.byte_count, 1);
    }

    #[test]
    fn test_parse_io_field_valid_bit_clear() {
        // Valid bit (bit 1) clear → SMI not caused by I/O → None, even though
        // the direction and size encodings are otherwise well-formed.
        let io_field: u32 = (0x0080 << 16) | IO_SIZE_BYTE | IO_DIRECTION_IN;
        assert!(parse_io_field(io_field).is_none());
    }

    #[test]
    fn test_io_info_supported_always_true_on_amd() {
        // AMD does not gate on SMMRevId; any value reports info as available.
        assert!(io_info_supported(0));
        assert!(io_info_supported(VENDOR_CONSTANTS.min_rev_id_io));
        assert!(io_info_supported(u32::MAX));
    }

    #[test]
    fn test_idt_base_hi_is_correct() {
        // Verify we use the correct hi offset (0x27C), not the buggy
        // C code value (0x278 = lo offset repeated).
        let idt = register_info(MmSaveStateRegister::IdtBase).unwrap();
        assert_eq!(idt.lo_offset, 0x0278);
        assert_eq!(idt.hi_offset, 0x027C);
        assert_ne!(idt.lo_offset, idt.hi_offset, "hi must differ from lo");
    }

    #[test]
    fn test_ldt_base_hi_is_correct() {
        // Same bug fix for LdtBase: hi should be 0x28C, not 0x288.
        let ldt = register_info(MmSaveStateRegister::LdtBase).unwrap();
        assert_eq!(ldt.lo_offset, 0x0288);
        assert_eq!(ldt.hi_offset, 0x028C);
        assert_ne!(ldt.lo_offset, ldt.hi_offset, "hi must differ from lo");
    }
}
