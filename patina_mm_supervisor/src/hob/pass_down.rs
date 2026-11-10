//! MM Supervisor `PassDown` HOB
//!
//! The payload the MM IPL passes to the supervisor, and its parser.
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

/// GUID of the MM Supervisor `PassDown` HOB.
pub(crate) const MM_SUPV_PASS_DOWN_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("3f2d2d1a-7c6a-4e2e-912e-5c4f5b8c2a9d");

/// The only `PassDown` HOB revision the supervisor accepts.
pub(crate) const MM_SUPV_PASS_DOWN_HOB_REVISION: u32 = 2;

/// Errors produced while reading or checking the MM Supervisor `PassDown` HOB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassDownHobError {
    /// The `PassDown` HOB is missing.
    Missing,
    /// The payload is smaller than [`MmSupvPassDownHobData`].
    TooSmall,
    /// The HOB reported an unexpected revision.
    InvalidRevision {
        /// The revision found in the HOB.
        found: u32,
        /// The revision the supervisor expected.
        expected: u32,
    },
    /// A `PassDown` pointer references memory outside MMRAM.
    PointerOutsideMmram {
        /// Name of the offending `PassDown` field.
        field: &'static str,
        /// The range that was checked for containment.
        range: MemoryRange,
    },
    /// The firmware policy buffer pointer or its size is zero.
    NullFirmwarePolicyBuffer,
    /// The firmware policy buffer size does not fit in `usize`.
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
            Self::PointerOutsideMmram { field, range } => {
                write!(f, "PassDown pointer `{field}` at {range} is outside MMRAM")
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

/// Payload of the MM Supervisor `PassDown` HOB: the buffers the MM IPL hands to the supervisor.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, Immutable)]
pub(crate) struct MmSupvPassDownHobData {
    /// Revision of this HOB structure.
    pub revision: u32,
    /// Reserved.
    pub reserved: u32,
    /// Base address of the per-CPU Ring 3 (CPL3) stacks.
    pub cpl3_stack_base: u64,
    /// Size of each CPU's Ring 3 stack.
    pub cpl3_stack_size: u64,
    /// Pointer to the per-CPU SMBASE array.
    pub sm_base_array: u64,
    /// Base address of the per-core "MM initialized" bytes.
    pub mm_initialized_buffer: u64,
    /// Base address of the firmware policy buffer.
    pub firmware_policy_buffer: u64,
    /// Size of the firmware policy buffer.
    pub firmware_policy_buffer_size: u64,
    /// Size of each core's MMI entry, which bounds the fixup structure the supervisor patches.
    pub mmi_entry_size: u64,
}

impl MmSupvPassDownHobData {
    /// Reads the `PassDown` payload from the raw HOB bytes.
    ///
    /// Checks the length, the revision, and that the firmware policy buffer is non-null,
    /// non-empty and does not overflow. The other pointers are checked in
    /// [`crate::hob::validation`].
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

        if MemoryRange::new(pass_down.firmware_policy_buffer, pass_down.firmware_policy_buffer_size).is_err() {
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
