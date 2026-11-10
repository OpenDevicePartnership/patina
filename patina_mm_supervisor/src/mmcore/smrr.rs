//! System Management Range Register (SMRR) configuration.
//!
//! Programs the SMRR base and mask MSRs so SMRAM cannot be reached from outside SMM, and
//! enables SMM Code Access Check.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::error::MmSupervisorResult;
use crate::intrinsics::CPUID_VERSION_INFO;
use crate::intrinsics::read_msr;
use crate::intrinsics::write_msr;
use crate::memory::mmram::MmramRegion;
use core::arch::x86_64::__cpuid;
use patina::{MemoryRange, SIZE_256KB, UEFI_PAGE_SIZE};

// SMM Code Access Check related MSR and bit definitions
const MSR_SMM_MCA_CAP: u32 = 0x17D;
const SMM_CODE_ACCESS_CHK_BIT: u64 = 1 << 58;

const MSR_SMM_FEATURE_CONTROL: u32 = 0x4E0;
const SMM_FEATURE_CONTROL_LOCK_BIT: u64 = 1 << 0;
const SMM_CODE_CHK_EN_BIT: u64 = 1 << 2;

// SMM Range Register (SMRR) related MSR and bit definitions
const MSR_MTRR_CAP: u32 = 0x0FE;
const MTRR_CAP_SMRR_BIT: u64 = 1 << 11;
const MTRR_CAP_SMRR_EXT_BIT: u64 = 1 << 14;

/// CPUID leaf 1 `EDX` bit 12: MTRRs are supported.
const CPUID_EDX_MTRR: u32 = 1 << 12;

const MSR_SMRR_BASE: u32 = 0x1F2;
const MSR_SMRR_MASK: u32 = 0x1F3;

const MTRR_CACHE_WRITE_BACK: u8 = 6;

const PHYS_ADDR_MASK: u64 = 0xFFFF_F000; // bits [31:12]
/// The SMRR registers hold address bits [31:12], so a range must end at or below 4 GiB.
const SMRR_MAX_ADDRESS: u64 = 1 << 32;
const MEMTYPE_MASK: u64 = 0x0000_00FF; // bits [7:0]
const MASK_BIT_10: u64 = 1 << 10;
const MASK_VALID_BIT: u64 = 1 << 11;

/// Errors produced while programming the SMRRs to protect MMRAM.
///
/// Every one of these is fatal. The SMRRs keep MMRAM inaccessible from outside MM, so a range
/// that cannot be programmed must never be treated as merely unprotected and carried on from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmrrError {
    /// The CPU does not report MTRR support.
    MtrrUnsupported,
    /// The CPU does not report SMRR support.
    SmrrUnsupported,
    /// The CPU does not report the extended SMRR capability.
    SmrrExtUnsupported,
    /// The CPU does not report SMM Code Access Check support.
    SmmCodeAccessCheckUnsupported,
    /// The region lies outside the 4 GiB the 32-bit SMRR registers can describe.
    ///
    /// Truncating would program a different region than the one handed in and leave the real
    /// MMRAM unprotected, so this is fatal rather than narrowed.
    RangeNotAddressable {
        /// The region that did not fit.
        range: MemoryRange,
    },
    /// The region is not a power-of-two size of at least 4 KiB on a naturally aligned base.
    RangeNotAligned {
        /// The region that failed the check.
        range: MemoryRange,
    },
}

impl core::error::Error for SmrrError {}

impl core::fmt::Display for SmrrError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MtrrUnsupported => write!(f, "unsupported CPU: MTRRs are not supported"),
            Self::SmrrUnsupported => write!(f, "unsupported CPU: SMRRs are not supported"),
            Self::SmrrExtUnsupported => {
                write!(f, "unsupported CPU: the extended SMRR capability is not supported")
            }
            Self::SmmCodeAccessCheckUnsupported => {
                write!(f, "unsupported CPU: SMM Code Access Check is not supported")
            }
            Self::RangeNotAddressable { range } => {
                write!(f, "SMRAM region {range} does not fit the 32-bit SMRR registers")
            }
            Self::RangeNotAligned { range } => {
                write!(f, "SMRAM region {range} does not meet the SMRR alignment and size requirements")
            }
        }
    }
}

/// Returns `true` if a raw `MSR_SMM_MCA_CAP` value reports SMM Code Access Check support.
const fn smm_code_access_supported(mca_cap: u64) -> bool {
    (mca_cap & SMM_CODE_ACCESS_CHK_BIT) != 0
}

/// Returns the value to write to `MSR_SMM_FEATURE_CONTROL` to enable and lock SMM
/// Code Access Check, or `None` if both bits are already set in `current`.
const fn smm_feature_control_update(current: u64) -> Option<u64> {
    let updated = current | SMM_CODE_CHK_EN_BIT | SMM_FEATURE_CONTROL_LOCK_BIT;
    if updated == current { None } else { Some(updated) }
}

/// Enables and locks SMM Code Access Check.
///
/// When enabled, the CPU raises a machine check if code is fetched from outside the SMRR
/// protected region while in SMM. The lock bit keeps the configuration fixed until the next
/// processor reset.
///
/// ## Panics
///
/// Panics if the CPU does not report SMM Code Access Check support.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn configure_smm_code_access() {
    // SAFETY: MSR_SMM_MCA_CAP is a read-only architectural capability MSR that is
    // valid on all CPUs targeted by this code; reading it has no side effects.
    let mca_cap = unsafe { read_msr(MSR_SMM_MCA_CAP) };
    assert!(smm_code_access_supported(mca_cap), "{}", SmrrError::SmmCodeAccessCheckUnsupported);

    // SAFETY: MSR_SMM_FEATURE_CONTROL is an architectural MSR; reading it has no
    // side effects.
    let current = unsafe { read_msr(MSR_SMM_FEATURE_CONTROL) };
    if let Some(updated) = smm_feature_control_update(current) {
        // SAFETY: only the code check enable and lock bits are set, which is the
        // architecturally defined way to enable SMM Code Access Check. The write
        // happens only when the value changes.
        unsafe { write_msr(MSR_SMM_FEATURE_CONTROL, updated) };
    }
}

/// Returns `true` if bit 10 is set in a raw SMRR mask register value.
const fn mask_reg_bit10_set(raw: u64) -> bool {
    (raw & MASK_BIT_10) != 0
}

/// Validates that `range` satisfies the SMRR alignment and size constraints.
///
/// A valid region must be at least 4 KiB and smaller than 4 GiB, end at or below 4 GiB, and be
/// naturally aligned: its size is a power of two and its base is a multiple of its size.
pub(crate) fn verify_smrr_base_size(range: &MemoryRange) -> bool {
    (UEFI_PAGE_SIZE as u64..SMRR_MAX_ADDRESS).contains(&range.size())
        && range.end() <= SMRR_MAX_ADDRESS
        && range.is_naturally_aligned()
}

/// Returns `true` if a raw CPUID leaf 1 `edx` value reports MTRR support.
const fn mtrr_supported(cpuid_edx: u32) -> bool {
    (cpuid_edx & CPUID_EDX_MTRR) != 0
}

/// Returns `true` if a raw `MSR_MTRR_CAP` value reports SMRR support.
const fn smrr_supported(mtrr_cap: u64) -> bool {
    (mtrr_cap & MTRR_CAP_SMRR_BIT) != 0
}

/// Returns `true` if a raw `MSR_MTRR_CAP` value reports the extended SMRR capability.
const fn smrr_ext_supported(mtrr_cap: u64) -> bool {
    (mtrr_cap & MTRR_CAP_SMRR_EXT_BIT) != 0
}

/// Programs the SMRR base and mask registers to protect the given SMRAM region.
///
/// The region is configured as write-back cacheable but is not yet enabled; call
/// [`smrr_enable`] to set the valid and bit-10 fields and activate the range.
///
/// Returns without touching the registers when bit 10 of this processor's SMRR mask register is
/// already set. Otherwise a repeated call writes the same values again: the BSP programs its SMRR in one-time initialization and again
/// in per-core initialization.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn smrr_initialize(range: MemoryRange) -> MmSupervisorResult<()> {
    if !mtrr_supported(__cpuid(CPUID_VERSION_INFO).edx) {
        return Err(SmrrError::MtrrUnsupported.into());
    }

    // SAFETY: MSR_MTRR_CAP is a read-only architectural capability MSR; reading
    // it has no side effects.
    let mtrr_cap = unsafe { read_msr(MSR_MTRR_CAP) };

    if !smrr_supported(mtrr_cap) {
        return Err(SmrrError::SmrrUnsupported.into());
    }

    if !smrr_ext_supported(mtrr_cap) {
        return Err(SmrrError::SmrrExtUnsupported.into());
    }

    // SAFETY: SMRR support was verified above, so MSR_SMRR_MASK is a valid architectural MSR and
    // reading it has no side effects.
    if mask_reg_bit10_set(unsafe { read_msr(MSR_SMRR_MASK) }) {
        return Ok(());
    }

    // SMRR_BASE and SMRR_MASK only describe addresses below 4 GiB. Truncating here would
    // validate and program a different region than the one handed in, leaving the real MMRAM
    // unprotected, so a region that does not fit is fatal rather than narrowed.
    if range.end() > SMRR_MAX_ADDRESS {
        return Err(SmrrError::RangeNotAddressable { range }.into());
    }

    if !verify_smrr_base_size(&range) {
        return Err(SmrrError::RangeNotAligned { range }.into());
    }

    let (base, size) = range.into_parts();

    // SAFETY: SMRR support was verified above, so MSR_SMRR_BASE/MSR_SMRR_MASK
    // are valid architectural MSRs. `base`/`size` were validated by
    // `verify_smrr_base_size`, so the values written form a well-formed SMRR
    // range. The valid bit is left clear, so the range is not yet enforced.
    unsafe {
        // Bits [7:0] hold the memory type and bits [31:12] the base. `base` is aligned to at
        // least 4 KiB and is below 4 GiB, so it has no bits outside [31:12].
        let base_reg = read_msr(MSR_SMRR_BASE) & !(PHYS_ADDR_MASK | MEMTYPE_MASK);
        write_msr(MSR_SMRR_BASE, base_reg | base | u64::from(MTRR_CACHE_WRITE_BACK));

        // Bits [31:12] hold the mask, which is !(size - 1) for a power-of-two size.
        let mask_reg = read_msr(MSR_SMRR_MASK) & !PHYS_ADDR_MASK;
        write_msr(MSR_SMRR_MASK, mask_reg | (!(size - 1) & PHYS_ADDR_MASK));
    }

    Ok(())
}

/// Returns the value to write to `MSR_SMRR_MASK` to set the valid and bit-10 fields, or `None`
/// if bit 10 is already set.
const fn smrr_enable_mask(mask: u64) -> Option<u64> {
    if mask_reg_bit10_set(mask) { None } else { Some(mask | MASK_VALID_BIT | MASK_BIT_10) }
}

/// Enables the SMRR by setting the valid and bit-10 fields on the mask register.
///
/// Does nothing if bit 10 is already set.
///
/// This runs on SMI entry rather than at the end of the first SMI. Setting these fields while the
/// supervisor still holds the CPU leaves it enforcing across the `RSM` back to the non-MM world,
/// which faults on platforms whose pre-boot firmware still reaches into that range.
///
/// CPU MTRR/SMRR support is verified in [`smrr_initialize`] before this is first
/// reached, so it is not re-checked here on every SMI.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn smrr_enable() {
    // SAFETY: SMRR support was verified in `smrr_initialize` before this point,
    // so MSR_SMRR_MASK is a valid architectural MSR. Only the valid and bit-10
    // fields are set, to enable the previously programmed range, preserving all
    // other bits. The write is skipped if bit 10 is already set.
    unsafe {
        if let Some(mask) = smrr_enable_mask(read_msr(MSR_SMRR_MASK)) {
            write_msr(MSR_SMRR_MASK, mask);
        }
    }
}

/// Selects the primary SMRR range from the scanned SMRAM regions and coalesces
/// any physically adjacent regions into it.
///
/// It picks the largest non pre-allocated region in [1 MiB, 4 GiB] that is at
/// least 256 KiB - 4 KiB, then extends it downward and upward across every
/// region that is physically contiguous with it (regardless of allocation
/// state), scanning repeatedly until no further adjacent region is found.
///
/// The result is not simply the smallest base to the largest end of all the
/// regions. The range grows only by merging a region that starts exactly where
/// it ends, or ends exactly where it starts. Any gap between the range and the
/// next region stops it. A region past a gap is left out, rather than pulling
/// the non-SMRAM memory in the gap under the SMRR.
///
/// This is also where the range is validated for the SMRR registers: the
/// selected region must meet the base and size limits above, and the
/// coalesced range must pass [`verify_smrr_base_size`] (a power-of-two size,
/// a base aligned to that size, and an end at or below 4 GiB).
///
/// Returns the coalesced range on success, or `None` if no scanned region meets
/// the SMRR base/size requirements.
pub(crate) fn coalesced_smrr_range(regions: &[MmramRegion]) -> Option<MemoryRange> {
    /// Lowest CPU start address a candidate SMRR range may have.
    const BASE_1MB: u64 = 0x0010_0000;
    /// Smallest region that can be selected (256 KiB - 4 KiB).
    const MIN_SIZE: u64 = SIZE_256KB as u64 - UEFI_PAGE_SIZE as u64;

    // Find the largest usable (non-pre-allocated) range in [1 MiB, 4 GiB] that is at least
    // 256 KiB - 4 KiB. Of two equally large ranges, the later one is selected.
    let mut smrr = regions
        .iter()
        .filter(|region| !region.pre_allocated)
        .map(|region| region.range)
        .filter(|range| range.base() >= BASE_1MB && range.end() <= SMRR_MAX_ADDRESS && range.size() >= MIN_SIZE)
        .reduce(|largest, range| if range.size() >= largest.size() { range } else { largest })?;

    // Coalesce any physically adjacent ranges into the selected range. This
    // scans the (unsorted) region array repeatedly until no further
    // adjacent range is found, so ordering does not matter. Adjacency is
    // considered regardless of allocation state, because the SMRR must
    // cover a single contiguous physical range.
    loop {
        let mut found = false;
        for region in regions {
            if region.range.is_adjacent(&smrr) {
                smrr = smrr.union(&region.range).ok()?;
                found = true;
            }
        }
        if !found {
            break;
        }
    }

    let (smrr_base, smrr_size) = smrr.into_parts();
    if !verify_smrr_base_size(&smrr) {
        log::warn!(
            "Coalesced SMRR range base=0x{smrr_base:x} size=0x{smrr_size:x} does not meet SMRR alignment/size requirements"
        );
        return None;
    }

    log::info!("SMRR Base: 0x{smrr_base:x}, SMRR Size: 0x{smrr_size:x}");
    Some(smrr)
}
