//! SMI Handler IDT Patching
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

/// Offset from SMBASE where the SMI handler code is located.
pub(crate) const SMM_HANDLER_OFFSET: u64 = 0x8000;

/// Index into the Fixup64 array for the SMI handler IDTR pointer.
pub(crate) const FIXUP64_SMI_HANDLER_IDTR: usize = 5;

/// Errors produced while applying the SMI handler IDT patch.
///
/// The first group rejects the arguments the caller supplied, and stops the patch before any
/// core is touched. The second group rejects the fixup structure the producer wrote into one
/// core's MMI entry, and skips that core. Both are reported the same way, by
/// [`patch_smi_handler_idt`], so they share one type.
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
    /// The MMI entry is too small to hold the trailing structure-size field.
    EntryTooSmall,
    /// The fixup structure the trailer points at lies outside the MMI entry.
    FixupStructureOutOfBounds,
    /// The fixup structure is smaller than its own header.
    FixupHeaderTooSmall,
    /// The Fixup64 array holds fewer entries than the IDTR slot index requires.
    Fixup64ArrayTooSmall {
        /// Number of Fixup64 entries the header reported.
        found: u8,
    },
    /// The addressed Fixup64 entry lies outside the MMI entry.
    Fixup64EntryOutOfBounds,
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
            Self::EntryTooSmall => write!(f, "the MMI entry is too small to hold its trailing structure-size field"),
            Self::FixupStructureOutOfBounds => {
                write!(f, "the fixup structure the MMI entry trailer points at lies outside the entry")
            }
            Self::FixupHeaderTooSmall => write!(f, "the fixup structure is smaller than its own header"),
            Self::Fixup64ArrayTooSmall { found } => {
                write!(f, "the Fixup64 array holds {found} entries, too few for the IDTR slot the supervisor patches")
            }
            Self::Fixup64EntryOutOfBounds => write!(f, "the addressed Fixup64 entry lies outside the MMI entry"),
        }
    }
}

/// Header of the fixup structure embedded at the end of each per-core MMI entry.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub(crate) struct PerCoreMmiEntryStructHdr {
    /// Header version (4 for version 4).
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

/// The patch inputs after they were checked, narrowed to the widths the loop needs.
///
/// Produced by [`validate_smi_handler_idt_patch_inputs`] so the loop over CPUs does no
/// conversion of its own: it has the array length and the entry size in both the pointer width it
/// indexes with and the `u64` it does address arithmetic with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SmiHandlerIdtPatchInputs {
    /// Byte length of the SMBASE array, known to be addressable and inside MMRAM.
    pub(crate) sm_base_array_size: usize,
    /// MMI entry size as a length usable for slicing.
    pub(crate) mmi_entry_size: usize,
    /// The same entry size kept as a `u64` for address arithmetic.
    pub(crate) mmi_entry_size_u64: u64,
}

/// Checks the patch inputs before any core's MMI entry is read.
///
/// `is_inside_mmram` is taken as a parameter so the check can be driven in a test without a live
/// page allocator; production passes
/// [`is_range_inside_mmram`](crate::memory::mmram::is_range_inside_mmram).
fn validate_smi_handler_idt_patch_inputs(
    sm_base_array: u64,
    number_of_cpus: usize,
    mmi_entry_size: u64,
    is_inside_mmram: impl Fn(&MemoryRange) -> bool,
) -> MmSupervisorResult<SmiHandlerIdtPatchInputs> {
    if mmi_entry_size == 0 {
        return Err(SmiHandlerIdtPatchError::ZeroEntrySize.into());
    }
    if sm_base_array == 0 || number_of_cpus == 0 {
        return Err(SmiHandlerIdtPatchError::MissingSmBaseArray.into());
    }

    let sm_base_array_size = number_of_cpus
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

    Ok(SmiHandlerIdtPatchInputs {
        sm_base_array_size,
        mmi_entry_size: mmi_entry_size_usize,
        mmi_entry_size_u64: mmi_entry_size,
    })
}

/// Returns the address of the `IA32_DESCRIPTOR` one core's SMI entry loads.
///
/// The MMI entry ends with a `u32` giving the size of the fixup structure that precedes it. That
/// structure starts with a [`PerCoreMmiEntryStructHdr`], whose Fixup64 array holds the IDTR slot
/// at [`FIXUP64_SMI_HANDLER_IDTR`]. Every offset read here comes from the producer, so each step
/// is bounds-checked against `mmi_entry` rather than trusted.
fn parse_smi_handler_idt_descriptor(mmi_entry: &[u8]) -> MmSupervisorResult<u64> {
    const TRAILING_SIZE_FIELD_SIZE: usize = core::mem::size_of::<u32>();

    let trailer_start =
        mmi_entry.len().checked_sub(TRAILING_SIZE_FIELD_SIZE).ok_or(SmiHandlerIdtPatchError::EntryTooSmall)?;
    let trailer = mmi_entry.get(trailer_start..).ok_or(SmiHandlerIdtPatchError::EntryTooSmall)?;
    let whole_struct_size =
        u32::from_ne_bytes(trailer.try_into().map_err(|_| SmiHandlerIdtPatchError::EntryTooSmall)?) as usize;
    let header_start =
        trailer_start.checked_sub(whole_struct_size).ok_or(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)?;
    let fixup_structure =
        mmi_entry.get(header_start..trailer_start).ok_or(SmiHandlerIdtPatchError::FixupStructureOutOfBounds)?;
    let (header, _) = PerCoreMmiEntryStructHdr::read_from_prefix(fixup_structure)
        .map_err(|_| SmiHandlerIdtPatchError::FixupHeaderTooSmall)?;

    if FIXUP64_SMI_HANDLER_IDTR >= usize::from(header.fixup64_num) {
        return Err(SmiHandlerIdtPatchError::Fixup64ArrayTooSmall { found: header.fixup64_num }.into());
    }

    let fixup64_entry_start = usize::from(header.fixup64_offset)
        .checked_add(
            FIXUP64_SMI_HANDLER_IDTR
                .checked_mul(core::mem::size_of::<u64>())
                .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?,
        )
        .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?;
    let fixup64_entry_end = fixup64_entry_start
        .checked_add(core::mem::size_of::<u64>())
        .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?;
    let fixup64_entry = fixup_structure
        .get(fixup64_entry_start..fixup64_entry_end)
        .ok_or(SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?;

    Ok(u64::from_ne_bytes(fixup64_entry.try_into().map_err(|_| SmiHandlerIdtPatchError::Fixup64EntryOutOfBounds)?))
}

/// Patches every core's SMI-handler IDT descriptor to point to the Rust IDT.
///
/// Each per-core MMI entry (copied to `sm_base_array[i] + 0x8000` during C relocation)
/// carries a `Fixup64[FIXUP64_SMI_HANDLER_IDTR]` slot holding the address of the
/// `IA32_DESCRIPTOR` that core's SMI entry `lidt`s.
///
/// `sm_base_array` is the per-CPU SMBASE array (`u64[number_of_cpus]`) from the `PassDown`
/// HOB; `number_of_cpus` is its length.
pub(crate) fn patch_smi_handler_idt(sm_base_array: u64, number_of_cpus: usize, mmi_entry_size: u64) {
    let inputs = match validate_smi_handler_idt_patch_inputs(
        sm_base_array,
        number_of_cpus,
        mmi_entry_size,
        is_range_inside_mmram,
    ) {
        Ok(inputs) => inputs,
        Err(error) => {
            log::warn!("Cannot patch SMI handler IDT: {error:?}");
            return;
        }
    };

    let idtr = read_idtr();
    // Copy packed fields into aligned locals before formatting; taking a reference to a
    // field of a `packed(2)` struct (as `log::info!` would) is undefined behavior.
    let idtr_base = idtr.base;
    let idtr_limit = idtr.limit;

    // Read the SMBASE array as bytes so an unaligned producer address does not create an
    // invalid `&[u64]`.
    // SAFETY: `sm_base_array` is non-zero and references `sm_base_array_size` initialized
    // bytes in MMRAM, as validated above, for the duration of initialization.
    let sm_base_bytes = unsafe { core::slice::from_raw_parts(sm_base_array as *const u8, inputs.sm_base_array_size) };

    let mut patched = 0usize;
    let (smbase_chunks, _) = sm_base_bytes.as_chunks::<{ core::mem::size_of::<u64>() }>();
    for (cpu, smbase_bytes) in smbase_chunks.iter().enumerate() {
        let smbase = u64::from_ne_bytes(*smbase_bytes);
        if smbase == 0 {
            log::warn!("CPU {cpu}: SMBASE is 0, skipping SMI handler IDT patch");
            continue;
        }

        let Some(mmi_entry_base) = smbase.checked_add(SMM_HANDLER_OFFSET) else {
            log::error!("CPU {cpu}: SMBASE 0x{smbase:016x} overflows the SMI handler address");
            continue;
        };
        if !MemoryRange::new(mmi_entry_base, inputs.mmi_entry_size_u64).is_ok_and(|range| is_range_inside_mmram(&range))
        {
            log::error!(
                "CPU {cpu}: SMI handler at 0x{mmi_entry_base:016x} with size 0x{:x} is not inside MMRAM",
                inputs.mmi_entry_size
            );
            continue;
        }

        // SAFETY: the range check above establishes that the initialized SMI handler template
        // is fully contained in MMRAM.
        let mmi_entry = unsafe { core::slice::from_raw_parts(mmi_entry_base as *const u8, inputs.mmi_entry_size) };
        let idt_desc_addr = match parse_smi_handler_idt_descriptor(mmi_entry) {
            Ok(address) => address,
            Err(error) => {
                log::error!("CPU {cpu}: invalid SMI handler fixup metadata: {error:?}");
                continue;
            }
        };

        if idt_desc_addr == 0 {
            log::warn!("CPU {cpu}: Fixup64[{FIXUP64_SMI_HANDLER_IDTR}] (SMI_HANDLER_IDTR) is null");
            continue;
        }
        let idt_desc = MemoryRange::new(idt_desc_addr, core::mem::size_of::<DescriptorTablePointer>() as u64);
        if !idt_desc.is_ok_and(|range| is_range_inside_mmram(&range)) {
            log::error!("CPU {cpu}: SMI handler IDT descriptor at 0x{idt_desc_addr:016x} is not inside MMRAM");
            continue;
        }

        // SAFETY: the range check above establishes that the destination is a complete
        // writable descriptor in MMRAM.
        unsafe { core::ptr::write_unaligned(idt_desc_addr as *mut DescriptorTablePointer, idtr) };
        patched += 1;

        log::debug!(
            "CPU {cpu}: patched SMI handler IDT descriptor at 0x{idt_desc_addr:016x}: base=0x{idtr_base:016x}, limit=0x{idtr_limit:04x}"
        );
    }

    log::info!(
        "Patched the SMI handler IDT descriptor on {patched}/{number_of_cpus} CPU(s): base=0x{idtr_base:016x}, limit=0x{idtr_limit:04x}"
    );
}
