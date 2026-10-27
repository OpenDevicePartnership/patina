//! X64 GDT initialization
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(any(test, not(target_os = "uefi")), allow(dead_code))]
#![cfg_attr(test, allow(unused_imports))]
use core::ptr::{addr_of, addr_of_mut};

// The AP GDT only exists for the `mp` module, which needs `alloc`.
#[cfg(any(feature = "alloc", test))]
pub(crate) mod ap;

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
        u64::from(entry.limit15_0)
            | (u64::from(entry.base15_0) << 16)
            | (u64::from(entry.base23_16) << 32)
            | (u64::from(entry.type_) << 40)
            | (u64::from(entry.limit19_16_and_flags) << 48)
            | (u64::from(entry.base31_24) << 56)
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

const STACK_SIZE: usize = 4096 * 5;

const GDT_ENTRY_COUNT: usize = 11;

// Segment selector values (GDT index * 8, RPL = 0)
pub(crate) const CODE_SELECTOR: u16 = 7 * 8; // LINEAR_CODE64_SEL at index 7
pub(crate) const DATA_SELECTOR: u16 = 6 * 8; // LINEAR_DATA64_SEL at index 6
pub(crate) const TSS_SELECTOR: u16 = 8 * 8; // TSS descriptor at index 8

static mut SEPARATE_EXCEPTION_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

/// 64-bit task state segment containing the interrupt-stack table.
#[repr(C, packed)]
pub(crate) struct TaskStateSegment {
    reserved0: u32,
    privilege_stacks: [u64; 3],
    reserved1: u64,
    interrupt_stacks: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    io_map_base: u16,
}

impl TaskStateSegment {
    pub(crate) const fn new(ist1: u64) -> Self {
        Self {
            reserved0: 0,
            privilege_stacks: [0; 3],
            reserved1: 0,
            interrupt_stacks: [ist1, 0, 0, 0, 0, 0, 0],
            reserved2: 0,
            reserved3: 0,
            io_map_base: core::mem::size_of::<Self>() as u16,
        }
    }

    #[cfg(test)]
    pub(crate) fn ist1(&self) -> u64 {
        // SAFETY: `addr_of!` does not create an unaligned reference to the packed field.
        unsafe { core::ptr::addr_of!(self.interrupt_stacks[0]).read_unaligned() }
    }
}

const _: () = assert!(core::mem::size_of::<TaskStateSegment>() == 104);
const _: () = assert!(core::mem::offset_of!(TaskStateSegment, interrupt_stacks) == 36);

static mut TSS: TaskStateSegment = TaskStateSegment::new(0);

/// Build a 128-bit (two u64) TSS system segment descriptor from base address and limit.
fn tss_descriptor(base: u64) -> (u64, u64) {
    let limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u32;
    let low: u64 =
        // Limit [15:0]
        (u64::from(limit) & 0xFFFF)
        // Base [15:0] at bits 16..31
        | ((base & 0xFFFF) << 16)
        // Base [23:16] at bits 32..39
        | (((base >> 16) & 0xFF) << 32)
        // Type = 0x9 (64-bit TSS Available) at bits 40..43
        | (0x9u64 << 40)
        // Present bit at bit 47
        | (1u64 << 47)
        // Limit [19:16] at bits 48..51
        | ((u64::from(limit >> 16) & 0xF) << 48)
        // Base [31:24] at bits 56..63
        | (((base >> 24) & 0xFF) << 56);
    // High 8 bytes: Base [63:32]
    let high: u64 = base >> 32;
    (low, high)
}

static GDT: spin::LazyLock<[u64; GDT_ENTRY_COUNT]> = spin::LazyLock::new(|| {
    // Initialize TSS with separate exception stack in IST1
    // SAFETY: Single-threaded initialization guaranteed by LazyLock.
    unsafe {
        let ist_addr = addr_of!(SEPARATE_EXCEPTION_STACK) as u64 + STACK_SIZE as u64;
        addr_of_mut!(TSS).write(TaskStateSegment::new(ist_addr));
    }

    let tss_base = addr_of!(TSS) as u64;
    let (tss_low, tss_high) = tss_descriptor(tss_base);

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
});

/// Operand layout for the `lgdt` and `lidt` instructions.
#[repr(C, packed)]
pub(crate) struct DescriptorTablePointer {
    pub(crate) limit: u16,
    pub(crate) base: u64,
}

/// Descriptor-table pointer for the GDT this module owns, for callers that need to
/// load it on another processor.
pub(crate) fn descriptor() -> DescriptorTablePointer {
    DescriptorTablePointer {
        limit: (core::mem::size_of::<[u64; GDT_ENTRY_COUNT]>() - 1) as u16,
        base: GDT.as_ptr() as u64,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
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
