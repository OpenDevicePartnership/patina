//! APIC code for the BSP to start and quiesce APs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// APIC code cannot be meaningfully tested in unittests as it requires
// hardware access.
#![cfg_attr(coverage, coverage(off))]

use core::{num::NonZeroUsize, ptr::NonNull};
use patina::{
    arch::x64::{read_msr, write_msr},
    bit,
    mmio::{
        UniqueMmioPointer,
        fields::{ReadPureWrite, WriteOnly},
    },
};

use super::cpu_state::cpuid;

pub(super) struct XApic {
    base: Option<NonZeroUsize>,
}

pub(super) struct X2Apic;

#[cfg_attr(test, mockall::automock)]
pub(super) trait ApicBackend {
    fn max_apic_id(&self) -> u32;
    fn current_apic_id(&self) -> u32;
    fn send_icr(&self, apic_id: u32, command: u32);
    fn mask_local_interrupts(&self);
}

pub(super) enum Apic {
    XApic(XApic),
    X2Apic(X2Apic),
    #[cfg(test)]
    Mock(MockApicBackend),
}

impl XApic {
    const MAX_APIC_ID: u32 = 0xFE;
    const APIC_BASE_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    const VERSION_OFFSET: usize = 0x30;
    const ICR_LOW_OFFSET: usize = 0x300;
    const ICR_HIGH_OFFSET: usize = 0x310;
    const ICR_DELIVERY_STATUS: u32 = bit!(12);
    const LVT_OFFSETS: [usize; 6] = [0x320, 0x330, 0x340, 0x350, 0x360, 0x370];

    fn new(apic_base: u64) -> Self {
        Self { base: NonZeroUsize::new((apic_base & Self::APIC_BASE_MASK) as usize) }
    }
}

impl ApicBackend for XApic {
    fn max_apic_id(&self) -> u32 {
        Self::MAX_APIC_ID
    }

    fn current_apic_id(&self) -> u32 {
        cpuid(1, 0).ebx >> 24
    }

    fn send_icr(&self, apic_id: u32, command: u32) {
        let Some(base) = self.base.and_then(|base| NonNull::new(base.get() as *mut u8)) else {
            return;
        };

        // SAFETY: These addresses are the high and low halves of the local
        // xAPIC interrupt-command register.
        let mut high = unsafe { UniqueMmioPointer::new(base.byte_add(Self::ICR_HIGH_OFFSET).cast::<WriteOnly<u32>>()) };
        // SAFETY: See the ICR-high safety argument above.
        let mut low =
            unsafe { UniqueMmioPointer::new(base.byte_add(Self::ICR_LOW_OFFSET).cast::<ReadPureWrite<u32>>()) };
        while low.read() & Self::ICR_DELIVERY_STATUS != 0 {
            core::hint::spin_loop();
        }
        high.write(apic_id << 24);
        low.write(command);
        while low.read() & Self::ICR_DELIVERY_STATUS != 0 {
            core::hint::spin_loop();
        }
    }

    fn mask_local_interrupts(&self) {
        let Some(base) = self.base.and_then(|base| NonNull::new(base.get() as *mut u8)) else {
            return;
        };

        // SAFETY: `base` is the local APIC register page and this is its version register.
        let version =
            unsafe { UniqueMmioPointer::new(base.byte_add(Self::VERSION_OFFSET).cast::<ReadPureWrite<u32>>()) };
        let lvt_count = (((version.read() >> Apic::MAX_LVT_ENTRY_SHIFT) & Apic::MAX_LVT_ENTRY_MASK) as usize + 1)
            .min(Self::LVT_OFFSETS.len());
        for &offset in Self::LVT_OFFSETS.iter().take(lvt_count) {
            // SAFETY: `base` is the local APIC register page and `offset` identifies
            // one of its local-vector-table registers.
            let mut lvt = unsafe { UniqueMmioPointer::new(base.byte_add(offset).cast::<ReadPureWrite<u32>>()) };
            let value = lvt.read() | Apic::LVT_MASKED;
            lvt.write(value);
        }
    }
}

impl X2Apic {
    const MAX_APIC_ID: u32 = u32::MAX - 1;
    const VERSION_MSR: u32 = 0x803;
    const ICR_MSR: u32 = 0x830;
    const LVT_MSRS: [u32; 6] = [0x832, 0x833, 0x834, 0x835, 0x836, 0x837];
}

impl ApicBackend for X2Apic {
    fn max_apic_id(&self) -> u32 {
        Self::MAX_APIC_ID
    }

    fn current_apic_id(&self) -> u32 {
        cpuid(0xB, 0).edx
    }

    fn send_icr(&self, apic_id: u32, command: u32) {
        let icr = (u64::from(apic_id) << 32) | u64::from(command);
        // SAFETY: IA32_X2APIC_ICR is the architectural x2APIC interrupt-command MSR.
        unsafe { write_msr(Self::ICR_MSR, icr) };
    }

    fn mask_local_interrupts(&self) {
        // SAFETY: This is the architectural x2APIC version register.
        let version = unsafe { read_msr(Self::VERSION_MSR) } as u32;
        let lvt_count = (((version >> Apic::MAX_LVT_ENTRY_SHIFT) & Apic::MAX_LVT_ENTRY_MASK) as usize + 1)
            .min(Self::LVT_MSRS.len());
        for &msr in Self::LVT_MSRS.iter().take(lvt_count) {
            // SAFETY: These architectural x2APIC MSRs are the calling processor's
            // local-vector-table registers.
            let value = unsafe { read_msr(msr) } | u64::from(Apic::LVT_MASKED);
            // SAFETY: Preserving the register and setting its mask bit is valid for
            // every maskable local-vector-table entry.
            unsafe { write_msr(msr, value) };
        }
    }
}

impl Apic {
    const BASE_MSR: u32 = 0x1B;
    const X2APIC_ENABLE_BIT: u64 = bit!(10);
    const ICR_DELIVERY_MODE_INIT: u32 = 0b101 << 8;
    const ICR_DELIVERY_MODE_STARTUP: u32 = 0b110 << 8;
    const ICR_LEVEL_ASSERT: u32 = bit!(14);
    const LVT_MASKED: u32 = bit!(16);
    const MAX_LVT_ENTRY_SHIFT: u32 = 16;
    const MAX_LVT_ENTRY_MASK: u32 = 0xFF;

    pub(super) fn current() -> Self {
        // SAFETY: Reading IA32_APIC_BASE is valid on any x86_64 platform with a local APIC.
        let apic_base = unsafe { read_msr(Self::BASE_MSR) };
        if apic_base & Self::X2APIC_ENABLE_BIT != 0 { Self::X2Apic(X2Apic) } else { Self::XApic(XApic::new(apic_base)) }
    }

    #[cfg(test)]
    pub(super) fn mock(mock: MockApicBackend) -> Self {
        Self::Mock(mock)
    }

    fn backend(&self) -> &dyn ApicBackend {
        match self {
            Self::XApic(apic) => apic,
            Self::X2Apic(apic) => apic,
            #[cfg(test)]
            Self::Mock(apic) => apic,
        }
    }

    pub(super) fn max_apic_id(&self) -> u32 {
        self.backend().max_apic_id()
    }

    pub(super) fn current_apic_id(&self) -> u32 {
        self.backend().current_apic_id()
    }

    /// Sends an INIT IPI to one processor by APIC ID.
    pub(super) fn send_init(&self, apic_id: u32) {
        self.backend().send_icr(apic_id, Self::ICR_DELIVERY_MODE_INIT | Self::ICR_LEVEL_ASSERT);
    }

    /// Sends a STARTUP IPI to one processor by APIC ID.
    pub(super) fn send_startup(&self, apic_id: u32, startup_vector: u8) {
        self.backend()
            .send_icr(apic_id, Self::ICR_DELIVERY_MODE_STARTUP | Self::ICR_LEVEL_ASSERT | u32::from(startup_vector));
    }

    pub(super) fn mask_local_interrupts(&self) {
        self.backend().mask_local_interrupts();
    }
}
