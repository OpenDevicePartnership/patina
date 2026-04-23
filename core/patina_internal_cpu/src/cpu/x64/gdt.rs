//! X64 GDT initialization
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(test, allow(dead_code))]
#![cfg_attr(test, allow(unused_imports))]
use core::ptr::{addr_of, addr_of_mut};
use lazy_static::lazy_static;
use patina::base::SIZE_4GB;

struct GdtEntry {
    limit15_0: u16,
    base15_0: u16,
    base23_16: u8,
    type_: u8,
    limit19_16_and_flags: u8,
    base31_24: u8,
}

// SAFETY: This also automatically defines the Into trait for u64. This is safe to do
// but any malformed GdtEntries will cause general protection faults. Only the defaults
// defined here should be used.
impl From<GdtEntry> for u64 {
    fn from(entry: GdtEntry) -> Self {
        (entry.limit15_0 as u64)
            | ((entry.base15_0 as u64) << 16)
            | ((entry.base23_16 as u64) << 32)
            | ((entry.type_ as u64) << 40)
            | ((entry.limit19_16_and_flags as u64) << 48)
            | ((entry.base31_24 as u64) << 56)
    }
}

const NULL_SEL: GdtEntry = GdtEntry {
    limit15_0: 0,
    base15_0: 0,
    base23_16: 0,
    type_: 0, // NULL
    limit19_16_and_flags: 0,
    base31_24: 0,
};

const LINEAR_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x92,                // present, ring 0, data, read/write
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0x00,
};

const LINEAR_CODE_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x9F,                // present, ring 0, code, execute/read, conforming, accessed
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0x00,
};

const SYS_DATA_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x93,                // present, ring 0, data, read/write, accessed
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0x00,
};

const SYS_CODE_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x9A,                // present, ring 0, code, execute/read
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0x00,
};

const SYS_CODE16_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x9A,                // present, ring 0, code, execute/read
    limit19_16_and_flags: 0x8F, // page-granular, 16-bit
    base31_24: 0x00,
};

const LINEAR_DATA64_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x92,                // present, ring 0, data, read/write
    limit19_16_and_flags: 0xCF, // page-granular, 32-bit
    base31_24: 0x00,
};

const LINEAR_CODE64_SEL: GdtEntry = GdtEntry {
    limit15_0: 0xffff,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x9A,                // present, ring 0, code, execute/read
    limit19_16_and_flags: 0xAF, // page-granular, 64-bit code
    base31_24: 0x00,
};

const SPARE5_SEL: GdtEntry = GdtEntry {
    limit15_0: 0x0000,
    base15_0: 0x0000,
    base23_16: 0x00,
    type_: 0x00,
    limit19_16_and_flags: 0x00,
    base31_24: 0x00,
};

/// Size of the 64-bit TSS structure in bytes.
const TSS_SIZE: usize = 104;

/// Byte offset of IST1 within the TSS (DOUBLE_FAULT_IST_INDEX 0 maps to IST1).
const TSS_IST1_OFFSET: usize = 36;

const STACK_SIZE: usize = 4096 * 5;

const GDT_ENTRY_COUNT: usize = 11;

// Segment selector values (GDT index * 8, RPL = 0)
pub(crate) const CODE_SELECTOR: u16 = 7 * 8; // LINEAR_CODE64_SEL at index 7
const DATA_SELECTOR: u16 = 6 * 8; // LINEAR_DATA64_SEL at index 6
const TSS_SELECTOR: u16 = 8 * 8; // TSS descriptor at index 8

static mut DOUBLE_FAULT_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
static mut TSS: [u8; TSS_SIZE] = [0; TSS_SIZE];

/// Build a 128-bit (two u64) TSS system segment descriptor from base address and limit.
fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let low: u64 =
        // Limit [15:0]
        (limit as u64 & 0xFFFF)
        // Base [15:0] at bits 16..31
        | ((base & 0xFFFF) << 16)
        // Base [23:16] at bits 32..39
        | (((base >> 16) & 0xFF) << 32)
        // Type = 0x9 (64-bit TSS Available) at bits 40..43
        | (0x9u64 << 40)
        // Present bit at bit 47
        | (1u64 << 47)
        // Limit [19:16] at bits 48..51
        | ((((limit >> 16) as u64) & 0xF) << 48)
        // Base [31:24] at bits 56..63
        | (((base >> 24) & 0xFF) << 56);
    // High 8 bytes: Base [63:32]
    let high: u64 = base >> 32;
    (low, high)
}

lazy_static! {
    static ref GDT: [u64; GDT_ENTRY_COUNT] = {
        // Initialize TSS with double-fault stack in IST1
        // SAFETY: Single-threaded initialization guaranteed by lazy_static.
        unsafe {
            let ist_addr = addr_of!(DOUBLE_FAULT_STACK) as u64 + STACK_SIZE as u64;
            let ist_bytes = ist_addr.to_ne_bytes();
            core::ptr::copy_nonoverlapping(
                ist_bytes.as_ptr(),
                addr_of_mut!(TSS).cast::<u8>().add(TSS_IST1_OFFSET),
                8,
            );
        }

        let tss_base = addr_of!(TSS) as u64;
        let (tss_low, tss_high) = tss_descriptor(tss_base, (TSS_SIZE - 1) as u32);

        // We need valid 32-bit code segments for MpServices as they start in real mode, go through
        // protected mode, then switch to long mode. They must come before the TSS entry as the
        // MpDxe C code matches the TSS selector to the code selector, even though it is not.
        [
            NULL_SEL.into(),
            LINEAR_SEL.into(),
            LINEAR_CODE_SEL.into(),
            SYS_DATA_SEL.into(),
            SYS_CODE_SEL.into(),
            SYS_CODE16_SEL.into(),
            LINEAR_DATA64_SEL.into(),
            LINEAR_CODE64_SEL.into(),
            tss_low,
            tss_high,
            SPARE5_SEL.into(),
        ]
    };
}

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

#[coverage(off)]
pub fn init() {
    let gdt_ptr = GDT.as_ptr() as usize;
    if gdt_ptr >= SIZE_4GB {
        panic!("GDT above 4GB, MP services will fail");
    }

    let gdtr = GdtPointer { limit: (core::mem::size_of::<[u64; GDT_ENTRY_COUNT]>() - 1) as u16, base: gdt_ptr as u64 };

    // SAFETY: We are constructing a well known GDT that maps all segments in a flat map
    unsafe {
        core::arch::asm!("lgdt [{}]", in(reg) &gdtr, options(nostack, preserves_flags));

        // Reload CS via a far return
        core::arch::asm!(
            "push {sel}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) CODE_SELECTOR as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );

        // These segments need to be valid, but can be all the same. Program them to the same GDT entry,
        // following what the C codebase does, as these are unused in long mode.
        core::arch::asm!("mov ss, {0:x}", in(reg) DATA_SELECTOR, options(nostack, preserves_flags));
        core::arch::asm!("mov ds, {0:x}", in(reg) DATA_SELECTOR, options(nostack, preserves_flags));
        core::arch::asm!("mov es, {0:x}", in(reg) DATA_SELECTOR, options(nostack, preserves_flags));
        core::arch::asm!("mov fs, {0:x}", in(reg) DATA_SELECTOR, options(nostack, preserves_flags));
        core::arch::asm!("mov gs, {0:x}", in(reg) DATA_SELECTOR, options(nostack, preserves_flags));

        // Load TSS
        core::arch::asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }

    log::info!("Loaded GDT @ {:p}", GDT.as_ptr());
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    pub fn test_dxe_default_entries() {
        for (i, &entry) in GDT.iter().enumerate() {
            match i {
                0 => assert_eq!(entry, NULL_SEL.into()),
                1 => assert_eq!(entry, LINEAR_SEL.into()),
                2 => assert_eq!(entry, LINEAR_CODE_SEL.into()),
                3 => assert_eq!(entry, SYS_DATA_SEL.into()),
                4 => assert_eq!(entry, SYS_CODE_SEL.into()),
                5 => assert_eq!(entry, SYS_CODE16_SEL.into()),
                6 => assert_eq!(entry, LINEAR_DATA64_SEL.into()),
                7 => assert_eq!(entry, LINEAR_CODE64_SEL.into()),
                8 => assert!(
                    entry & 0xFF > 0                                          // Limit > 0
                    && ((entry & (((1 << 4) - 1) << 40)) >> 40  == 0x9)       // Type is 9 (TSS Available)
                    && entry & (0x1 << 47) > 0, // Present set
                    "TSS Segment Descriptor is Not Valid"
                ),
                9 => assert!(entry & 0xFFFFFFFF > 0, "TSS Segment Descriptor Base must be set"), // TSS segment limit > 0
                10 => assert_eq!(entry, SPARE5_SEL.into()),
                _ => panic!("Unexpected GDT entry"),
            }
        }
        assert_eq!(GDT.len(), 11);
    }
}
