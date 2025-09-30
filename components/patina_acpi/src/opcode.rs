use phf::phf_map;

pub(crate) const AML_OPCODE_EXTENDED_PREFIX: u8 = 0x5B;
pub(crate) const AML_OPCODE_EXTENDED_BYTE_SIZE: usize = 2;
pub(crate) const AML_OPCODE_BYTE_SIZE: usize = 1;

#[derive(Debug, Clone, Copy)]
pub(crate) struct OpcodeInfo {
    pub(crate) has_pkg_length: bool,
    pub(crate) operands: &'static [OperandType],
}

#[derive(Debug, Clone, Copy)]
pub enum OperandType {
    Byte,          // 1 byte immediate
    Word,          // 2 bytes immediate
    DWord,         // 4 bytes immediate
    QWord,         // 8 bytes immediate
    String,        // Null-terminated ASCII string
    NameString,    // Variable-length ACPI NameString
    PkgLength,     // Special AML length field
    TermList,      // Sequence of terms (children)
    DataRefObject, // e.g., constant, buffer, package, etc.
}

pub(crate) static FIXED_SIZE_OPCODES: phf::Map<u16, OpcodeInfo> = phf_map! {};

// this might need more ops in it
pub static OPCODE_TABLE: phf::Map<u16, OpcodeInfo> = phf_map! {
    // -------------------------
    // Namespace / Definition Ops
    // -------------------------
    0x08u16 => OpcodeInfo { // NameOp
        has_pkg_length: false,
        operands: &[OperandType::NameString, OperandType::DataRefObject],
    },
    0x09u16 => OpcodeInfo { // AliasOp
        has_pkg_length: false,
        operands: &[OperandType::NameString, OperandType::NameString],
    },
    0x10u16 => OpcodeInfo { // ScopeOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::TermList],
    },
    0x5B82u16 => OpcodeInfo { // DeviceOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::TermList],
    },
    0x5B14u16 => OpcodeInfo { // MethodOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::Byte, OperandType::TermList],
    },
    0x12u16 => OpcodeInfo { // PackageOp
        has_pkg_length: true,
        operands: &[OperandType::Byte, OperandType::DataRefObject /* repeated */],
    },
    0x13u16 => OpcodeInfo { // VarPackageOp
        has_pkg_length: true,
        operands: &[OperandType::DataRefObject /* count */, OperandType::DataRefObject /* repeated */],
    },
    0x5B80u16 => OpcodeInfo { // RegionOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::Byte, OperandType::DataRefObject, OperandType::DataRefObject, OperandType::TermList],
    },
    0x5B81u16 => OpcodeInfo { // FieldOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::TermList],
    },
    0x5B83u16 => OpcodeInfo { // ProcessorOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::Byte, OperandType::DWord, OperandType::Byte, OperandType::TermList],
    },
    0x5B84u16 => OpcodeInfo { // PowerResOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::Byte, OperandType::Word, OperandType::TermList],
    },
    0x5B85u16 => OpcodeInfo { // ThermalZoneOp
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::TermList],
    },

    // -------------------------
    // Control Flow Ops
    // -------------------------
    0xA0u16 => OpcodeInfo { // IfOp
        has_pkg_length: true,
        operands: &[OperandType::DataRefObject, OperandType::TermList],
    },
    0xA1u16 => OpcodeInfo { // ElseOp
        has_pkg_length: true,
        operands: &[OperandType::TermList],
    },
    0xA2u16 => OpcodeInfo { // WhileOp
        has_pkg_length: true,
        operands: &[OperandType::DataRefObject, OperandType::TermList],
    },
    0xA4u16 => OpcodeInfo { // ReturnOp
        has_pkg_length: false,
        operands: &[OperandType::DataRefObject],
    },
    0xA5u16 => OpcodeInfo { // BreakOp
        has_pkg_length: false,
        operands: &[],
    },
    0xA6u16 => OpcodeInfo { // ContinueOp
        has_pkg_length: false,
        operands: &[],
    },

    // -------------------------
    // Constants / Literals
    // -------------------------
    0x00u16 => OpcodeInfo { // ZeroOp
        has_pkg_length: false,
        operands: &[],
    },
    0x01u16 => OpcodeInfo { // OneOp
        has_pkg_length: false,
        operands: &[],
    },
    0xFFu16 => OpcodeInfo { // OnesOp
        has_pkg_length: false,
        operands: &[],
    },
    0x0Au16 => OpcodeInfo { // ByteConst
        has_pkg_length: false,
        operands: &[OperandType::Byte],
    },
    0x0Bu16 => OpcodeInfo { // WordConst
        has_pkg_length: false,
        operands: &[OperandType::Word],
    },
    0x0Cu16 => OpcodeInfo { // DWordConst
        has_pkg_length: false,
        operands: &[OperandType::DWord],
    },
    0x0Eu16 => OpcodeInfo { // QWordConst
        has_pkg_length: false,
        operands: &[OperandType::QWord],
    },
    0x0Du16 => OpcodeInfo { // StringOp
        has_pkg_length: false,
        operands: &[OperandType::String],
    },
};
