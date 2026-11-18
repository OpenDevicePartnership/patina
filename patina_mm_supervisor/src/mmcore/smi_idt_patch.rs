//! SMI Handler IDT Patching
//!
//! Points every core's SMI entry at the IDT the supervisor loaded, by rewriting the IDT
//! descriptor named in the fixup structure of that core's MMI entry. Also defines that fixup
//! structure and its slot indices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use patina::MemoryRange;

use zerocopy::FromBytes;
use zerocopy_derive::Immutable;

use crate::error::MmSupervisorResult;
use crate::intrinsics::{DescriptorTablePointer, read_idtr};
use crate::memory::mmram::is_range_inside_mmram;

/// Offset from SMBASE of the MMI entry code.
pub(crate) const SMM_HANDLER_OFFSET: u64 = 0x8000;

// The header versions and the fixup slot indices below match `SeaResponder.h`.

/// Oldest MMI entry header version whose layout matches [`PerCoreMmiEntryStructHdr`].
const MMI_ENTRY_STRUCT_V4: u32 = 4;

/// Header version of the V5 entry code. Version 5 keeps the version 4 header and IDTR slot, so
/// the patch accepts either, but per-core validation requires this one.
pub(crate) const MMI_ENTRY_STRUCT_VERSION: u32 = 5;

/// Fixup64 slot for `CpuSmmDebugEntry`. Not used by the V5 entry code.
pub(crate) const FIXUP64_SMM_DBG_ENTRY: usize = 0;
/// Fixup64 slot for `CpuSmmDebugExit`. Not used by the V5 entry code.
pub(crate) const FIXUP64_SMM_DBG_EXIT: usize = 1;
/// Fixup64 slot for the routine the entry code calls on every SMI.
pub(crate) const FIXUP64_SMI_RDZ_ENTRY: usize = 2;
/// Fixup64 slot for the address of `mXdSupported`. Not used by the V5 entry code.
pub(crate) const FIXUP64_XD_SUPPORTED: usize = 3;
/// Fixup64 slot for the address of `mCetSupported`. Not used by the V5 entry code.
pub(crate) const FIXUP64_CET_SUPPORTED: usize = 4;
/// Fixup64 slot for the address of the SMI handler's IDT descriptor.
pub(crate) const FIXUP64_SMI_HANDLER_IDTR: usize = 5;
/// Fixup64 slot for the HOB list the entry code passes to the supervisor.
pub(crate) const FIXUP64_HOB_START: usize = 6;

/// Fixup32 slot for the CET Ring 0 shadow stack pointer.
pub(crate) const FIXUP32_CET_PL0_SSP: usize = 0;
/// Fixup32 slot for the address of the `IA32_DESCRIPTOR` the entry code loads into GDTR.
pub(crate) const FIXUP32_GDTR: usize = 1;
/// Fixup32 slot for the CR3 the entry code loads.
pub(crate) const FIXUP32_CR3: usize = 2;
/// Fixup32 slot for the CET interrupt shadow stack pointer.
pub(crate) const FIXUP32_CET_INTERRUPT_SSP: usize = 3;
/// Fixup32 slot for the CET interrupt shadow stack table.
pub(crate) const FIXUP32_CET_INTERRUPT_SSP_TABLE: usize = 4;
/// Fixup32 slot for the top of the Ring 0 stack the entry code switches to.
pub(crate) const FIXUP32_STACK_CPL0: usize = 5;
/// Fixup32 slot for the SMBASE the entry code was copied for.
pub(crate) const FIXUP32_MSR_SMM_BASE: usize = 6;

/// Fixup8 flag: enable NX (`EFER.NXE`) on entry.
pub(crate) const FIXUP8_XD_SUPPORTED: usize = 0;
/// Fixup8 flag: `MSR_IA32_MISC_ENABLE` exists, so its XD Disable bit is cleared on entry.
pub(crate) const FIXUP8_MISC_ENABLE_SUPPORTED: usize = 1;
/// Fixup8 flag: set CR4.LA57 on entry.
pub(crate) const FIXUP8_5_LEVEL_PAGING: usize = 2;
/// Fixup8 flag: enable CET on entry.
pub(crate) const FIXUP8_CET_SUPPORTED: usize = 3;
/// First reserved Fixup8 slot. Every slot from here to the end of the array is reserved.
pub(crate) const FIXUP8_FIRST_RESERVED: usize = 4;

/// Errors produced while locating and patching a core's SMI handler IDT descriptor.
///
/// Any of them fails supervisor initialization, because an unpatched core would take its next
/// SMI through an IDT the supervisor does not control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmiHandlerIdtPatchError {
    /// The MMI entry size is zero.
    ZeroEntrySize,
    /// The SMBASE array pointer is null, or no CPUs were reported.
    MissingSmBaseArray,
    /// The SMBASE array size overflows, so the array cannot be addressed.
    SmBaseArraySizeOverflow,
    /// The SMBASE array is not entirely inside MMRAM.
    SmBaseArrayOutsideMmram,
    /// The MMI entry size does not fit the target architecture.
    EntrySizeTooLarge,
    /// A core's SMBASE is zero, so it was never relocated.
    SmBaseZero {
        /// Index of the core in the SMBASE array.
        cpu: usize,
    },
    /// A core's MMI entry does not lie entirely inside MMRAM, or its address overflows.
    MmiEntryOutsideMmram {
        /// Index of the core in the SMBASE array.
        cpu: usize,
    },
    /// A core's Fixup64 IDTR slot is null.
    IdtDescriptorNull {
        /// Index of the core in the SMBASE array.
        cpu: usize,
    },
    /// A core's IDT descriptor is not entirely inside MMRAM.
    IdtDescriptorOutsideMmram {
        /// Index of the core in the SMBASE array.
        cpu: usize,
    },
    /// The MMI entry is too small to hold the trailing structure-size field.
    EntryTooSmall,
    /// The fixup structure the trailer points at lies outside the MMI entry.
    FixupStructureOutOfBounds,
    /// The fixup structure is smaller than its own header.
    FixupHeaderTooSmall,
    /// The fixup structure header has a version whose layout the supervisor does not know.
    UnsupportedHeaderVersion {
        /// Header version the MMI entry reported.
        found: u32,
    },
    /// A fixup array holds fewer entries than the slot being read requires.
    FixupArrayTooSmall {
        /// Width in bits of the array's entries: 64, 32 or 8.
        width: u8,
        /// Index of the slot being read.
        index: usize,
        /// Number of entries the header reported.
        found: u8,
    },
    /// The addressed fixup entry lies outside the MMI entry.
    FixupEntryOutOfBounds {
        /// Width in bits of the array's entries: 64, 32 or 8.
        width: u8,
        /// Index of the slot being read.
        index: usize,
    },
}

impl core::error::Error for SmiHandlerIdtPatchError {}

impl fmt::Display for SmiHandlerIdtPatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroEntrySize => write!(f, "the MMI entry size is zero"),
            Self::MissingSmBaseArray => write!(f, "the SMBASE array pointer is null, or no CPUs were reported"),
            Self::SmBaseArraySizeOverflow => {
                write!(f, "the SMBASE array size overflows, so the array cannot be addressed")
            }
            Self::SmBaseArrayOutsideMmram => write!(f, "the SMBASE array is not entirely inside MMRAM"),
            Self::EntrySizeTooLarge => write!(f, "the MMI entry size does not fit the target architecture"),
            Self::SmBaseZero { cpu } => write!(f, "CPU {cpu}: SMBASE is 0, so the core was never relocated"),
            Self::MmiEntryOutsideMmram { cpu } => write!(f, "CPU {cpu}: the MMI entry is not inside MMRAM"),
            Self::IdtDescriptorNull { cpu } => write!(f, "CPU {cpu}: the Fixup64 IDTR slot is null"),
            Self::IdtDescriptorOutsideMmram { cpu } => write!(f, "CPU {cpu}: the IDT descriptor is not inside MMRAM"),
            Self::EntryTooSmall => write!(f, "the MMI entry is too small to hold its trailing structure-size field"),
            Self::FixupStructureOutOfBounds => {
                write!(f, "the fixup structure the MMI entry trailer points at lies outside the entry")
            }
            Self::FixupHeaderTooSmall => write!(f, "the fixup structure is smaller than its own header"),
            Self::UnsupportedHeaderVersion { found } => write!(
                f,
                "the fixup structure header version {found} is not between {MMI_ENTRY_STRUCT_V4} and \
                 {MMI_ENTRY_STRUCT_VERSION}"
            ),
            Self::FixupArrayTooSmall { width, index, found } => {
                write!(f, "the Fixup{width} array holds {found} entries, too few to read slot {index}")
            }
            Self::FixupEntryOutOfBounds { width, index } => {
                write!(f, "Fixup{width} slot {index} lies outside the MMI entry")
            }
        }
    }
}

/// Header of the fixup structure embedded at the end of each per-core MMI entry.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub(crate) struct PerCoreMmiEntryStructHdr {
    /// Header version, from [`MMI_ENTRY_STRUCT_V4`] to [`MMI_ENTRY_STRUCT_VERSION`].
    pub(crate) header_version: u32,
    /// Offset from header start to `FixUpStruct` array.
    pub(crate) fixup_struct_offset: u8,
    /// Number of `FixUpStruct` array entries.
    pub(crate) fixup_struct_num: u8,
    /// Offset from header start to Fixup64 array.
    pub(crate) fixup64_offset: u8,
    /// Number of Fixup64 array entries.
    pub(crate) fixup64_num: u8,
    /// Offset from header start to Fixup32 array.
    pub(crate) fixup32_offset: u8,
    /// Number of Fixup32 array entries.
    pub(crate) fixup32_num: u8,
    /// Offset from header start to Fixup8 array.
    pub(crate) fixup8_offset: u8,
    /// Number of Fixup8 array entries.
    pub(crate) fixup8_num: u8,
    /// SMI entry binary version.
    pub(crate) binary_version: u16,
    /// SPL value for SMI entry binary.
    pub(crate) spl_value: u32,
    /// Reserved for future use.
    pub(crate) reserved: u32,
}

/// The checked patch inputs, in the widths the per-CPU loop indexes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SmiHandlerIdtPatchInputs {
    /// Byte length of the SMBASE array, known to be addressable and inside MMRAM.
    pub(crate) sm_base_array_size: usize,
    /// MMI entry size as a length usable for slicing.
    pub(crate) mmi_entry_size: usize,
}

/// Checks the patch inputs before any core's MMI entry is read.
///
/// Checks:
/// - `mmi_entry_size` is not zero and fits an `isize`.
/// - `sm_base_array` is not null and `number_of_cores` is not zero.
/// - The array's byte size fits an `isize`.
/// - The array lies inside MMRAM.
///
/// `is_inside_mmram` is a parameter so tests can run without a live page allocator.
fn validate_smi_handler_idt_patch_inputs(
    sm_base_array: u64,
    number_of_cores: usize,
    mmi_entry_size: u64,
    is_inside_mmram: impl Fn(&MemoryRange) -> bool,
) -> MmSupervisorResult<SmiHandlerIdtPatchInputs> {
    if mmi_entry_size == 0 {
        return Err(SmiHandlerIdtPatchError::ZeroEntrySize.into());
    }
    if sm_base_array == 0 || number_of_cores == 0 {
        return Err(SmiHandlerIdtPatchError::MissingSmBaseArray.into());
    }

    let sm_base_array_size = number_of_cores
        .checked_mul(core::mem::size_of::<u64>())
        .filter(|size| isize::try_from(*size).is_ok())
        .ok_or(SmiHandlerIdtPatchError::SmBaseArraySizeOverflow)?;
    let sm_base_array_size_u64 =
        u64::try_from(sm_base_array_size).map_err(|_| SmiHandlerIdtPatchError::SmBaseArraySizeOverflow)?;
    if !MemoryRange::new(sm_base_array, sm_base_array_size_u64).is_ok_and(|range| is_inside_mmram(&range)) {
        return Err(SmiHandlerIdtPatchError::SmBaseArrayOutsideMmram.into());
    }

    let mmi_entry_size_usize = usize::try_from(mmi_entry_size)
        .ok()
        .filter(|size| isize::try_from(*size).is_ok())
        .ok_or(SmiHandlerIdtPatchError::EntrySizeTooLarge)?;

    Ok(SmiHandlerIdtPatchInputs { sm_base_array_size, mmi_entry_size: mmi_entry_size_usize })
}

/// The fixup structure at the end of one core's MMI entry.
///
/// The entry ends with a `u32` giving the size of the structure before it, which starts with a
/// [`PerCoreMmiEntryStructHdr`] locating the Fixup64, Fixup32 and Fixup8 arrays. Every offset and
/// count comes from the producer, so each read is bounds-checked against the entry.
pub(crate) struct MmiEntryFixups<'a> {
    /// The fixup structure, from its header up to the trailing size field.
    structure: &'a [u8],
    /// The structure's header.
    header: PerCoreMmiEntryStructHdr,
}

impl<'a> MmiEntryFixups<'a> {
    /// Locates the fixup structure at the end of `mmi_entry` and reads its header, which must be
    /// a version from [`MMI_ENTRY_STRUCT_V4`] to [`MMI_ENTRY_STRUCT_VERSION`].
    pub(crate) fn parse(mmi_entry: &'a [u8]) -> MmSupervisorResult<Self> {
        const TRAILING_SIZE_FIELD_SIZE: usize = core::mem::size_of::<u32>();

        let trailer_start =
            mmi_entry.len().checked_sub(TRAILING_SIZE_FIELD_SIZE).ok_or(SmiHandlerIdtPatchError::EntryTooSmall)?;
        let trailer = mmi_entry.get(trailer_start..).ok_or(SmiHandlerIdtPatchError::EntryTooSmall)?;
        let whole_struct_size =
            u32::from_ne_bytes(trailer.try_into().map_err(|_| SmiHandlerIdtPatchError::EntryTooSmall)?) as usize;
        let header_start =
            trailer_start.checked_sub(whole_struct_size).ok_or(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)?;
        let structure =
            mmi_entry.get(header_start..trailer_start).ok_or(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)?;
        let (header, _) = PerCoreMmiEntryStructHdr::read_from_prefix(structure)
            .map_err(|_| SmiHandlerIdtPatchError::FixupHeaderTooSmall)?;

        let header_version = header.header_version;
        if !(MMI_ENTRY_STRUCT_V4..=MMI_ENTRY_STRUCT_VERSION).contains(&header_version) {
            return Err(SmiHandlerIdtPatchError::UnsupportedHeaderVersion { found: header_version }.into());
        }

        Ok(Self { structure, header })
    }

    /// Returns the structure's header.
    pub(crate) fn header(&self) -> &PerCoreMmiEntryStructHdr {
        &self.header
    }

    /// Returns Fixup64 slot `index`.
    pub(crate) fn fixup64(&self, index: usize) -> MmSupervisorResult<u64> {
        self.entry(64, self.header.fixup64_offset, self.header.fixup64_num, index).map(u64::from_ne_bytes)
    }

    /// Returns Fixup32 slot `index`.
    pub(crate) fn fixup32(&self, index: usize) -> MmSupervisorResult<u32> {
        self.entry(32, self.header.fixup32_offset, self.header.fixup32_num, index).map(u32::from_ne_bytes)
    }

    /// Returns Fixup8 slot `index`.
    pub(crate) fn fixup8(&self, index: usize) -> MmSupervisorResult<u8> {
        self.entry(8, self.header.fixup8_offset, self.header.fixup8_num, index).map(u8::from_ne_bytes)
    }

    /// Returns the bytes of slot `index` of the array of `N`-byte entries that starts `offset`
    /// bytes into the structure and holds `count` entries.
    fn entry<const N: usize>(&self, width: u8, offset: u8, count: u8, index: usize) -> MmSupervisorResult<[u8; N]> {
        if index >= usize::from(count) {
            return Err(SmiHandlerIdtPatchError::FixupArrayTooSmall { width, index, found: count }.into());
        }
        let out_of_bounds = SmiHandlerIdtPatchError::FixupEntryOutOfBounds { width, index };
        // `index` is below a `u8` count and `N` is at most 8, so neither product can overflow.
        let start = usize::from(offset) + index * N;
        let bytes = self.structure.get(start..start + N).ok_or(out_of_bounds)?;
        Ok(bytes.try_into().map_err(|_| out_of_bounds)?)
    }
}

/// Returns the `mmi_entry_size` bytes of MMI entry at `smbase + SMM_HANDLER_OFFSET`, which must
/// lie entirely inside MMRAM. `cpu_index` names the core in the error.
///
/// ## Safety
///
/// The entry must stay readable and unmodified for as long as the returned slice is used.
pub(crate) unsafe fn locate_mmi_entry<'a>(
    cpu_index: usize,
    smbase: u64,
    mmi_entry_size: usize,
) -> MmSupervisorResult<&'a [u8]> {
    let base = smbase
        .checked_add(SMM_HANDLER_OFFSET)
        .filter(|base| {
            u64::try_from(mmi_entry_size)
                .ok()
                .and_then(|size| MemoryRange::new(*base, size).ok())
                .is_some_and(|range| is_range_inside_mmram(&range))
        })
        .ok_or(SmiHandlerIdtPatchError::MmiEntryOutsideMmram { cpu: cpu_index })?;
    // SAFETY: the range check above places the entry entirely inside MMRAM, and the caller
    // guarantees it stays readable while the slice is used.
    Ok(unsafe { core::slice::from_raw_parts(base as *const u8, mmi_entry_size) })
}

/// Patches every core's SMI handler IDT descriptor to point to the IDT loaded on this core.
///
/// Each core's MMI entry at `SMBASE + SMM_HANDLER_OFFSET` holds, in
/// `Fixup64[FIXUP64_SMI_HANDLER_IDTR]`, the address of the descriptor its entry code loads with
/// `lidt`. `sm_base_array` is the `PassDown` HOB's array of `number_of_cores` SMBASE values.
///
/// ## Errors
///
/// Returns a [`SmiHandlerIdtPatchError`] if the arguments are invalid, or at the first core
/// whose SMBASE, MMI entry, or IDT descriptor cannot be used. Earlier cores stay patched.
pub(crate) fn patch_smi_handler_idt(
    sm_base_array: u64,
    number_of_cores: usize,
    mmi_entry_size: u64,
) -> MmSupervisorResult<()> {
    let inputs =
        validate_smi_handler_idt_patch_inputs(sm_base_array, number_of_cores, mmi_entry_size, is_range_inside_mmram)?;

    let idtr = read_idtr();
    // Copied out of the packed struct, since formatting a packed field takes a reference.
    let idtr_base = idtr.base;
    let idtr_limit = idtr.limit;

    // Read as bytes, since the producer's array may be unaligned for `u64`.
    // SAFETY: `sm_base_array` names `sm_base_array_size` bytes in MMRAM, as validated above,
    // which stay resident during initialization.
    let sm_base_bytes = unsafe { core::slice::from_raw_parts(sm_base_array as *const u8, inputs.sm_base_array_size) };

    let (smbase_chunks, _) = sm_base_bytes.as_chunks::<{ core::mem::size_of::<u64>() }>();
    for (cpu, smbase_bytes) in smbase_chunks.iter().enumerate() {
        let smbase = u64::from_ne_bytes(*smbase_bytes);
        if smbase == 0 {
            return Err(SmiHandlerIdtPatchError::SmBaseZero { cpu }.into());
        }

        // SAFETY: `locate_mmi_entry` checks that the entry lies inside MMRAM, which stays
        // resident during initialization.
        let mmi_entry = unsafe { locate_mmi_entry(cpu, smbase, inputs.mmi_entry_size) }?;
        let idt_desc_addr = MmiEntryFixups::parse(mmi_entry)
            .and_then(|fixups| fixups.fixup64(FIXUP64_SMI_HANDLER_IDTR))
            .inspect_err(|error| log::error!("CPU {cpu}: invalid SMI handler fixup metadata: {error}"))?;

        if idt_desc_addr == 0 {
            return Err(SmiHandlerIdtPatchError::IdtDescriptorNull { cpu }.into());
        }
        let idt_desc = MemoryRange::new(idt_desc_addr, core::mem::size_of::<DescriptorTablePointer>() as u64);
        if !idt_desc.is_ok_and(|range| is_range_inside_mmram(&range)) {
            return Err(SmiHandlerIdtPatchError::IdtDescriptorOutsideMmram { cpu }.into());
        }

        // SAFETY: the range check above places a whole descriptor in MMRAM.
        unsafe { core::ptr::write_unaligned(idt_desc_addr as *mut DescriptorTablePointer, idtr) };

        log::debug!(
            "CPU {cpu}: patched SMI handler IDT descriptor at 0x{idt_desc_addr:016x}: base=0x{idtr_base:016x}, limit=0x{idtr_limit:04x}"
        );
    }

    log::info!(
        "Patched the SMI handler IDT descriptor on {number_of_cores} CPU(s): base=0x{idtr_base:016x}, limit=0x{idtr_limit:04x}"
    );
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::mem::size_of;

    use serial_test::serial;

    use crate::memory;
    use crate::state::security_state;
    use crate::test_support::{self, PageAlignedMemory, init_global_state_over, mmi_entry};

    #[test]
    fn test_smi_handler_idt_patch_errors_render_each_variant_distinctly() {
        // Both the arguments and the MMI entry come from outside the supervisor, so the message
        // is what tells a reader which check rejected the patch. The table is exhaustive, so a
        // variant added without a Display arm fails here rather than rendering as another
        // variant's text.
        let all = [
            (SmiHandlerIdtPatchError::ZeroEntrySize, "the MMI entry size is zero"),
            (SmiHandlerIdtPatchError::MissingSmBaseArray, "the SMBASE array pointer is null, or no CPUs were reported"),
            (
                SmiHandlerIdtPatchError::SmBaseArraySizeOverflow,
                "the SMBASE array size overflows, so the array cannot be addressed",
            ),
            (SmiHandlerIdtPatchError::SmBaseArrayOutsideMmram, "the SMBASE array is not entirely inside MMRAM"),
            (SmiHandlerIdtPatchError::EntrySizeTooLarge, "the MMI entry size does not fit the target architecture"),
            (SmiHandlerIdtPatchError::SmBaseZero { cpu: 1 }, "CPU 1: SMBASE is 0, so the core was never relocated"),
            (SmiHandlerIdtPatchError::MmiEntryOutsideMmram { cpu: 1 }, "CPU 1: the MMI entry is not inside MMRAM"),
            (SmiHandlerIdtPatchError::IdtDescriptorNull { cpu: 1 }, "CPU 1: the Fixup64 IDTR slot is null"),
            (
                SmiHandlerIdtPatchError::IdtDescriptorOutsideMmram { cpu: 1 },
                "CPU 1: the IDT descriptor is not inside MMRAM",
            ),
            (
                SmiHandlerIdtPatchError::EntryTooSmall,
                "the MMI entry is too small to hold its trailing structure-size field",
            ),
            (
                SmiHandlerIdtPatchError::FixupStructureOutOfBounds,
                "the fixup structure the MMI entry trailer points at lies outside the entry",
            ),
            (SmiHandlerIdtPatchError::FixupHeaderTooSmall, "the fixup structure is smaller than its own header"),
            (
                SmiHandlerIdtPatchError::UnsupportedHeaderVersion { found: 6 },
                "the fixup structure header version 6 is not between 4 and 5",
            ),
            (
                SmiHandlerIdtPatchError::FixupArrayTooSmall { width: 64, index: 5, found: 2 },
                "the Fixup64 array holds 2 entries, too few to read slot 5",
            ),
            (
                SmiHandlerIdtPatchError::FixupEntryOutOfBounds { width: 32, index: 1 },
                "Fixup32 slot 1 lies outside the MMI entry",
            ),
        ];

        for (error, expected) in all {
            assert_eq!(format!("{error}"), expected);
        }

        // The count is carried in the message, so two short arrays stay distinguishable.
        assert_ne!(
            format!("{}", SmiHandlerIdtPatchError::FixupArrayTooSmall { width: 64, index: 5, found: 2 }),
            format!("{}", SmiHandlerIdtPatchError::FixupArrayTooSmall { width: 64, index: 5, found: 3 })
        );
    }

    #[test]
    fn test_validate_smi_handler_idt_patch_inputs() {
        let inputs = validate_smi_handler_idt_patch_inputs(0x1000, 4, 0x200, |range| {
            *range == MemoryRange::new(0x1000, 4 * size_of::<u64>() as u64).unwrap()
        })
        .expect("valid patch inputs should pass");

        assert_eq!(
            inputs,
            SmiHandlerIdtPatchInputs { sm_base_array_size: 4 * size_of::<u64>(), mmi_entry_size: 0x200 }
        );
    }

    #[test]
    fn test_validate_smi_handler_idt_patch_inputs_rejects_invalid_values() {
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 1, 0, |_| true),
            Err(SmiHandlerIdtPatchError::ZeroEntrySize.into())
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0, 1, 0x100, |_| true),
            Err(SmiHandlerIdtPatchError::MissingSmBaseArray.into())
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 0, 0x100, |_| true),
            Err(SmiHandlerIdtPatchError::MissingSmBaseArray.into())
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, usize::MAX, 0x100, |_| true),
            Err(SmiHandlerIdtPatchError::SmBaseArraySizeOverflow.into())
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 1, 0x100, |_| false),
            Err(SmiHandlerIdtPatchError::SmBaseArrayOutsideMmram.into())
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(u64::MAX - 3, 1, 0x100, |_| true),
            Err(SmiHandlerIdtPatchError::SmBaseArrayOutsideMmram.into())
        );
        assert_eq!(
            validate_smi_handler_idt_patch_inputs(0x1000, 1, isize::MAX as u64 + 1, |_| true),
            Err(SmiHandlerIdtPatchError::EntrySizeTooLarge.into())
        );
    }

    /// Returns the Fixup64 IDTR slot of `entry`, as the patch reads it.
    fn idt_descriptor_slot(entry: &[u8]) -> MmSupervisorResult<u64> {
        MmiEntryFixups::parse(entry)?.fixup64(FIXUP64_SMI_HANDLER_IDTR)
    }

    #[test]
    fn test_mmi_entry_fixups_read_each_array() {
        let entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0x1234_5678_9ABC_DEF0);
        assert_eq!(idt_descriptor_slot(&entry), Ok(0x1234_5678_9ABC_DEF0));

        let entry = test_support::mmi_entry_with(&[1, 2], &[3, 4, 5], &[6, 7]);
        let fixups = MmiEntryFixups::parse(&entry).expect("a well-formed fixup structure");
        let version = fixups.header().header_version;
        assert_eq!(version, MMI_ENTRY_STRUCT_VERSION);
        assert_eq!((fixups.fixup64(0), fixups.fixup64(1)), (Ok(1), Ok(2)));
        assert_eq!((fixups.fixup32(0), fixups.fixup32(2)), (Ok(3), Ok(5)));
        assert_eq!((fixups.fixup8(0), fixups.fixup8(1)), (Ok(6), Ok(7)));
        assert_eq!(
            fixups.fixup32(3),
            Err(SmiHandlerIdtPatchError::FixupArrayTooSmall { width: 32, index: 3, found: 3 }.into())
        );
        assert_eq!(
            fixups.fixup8(2),
            Err(SmiHandlerIdtPatchError::FixupArrayTooSmall { width: 8, index: 2, found: 2 }.into())
        );
    }

    #[test]
    fn test_mmi_entry_fixups_check_the_header_version() {
        const HEADER_START: usize = 8;
        let mut entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0x1234_5678_9ABC_DEF0);

        // Version 5 keeps the version 4 header, so the IDTR slot is read the same way.
        for found in [4_u32, 5] {
            entry[HEADER_START..HEADER_START + 4].copy_from_slice(&found.to_ne_bytes());
            assert_eq!(idt_descriptor_slot(&entry), Ok(0x1234_5678_9ABC_DEF0), "version {found}");
        }

        for found in [0, 3, 6, u32::MAX] {
            entry[HEADER_START..HEADER_START + 4].copy_from_slice(&found.to_ne_bytes());
            assert_eq!(
                idt_descriptor_slot(&entry),
                Err(SmiHandlerIdtPatchError::UnsupportedHeaderVersion { found }.into())
            );
        }
    }

    #[test]
    fn test_mmi_entry_fixups_reject_malformed_metadata() {
        assert_eq!(idt_descriptor_slot(&[0; 3]), Err(SmiHandlerIdtPatchError::EntryTooSmall.into()));

        let mut oversized_structure = [0_u8; 4];
        oversized_structure.copy_from_slice(&1_u32.to_ne_bytes());
        assert_eq!(
            idt_descriptor_slot(&oversized_structure),
            Err(SmiHandlerIdtPatchError::FixupStructureOutOfBounds.into())
        );

        let mut short_header = vec![0_u8; 5];
        short_header[1..].copy_from_slice(&1_u32.to_ne_bytes());
        assert_eq!(idt_descriptor_slot(&short_header), Err(SmiHandlerIdtPatchError::FixupHeaderTooSmall.into()));

        let too_few_fixups = mmi_entry(FIXUP64_SMI_HANDLER_IDTR as u8, 0);
        assert_eq!(
            idt_descriptor_slot(&too_few_fixups),
            Err(SmiHandlerIdtPatchError::FixupArrayTooSmall {
                width: 64,
                index: FIXUP64_SMI_HANDLER_IDTR,
                found: FIXUP64_SMI_HANDLER_IDTR as u8
            }
            .into())
        );

        let mut out_of_bounds_fixup = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0);
        out_of_bounds_fixup[8 + 6] = u8::MAX;
        assert_eq!(
            idt_descriptor_slot(&out_of_bounds_fixup),
            Err(SmiHandlerIdtPatchError::FixupEntryOutOfBounds { width: 64, index: FIXUP64_SMI_HANDLER_IDTR }.into())
        );
    }

    #[test]
    fn test_mmi_entry_header_layout_matches_c_abi() {
        assert_eq!(size_of::<PerCoreMmiEntryStructHdr>(), 22);
    }

    /// Pages reserved inside the test MMRAM region, laid out the way the C relocation code
    /// leaves one core: the MMI entry sits at `smbase + SMM_HANDLER_OFFSET`, so the block has
    /// to be longer than that offset.
    const CPU_BLOCK_PAGES: usize = 16;

    /// Reserves one core's block inside MMRAM and writes `entry` where the patch expects it.
    fn place_cpu_block(entry: &[u8]) -> u64 {
        let smbase = security_state().page_allocator().allocate_pages(CPU_BLOCK_PAGES).expect("a CPU block in MMRAM");
        // SAFETY: the block is a live allocation of CPU_BLOCK_PAGES pages, and the entry ends
        // well inside it because SMM_HANDLER_OFFSET covers only the first eight pages.
        unsafe {
            core::ptr::copy_nonoverlapping(entry.as_ptr(), (smbase + SMM_HANDLER_OFFSET) as *mut u8, entry.len());
        }
        smbase
    }

    /// Writes `bases` into a page inside MMRAM and returns its address.
    fn place_sm_base_array(bases: &[u64]) -> u64 {
        let array = security_state().page_allocator().allocate_pages(1).expect("an SMBASE array page in MMRAM");
        // SAFETY: `array` is a live page, and one page holds far more than the handful of
        // entries any of these tests uses.
        unsafe { core::ptr::copy_nonoverlapping(bases.as_ptr(), array as *mut u64, bases.len()) };
        array
    }

    /// Reserves a descriptor slot inside MMRAM and fills it so a patch is visible as a change.
    fn place_descriptor() -> u64 {
        let descriptor = security_state().page_allocator().allocate_pages(1).expect("a descriptor page in MMRAM");
        // SAFETY: `descriptor` is a live page and a DescriptorTablePointer is 10 bytes.
        unsafe { core::ptr::write_bytes(descriptor as *mut u8, 0xFF, size_of::<DescriptorTablePointer>()) };
        descriptor
    }

    /// Reads back a descriptor the patch may have rewritten, as `(limit, base)`.
    ///
    /// The fields are copied out rather than compared in place, because the struct is
    /// `packed(2)` and a reference to a field of a packed struct does not compile (E0793).
    fn read_descriptor(address: u64) -> (u16, u64) {
        // SAFETY: the address came from `place_descriptor`, so it is a live, initialized slot.
        let descriptor = unsafe { core::ptr::read_unaligned(address as *const DescriptorTablePointer) };
        (descriptor.limit, descriptor.base)
    }

    #[test]
    #[serial]
    fn test_patch_smi_handler_idt_writes_the_supervisor_idt_to_every_cpu() {
        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 64);
        init_global_state_over(&memory);

        let descriptors = [place_descriptor(), place_descriptor()];
        let entries: Vec<Vec<u8>> =
            descriptors.iter().map(|&descriptor| mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, descriptor)).collect();
        let bases: Vec<u64> = entries.iter().map(|entry| place_cpu_block(entry)).collect();
        let sm_base_array = place_sm_base_array(&bases);

        patch_smi_handler_idt(sm_base_array, bases.len(), entries[0].len() as u64).expect("every core can be patched");

        // `read_idtr` reads as zero in a host test, so a patched slot is the one that no longer
        // holds the 0xFF fill. Both cores are patched from the same IDTR.
        for descriptor in descriptors {
            assert_eq!(read_descriptor(descriptor), (0, 0));
        }
    }

    #[test]
    #[serial]
    fn test_patch_smi_handler_idt_rejects_bad_arguments_before_touching_any_core() {
        test_support::init_test_logger();
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 64);
        init_global_state_over(&memory);

        let descriptor = place_descriptor();
        let entry = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, descriptor);
        let smbase = place_cpu_block(&entry);
        let sm_base_array = place_sm_base_array(&[smbase]);
        let untouched = read_descriptor(descriptor);

        // A zero entry size, a null array, a CPU count of zero and an array outside MMRAM are
        // all rejected by the up-front check, so no core's entry is read.
        let len = entry.len() as u64;
        assert_eq!(patch_smi_handler_idt(sm_base_array, 1, 0), Err(SmiHandlerIdtPatchError::ZeroEntrySize.into()));
        assert_eq!(patch_smi_handler_idt(0, 1, len), Err(SmiHandlerIdtPatchError::MissingSmBaseArray.into()));
        assert_eq!(
            patch_smi_handler_idt(sm_base_array, 0, len),
            Err(SmiHandlerIdtPatchError::MissingSmBaseArray.into())
        );
        assert_eq!(patch_smi_handler_idt(0x1000, 1, len), Err(SmiHandlerIdtPatchError::SmBaseArrayOutsideMmram.into()));

        assert_eq!(read_descriptor(descriptor), untouched);
    }

    #[test]
    #[serial]
    fn test_patch_smi_handler_idt_fails_on_a_core_it_cannot_patch() {
        test_support::init_test_logger();
        // Three bad core blocks, plus a descriptor, a core block and an array for each case.
        let memory = PageAlignedMemory::new(memory::DEFAULT_PAGING_POOL_PAGES + 192);
        init_global_state_over(&memory);

        let entry_len = mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0).len() as u64;
        // A core whose fixup structure does not parse.
        let malformed = place_cpu_block(&vec![0_u8; entry_len as usize]);
        // A core whose Fixup64 slot is null.
        let null_slot = place_cpu_block(&mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0));
        // A core whose Fixup64 slot points outside MMRAM.
        let outside = place_cpu_block(&mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, 0x1000));

        let cases = [
            (0, SmiHandlerIdtPatchError::SmBaseZero { cpu: 1 }),
            (u64::MAX - SMM_HANDLER_OFFSET + 1, SmiHandlerIdtPatchError::MmiEntryOutsideMmram { cpu: 1 }),
            (0x1000, SmiHandlerIdtPatchError::MmiEntryOutsideMmram { cpu: 1 }),
            (malformed, SmiHandlerIdtPatchError::FixupHeaderTooSmall),
            (null_slot, SmiHandlerIdtPatchError::IdtDescriptorNull { cpu: 1 }),
            (outside, SmiHandlerIdtPatchError::IdtDescriptorOutsideMmram { cpu: 1 }),
        ];

        for (bad_smbase, expected) in cases {
            // The healthy core in front is patched, and the bad one behind it fails the patch.
            let good_descriptor = place_descriptor();
            let good_smbase = place_cpu_block(&mmi_entry((FIXUP64_SMI_HANDLER_IDTR + 1) as u8, good_descriptor));
            let sm_base_array = place_sm_base_array(&[good_smbase, bad_smbase]);

            assert_eq!(patch_smi_handler_idt(sm_base_array, 2, entry_len), Err(expected.into()));
            assert_eq!(read_descriptor(good_descriptor), (0, 0));
        }
    }
}
