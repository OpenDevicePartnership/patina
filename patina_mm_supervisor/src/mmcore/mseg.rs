//! MSEG (Monitor Segment) Support
//!
//! Reads the MSEG region from the MSEG SMRAM HOB and programs its base into each core's
//! `IA32_SMM_MONITOR_CTL`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use zerocopy::FromBytes;

use crate::{
    intrinsics::{cpuid_version_info, write_msr},
    state::init_state,
};
use patina::pi::mmram::SmramDescriptor;

/// GUID of the MSEG SMRAM HOB (`gMsegSmramGuid`), which carries the `EFI_SMRAM_DESCRIPTOR` of
/// the MSEG region reserved in SMRAM for an STM. Only platforms with STM support publish it.
pub(crate) const MSEG_SMRAM_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("5802bce4-eeee-4e33-a130-ebad27f0e439");

/// `IA32_SMM_MONITOR_CTL`, which holds the MSEG base (Intel SDM Vol. 4).
const IA32_SMM_MONITOR_CTL_MSR: u32 = 0x9b;

/// `IA32_SMM_MONITOR_CTL.Valid` (bit 0). An STM may only be invoked when set.
const SMM_MONITOR_CTL_VALID: u64 = 1;

/// `IA32_SMM_MONITOR_CTL.MsegBase` (bits 31:12).
const SMM_MONITOR_CTL_MSEG_BASE_MASK: u64 = 0xffff_f000;

/// CPUID leaf 1 `ECX` bit 5: VMX is supported.
const CPUID_ECX_VMX: u32 = 1 << 5;

/// Parses the MSEG SMRAM HOB payload, a single [`SmramDescriptor`].
///
/// Returns the MSEG base and length, or `None` if the payload is too short or the region is
/// empty, not 4 KiB aligned, or not below 4 GiB.
pub(crate) fn parse_mseg_smram_hob(data: &[u8]) -> Option<(u64, u64)> {
    // `read_from_prefix` checks the length and copies the bytes, so the buffer needs no alignment.
    let (descriptor, _) = SmramDescriptor::read_from_prefix(data)
        .inspect_err(|_| {
            log::error!("MSEG SMRAM HOB too small: {} < {}", data.len(), core::mem::size_of::<SmramDescriptor>());
        })
        .ok()?;

    if descriptor.physical_size == 0 {
        log::warn!("MSEG SMRAM HOB describes an empty region");
        return None;
    }
    if descriptor.physical_size & !SMM_MONITOR_CTL_MSEG_BASE_MASK != 0 {
        log::error!("MSEG size 0x{:x} is not 4 KiB aligned or lies above 4 GiB", descriptor.physical_size);
        return None;
    }

    let base = descriptor.cpu_start;
    if base & !SMM_MONITOR_CTL_MSEG_BASE_MASK != 0 {
        log::error!("MSEG base 0x{base:x} is not 4 KiB aligned or lies above 4 GiB");
        return None;
    }

    Some((base, descriptor.physical_size))
}

/// Writes the recorded MSEG base and the Valid bit to this logical processor's
/// `IA32_SMM_MONITOR_CTL`, so an STM can be activated later.
///
/// Does nothing when no MSEG base was recorded, and only logs a warning when the CPU does not
/// report VMX.
pub(crate) fn program_mseg_base(cpu_id: u32) {
    let Some(mseg_base) = init_state().mseg_base() else {
        return;
    };

    let value = (mseg_base & SMM_MONITOR_CTL_MSEG_BASE_MASK) | SMM_MONITOR_CTL_VALID;

    if (cpuid_version_info().ecx & CPUID_ECX_VMX) == 0 {
        log::warn!("CPU {cpu_id} does not support VMX (CPUID.01H:ECX.VMX=0), cannot program IA32_SMM_MONITOR_CTL");
        return;
    }

    // SAFETY: the MSR exists whenever CPUID.01H:ECX.VMX is set, as checked above, and only its
    // Valid and MsegBase fields are set, so the write cannot fault on a reserved bit.
    unsafe { write_msr(IA32_SMM_MONITOR_CTL_MSR, value) };
    log::debug!("CPU {cpu_id} programmed IA32_SMM_MONITOR_CTL = 0x{value:x}");
}
