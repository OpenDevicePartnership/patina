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

// GUID for gMsegSmramGuid (UefiCpuPkg/UefiCpuPkg.dec)
// { 0x5802bce4, 0xeeee, 0x4e33, { 0xa1, 0x30, 0xeb, 0xad, 0x27, 0xf0, 0xe4, 0x39 } }
/// GUID for the MSEG SMRAM HOB, which carries the `EFI_SMRAM_DESCRIPTOR` for the
/// MSEG region carved out of SMRAM for an STM. Only published by platforms that
/// integrate STM/SEA support.
pub(crate) const MSEG_SMRAM_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("5802bce4-eeee-4e33-a130-ebad27f0e439");

/// MSR index for `IA32_SMM_MONITOR_CTL`, which holds the MSEG base used to
/// activate the dual-monitor treatment (Intel SDM Vol. 4).
const IA32_SMM_MONITOR_CTL_MSR: u32 = 0x9b;

/// `IA32_SMM_MONITOR_CTL.Valid` (bit 0). An STM may only be invoked when set.
const SMM_MONITOR_CTL_VALID: u64 = 1;

/// `IA32_SMM_MONITOR_CTL.MsegBase` (bits 31:12).
const SMM_MONITOR_CTL_MSEG_BASE_MASK: u64 = 0xffff_f000;

/// CPUID leaf 1 `ECX` bit 5: VMX is supported.
const CPUID_ECX_VMX: u32 = 1 << 5;

/// Parses the MSEG SMRAM HOB payload (`gMsegSmramGuid`), a single
/// [`SmramDescriptor`] describing the MSEG region carved out of SMRAM.
///
/// Returns the MSEG base address, and its length, or `None` if the region is empty.
pub(crate) fn parse_mseg_smram_hob(data: &[u8]) -> Option<(u64, u64)> {
    // `read_from_prefix` validates the length and copies the bytes out, so it imposes no
    // alignment or validity precondition on the HOB buffer and needs no `unsafe`.
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

/// Programs this logical processor's `IA32_SMM_MONITOR_CTL` with the MSEG base.
///
/// Writes the MSEG base from the MSEG SMRAM HOB together with the Valid bit, so an STM can be
/// activated later and software can read the region back.
///
/// Does nothing when the platform publishes no MSEG SMRAM HOB, and logs a warning without
/// writing when the CPU does not report VMX.
pub(crate) fn program_mseg_base(cpu_id: u32) {
    let Some(mseg_base) = init_state().mseg_base() else {
        return;
    };

    let value = (mseg_base & SMM_MONITOR_CTL_MSEG_BASE_MASK) | SMM_MONITOR_CTL_VALID;

    if (cpuid_version_info().ecx & CPUID_ECX_VMX) == 0 {
        log::warn!("CPU {cpu_id} does not support VMX (CPUID.01H:ECX.VMX=0), cannot program IA32_SMM_MONITOR_CTL");
        return;
    }

    // SAFETY: IA32_SMM_MONITOR_CTL is an architectural MSR available whenever VMX is
    // reported by CPUID.01H:ECX.VMX. After the check above, only the architecturally
    // defined Valid and MsegBase fields are set; reserved bits are masked off above,
    // so the write cannot #GP on a reserved-bit violation. The write affects only this
    // logical processor's MSR.
    unsafe { write_msr(IA32_SMM_MONITOR_CTL_MSR, value) };
    log::debug!("CPU {cpu_id} programmed IA32_SMM_MONITOR_CTL = 0x{value:x}");
}
