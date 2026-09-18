//! APIC code for the BSP to start and quiesce APs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ptr::NonNull;
use patina::{
    arch::x64::{read_msr, write_msr},
    bit,
    mmio::{
        UniqueMmioPointer,
        fields::{ReadPureWrite, WriteOnly},
    },
};

/// Physical base address of the xAPIC register page, in `IA32_APIC_BASE[51:12]`.
const IA32_APIC_BASE_MSR: u32 = 0x1B;
/// Bit in `IA32_APIC_BASE` that indicates x2APIC mode.
const X2APIC_ENABLE_BIT: u64 = bit!(10);
/// Mask for the physical base address of the xAPIC register page.
const APIC_BASE_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// x2APIC local APIC version register.
const X2APIC_VERSION_MSR: u32 = 0x803;
/// Byte offset of the xAPIC local APIC version register.
const XAPIC_VERSION_OFFSET: usize = 0x30;

/// x2APIC interrupt-command register.
const X2APIC_ICR_MSR: u32 = 0x830;
/// Byte offsets of the xAPIC interrupt-command register halves.
const XAPIC_ICR_LOW_OFFSET: usize = 0x300;
const XAPIC_ICR_HIGH_OFFSET: usize = 0x310;
/// Interrupt-command delivery modes used by the universal startup algorithm.
const ICR_DELIVERY_MODE_INIT: u32 = 0b101 << 8;
const ICR_DELIVERY_MODE_STARTUP: u32 = 0b110 << 8;
const ICR_LEVEL_ASSERT: u32 = bit!(14);
/// Set while the xAPIC is still sending the previous interrupt command.
const ICR_DELIVERY_STATUS: u32 = bit!(12);

/// Local-vector-table registers that can deliver maskable interrupts or NMIs.
const XAPIC_LVT_OFFSETS: [usize; 6] = [0x320, 0x330, 0x340, 0x350, 0x360, 0x370];
const X2APIC_LVT_MSRS: [u32; 6] = [0x832, 0x833, 0x834, 0x835, 0x836, 0x837];
const LVT_MASKED: u32 = bit!(16);
const MAX_LVT_ENTRY_SHIFT: u32 = 16;
const MAX_LVT_ENTRY_MASK: u32 = 0xFF;

/// Whether the local APIC is operating in x2APIC mode.
pub(super) fn is_x2apic_enabled() -> bool {
    // SAFETY: Reading IA32_APIC_BASE is valid on any x86_64 platform with a local APIC.
    let base = unsafe { read_msr(IA32_APIC_BASE_MSR) };
    (base & X2APIC_ENABLE_BIT) != 0
}

fn send_icr(apic_id: u32, command: u32) {
    if is_x2apic_enabled() {
        let icr = (u64::from(apic_id) << 32) | u64::from(command);
        // SAFETY: IA32_X2APIC_ICR is the architectural x2APIC interrupt-command MSR.
        unsafe { write_msr(X2APIC_ICR_MSR, icr) };
    } else {
        // SAFETY: Reading IA32_APIC_BASE is valid on x86_64 systems with a local APIC.
        let base = (unsafe { read_msr(IA32_APIC_BASE_MSR) } & APIC_BASE_MASK) as usize;
        let Some(base) = NonNull::new(base as *mut u8) else {
            return;
        };

        // SAFETY: These addresses are the high and low halves of the local
        // xAPIC interrupt-command register.
        let mut high = unsafe { UniqueMmioPointer::new(base.byte_add(XAPIC_ICR_HIGH_OFFSET).cast::<WriteOnly<u32>>()) };
        // SAFETY: See the ICR-high safety argument above.
        let mut low =
            unsafe { UniqueMmioPointer::new(base.byte_add(XAPIC_ICR_LOW_OFFSET).cast::<ReadPureWrite<u32>>()) };
        while low.read() & ICR_DELIVERY_STATUS != 0 {
            core::hint::spin_loop();
        }
        high.write(apic_id << 24);
        low.write(command);
        while low.read() & ICR_DELIVERY_STATUS != 0 {
            core::hint::spin_loop();
        }
    }
}

/// Sends an INIT IPI to one processor by APIC ID.
pub(super) fn send_init(apic_id: u32) {
    send_icr(apic_id, ICR_DELIVERY_MODE_INIT | ICR_LEVEL_ASSERT);
}

/// Sends a STARTUP IPI to one processor by APIC ID.
pub(super) fn send_startup(apic_id: u32, startup_vector: u8) {
    send_icr(apic_id, ICR_DELIVERY_MODE_STARTUP | ICR_LEVEL_ASSERT | u32::from(startup_vector));
}

pub(super) fn mask_local_interrupts() {
    if is_x2apic_enabled() {
        // SAFETY: This is the architectural x2APIC version register.
        let version = unsafe { read_msr(X2APIC_VERSION_MSR) } as u32;
        let lvt_count =
            (((version >> MAX_LVT_ENTRY_SHIFT) & MAX_LVT_ENTRY_MASK) as usize + 1).min(X2APIC_LVT_MSRS.len());
        for &msr in X2APIC_LVT_MSRS.iter().take(lvt_count) {
            // SAFETY: These architectural x2APIC MSRs are the calling processor's
            // local-vector-table registers.
            let value = unsafe { read_msr(msr) } | u64::from(LVT_MASKED);
            // SAFETY: Preserving the register and setting its mask bit is valid for
            // every maskable local-vector-table entry.
            unsafe { write_msr(msr, value) };
        }
    } else {
        // SAFETY: Reading IA32_APIC_BASE is valid on x86_64 systems with a local APIC.
        let base = (unsafe { read_msr(IA32_APIC_BASE_MSR) } & APIC_BASE_MASK) as usize;
        let Some(base) = NonNull::new(base as *mut u8) else {
            return;
        };

        // SAFETY: `base` is the local APIC register page and this is its version register.
        let version =
            unsafe { UniqueMmioPointer::new(base.byte_add(XAPIC_VERSION_OFFSET).cast::<ReadPureWrite<u32>>()) };
        let lvt_count =
            (((version.read() >> MAX_LVT_ENTRY_SHIFT) & MAX_LVT_ENTRY_MASK) as usize + 1).min(XAPIC_LVT_OFFSETS.len());
        for &offset in XAPIC_LVT_OFFSETS.iter().take(lvt_count) {
            // SAFETY: `base` is the local APIC register page and `offset` identifies
            // one of its local-vector-table registers.
            let mut lvt = unsafe { UniqueMmioPointer::new(base.byte_add(offset).cast::<ReadPureWrite<u32>>()) };
            let value = lvt.read() | LVT_MASKED;
            lvt.write(value);
        }
    }
}
