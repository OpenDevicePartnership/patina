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
use core::ptr::addr_of;
use lazy_static::lazy_static;
use patina::base::SIZE_4GB;
use x86_64::instructions::{
    segmentation::{CS, DS, ES, FS, GS, SS, Segment},
    tables::load_tss,
};
use x86_64::{
    VirtAddr,
    structures::{
        gdt::{Descriptor, DescriptorFlags, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

// 0xcf92000000ffff
pub const LINEAR_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()     // 0xFFFF
    // 0x2 [41]
    | DescriptorFlags::WRITABLE.bits()          // 41
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0xC [55, 54]
    | DescriptorFlags::DEFAULT_SIZE.bits()      // 54
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0xcf9f000000ffff
pub const LINEAR_CODE_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()     // 0xFFFF
    // 0xF [43, 42, 41, 40]
    | DescriptorFlags::ACCESSED.bits()          // 40
    | DescriptorFlags::WRITABLE.bits()          // 41
    | DescriptorFlags::CONFORMING.bits()        // 42
    | DescriptorFlags::EXECUTABLE.bits()        // 43
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0xC [55, 54]
    | DescriptorFlags::DEFAULT_SIZE.bits()      // 54
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0xcf93000000ffff
pub const SYS_DATA_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()     // 0xFFFF
    // 0x3 [41, 40]
    | DescriptorFlags::ACCESSED.bits()          // 40
    | DescriptorFlags::WRITABLE.bits()          // 41
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0xC [55, 54]
    | DescriptorFlags::DEFAULT_SIZE.bits()      // 54
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0xcf9a000000ffff
pub const SYS_CODE_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()     // 0xFFFF
    // 0xA [43, 41]
    | DescriptorFlags::WRITABLE.bits()          // 41
    | DescriptorFlags::EXECUTABLE.bits()        // 43
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0xC [55, 54]
    | DescriptorFlags::DEFAULT_SIZE.bits()      // 54
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0x8f9a000000ffff
pub const SYS_CODE16_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()    // 0xFFFF
    // 0xA [43, 41]
    | DescriptorFlags::WRITABLE.bits()          // 41
    | DescriptorFlags::EXECUTABLE.bits()        // 43
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0x8 [55]
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0xcf92000000ffff
pub const LINEAR_DATA64_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()     // 0xFFFF
    // 0x2 [41]
    | DescriptorFlags::WRITABLE.bits()          // 41
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0xC [55, 54]
    | DescriptorFlags::DEFAULT_SIZE.bits()      // 54
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0xaf9a000000ffff
pub const LINEAR_CODE64_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(
    // 0xFFFF
    DescriptorFlags::LIMIT_0_15.bits()     // 0xFFFF
    // 0xA [43, 41]
    | DescriptorFlags::WRITABLE.bits()          // 41
    | DescriptorFlags::EXECUTABLE.bits()        // 43
    // 0x9 [47, 44]
    | DescriptorFlags::USER_SEGMENT.bits()      // 44
    | DescriptorFlags::PRESENT.bits()           // 47
    // 0xF [51, 50, 49, 48]
    | DescriptorFlags::LIMIT_16_19.bits()       // 0xF << 48
    // 0xa [55, 53]
    | DescriptorFlags::LONG_MODE.bits()         // 53
    | DescriptorFlags::GRANULARITY.bits(), // 55
);

// 0x0
pub const SPARE5_SEL: DescriptorFlags = DescriptorFlags::from_bits_truncate(0);

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            VirtAddr::from_ptr(addr_of!(STACK)) + STACK_SIZE as u64
        };
        tss
    };
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable::<11>, Selectors) = {
        let mut gdt = GlobalDescriptorTable::<11>::empty();

        // We need a valid 32 bit code segment for MpServices as they start in real mode, go through protected mode,
        // then switch to long mode. It also must come before the TSS entry as the MpDxe C code matches the TSS
        // selector to the code selector, even though it is not.
        gdt.append(Descriptor::UserSegment(LINEAR_SEL.bits()));
        gdt.append(Descriptor::UserSegment(LINEAR_CODE_SEL.bits()));
        gdt.append(Descriptor::UserSegment(SYS_DATA_SEL.bits()));
        gdt.append(Descriptor::UserSegment(SYS_CODE_SEL.bits()));
        gdt.append(Descriptor::UserSegment(SYS_CODE16_SEL.bits()));
        // Always load 64-bit code & data segments
        let data_selector = gdt.append(Descriptor::UserSegment(LINEAR_DATA64_SEL.bits()));
        let code_selector = gdt.append(Descriptor::UserSegment(LINEAR_CODE64_SEL.bits()));
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
        gdt.append(Descriptor::UserSegment(SPARE5_SEL.bits()));
        (gdt, Selectors { code_selector, data_selector, tss_selector })
    };
}

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    if &GDT.0 as *const _ as usize >= SIZE_4GB {
        panic!("GDT above 4GB, MP services will fail");
    }
    GDT.0.load();

    // SAFETY: We are constructing a well known GDT that maps all segments in a flat map
    unsafe {
        CS::set_reg(GDT.1.code_selector);

        // These segments need to be valid, but can be all the same. Program them to the same GDT entry,
        // following what the C codebase does, as these are unused in long mode.
        SS::set_reg(GDT.1.data_selector);
        DS::set_reg(GDT.1.data_selector);
        ES::set_reg(GDT.1.data_selector);
        FS::set_reg(GDT.1.data_selector);
        GS::set_reg(GDT.1.data_selector);
        load_tss(GDT.1.tss_selector);
    }
    log::info!("Loaded GDT @ {:p}", &GDT.0);
    log::info!("GDT is: {:?}", GDT.0);
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::GDT;
    use super::{
        LINEAR_CODE_SEL, LINEAR_CODE64_SEL, LINEAR_DATA64_SEL, LINEAR_SEL, SPARE5_SEL, SYS_CODE_SEL, SYS_CODE16_SEL,
        SYS_DATA_SEL,
    };

    #[test]
    pub fn test_dxe_default_entries() {
        println!("{:?}", GDT.0.entries()[8]);

        for (i, entry) in GDT.0.entries().iter().enumerate() {
            match i {
                0 => assert_eq!(entry.raw(), 0),
                1 => assert_eq!(entry.raw(), LINEAR_SEL.bits()),
                2 => assert_eq!(entry.raw(), LINEAR_CODE_SEL.bits()),
                3 => assert_eq!(entry.raw(), SYS_DATA_SEL.bits()),
                4 => assert_eq!(entry.raw(), SYS_CODE_SEL.bits()),
                5 => assert_eq!(entry.raw(), SYS_CODE16_SEL.bits()),
                6 => assert_eq!(entry.raw(), LINEAR_DATA64_SEL.bits()),
                7 => assert_eq!(entry.raw(), LINEAR_CODE64_SEL.bits()),
                8 => assert!(
                    entry.raw() & 0xFF > 0                                          // Limit > 0
                    && ((entry.raw() & (((1 << 4) - 1) << 40)) >> 40  == 0x9)       // Type is 9 (TSS Available)
                    && entry.raw() & (0x1 << 47) > 0, // Present set
                    "TSS Segment Descriptor is Not Valid"
                ),
                9 => assert!(entry.raw() & 0xFFFFFFFF > 0, "TSS Segment Descriptor Base must be set"), // TSS segment limit > 0
                10 => assert_eq!(entry.raw(), SPARE5_SEL.bits()),
                _ => panic!("Unexpected GDT entry"),
            }
        }
        assert_eq!(GDT.0.entries().len(), 11);
    }

    #[test]
    fn test_dxe_default_segment_values() {
        assert_eq!(LINEAR_SEL.bits(), 0xcf92000000ffff);
        assert_eq!(LINEAR_CODE_SEL.bits(), 0xcf9f000000ffff);
        assert_eq!(SYS_DATA_SEL.bits(), 0xcf93000000ffff);
        assert_eq!(SYS_CODE_SEL.bits(), 0xcf9a000000ffff);
        assert_eq!(SYS_CODE16_SEL.bits(), 0x8f9a000000ffff);
        assert_eq!(LINEAR_DATA64_SEL.bits(), 0xcf92000000ffff);
        assert_eq!(LINEAR_CODE64_SEL.bits(), 0xaf9a000000ffff);
        assert_eq!(SPARE5_SEL.bits(), 0x0);
    }
}
