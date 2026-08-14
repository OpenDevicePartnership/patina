//! SMRAM Save State Architecture Definitions
//!
//! Provides platform-independent types and feature-gated vendor-specific
//! register maps for reading the SMRAM save state area.
//!
//! ## Vendor Selection
//!
//! Both vendor lookup tables (`intel` and `amd`) are always compiled. The
//! *active* vendor used by the dispatch functions ([`vendor_constants`],
//! [`register_info`], [`parse_io_field`]) is selected at **build time** via
//! Cargo features on the `patina_internal_cpu` crate:
//!
//! - `save_state_intel` — Intel x64 SMRAM save state map.
//! - `save_state_amd`   — AMD64 SMRAM save state map.
//!
//! Intel is the default: AMD is active only when `save_state_amd` is enabled
//! and `save_state_intel` is not. When both (or neither) feature is enabled,
//! Intel is used. This keeps `--all-features` builds (used by CI for clippy,
//! coverage, and docs) unambiguous while still compiling and testing both
//! vendor tables.
//!
//! ## Overview
//!
//! The common types (`MmSaveStateRegister`, `RegisterInfo`, etc.) are always
//! available.  The vendor modules contain only the register-to-offset lookup
//! table and any vendor-specific constants (e.g. IO trap field layout).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// Vendor-specific submodules.
//
// Both vendor lookup tables are always compiled so that each one is type-checked
// and unit-tested in every build (including `--all-features`, which CI uses for
// clippy/coverage/doc). The *active* vendor for the dispatch functions below is
// still selected by feature, with Intel as the default (see `vendor_constants`,
// `register_info`, and `parse_io_field`).
pub mod intel;

pub mod amd;

use zerocopy_derive::{Immutable, IntoBytes};

/// `EFI_MM_SAVE_STATE_REGISTER` values from the PI Specification.
///
/// These correspond to the registers that can be read from the SMRAM save
/// state area via `EFI_MM_CPU_PROTOCOL.ReadSaveState()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum MmSaveStateRegister {
    // Descriptor table bases
    /// GDT base address.
    GdtBase = 4,
    /// IDT base address.
    IdtBase = 5,
    /// LDT base address.
    LdtBase = 6,
    /// GDT limit.
    GdtLimit = 7,
    /// IDT limit.
    IdtLimit = 8,
    /// LDT limit.
    LdtLimit = 9,
    /// LDT information.
    LdtInfo = 10,

    // Segment selectors
    /// ES selector.
    Es = 20,
    /// CS selector.
    Cs = 21,
    /// SS selector.
    Ss = 22,
    /// DS selector.
    Ds = 23,
    /// FS selector.
    Fs = 24,
    /// GS selector.
    Gs = 25,
    /// LDTR selector.
    LdtrSel = 26,
    /// TR selector.
    TrSel = 27,

    // Debug registers
    /// DR7.
    Dr7 = 28,
    /// DR6.
    Dr6 = 29,

    // Extended general-purpose registers (x86_64 only)
    /// R8.
    R8 = 30,
    /// R9.
    R9 = 31,
    /// R10.
    R10 = 32,
    /// R11.
    R11 = 33,
    /// R12.
    R12 = 34,
    /// R13.
    R13 = 35,
    /// R14.
    R14 = 36,
    /// R15.
    R15 = 37,

    // General-purpose registers
    /// RAX.
    Rax = 38,
    /// RBX.
    Rbx = 39,
    /// RCX.
    Rcx = 40,
    /// RDX.
    Rdx = 41,
    /// RSP.
    Rsp = 42,
    /// RBP.
    Rbp = 43,
    /// RSI.
    Rsi = 44,
    /// RDI.
    Rdi = 45,
    /// RIP.
    Rip = 46,

    // Flags and control registers
    /// RFLAGS.
    Rflags = 51,
    /// CR0.
    Cr0 = 52,
    /// CR3.
    Cr3 = 53,
    /// CR4.
    Cr4 = 54,

    // Pseudo-registers
    /// I/O operation information.
    Io = 512,
    /// Long Mode Active indicator.
    Lma = 513,
    /// Processor identifier (APIC ID).
    ProcessorId = 514,
}

impl MmSaveStateRegister {
    /// Creates a register from a raw `EFI_MM_SAVE_STATE_REGISTER` value.
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            4 => Some(Self::GdtBase),
            5 => Some(Self::IdtBase),
            6 => Some(Self::LdtBase),
            7 => Some(Self::GdtLimit),
            8 => Some(Self::IdtLimit),
            9 => Some(Self::LdtLimit),
            10 => Some(Self::LdtInfo),
            20 => Some(Self::Es),
            21 => Some(Self::Cs),
            22 => Some(Self::Ss),
            23 => Some(Self::Ds),
            24 => Some(Self::Fs),
            25 => Some(Self::Gs),
            26 => Some(Self::LdtrSel),
            27 => Some(Self::TrSel),
            28 => Some(Self::Dr7),
            29 => Some(Self::Dr6),
            30 => Some(Self::R8),
            31 => Some(Self::R9),
            32 => Some(Self::R10),
            33 => Some(Self::R11),
            34 => Some(Self::R12),
            35 => Some(Self::R13),
            36 => Some(Self::R14),
            37 => Some(Self::R15),
            38 => Some(Self::Rax),
            39 => Some(Self::Rbx),
            40 => Some(Self::Rcx),
            41 => Some(Self::Rdx),
            42 => Some(Self::Rsp),
            43 => Some(Self::Rbp),
            44 => Some(Self::Rsi),
            45 => Some(Self::Rdi),
            46 => Some(Self::Rip),
            51 => Some(Self::Rflags),
            52 => Some(Self::Cr0),
            53 => Some(Self::Cr3),
            54 => Some(Self::Cr4),
            512 => Some(Self::Io),
            513 => Some(Self::Lma),
            514 => Some(Self::ProcessorId),
            _ => None,
        }
    }
}

/// Size of [`MmSaveStateIoInfo`] in bytes.
pub const IO_INFO_SIZE: usize = 24;

/// `EFI_MM_SAVE_STATE_IO_TYPE_INPUT` (1).
pub const IO_TYPE_INPUT: u32 = 1;
/// `EFI_MM_SAVE_STATE_IO_TYPE_OUTPUT` (2).
pub const IO_TYPE_OUTPUT: u32 = 2;

/// `EFI_MM_SAVE_STATE_IO_WIDTH_UINT8` (0).
pub const IO_WIDTH_UINT8: u32 = 0;
/// `EFI_MM_SAVE_STATE_IO_WIDTH_UINT16` (1).
pub const IO_WIDTH_UINT16: u32 = 1;
/// `EFI_MM_SAVE_STATE_IO_WIDTH_UINT32` (2).
pub const IO_WIDTH_UINT32: u32 = 2;

/// IA32_EFER.LMA bit (bit 10).
pub const IA32_EFER_LMA: u64 = 1 << 10;

/// LMA value: processor was in 32-bit mode.
pub const LMA_32BIT: u64 = 32;
/// LMA value: processor was in 64-bit mode.
pub const LMA_64BIT: u64 = 64;

/// Size of one `EFI_PROCESSOR_INFORMATION` entry (PI 1.7+).
///
/// Layout: `ProcessorId`(8) + `StatusFlag`(4) + `CpuPhysicalLocation`(12) +
/// `ExtendedProcessorInformation`(24) = 48 bytes.
pub const PROCESSOR_INFO_ENTRY_SIZE: usize = 48;

/// Layout descriptor for a register in the SMRAM save state map.
///
/// For 8-byte registers the value may be stored as two u32 at potentially
/// non-contiguous offsets (e.g. Intel GDT/IDT/LDT base).  `lo_offset` gives
/// the low dword; `hi_offset` gives the high dword.
///
/// For registers narrower than 8 bytes, `hi_offset` is 0 and only the low
/// dword location is used.
#[derive(Debug, Clone, Copy)]
pub struct RegisterInfo {
    /// Offset of the low (or only) dword from the save state base.
    pub lo_offset: u16,
    /// Offset of the high dword.  0 for registers < 8 bytes.
    pub hi_offset: u16,
    /// Native width in bytes as stored in the save state (1, 2, 4, or 8).
    pub native_width: u8,
}

/// Vendor-specific save state field offsets and behaviour that differ between
/// Intel and AMD.  Provided by the active vendor module.
pub struct VendorConstants {
    /// Offset of `SMMRevId` within the save state map.
    pub smmrevid_offset: u16,
    /// Offset of the IO information field (Intel `IOMisc`, AMD `IO_DWord`).
    pub io_info_offset: u16,
    /// Offset of `IA32_EFER` / `EFER`.
    pub efer_offset: u16,
    /// Offset of `_RAX` (used for IO data reads).
    pub rax_offset: u16,
    /// Minimum `SMMRevId` that supports IO information.
    pub min_rev_id_io: u32,
    /// Whether the LMA pseudo-register always returns 64-bit.
    ///
    /// AMD64 processors always operate in 64-bit mode during SMM, so
    /// they skip the EFER.LMA check and always return LMA_64BIT.
    pub lma_always_64: bool,
}

/// Parsed I/O trap information extracted from the vendor-specific IO field.
///
/// Returned by [`parse_io_field`] after decoding the vendor's raw IO bits.
#[derive(Debug, Clone, Copy)]
pub struct ParsedIoInfo {
    /// I/O type: [`IO_TYPE_INPUT`] (1) or [`IO_TYPE_OUTPUT`] (2).
    pub io_type: u32,
    /// I/O width: [`IO_WIDTH_UINT8`], [`IO_WIDTH_UINT16`], or [`IO_WIDTH_UINT32`].
    pub io_width: u32,
    /// Number of bytes transferred (1, 2, or 4).
    pub byte_count: usize,
    /// I/O port address.
    pub io_port: u16,
}

/// `EFI_MM_SAVE_STATE_IO_INFO` — written to the user buffer when reading
/// the IO pseudo-register.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, IntoBytes, Immutable)]
pub struct MmSaveStateIoInfo {
    /// I/O data value (from RAX, zero-extended).
    pub io_data: u64,
    /// I/O port address.
    pub io_port: u16,
    /// Explicit padding required by the C ABI.
    pub _pad0: [u8; 2],
    /// I/O width enum (`EFI_MM_SAVE_STATE_IO_WIDTH`).
    pub io_width: u32,
    /// I/O type enum (`EFI_MM_SAVE_STATE_IO_TYPE`).
    pub io_type: u32,
    /// Explicit trailing padding required by the C ABI.
    pub _pad1: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<MmSaveStateIoInfo>() == IO_INFO_SIZE);

/// Returns the [`VendorConstants`] for the active vendor (selected at build time).
///
/// The active vendor is AMD only when `save_state_amd` is enabled and
/// `save_state_intel` is not; otherwise Intel is used (this includes the
/// default case where neither feature is enabled).
#[cfg(all(feature = "save_state_amd", not(feature = "save_state_intel")))]
pub fn vendor_constants() -> &'static VendorConstants {
    &amd::VENDOR_CONSTANTS
}

/// Returns the [`VendorConstants`] for the active vendor (selected at build time).
///
/// Intel is the default vendor and is used whenever `save_state_amd` is not the
/// sole selected vendor (i.e. Intel-only, both vendors, or no vendor enabled).
#[cfg(not(all(feature = "save_state_amd", not(feature = "save_state_intel"))))]
pub fn vendor_constants() -> &'static VendorConstants {
    &intel::VENDOR_CONSTANTS
}

/// Returns the [`RegisterInfo`] for a given register in the active vendor's
/// save state map.
///
/// Returns `None` for pseudo-registers (IO, LMA, ProcessorId) and for
/// registers not supported by the active vendor's 64-bit save state layout.
///
/// AMD is the active vendor only when `save_state_amd` is enabled and
/// `save_state_intel` is not; otherwise Intel is used (the default).
#[cfg(all(feature = "save_state_amd", not(feature = "save_state_intel")))]
pub fn register_info(reg: MmSaveStateRegister) -> Option<RegisterInfo> {
    amd::register_info(reg)
}

/// Returns the [`RegisterInfo`] for a given register in the active vendor's
/// save state map.
///
/// Returns `None` for pseudo-registers (IO, LMA, ProcessorId) and for
/// registers not supported by the active vendor's 64-bit save state layout.
#[cfg(not(all(feature = "save_state_amd", not(feature = "save_state_intel"))))]
pub fn register_info(reg: MmSaveStateRegister) -> Option<RegisterInfo> {
    intel::register_info(reg)
}

/// Parses the vendor-specific I/O information field from the save state.
///
/// Returns `None` if the field indicates no I/O instruction triggered the SMI
/// (Intel: `SmiFlag` not set) or if the I/O type/width is not a simple IN/OUT.
///
/// AMD is the active vendor only when `save_state_amd` is enabled and
/// `save_state_intel` is not; otherwise Intel is used (the default).
#[cfg(all(feature = "save_state_amd", not(feature = "save_state_intel")))]
pub fn parse_io_field(io_field: u32) -> Option<ParsedIoInfo> {
    amd::parse_io_field(io_field)
}

/// Parses the vendor-specific I/O information field from the save state.
///
/// Returns `None` if the field indicates no I/O instruction triggered the SMI
/// (Intel: `SmiFlag` not set) or if the I/O type/width is not a simple IN/OUT.
#[cfg(not(all(feature = "save_state_amd", not(feature = "save_state_intel"))))]
pub fn parse_io_field(io_field: u32) -> Option<ParsedIoInfo> {
    intel::parse_io_field(io_field)
}

/// Returns whether the AMD's SMRAM save state map exposes I/O trap
/// information for a save state with the given raw `SMMRevId`.
///
#[cfg(all(feature = "save_state_amd", not(feature = "save_state_intel")))]
pub fn io_info_supported(smm_rev_id: u32) -> bool {
    amd::io_info_supported(smm_rev_id)
}

/// Returns whether the Intel's SMRAM save state map exposes I/O trap
/// information for a save state with the given raw `SMMRevId`.
///
#[cfg(not(all(feature = "save_state_amd", not(feature = "save_state_intel"))))]
pub fn io_info_supported(smm_rev_id: u32) -> bool {
    intel::io_info_supported(smm_rev_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_from_u64() {
        assert_eq!(MmSaveStateRegister::from_u64(38), Some(MmSaveStateRegister::Rax));
        assert_eq!(MmSaveStateRegister::from_u64(512), Some(MmSaveStateRegister::Io));
        assert_eq!(MmSaveStateRegister::from_u64(514), Some(MmSaveStateRegister::ProcessorId));
        assert_eq!(MmSaveStateRegister::from_u64(999), None);
        assert_eq!(MmSaveStateRegister::from_u64(0), None);
    }

    #[test]
    fn test_io_info_struct_layout() {
        assert_eq!(core::mem::size_of::<MmSaveStateIoInfo>(), 24);
        assert_eq!(core::mem::offset_of!(MmSaveStateIoInfo, io_data), 0);
        assert_eq!(core::mem::offset_of!(MmSaveStateIoInfo, io_port), 8);
        assert_eq!(core::mem::offset_of!(MmSaveStateIoInfo, _pad0), 10);
        assert_eq!(core::mem::offset_of!(MmSaveStateIoInfo, io_width), 12);
        assert_eq!(core::mem::offset_of!(MmSaveStateIoInfo, io_type), 16);
        assert_eq!(core::mem::offset_of!(MmSaveStateIoInfo, _pad1), 20);

        let info = MmSaveStateIoInfo { io_data: 0, io_port: 0, _pad0: [0; 2], io_width: 0, io_type: 0, _pad1: [0; 4] };
        let bytes = zerocopy::IntoBytes::as_bytes(&info);
        assert_eq!(&bytes[10..12], &[0; 2]);
        assert_eq!(&bytes[20..24], &[0; 4]);
    }

    /// Verifies the active-vendor dispatch resolves to AMD only when
    /// `save_state_amd` is enabled and `save_state_intel` is not.
    #[cfg(all(feature = "save_state_amd", not(feature = "save_state_intel")))]
    #[test]
    fn test_active_vendor_dispatches_to_amd() {
        // `Rax` lives at a different offset on each vendor, so its low offset
        // distinguishes which table the dispatch functions resolved to.
        assert_eq!(register_info(MmSaveStateRegister::Rax).unwrap().lo_offset, 0x03F8);
        assert_eq!(vendor_constants().io_info_offset, amd::VENDOR_CONSTANTS.io_info_offset);
        assert!(vendor_constants().lma_always_64);
        assert!(io_info_supported(0));
    }

    /// Verifies the active-vendor dispatch resolves to Intel by default — i.e.
    /// when `save_state_intel` is enabled, when both vendors are enabled, or
    /// when neither vendor is enabled.
    #[cfg(not(all(feature = "save_state_amd", not(feature = "save_state_intel"))))]
    #[test]
    fn test_active_vendor_dispatches_to_intel_by_default() {
        // `Rax` lives at a different offset on each vendor, so its low offset
        // distinguishes which table the dispatch functions resolved to.
        assert_eq!(register_info(MmSaveStateRegister::Rax).unwrap().lo_offset, 0x035C);
        assert_eq!(vendor_constants().io_info_offset, intel::VENDOR_CONSTANTS.io_info_offset);
        assert!(!vendor_constants().lma_always_64);
        assert!(!io_info_supported(intel::VENDOR_CONSTANTS.min_rev_id_io - 1));
        assert!(io_info_supported(intel::VENDOR_CONSTANTS.min_rev_id_io));
    }
}
