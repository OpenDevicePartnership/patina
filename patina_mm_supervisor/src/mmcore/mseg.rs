//! MSEG (SMM Transfer Monitor Range) Support
//!
//! Owns the `IA32_SMM_MONITOR_CTL` MSR layout and the MSEG SMRAM HOB that describes the
//! region carved out of SMRAM for an SMM Transfer Monitor (STM). Parsing the HOB and
//! programming the MSR are kept together so the base-address mask is applied identically
//! in both places.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use zerocopy::FromBytes;

use crate::{
    intrinsics::{get_current_cpu_id, write_msr},
    memory::page_allocator::SmramDescriptor,
    state::init_state,
};

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

/// Parses the MSEG SMRAM HOB payload (`gMsegSmramGuid`), a single
/// [`SmramDescriptor`] describing the MSEG region carved out of SMRAM.
///
/// Returns the MSEG base address, or `None` if the region is empty.
pub(crate) fn parse_mseg_smram_hob(data: &[u8]) -> Option<u64> {
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

    let base = descriptor.cpu_start;
    if base & !SMM_MONITOR_CTL_MSEG_BASE_MASK != 0 {
        log::error!("MSEG base 0x{base:x} is not 4 KiB aligned or lies above 4 GiB");
        return None;
    }

    Some(base)
}

/// Programs this logical processor's `IA32_SMM_MONITOR_CTL` with the MSEG base.
///
/// The MSEG base discovered from the MSEG SMRAM HOB is written along with the
/// Valid bit so an STM can later be activated, and so software can read the region back.
///
/// No-op when the platform publishes no MSEG SMRAM HOB.
pub(crate) fn program_mseg_base(cpu_id: u32) {
    let Some(mseg_base) = init_state().mseg_base() else {
        return;
    };

    let value = (mseg_base & SMM_MONITOR_CTL_MSEG_BASE_MASK) | SMM_MONITOR_CTL_VALID;

    if (get_current_cpu_id().ecx & (1 << 5)) == 0 {
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn test_parse_mseg_smram_hob_returns_cpu_start() {
        let data = mseg_smram_hob_data(0x0080_0000, 0x0040_0000, 0x0002_0000);

        assert_eq!(parse_mseg_smram_hob(&data), Some(0x0040_0000));
    }

    #[test]
    fn test_parse_mseg_smram_hob_rejects_truncated_descriptor() {
        let data = mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0x0002_0000);

        assert_eq!(parse_mseg_smram_hob(&data[..data.len() - 1]), None);
    }

    #[test]
    fn test_parse_mseg_smram_hob_rejects_empty_region() {
        let data = mseg_smram_hob_data(0x0040_0000, 0x0040_0000, 0);

        assert_eq!(parse_mseg_smram_hob(&data), None);
    }

    #[test]
    fn test_parse_mseg_smram_hob_rejects_invalid_base() {
        for invalid_base in [0x0040_0001, 0x1_0000_0000] {
            let data = mseg_smram_hob_data(invalid_base, invalid_base, 0x0002_0000);

            assert_eq!(parse_mseg_smram_hob(&data), None);
        }
    }
}
