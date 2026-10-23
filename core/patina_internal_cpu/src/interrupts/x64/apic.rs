//! X64 APIC initialization.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    arch::x64::{read_msr, write_msr},
    mmio::{UniqueMmioPointer, fields::ReadPureWrite},
};
use patina::{bit, error::EfiError};

const APIC_BASE_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const APIC_ENABLE: u64 = bit!(11);
const X2APIC_ENABLE: u64 = bit!(10);
const SVR_OFFSET: usize = 0xF0;
const LINT0_OFFSET: usize = 0x350;
const LINT1_OFFSET: usize = 0x360;
const SPURIOUS_VECTOR_MASK: u32 = 0xFF;
const SOFTWARE_ENABLE: u32 = bit!(8);
const DELIVERY_MODE_MASK: u32 = bit!(8) | bit!(9) | bit!(10);
const ACTIVE_LOW: u32 = bit!(13);
const LEVEL_TRIGGERED: u32 = bit!(15);
const MASKED: u32 = bit!(16);
const EXTINT: u32 = bit!(8) | bit!(9) | bit!(10);
const NMI: u32 = bit!(10);

const APIC_BASE_MSR: u32 = 0x1B;
const X2APIC_MSR_BASE: u32 = 0x800;

#[derive(Debug, PartialEq)]
enum ApicMode {
    XApic(usize),
    X2Apic,
}

impl ApicMode {
    fn from_base(apic_base: u64) -> Result<Self, EfiError> {
        if apic_base & APIC_ENABLE == 0 {
            return Err(EfiError::Unsupported);
        }
        if apic_base & X2APIC_ENABLE != 0 {
            Ok(Self::X2Apic)
        } else {
            let base = (apic_base & APIC_BASE_MASK) as usize;
            if base == 0 {
                return Err(EfiError::Unsupported);
            }
            Ok(Self::XApic(base))
        }
    }
}

fn program_virtual_wire_mode(mut update: impl FnMut(usize, u32, u32)) {
    update(SVR_OFFSET, SPURIOUS_VECTOR_MASK, SOFTWARE_ENABLE | 0x0F);
    let lint_mask = DELIVERY_MODE_MASK | ACTIVE_LOW | LEVEL_TRIGGERED | MASKED;
    update(LINT0_OFFSET, lint_mask, EXTINT);
    update(LINT1_OFFSET, lint_mask, NMI);
}

fn apic_base_msr_supported(features: u32) -> bool {
    let required_features = bit!(5) | bit!(9);
    features & required_features == required_features
}

pub(super) fn initialize_virtual_wire_mode() -> Result<(), EfiError> {
    #[allow(unused_unsafe)]
    // SAFETY: It is safe to read CPUID leaf 1
    let cpu_features = unsafe { core::arch::x86_64::__cpuid(1) };
    if !apic_base_msr_supported(cpu_features.edx) {
        return Err(EfiError::Unsupported);
    }

    // SAFETY: It is safe to read this MSR once we have confirmed support for it
    let mode = ApicMode::from_base(unsafe { read_msr(APIC_BASE_MSR) })?;
    program_virtual_wire_mode(|offset, clear, set| match mode {
        ApicMode::X2Apic => {
            let msr = X2APIC_MSR_BASE + (offset >> 4) as u32;
            // SAFETY: The enabled x2APIC exposes SVR and LINT0/1 at these architectural MSRs.
            let value = unsafe { read_msr(msr) } as u32;
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            // SAFETY: Only the documented SVR and LINT configuration fields are changed.
            unsafe { write_msr(msr, u64::from((value & !clear) | set)) };
        }
        ApicMode::XApic(base) => {
            let address = core::ptr::NonNull::new((base + offset) as *mut ReadPureWrite<u32>)
                .expect("Validated nonzero APIC base");
            // SAFETY: The enabled xAPIC's base MSR identifies its mapped register page. CPU
            // initialization exclusively accesses these aligned SVR and LINT registers.
            let mut register = unsafe { UniqueMmioPointer::new(address) };
            let value = register.read();
            register.write((value & !clear) | set);
        }
    });
    Ok(())
}
