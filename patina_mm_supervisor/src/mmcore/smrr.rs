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
use core::arch::x86_64::__cpuid;
use patina::{MemoryRange, UEFI_PAGE_SIZE};

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

const PHYS_ADDR_MASK: u64 = 0xFFFF_F000; // bits 31:12
/// The SMRR registers hold address bits 31:12, so a range must end at or below 4 GiB.
const SMRR_MAX_ADDRESS: u64 = 1 << 32;
const MEMTYPE_MASK: u64 = 0x0000_00FF; // bits 7:0
const MASK_BIT_10: u64 = 1 << 10;
const MASK_VALID_BIT: u64 = 1 << 11;

/// Errors produced while programming the SMRRs to protect MMRAM. All are fatal, since the SMRRs
/// are what keep MMRAM inaccessible from outside MM.
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
    /// The region extends past the 4 GiB the SMRR registers can describe. Truncating it would
    /// leave the rest of MMRAM unprotected.
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

/// Enables and locks SMM Code Access Check, unless both bits are already set.
///
/// With it enabled, fetching code from outside the SMRR range in SMM raises a machine check. The
/// lock bit keeps the setting until the next reset.
///
/// ## Panics
///
/// Panics if the CPU does not report SMM Code Access Check support.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn configure_smm_code_access() {
    // SAFETY: MSR_SMM_MCA_CAP is a read-only capability MSR, and reading it has no side effects.
    let mca_cap = unsafe { read_msr(MSR_SMM_MCA_CAP) };
    assert!(smm_code_access_supported(mca_cap), "{}", SmrrError::SmmCodeAccessCheckUnsupported);

    // SAFETY: reading MSR_SMM_FEATURE_CONTROL has no side effects.
    let current = unsafe { read_msr(MSR_SMM_FEATURE_CONTROL) };
    if let Some(updated) = smm_feature_control_update(current) {
        // SAFETY: support was checked above, and only the code check enable and lock bits are
        // added to the current value.
        unsafe { write_msr(MSR_SMM_FEATURE_CONTROL, updated) };
    }
}

/// Returns `true` if bit 10 is set in a raw SMRR mask register value.
const fn mask_reg_bit10_set(raw: u64) -> bool {
    (raw & MASK_BIT_10) != 0
}

/// Returns whether `range` meets the SMRR alignment and size constraints.
///
/// Checks:
/// - The size is at least 4 KiB and less than 4 GiB.
/// - The range ends at or below 4 GiB.
/// - The size is a power of two and the base is a multiple of it.
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

/// Programs the SMRR base and mask registers for `range` as write-back, without enabling it.
/// [`smrr_enable`] activates the range.
///
/// Leaves the registers untouched if bit 10 of the mask register is already set. Otherwise a
/// repeated call writes the same values again, as the BSP does in per-core initialization.
///
/// ## Errors
///
/// Returns an [`SmrrError`] if the CPU lacks MTRR, SMRR or extended SMRR support, or `range`
/// does not fit the SMRR constraints.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn smrr_initialize(range: MemoryRange) -> MmSupervisorResult<()> {
    if !mtrr_supported(__cpuid(CPUID_VERSION_INFO).edx) {
        return Err(SmrrError::MtrrUnsupported.into());
    }

    // SAFETY: MSR_MTRR_CAP is a read-only capability MSR, and reading it has no side effects.
    let mtrr_cap = unsafe { read_msr(MSR_MTRR_CAP) };

    if !smrr_supported(mtrr_cap) {
        return Err(SmrrError::SmrrUnsupported.into());
    }

    if !smrr_ext_supported(mtrr_cap) {
        return Err(SmrrError::SmrrExtUnsupported.into());
    }

    // SAFETY: SMRR support was checked above, and reading MSR_SMRR_MASK has no side effects.
    if mask_reg_bit10_set(unsafe { read_msr(MSR_SMRR_MASK) }) {
        return Ok(());
    }

    if range.end() > SMRR_MAX_ADDRESS {
        return Err(SmrrError::RangeNotAddressable { range }.into());
    }

    if !verify_smrr_base_size(&range) {
        return Err(SmrrError::RangeNotAligned { range }.into());
    }

    let (base, size) = range.into_parts();

    // SAFETY: SMRR support was checked above, and `verify_smrr_base_size` accepted the range, so
    // the values written form a well-formed SMRR range. The valid bit stays clear.
    unsafe {
        // Bits 7:0 hold the memory type and bits 31:12 the base, which is 4 KiB aligned and
        // below 4 GiB.
        let base_reg = read_msr(MSR_SMRR_BASE) & !(PHYS_ADDR_MASK | MEMTYPE_MASK);
        write_msr(MSR_SMRR_BASE, base_reg | base | u64::from(MTRR_CACHE_WRITE_BACK));

        // Bits 31:12 hold the mask, which is !(size - 1) for a power-of-two size.
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

/// Enables the SMRR by setting the valid and bit-10 fields of the mask register, unless bit 10
/// is already set.
///
/// This runs on SMI entry rather than during the first SMI, because enabling the range then
/// keeps it enforcing after `RSM`, which faults non-MM code that still reaches into it. SMRR
/// support was checked by [`smrr_initialize`], so it is not checked again on every SMI.
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn smrr_enable() {
    // SAFETY: `smrr_initialize` checked SMRR support before this point. Only the valid and
    // bit-10 fields are set, enabling the range it programmed.
    unsafe {
        if let Some(mask) = smrr_enable_mask(read_msr(MSR_SMRR_MASK)) {
            write_msr(MSR_SMRR_MASK, mask);
        }
    }
}
