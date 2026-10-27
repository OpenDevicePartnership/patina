//! Intel TDX guest detection using CPUID.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::x86_64::__cpuid_count;

const CPUID_SIGNATURE: u32 = 0;
const CPUID_VERSION_INFO: u32 = 1;
const CPUID_GUEST_TD_RUNTIME_ENVIRONMENT: u32 = 0x21;
const CPUID_HYPERVISOR_PRESENT: u32 = 1 << 31;
const INTEL_VENDOR_EBX: u32 = u32::from_le_bytes(*b"Genu");
const INTEL_VENDOR_EDX: u32 = u32::from_le_bytes(*b"ineI");
const INTEL_VENDOR_ECX: u32 = u32::from_le_bytes(*b"ntel");
const TDX_SIGNATURE_EBX: u32 = u32::from_le_bytes(*b"Inte");
const TDX_SIGNATURE_EDX: u32 = u32::from_le_bytes(*b"lTDX");
const TDX_SIGNATURE_ECX: u32 = u32::from_le_bytes(*b"    ");

/// Returns whether CPUID identifies an Intel TDX guest environment.
///
/// Uses the Intel vendor signature, hypervisor-present bit, and TDX leaf 0x21
/// signature, following EDK II's `TdIsEnabled` probe. Platform exception handlers
/// must be installed before calling this function if CPUID can raise #VE.
#[allow(unused_unsafe)]
pub fn is_td_enabled() -> bool {
    #[allow(unused_unsafe)]
    // SAFETY: CPUID with EAX=0 is always safe to call on x64.
    let vendor = unsafe { __cpuid_count(CPUID_SIGNATURE, 0) };
    if vendor.ebx != INTEL_VENDOR_EBX || vendor.edx != INTEL_VENDOR_EDX || vendor.ecx != INTEL_VENDOR_ECX {
        return false;
    }
    #[allow(unused_unsafe)]
    // SAFETY: CPUID with EAX=1 is always safe to call on x64.
    let version = unsafe { __cpuid_count(CPUID_VERSION_INFO, 0) };
    if version.ecx & CPUID_HYPERVISOR_PRESENT == 0 || vendor.eax < CPUID_GUEST_TD_RUNTIME_ENVIRONMENT {
        return false;
    }
    #[allow(unused_unsafe)]
    // SAFETY: CPUID with EAX=0x21 is always safe to call on x64.
    let tdx = unsafe { __cpuid_count(CPUID_GUEST_TD_RUNTIME_ENVIRONMENT, 0) };
    tdx.ebx == TDX_SIGNATURE_EBX && tdx.edx == TDX_SIGNATURE_EDX && tdx.ecx == TDX_SIGNATURE_ECX
}
