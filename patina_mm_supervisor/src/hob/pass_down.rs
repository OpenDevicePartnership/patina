//! MM Supervisor `PassDown` HOB
//!
//! Defines the payload the MM IPL hands down from the PEI phase and parses it from the raw
//! HOB bytes. Parsing checks the revision and the length before any field is read; the
//! producer sits outside the supervisor's trust boundary, so the payload is treated as
//! untrusted input.
//!
//! Range and ownership checks on the pointers this payload carries live in
//! [`super::validation`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use zerocopy::FromBytes;
use zerocopy_derive::Immutable;

use crate::error::MmSupervisorResult;

// GUID for gMmSupervisorPassDownHobGuid
// { 0x3f2d2d1a, 0x7c6a, 0x4e2e, { 0x91, 0x2e, 0x5c, 0x4f, 0x5b, 0x8c, 0x2a, 0x9d } }
/// GUID for the MM Supervisor `PassDown` HOB.
pub(crate) const MM_SUPV_PASS_DOWN_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("3f2d2d1a-7c6a-4e2e-912e-5c4f5b8c2a9d");

/// MM Supervisor `PassDown` HOB Revision
pub(crate) const MM_SUPV_PASS_DOWN_HOB_REVISION: u32 = 2;

/// Why the MM Supervisor `PassDown` HOB could not be used.
///
/// The MM IPL produces this HOB outside the supervisor's trust boundary, so its revision,
/// payload length, and every pointer it carries are checked before any field is acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassDownHobError {
    /// The `PassDown` HOB was not present in the HOB list.
    Missing,
    /// The `PassDown` HOB payload was smaller than its defined structure.
    TooSmall,
    /// The `PassDown` HOB reported an unexpected revision.
    InvalidRevision {
        /// The revision found in the HOB.
        found: u32,
        /// The revision the supervisor expected.
        expected: u32,
    },
    /// A pointer reported in the `PassDown` HOB references memory outside MMRAM.
    PointerOutsideMmram {
        /// Name of the offending `PassDown` field.
        field: &'static str,
        /// The reported address.
        addr: u64,
        /// The size that was checked for containment.
        size: u64,
    },
    /// The firmware policy buffer pointer or its size is zero.
    NullFirmwarePolicyBuffer,
    /// The firmware policy buffer size does not fit the target architecture.
    FirmwarePolicyBufferSizeUnsupported {
        /// Size the HOB reported.
        size: u64,
    },
    /// The firmware policy buffer address plus its size overflows.
    FirmwarePolicyBufferOverflows {
        /// Base address the HOB reported.
        base: u64,
        /// Size the HOB reported.
        size: u64,
    },
}

impl core::error::Error for PassDownHobError {}

impl fmt::Display for PassDownHobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "PassDown HOB is missing from the HOB list"),
            Self::TooSmall => write!(f, "PassDown HOB payload is too small"),
            Self::InvalidRevision { found, expected } => {
                write!(f, "PassDown HOB revision {found} does not match expected {expected}")
            }
            Self::PointerOutsideMmram { field, addr, size } => {
                write!(f, "PassDown pointer `{field}` = 0x{addr:x} (size 0x{size:x}) is outside MMRAM")
            }
            Self::NullFirmwarePolicyBuffer => write!(f, "the firmware policy buffer is null or empty"),
            Self::FirmwarePolicyBufferSizeUnsupported { size } => {
                write!(f, "the firmware policy buffer size {size} does not fit the target architecture")
            }
            Self::FirmwarePolicyBufferOverflows { base, size } => {
                write!(f, "the firmware policy buffer at 0x{base:x} plus size 0x{size:x} overflows")
            }
        }
    }
}

/// MM Supervisor `PassDown` HOB Data Structure
///
/// This structure contains various buffer pointers and sizes passed from
/// the PEI phase to the MM Supervisor.
///
/// All fields are naturally aligned (`u32`, `u32`, then `u64`s), so `repr(C)`
/// has the same byte layout the C producer emits while still allowing safe,
/// reference-based field access once parsed via `zerocopy`.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub(crate) struct MmSupvPassDownHobData {
    /// Revision of this HOB structure
    pub revision: u32,
    /// Reserved for future use
    pub reserved: u32,
    /// Base address of CPL3 stack for MM Supervisor
    pub cpl3_stack_base: u64,
    /// Per-CPU stack size for CPL3
    pub cpl3_stack_size: u64,
    /// Pointer to the per-CPU SMBASE array (`u64[number_of_cpus]`), indexed by the
    /// UEFI processor index (the same `cpu_index` the supervisor registers).
    ///
    /// The save-state region base for a CPU is `sm_base[cpu_index] +
    /// SMRAM_SAVE_STATE_MAP_OFFSET`. The BSP's own entry also serves as the
    /// IDT-patch fallback when `IA32_MSR_SMBASE` reads 0 (e.g. on QEMU).
    pub sm_base: u64,
    /// MM Initialized buffer base address
    pub mm_initialized_buffer: u64,
    /// MM Supervisor firmware policy buffer base address
    pub firmware_policy_buffer: u64,
    /// Size of MM Supervisor firmware policy buffer
    pub firmware_policy_buffer_size: u64,
    /// Size of the MMI entry point structure (for validating against expected size in supervisor)
    pub mmi_entry_size: u64,
}

impl MmSupvPassDownHobData {
    /// Reads the `PassDown` payload from the raw HOB bytes.
    ///
    /// The MM IPL produces this HOB outside the supervisor's trust boundary, so the payload
    /// length, the revision, and the firmware policy buffer pointer are checked here before any
    /// caller reads a field. Range and ownership checks on the remaining pointers live in
    /// [`crate::hob::validation`].
    ///
    /// # Errors
    ///
    /// Returns [`PassDownHobError::TooSmall`] when the payload is shorter than this structure,
    /// [`PassDownHobError::InvalidRevision`] when the revision is not
    /// [`MM_SUPV_PASS_DOWN_HOB_REVISION`], and [`PassDownHobError::NullFirmwarePolicyBuffer`] or
    /// [`PassDownHobError::FirmwarePolicyBufferOverflows`] when the firmware policy buffer it
    /// reports cannot be used.
    pub(crate) fn parse(data: &[u8]) -> MmSupervisorResult<Self> {
        let (pass_down, _) = Self::read_from_prefix(data).map_err(|_| {
            log::error!("PassDown HOB data too small: {} < {}", data.len(), core::mem::size_of::<Self>());
            PassDownHobError::TooSmall
        })?;

        if pass_down.revision != MM_SUPV_PASS_DOWN_HOB_REVISION {
            log::error!(
                "Invalid PassDown HOB revision: {} (expected {})",
                pass_down.revision,
                MM_SUPV_PASS_DOWN_HOB_REVISION
            );
            return Err(PassDownHobError::InvalidRevision {
                found: pass_down.revision,
                expected: MM_SUPV_PASS_DOWN_HOB_REVISION,
            }
            .into());
        }

        if pass_down.firmware_policy_buffer == 0 || pass_down.firmware_policy_buffer_size == 0 {
            log::error!("Firmware policy buffer is null or empty");
            return Err(PassDownHobError::NullFirmwarePolicyBuffer.into());
        }

        if pass_down.firmware_policy_buffer.checked_add(pass_down.firmware_policy_buffer_size).is_none() {
            log::error!("Firmware policy buffer address range overflows");
            return Err(PassDownHobError::FirmwarePolicyBufferOverflows {
                base: pass_down.firmware_policy_buffer,
                size: pass_down.firmware_policy_buffer_size,
            }
            .into());
        }

        Ok(pass_down)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    use crate::test_support::*;

    #[test]
    fn test_parse_pass_down_hob() {
        let expected = valid_pass_down_hob();
        let parsed =
            MmSupvPassDownHobData::parse(&pass_down_hob_data(&expected)).expect("valid PassDown HOB should parse");

        assert_eq!(parsed.revision, expected.revision);
        assert_eq!(parsed.cpl3_stack_base, expected.cpl3_stack_base);
        assert_eq!(parsed.sm_base, expected.sm_base);
        assert_eq!(parsed.firmware_policy_buffer, expected.firmware_policy_buffer);
        assert_eq!(parsed.mmi_entry_size, expected.mmi_entry_size);
    }

    #[test]
    fn test_parse_pass_down_hob_rejects_truncated_data() {
        let data = pass_down_hob_data(&valid_pass_down_hob());

        assert_eq!(
            MmSupvPassDownHobData::parse(&data[..data.len() - 1]).expect_err("truncated PassDown HOB should fail"),
            PassDownHobError::TooSmall.into()
        );
    }

    #[test]
    fn test_parse_pass_down_hob_rejects_invalid_revision() {
        let mut pass_down = valid_pass_down_hob();
        pass_down.revision += 1;

        assert_eq!(
            MmSupvPassDownHobData::parse(&pass_down_hob_data(&pass_down))
                .expect_err("invalid PassDown HOB revision should fail"),
            PassDownHobError::InvalidRevision { found: pass_down.revision, expected: MM_SUPV_PASS_DOWN_HOB_REVISION }
                .into()
        );
    }

    #[test]
    fn test_parse_pass_down_hob_rejects_invalid_policy_buffer() {
        for (address, size, expected) in [
            (0, 0x1000, PassDownHobError::NullFirmwarePolicyBuffer),
            (0x1000, 0, PassDownHobError::NullFirmwarePolicyBuffer),
            (
                u64::MAX - 0xFFF,
                0x1000,
                PassDownHobError::FirmwarePolicyBufferOverflows { base: u64::MAX - 0xFFF, size: 0x1000 },
            ),
        ] {
            let mut pass_down = valid_pass_down_hob();
            pass_down.firmware_policy_buffer = address;
            pass_down.firmware_policy_buffer_size = size;

            assert_eq!(
                MmSupvPassDownHobData::parse(&pass_down_hob_data(&pass_down))
                    .expect_err("invalid policy buffer should fail"),
                expected.into()
            );
        }
    }
    #[test]
    fn test_pass_down_hob_error_displays_each_variant() {
        let errors = [
            PassDownHobError::Missing,
            PassDownHobError::TooSmall,
            PassDownHobError::InvalidRevision { found: 3, expected: 2 },
            PassDownHobError::PointerOutsideMmram { field: "sm_base", addr: 0x1000, size: 8 },
            PassDownHobError::NullFirmwarePolicyBuffer,
            PassDownHobError::FirmwarePolicyBufferSizeUnsupported { size: u64::MAX },
            PassDownHobError::FirmwarePolicyBufferOverflows { base: u64::MAX - 0xFFF, size: 0x1000 },
        ];
        for err in errors {
            assert!(!format!("{err}").is_empty(), "every variant must render a message");
        }
    }

    #[test]
    fn test_pass_down_hob_layout_matches_c_abi() {
        assert_eq!(core::mem::size_of::<MmSupvPassDownHobData>(), 64);
    }
}
