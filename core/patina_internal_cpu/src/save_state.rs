//! SMRAM Save State Architecture Definitions
//!
//! Provides platform-independent types and the vendor-specific register maps
//! for reading the SMRAM save state area.
//!
//! ## Vendor Selection
//!
//! The `intel` and `amd` modules hold each vendor's register-to-offset table
//! and I/O field decoder. The vendor is selected at **build time** by the
//! `save_state_amd` Cargo feature: with it the AMD64 layout is used, without it
//! the Intel x64 layout is used.
//!
//! [`SaveState`] is a copy of one CPU's save state map. [`SaveState::from_smbase`]
//! copies it out of SMRAM given that CPU's SMBASE, and its methods do every read
//! and decode, so callers never handle raw offsets or vendor field encodings.
//! Where the SMBASE values come from is up to the caller.
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

// Only the selected vendor is built. Unit tests build both, so each table keeps its own tests.
#[cfg(any(test, feature = "save_state_amd"))]
mod amd;
mod definitions;
#[cfg(any(test, not(feature = "save_state_amd")))]
mod intel;

#[cfg(feature = "save_state_amd")]
use amd as vendor;
#[cfg(not(feature = "save_state_amd"))]
use intel as vendor;

pub use definitions::*;

use zerocopy::IntoBytes as _;

/// Errors reported when locating or reading a CPU's save state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStateError {
    /// The CPU index is out of range, or that CPU has no usable SMBASE.
    InvalidParameter,
    /// The register has no entry in this vendor's save state map.
    RegisterNotMapped(MmSaveStateRegister),
    /// The register cannot be read at the requested width.
    UnsupportedWidth {
        /// The register that was requested.
        register: MmSaveStateRegister,
        /// The requested width in bytes.
        width: usize,
    },
    /// The output buffer is shorter than the requested width.
    BufferTooSmall {
        /// Bytes the read needs.
        needed: usize,
        /// Bytes the buffer holds.
        available: usize,
    },
    /// The SMI was not caused by an I/O instruction, so there is no I/O trap to report.
    NoIoTrap,
    /// The save state revision predates I/O trap information.
    IoInfoUnsupported {
        /// The `SMMRevId` the save state reported.
        smm_rev_id: u32,
    },
}

impl core::error::Error for SaveStateError {}

impl core::fmt::Display for SaveStateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidParameter => write!(f, "the CPU index is out of range or the CPU has no usable SMBASE"),
            Self::RegisterNotMapped(register) => {
                write!(f, "register {register:?} has no entry in the save state map")
            }
            Self::UnsupportedWidth { register, width } => {
                write!(f, "register {register:?} cannot be read as {width} bytes")
            }
            Self::BufferTooSmall { needed, available } => {
                write!(f, "the output buffer holds {available} bytes, need {needed}")
            }
            Self::NoIoTrap => write!(f, "the save state records no I/O trap"),
            Self::IoInfoUnsupported { smm_rev_id } => {
                write!(f, "SMMRevId {smm_rev_id:#x} does not expose I/O info")
            }
        }
    }
}

/// Name of the save state layout this build uses, for logs.
#[cfg(feature = "save_state_amd")]
pub const VENDOR_NAME: &str = "AMD";

/// Name of the save state layout this build uses, for logs.
#[cfg(not(feature = "save_state_amd"))]
pub const VENDOR_NAME: &str = "Intel";

/// Returns the field offsets and behaviour flags of the layout this build uses.
pub fn vendor_constants() -> &'static VendorConstants {
    &vendor::VENDOR_CONSTANTS
}

/// A copy of one CPU's SMRAM save state map, read through the layout this build uses.
///
/// Created by [`from_smbase`](Self::from_smbase). Every read decodes the vendor-specific
/// fields here, so callers never handle raw offsets or the I/O field encoding.
#[derive(Clone)]
pub struct SaveState {
    bytes: SaveStateBytes,
}

impl SaveState {
    /// Wraps a copy of a save state map.
    pub fn new(bytes: SaveStateBytes) -> Self {
        Self { bytes }
    }

    /// Returns where `register` lives in the save state map of the layout this build uses.
    ///
    /// Returns `None` for pseudo-registers (IO, LMA, `ProcessorId`), which are
    /// handled separately, and for registers the 64-bit layout does not record.
    pub fn register_info(register: MmSaveStateRegister) -> Option<RegisterInfo> {
        vendor::register_info(register)
    }

    /// Returns the address of the save state map of a CPU whose SMBASE is `smbase`.
    ///
    /// The processor saves its state [`SMRAM_SAVE_STATE_MAP_OFFSET`] bytes above its
    /// SMBASE. Every caller that turns an SMBASE into a map address uses this function,
    /// so a region validated with it is the same region later read through it.
    ///
    /// # Errors
    ///
    /// Returns [`SaveStateError::InvalidParameter`] when `smbase` is 0 or the end of the
    /// map does not fit in a `u64`.
    pub fn map_base(smbase: u64) -> Result<u64, SaveStateError> {
        if smbase == 0 {
            return Err(SaveStateError::InvalidParameter);
        }
        let base = smbase.checked_add(SMRAM_SAVE_STATE_MAP_OFFSET).ok_or(SaveStateError::InvalidParameter)?;
        base.checked_add(SMRAM_SAVE_STATE_MAP_SIZE as u64).ok_or(SaveStateError::InvalidParameter)?;
        Ok(base)
    }

    /// Copies the save state map of a CPU whose SMBASE is `smbase` out of SMRAM.
    ///
    /// The map address comes from [`map_base`](Self::map_base), and the map length is
    /// the fixed [`SMRAM_SAVE_STATE_MAP_SIZE`].
    ///
    /// # Errors
    ///
    /// Returns the errors of [`map_base`](Self::map_base).
    ///
    /// # Safety
    ///
    /// [`SMRAM_SAVE_STATE_MAP_SIZE`] bytes at the address `map_base` returns for `smbase`
    /// must be readable.
    pub unsafe fn from_smbase(smbase: u64) -> Result<Self, SaveStateError> {
        let base = Self::map_base(smbase)?;

        // SAFETY: the caller guarantees that `SMRAM_SAVE_STATE_MAP_SIZE` bytes at `base` are
        // readable. `SaveStateBytes` is a byte array of exactly that size with an alignment of
        // one, so any address is suitably aligned. The bytes are copied, so no reference to
        // SMRAM outlives this call.
        let bytes = unsafe { core::ptr::read(base as *const SaveStateBytes) };

        Ok(Self::new(bytes))
    }

    /// Copies `width` bytes of an architectural register into the start of `out`.
    ///
    /// A 2- or 4-byte read takes the low bytes of the field. An 8-byte read writes
    /// the low and high 32-bit halves next to each other, which also covers the Intel
    /// descriptor-table bases whose halves are stored apart.
    pub fn read_architectural_register(
        &self,
        register: MmSaveStateRegister,
        width: usize,
        out: &mut [u8],
    ) -> Result<(), SaveStateError> {
        let info = vendor::register_info(register).ok_or(SaveStateError::RegisterNotMapped(register))?;
        self.read_field(register, info, width, out)
    }

    /// Returns the I/O trap that caused the SMI, if any.
    ///
    /// Returns `Ok(None)` when the save state records no I/O trap, or records one
    /// that is not a simple IN or OUT.
    pub fn io_info(&self) -> Result<Option<MmSaveStateIoInfo>, SaveStateError> {
        let constants = vendor_constants();

        let smm_rev_id = u32::from_le_bytes(self.read_bytes(constants.smmrevid_offset));
        if !vendor::io_info_supported(smm_rev_id) {
            return Err(SaveStateError::IoInfoUnsupported { smm_rev_id });
        }

        let io_field = u32::from_le_bytes(self.read_bytes(constants.io_info_offset));
        let Some(parsed) = vendor::parse_io_field(io_field) else {
            return Ok(None);
        };

        // The I/O data is the low `byte_count` bytes of RAX. Both parsers only
        // report 1, 2, or 4 bytes.
        let rax = constants.rax_offset;
        let io_data = match parsed.byte_count {
            1 => u64::from(u8::from_le_bytes(self.read_bytes(rax))),
            2 => u64::from(u16::from_le_bytes(self.read_bytes(rax))),
            _ => u64::from(u32::from_le_bytes(self.read_bytes(rax))),
        };

        Ok(Some(MmSaveStateIoInfo {
            io_data,
            io_port: parsed.io_port,
            _pad0: [0; 2],
            io_width: parsed.io_width,
            io_type: parsed.io_type,
            _pad1: [0; 4],
        }))
    }

    /// Writes the IO pseudo-register, an `EFI_MM_SAVE_STATE_IO_INFO`, into the start of `out`.
    ///
    /// It describes the I/O instruction that triggered the SMI: the port, width,
    /// direction, and data value.
    pub fn read_io_register(&self, out: &mut [u8]) -> Result<(), SaveStateError> {
        let io_info = self.io_info()?.ok_or(SaveStateError::NoIoTrap)?;

        let available = out.len();
        let out =
            out.get_mut(..IO_INFO_SIZE).ok_or(SaveStateError::BufferTooSmall { needed: IO_INFO_SIZE, available })?;
        out.copy_from_slice(io_info.as_bytes());

        Ok(())
    }

    /// Returns the LMA pseudo-register: [`LMA_64BIT`] or [`LMA_32BIT`].
    ///
    /// AMD64 always runs in long mode during SMM. Intel reports the `IA32_EFER.LMA`
    /// bit saved in the map.
    pub fn lma(&self) -> u64 {
        let constants = vendor_constants();
        if constants.lma_always_64 {
            return LMA_64BIT;
        }

        let efer = u64::from_le_bytes(self.read_bytes(constants.efer_offset));
        if efer & IA32_EFER_LMA != 0 { LMA_64BIT } else { LMA_32BIT }
    }

    /// Writes the LMA pseudo-register as a 4- or 8-byte value into the start of `out`.
    pub fn read_lma_register(&self, width: usize, out: &mut [u8]) -> Result<(), SaveStateError> {
        if width != 4 && width != 8 {
            return Err(SaveStateError::UnsupportedWidth { register: MmSaveStateRegister::Lma, width });
        }

        let available = out.len();
        let out = out.get_mut(..width).ok_or(SaveStateError::BufferTooSmall { needed: width, available })?;
        let value = self.lma().to_le_bytes();
        out.copy_from_slice(value.get(..width).expect("the width is 4 or 8"));

        Ok(())
    }

    /// Reads the `N` bytes at `offset` in the map.
    ///
    /// Every offset in the vendor tables is below [`SMRAM_SAVE_STATE_MAP_SIZE`] with
    /// room for the field, which `test_vendor_tables_fit_the_save_state_map` checks,
    /// so the read cannot leave the map.
    fn read_bytes<const N: usize>(&self, offset: u16) -> [u8; N] {
        let start = usize::from(offset);
        self.bytes
            .get(start..start + N)
            .and_then(|field| field.try_into().ok())
            .expect("save state table offsets lie inside the save state map")
    }

    /// Copies `width` bytes of the field `info` describes into the start of `out`.
    ///
    /// `register` is carried through for the error only.
    fn read_field(
        &self,
        register: MmSaveStateRegister,
        info: RegisterInfo,
        width: usize,
        out: &mut [u8],
    ) -> Result<(), SaveStateError> {
        let supported = match width {
            2 => info.native_width >= 2,
            4 => info.native_width >= 4,
            8 => info.native_width == 8,
            _ => false,
        };
        if !supported {
            return Err(SaveStateError::UnsupportedWidth { register, width });
        }

        let available = out.len();
        let out = out.get_mut(..width).ok_or(SaveStateError::BufferTooSmall { needed: width, available })?;
        let value = match width {
            2 => u64::from(u16::from_le_bytes(self.read_bytes(info.lo_offset))),
            4 => u64::from(u32::from_le_bytes(self.read_bytes(info.lo_offset))),
            // An 8-byte read: the high dword may be stored apart from the low one.
            _ => {
                let low = u32::from_le_bytes(self.read_bytes(info.lo_offset));
                let high = u32::from_le_bytes(self.read_bytes(info.hi_offset));
                (u64::from(high) << 32) | u64::from(low)
            }
        };
        let value = value.to_le_bytes();
        out.copy_from_slice(value.get(..width).expect("supported widths are at most 8 bytes"));

        Ok(())
    }
}

// The save state holds the interrupted context's registers, so the bytes are
// deliberately left out of the debug output.
impl core::fmt::Debug for SaveState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("SaveState").field(&VENDOR_NAME).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes an Intel `IOMisc` field that decodes with the requested direction and size.
    #[cfg(not(feature = "save_state_amd"))]
    fn io_trap_field(port: u16, input: bool, byte_count: usize) -> u32 {
        // SmiFlag | Length (bits 3:1) | Type IN(1)/OUT(0) (bits 7:4).
        (u32::from(port) << 16) | (u32::from(input) << 4) | ((byte_count as u32) << 1) | 1
    }

    /// Encodes an AMD `IO_DWord` field that decodes with the requested direction and size.
    #[cfg(feature = "save_state_amd")]
    fn io_trap_field(port: u16, input: bool, byte_count: usize) -> u32 {
        // Valid (bit 1) | SZ8 (bit 4) / SZ16 (bit 5) / SZ32 (default) | Direction (bit 0).
        let size = match byte_count {
            1 => 1 << 4,
            2 => 1 << 5,
            _ => 0,
        };
        (u32::from(port) << 16) | size | (1 << 1) | u32::from(input)
    }

    /// Builds a zeroed map whose `SMMRevId` advertises I/O trap support.
    fn new_map() -> Box<SaveStateBytes> {
        let mut map = Box::new([0u8; SMRAM_SAVE_STATE_MAP_SIZE]);
        let constants = vendor_constants();
        write_u32(&mut map, constants.smmrevid_offset, constants.min_rev_id_io);
        map
    }

    fn write_u32(map: &mut SaveStateBytes, offset: u16, value: u32) {
        let offset = usize::from(offset);
        map[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(map: &mut SaveStateBytes, offset: u16, value: u64) {
        let offset = usize::from(offset);
        map[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn test_save_state_uses_the_vendor_selected_by_the_build() {
        // `Rax` lives at a different offset on each vendor, so its low offset shows which table
        // the build selected.
        let rax = SaveState::register_info(MmSaveStateRegister::Rax).unwrap();
        if cfg!(feature = "save_state_amd") {
            assert_eq!((VENDOR_NAME, rax.lo_offset), ("AMD", 0x03F8));
            assert!(vendor_constants().lma_always_64);
        } else {
            assert_eq!((VENDOR_NAME, rax.lo_offset), ("Intel", 0x035C));
            assert!(!vendor_constants().lma_always_64);
        }
    }

    #[test]
    fn test_vendor_tables_fit_the_save_state_map() {
        // `read_bytes` relies on every table offset leaving room for the field inside the map.
        // Both tables are built for unit tests, so both are checked whichever one is selected.
        type RegisterInfoFn = fn(MmSaveStateRegister) -> Option<RegisterInfo>;
        let tables: [(&str, &VendorConstants, RegisterInfoFn); 2] = [
            ("Intel", &intel::VENDOR_CONSTANTS, intel::register_info),
            ("AMD", &amd::VENDOR_CONSTANTS, amd::register_info),
        ];
        let size = SMRAM_SAVE_STATE_MAP_SIZE;
        for (name, constants, lookup) in tables {
            assert!(usize::from(constants.smmrevid_offset) + 4 <= size);
            assert!(usize::from(constants.io_info_offset) + 4 <= size);
            assert!(usize::from(constants.efer_offset) + 8 <= size);
            assert!(usize::from(constants.rax_offset) + 4 <= size);

            for raw in 0..=u64::from(u16::MAX) {
                let Some(register) = MmSaveStateRegister::from_u64(raw) else { continue };
                let Some(info) = lookup(register) else { continue };
                let low_width = usize::from(info.native_width.min(4));
                assert!(usize::from(info.lo_offset) + low_width <= size, "{name} {register:?} low half");
                if info.native_width == 8 {
                    assert!(usize::from(info.hi_offset) + 4 <= size, "{name} {register:?} high half");
                }
            }
        }
    }

    #[test]
    fn test_read_architectural_register_widths() {
        let rax = SaveState::register_info(MmSaveStateRegister::Rax).unwrap();
        let mut map = new_map();
        write_u32(&mut map, rax.lo_offset, 0x1122_3344);
        write_u32(&mut map, rax.hi_offset, 0x5566_7788);
        let state = SaveState::new(*map);

        let mut out = [0u8; 8];
        assert_eq!(state.read_architectural_register(MmSaveStateRegister::Rax, 2, &mut out), Ok(()));
        assert_eq!(u16::from_le_bytes(out[..2].try_into().unwrap()), 0x3344);

        assert_eq!(state.read_architectural_register(MmSaveStateRegister::Rax, 4, &mut out), Ok(()));
        assert_eq!(u32::from_le_bytes(out[..4].try_into().unwrap()), 0x1122_3344);

        assert_eq!(state.read_architectural_register(MmSaveStateRegister::Rax, 8, &mut out), Ok(()));
        assert_eq!(u64::from_le_bytes(out), 0x5566_7788_1122_3344);
    }

    #[cfg(not(feature = "save_state_amd"))]
    #[test]
    fn test_read_architectural_register_joins_split_halves() {
        // Intel stores the high dword of the GDT base far from the low dword.
        let gdt = SaveState::register_info(MmSaveStateRegister::GdtBase).unwrap();
        assert_ne!(gdt.hi_offset, gdt.lo_offset + 4);

        let mut map = new_map();
        write_u32(&mut map, gdt.lo_offset, 0xAAAA_0000);
        write_u32(&mut map, gdt.hi_offset, 0x0000_BBBB);

        let mut out = [0u8; 8];
        assert_eq!(SaveState::new(*map).read_architectural_register(MmSaveStateRegister::GdtBase, 8, &mut out), Ok(()));
        assert_eq!(u64::from_le_bytes(out), 0x0000_BBBB_AAAA_0000);
    }

    #[test]
    fn test_read_architectural_register_rejects_unmapped_registers() {
        let state = SaveState::new(*new_map());
        let mut out = [0u8; 8];

        // `LdtInfo` is absent from both maps, and pseudo-registers are read through other calls.
        for register in [MmSaveStateRegister::LdtInfo, MmSaveStateRegister::Io, MmSaveStateRegister::Lma] {
            assert_eq!(
                state.read_architectural_register(register, 8, &mut out),
                Err(SaveStateError::RegisterNotMapped(register))
            );
        }
    }

    #[test]
    fn test_read_architectural_register_rejects_unsupported_widths() {
        let state = SaveState::new(*new_map());
        let mut out = [0u8; 16];

        for width in [0, 1, 3, 16] {
            assert_eq!(
                state.read_architectural_register(MmSaveStateRegister::Rax, width, &mut out),
                Err(SaveStateError::UnsupportedWidth { register: MmSaveStateRegister::Rax, width })
            );
        }

        // CR4 is 8 bytes on AMD but only 4 on Intel.
        let cr4 = state.read_architectural_register(MmSaveStateRegister::Cr4, 8, &mut out);
        if cfg!(feature = "save_state_amd") {
            assert_eq!(cr4, Ok(()));
        } else {
            assert_eq!(cr4, Err(SaveStateError::UnsupportedWidth { register: MmSaveStateRegister::Cr4, width: 8 }));
        }
    }

    #[test]
    fn test_read_architectural_register_rejects_short_buffers() {
        let state = SaveState::new(*new_map());

        let mut out = [0u8; 4];
        assert_eq!(
            state.read_architectural_register(MmSaveStateRegister::Rax, 8, &mut out),
            Err(SaveStateError::BufferTooSmall { needed: 8, available: 4 })
        );
        assert_eq!(
            state.read_architectural_register(MmSaveStateRegister::Rax, 4, &mut out[..2]),
            Err(SaveStateError::BufferTooSmall { needed: 4, available: 2 })
        );
    }

    #[test]
    fn test_read_field_enforces_native_width() {
        let data = SaveState::new(*new_map());
        let reg = MmSaveStateRegister::Rax;
        let mut out = [0u8; 8];

        let byte = RegisterInfo { lo_offset: 0x10, hi_offset: 0, native_width: 1 };
        let word = RegisterInfo { lo_offset: 0x10, hi_offset: 0, native_width: 2 };
        let dword = RegisterInfo { lo_offset: 0x10, hi_offset: 0, native_width: 4 };

        let unsupported =
            |width| -> Result<(), SaveStateError> { Err(SaveStateError::UnsupportedWidth { register: reg, width }) };
        assert_eq!(data.read_field(reg, byte, 2, &mut out), unsupported(2));
        assert_eq!(data.read_field(reg, word, 4, &mut out), unsupported(4));
        assert_eq!(data.read_field(reg, dword, 8, &mut out), unsupported(8));
        assert_eq!(data.read_field(reg, word, 2, &mut out), Ok(()));
        assert_eq!(data.read_field(reg, dword, 4, &mut out), Ok(()));
    }

    #[test]
    fn test_io_info_reports_no_trap_for_a_zeroed_field() {
        assert_eq!(SaveState::new(*new_map()).io_info(), Ok(None));
    }

    #[test]
    fn test_io_info_decodes_direction_width_and_data() {
        const PORT: u16 = 0x70;

        let constants = vendor_constants();
        for (input, io_type) in [(true, IO_TYPE_INPUT), (false, IO_TYPE_OUTPUT)] {
            for (byte_count, io_width, io_data) in
                [(1usize, IO_WIDTH_UINT8, 0xDDu64), (2, IO_WIDTH_UINT16, 0xCCDD), (4, IO_WIDTH_UINT32, 0xAABB_CCDD)]
            {
                let mut map = new_map();
                write_u32(&mut map, constants.io_info_offset, io_trap_field(PORT, input, byte_count));
                write_u32(&mut map, constants.rax_offset, 0xAABB_CCDD);

                let expected = MmSaveStateIoInfo { io_data, io_port: PORT, io_width, io_type, ..Default::default() };
                assert_eq!(SaveState::new(*map).io_info(), Ok(Some(expected)), "{input} {byte_count}");
            }
        }
    }

    #[test]
    fn test_io_info_decodes_bit_zero_per_vendor() {
        // Bit 0 is the IN/OUT direction on AMD but the "SMI was an I/O trap" flag on Intel. Here
        // Intel decodes length 1 (bits 3:1) and type OUT (bits 7:4 clear), while AMD decodes a
        // valid record (bit 1) for a 32-bit IN.
        let field = (0xB2 << 16) | (1 << 1) | 1;
        let expected = if cfg!(feature = "save_state_amd") { IO_TYPE_INPUT } else { IO_TYPE_OUTPUT };

        let mut map = new_map();
        write_u32(&mut map, vendor_constants().io_info_offset, field);
        assert_eq!(SaveState::new(*map).io_info().unwrap().unwrap().io_type, expected);
    }

    #[cfg(not(feature = "save_state_amd"))]
    #[test]
    fn test_io_info_rejects_a_legacy_intel_revision() {
        // AMD always reports I/O info, so only the Intel layout can observe an older revision.
        let constants = vendor_constants();
        let mut map = new_map();
        let smm_rev_id = constants.min_rev_id_io - 1;
        write_u32(&mut map, constants.smmrevid_offset, smm_rev_id);

        assert_eq!(SaveState::new(*map).io_info(), Err(SaveStateError::IoInfoUnsupported { smm_rev_id }));

        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(
            SaveState::new(*map).read_io_register(&mut out),
            Err(SaveStateError::IoInfoUnsupported { smm_rev_id })
        );
    }

    #[test]
    fn test_lma_follows_efer_on_intel_and_is_always_64_on_amd() {
        for (efer, intel_lma) in [(IA32_EFER_LMA, LMA_64BIT), (0, LMA_32BIT)] {
            let expected = if cfg!(feature = "save_state_amd") { LMA_64BIT } else { intel_lma };
            let mut map = new_map();
            write_u64(&mut map, vendor_constants().efer_offset, efer);
            assert_eq!(SaveState::new(*map).lma(), expected, "EFER {efer:#x}");
        }
    }

    #[test]
    fn test_read_io_register_writes_the_io_info_structure() {
        const PORT: u16 = 0xB2;

        let constants = vendor_constants();
        let mut map = new_map();
        write_u32(&mut map, constants.io_info_offset, io_trap_field(PORT, true, 1));
        write_u32(&mut map, constants.rax_offset, 0x5A);
        let data = SaveState::new(*map);

        let mut out = [0xFF; IO_INFO_SIZE];
        assert_eq!(data.read_io_register(&mut out), Ok(()));

        let expected = MmSaveStateIoInfo {
            io_data: 0x5A,
            io_port: PORT,
            io_width: IO_WIDTH_UINT8,
            io_type: IO_TYPE_INPUT,
            ..Default::default()
        };
        assert_eq!(out, expected.as_bytes());
    }

    #[test]
    fn test_read_io_register_rejects_a_missing_trap_and_a_short_buffer() {
        let mut out = [0u8; IO_INFO_SIZE];
        assert_eq!(SaveState::new(*new_map()).read_io_register(&mut out), Err(SaveStateError::NoIoTrap));

        let mut map = new_map();
        write_u32(&mut map, vendor_constants().io_info_offset, io_trap_field(0x70, true, 1));
        assert_eq!(
            SaveState::new(*map).read_io_register(&mut out[..IO_INFO_SIZE - 1]),
            Err(SaveStateError::BufferTooSmall { needed: IO_INFO_SIZE, available: IO_INFO_SIZE - 1 })
        );
    }

    #[test]
    fn test_read_lma_register_widths_and_buffers() {
        let mut map = new_map();
        write_u64(&mut map, vendor_constants().efer_offset, IA32_EFER_LMA);
        let data = SaveState::new(*map);

        let mut out = [0u8; 8];
        assert_eq!(data.read_lma_register(8, &mut out), Ok(()));
        assert_eq!(u64::from_le_bytes(out), LMA_64BIT);

        let mut out = [0u8; 4];
        assert_eq!(data.read_lma_register(4, &mut out), Ok(()));
        assert_eq!(u32::from_le_bytes(out), LMA_64BIT as u32);

        for width in [0, 2, 16] {
            assert_eq!(
                data.read_lma_register(width, &mut [0u8; 16]),
                Err(SaveStateError::UnsupportedWidth { register: MmSaveStateRegister::Lma, width })
            );
        }
        assert_eq!(
            data.read_lma_register(8, &mut [0u8; 4]),
            Err(SaveStateError::BufferTooSmall { needed: 8, available: 4 })
        );
    }

    #[test]
    fn test_save_state_from_smbase_copies_the_map() {
        // The map sits at SMBASE + SMRAM_SAVE_STATE_MAP_OFFSET, so the buffer starts at SMBASE.
        let mut region = vec![0u8; SMRAM_SAVE_STATE_MAP_OFFSET as usize + SMRAM_SAVE_STATE_MAP_SIZE];
        let rax = SMRAM_SAVE_STATE_MAP_OFFSET as usize
            + usize::from(SaveState::register_info(MmSaveStateRegister::Rax).unwrap().lo_offset);
        region[rax..rax + 8].copy_from_slice(&0x1111_2222_3333_4444u64.to_le_bytes());

        // SAFETY: the map of this SMBASE lies inside `region`, which outlives the call.
        let state = unsafe { SaveState::from_smbase(region.as_ptr() as u64) }.expect("the SMBASE is usable");
        let mut out = [0u8; 8];
        assert_eq!(state.read_architectural_register(MmSaveStateRegister::Rax, 8, &mut out), Ok(()));
        assert_eq!(u64::from_le_bytes(out), 0x1111_2222_3333_4444);
    }

    #[test]
    fn test_save_state_from_smbase_rejects_an_unusable_smbase() {
        // A null SMBASE means the CPU was never relocated; the other one puts the map past the
        // end of the address space.
        for smbase in [0, u64::MAX] {
            // SAFETY: the SMBASE is rejected before any read.
            assert_eq!(unsafe { SaveState::from_smbase(smbase) }.err(), Some(SaveStateError::InvalidParameter));
        }
    }

    #[test]
    fn test_save_state_map_base() {
        assert_eq!(SaveState::map_base(0x3000_0000), Ok(0x3000_FC00));
        // The map ends at SMBASE + 0x10000, and that end address must itself fit in a u64.
        assert_eq!(SaveState::map_base(u64::MAX - 0x1_0000), Ok(u64::MAX - 0x400));
        for smbase in [0, u64::MAX - 0xFFFF, u64::MAX] {
            assert_eq!(SaveState::map_base(smbase), Err(SaveStateError::InvalidParameter), "SMBASE {smbase:#x}");
        }
    }

    #[test]
    fn test_save_state_error_renders_each_variant_distinctly() {
        let register = MmSaveStateRegister::Rax;
        let rendered = [
            format!("{}", SaveStateError::IoInfoUnsupported { smm_rev_id: 0x30003 }),
            format!("{}", SaveStateError::InvalidParameter),
            format!("{}", SaveStateError::RegisterNotMapped(register)),
            format!("{}", SaveStateError::UnsupportedWidth { register, width: 3 }),
            format!("{}", SaveStateError::BufferTooSmall { needed: 8, available: 4 }),
            format!("{}", SaveStateError::NoIoTrap),
        ];
        assert_eq!(rendered[0], "SMMRevId 0x30003 does not expose I/O info");
        for (i, left) in rendered.iter().enumerate() {
            for right in rendered.iter().skip(i + 1) {
                assert_ne!(left, right);
            }
        }
    }

    #[test]
    fn test_save_state_data_debug_hides_the_register_contents() {
        let mut map = new_map();
        write_u64(&mut map, vendor_constants().rax_offset, 0x5EC2_E7DA_7A00_0000);
        assert_eq!(format!("{:?}", SaveState::new(*map)), format!("SaveState({VENDOR_NAME:?})"));
    }
}
