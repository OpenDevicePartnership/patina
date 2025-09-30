use phf::phf_map;

pub(crate) const AML_OPCODE_EXTENDED_PREFIX: u8 = 0x5B;
pub(crate) const AML_OPCODE_EXTENDED_BYTE_SIZE: usize = 2;
pub(crate) const AML_OPCODE_BYTE_SIZE: usize = 1;

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

// this definitely needs more ops in it
pub static OPCODE_TABLE: phf::Map<u16, OpcodeInfo> = phf_map! {
    // DeviceOp: ExtOpPrefix 0x82
    0x5B82u16 => OpcodeInfo {
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::TermList],
    },

    // MethodOp: ExtOpPrefix 0x14
    0x5B14u16 => OpcodeInfo {
        has_pkg_length: true,
        operands: &[OperandType::NameString, OperandType::Byte, OperandType::TermList],
    },

    // IfOp
    0xA0u16 => OpcodeInfo {
        has_pkg_length: true,
        operands: &[OperandType::DataRefObject, OperandType::TermList],
    },

    // ByteConst
    0x0Au16 => OpcodeInfo {
        has_pkg_length: false,
        operands: &[OperandType::Byte],
    },

    // DWordConst
    0x0Cu16 => OpcodeInfo {
        has_pkg_length: false,
        operands: &[OperandType::DWord],
    },

    // NameOp
    0x08u16 => OpcodeInfo {
        has_pkg_length: false,
        operands: &[OperandType::NameString, OperandType::DataRefObject],
    },
};
