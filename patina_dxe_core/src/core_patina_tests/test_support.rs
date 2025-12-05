//! DXE Core Patina Test Support
//!
//! Code to help support running Patina Tests.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::slice;
use bitfield_struct::bitfield;
use core::arch::asm;
use patina::base::{SIZE_1GB, SIZE_2MB, SIZE_4KB, UEFI_PAGE_SHIFT};
use patina_paging::MemoryAttributes;

pub(super) struct PteInfo {
    pub(super) present: bool,
    pub(super) attributes: MemoryAttributes,
    pub(super) next_address: u64,
    pub(super) points_to_pa: bool,
}

#[rustfmt::skip]
#[bitfield(u64)]
#[cfg(target_arch = "x86_64")]
pub(in super) struct PageTableEntryX64 {
    pub(in super) present: bool,                // 1 bit -  0 = Not present in memory, 1 = Present in memory
    pub(in super) read_write: bool,             // 1 bit -  0 = Read-Only, 1= Read/Write
    pub(in super) user_supervisor: bool,        // 1 bit -  0 = Supervisor, 1=User
    pub(in super) write_through: bool,          // 1 bit -  0 = Write-Back caching, 1=Write-Through caching
    pub(in super) cache_disabled: bool,         // 1 bit -  0 = Cached, 1=Non-Cached
    pub(in super) accessed: bool,               // 1 bit -  0 = Not accessed, 1 = Accessed (set by CPU)
    pub(in super) dirty: bool,                  // 1 bit -  0 = Not Dirty, 1 = written by processor on access to page
    pub(in super) page_size: bool,              // 1 bit -  1 = 2MB page for PD, 1GB page for PDP, Must be 0 for others.
    pub(in super) global: bool,                 // 1 bit -  0 = Not global page, 1 = global page TLB not cleared on CR3 write
    #[bits(3)]
    pub(in super) available: u8,                // 3 bits -  Available for use by system software
    #[bits(40)]
    pub(in super) page_table_base_address: u64, // 40 bits -  Page Table Base Address
    #[bits(11)]
    pub(in super) available_high: u16,          // 11 bits -  Available for use by system software
    pub(in super) nx: bool,                     // 1 bit -  0 = Execute Code, 1 = No Code Execution
}

#[rustfmt::skip]
#[bitfield(u64)]
#[cfg(target_arch = "aarch64")]
pub(in super) struct PageTableEntryAArch64 {
    pub(in super) valid: bool,              // 1 bit -  Valid descriptor
    pub(in super) table_desc: bool,         // 1 bit -  Table descriptor, 1 = Table descriptor for look up level 0, 1, 2
    #[bits(3)]
    pub(in super) attribute_index: u8,      // 3 bits -  Used for caching attributes
    pub(in super) non_secure: bool,         // 1 bit  -  Non-secure
    #[bits(2)]
    pub(in super) access_permission: u8,    // 2 bits -  Access permissions
    #[bits(2)]
    pub(in super) shareable: u8,            // 2 bits -  SH 0 = Non-shareable, 2 = Outer Shareable, 3 = Inner Shareable
    pub(in super) access_flag: bool,        // 1 bit  -  Access flag
    pub(in super) not_global: bool,         // 1 bit  -  Not global
    #[bits(38)]
    pub(in super) page_frame_number: u64,   // 38 bits - Page frame number
    pub(in super) guarded_page: bool,       // 1 bit  -  Guarded page
    pub(in super) dirty_bit_modifier: bool, // 1 bit  -  DBM
    pub(in super) contiguous: bool,         // 1 bit  -  Contiguous
    pub(in super) pxn: bool,                // 1 bit  -  Privileged execute never
    pub(in super) uxn: bool,                // 1 bit  -  User execute never
    #[bits(4)]
    pub(in super) reserved0: u8,            // 4 bits -  Reserved for software use
    pub(in super) pxn_table: bool,           // 1 bit  -  Hierarchical permissions.
    pub(in super) uxn_table: bool,           // 1 bit  -  Hierarchical permissions.
    #[bits(2)]
    pub(in super) ap_table: u8,              // 2 bits -  Hierarchical permissions.
    pub(in super) ns_table: bool,            // 1 bit  -  Secure state, only for accessing in Secure IPA or PA space.
}

#[cfg(target_arch = "x86_64")]
pub(super) fn flush_tlb() {
    // SAFETY: We are reading CR3 and writing it back exactly as is
    unsafe {
        let cr3: usize;
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        asm!("mov cr3, {}", in(reg) cr3, options(nomem, nostack, preserves_flags));
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) fn flush_tlb() {
    let current_el = patina::read_sysreg!(CurrentEL);

    match current_el {
        0x8 =>
        // SAFETY: We are simply invalidating the TLB, which is a safe operation
        unsafe {
            asm!("tlbi alle2", "dsb nsh", "isb sy", options(nostack));
        },
        0x4 =>
        // SAFETY: We are simply invalidating the TLB, which is a safe operation
        unsafe {
            asm!("tlbi alle1", "dsb nsh", "isb sy", options(nostack));
        },
        _ => panic!("Unsupported Exception Level for TLB flush"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PageTableLevel {
    Level1,
    Level2,
    Level3,
    Level4,
}

fn level_start_bit(level: PageTableLevel) -> u64 {
    match level {
        PageTableLevel::Level4 => 39,
        PageTableLevel::Level3 => 30,
        PageTableLevel::Level2 => 21,
        PageTableLevel::Level1 => 12,
    }
}

pub(super) fn get_index(va: u64, level: PageTableLevel) -> u64 {
    /// Page index mask for 4KB pages with 64-bit page table entries.
    const PAGE_INDEX_MASK: u64 = 0x1FF;

    let shift = level_start_bit(level);
    (va >> shift) & PAGE_INDEX_MASK
}

#[cfg(target_arch = "x86_64")]
pub(super) fn get_self_mapped_page_table<'a>(va: u64, level: PageTableLevel) -> &'a [u64] {
    const FOUR_LEVEL_PML4_SELF_MAP_BASE: u64 = 0xFFFF_FFFF_FFFF_F000;
    const FOUR_LEVEL_PDP_SELF_MAP_BASE: u64 = 0xFFFF_FFFF_FFE0_0000;
    const FOUR_LEVEL_PD_SELF_MAP_BASE: u64 = 0xFFFF_FFFF_C000_0000;
    const FOUR_LEVEL_PT_SELF_MAP_BASE: u64 = 0xFFFF_FF80_0000_0000;

    let base = match level {
        PageTableLevel::Level4 => FOUR_LEVEL_PML4_SELF_MAP_BASE,
        PageTableLevel::Level3 => {
            FOUR_LEVEL_PDP_SELF_MAP_BASE + (SIZE_4KB as u64 * get_index(va, PageTableLevel::Level4))
        }
        PageTableLevel::Level2 => {
            FOUR_LEVEL_PD_SELF_MAP_BASE
                + (SIZE_2MB as u64 * get_index(va, PageTableLevel::Level4))
                + (SIZE_4KB as u64 * get_index(va, PageTableLevel::Level3))
        }
        PageTableLevel::Level1 => {
            FOUR_LEVEL_PT_SELF_MAP_BASE
                + (SIZE_1GB as u64 * get_index(va, PageTableLevel::Level4))
                + (SIZE_2MB as u64 * get_index(va, PageTableLevel::Level3))
                + (SIZE_4KB as u64 * get_index(va, PageTableLevel::Level2))
        }
    };

    // SAFETY: We are only executing this from a patina_test where we are guaranteed to be using patina_paging
    unsafe { slice::from_raw_parts(base as *const u64, 512) }
}

#[cfg(target_arch = "aarch64")]
pub(super) fn get_self_mapped_page_table<'a>(va: u64, level: PageTableLevel) -> &'a [u64] {
    const FOUR_LEVEL_LEVEL4_SELF_MAP_BASE: u64 = 0xFFFF_FFFF_F000;
    const FOUR_LEVEL_LEVEL3_SELF_MAP_BASE: u64 = 0xFFFF_FFE0_0000;
    const FOUR_LEVEL_LEVEL2_SELF_MAP_BASE: u64 = 0xFFFF_C000_0000;
    const FOUR_LEVEL_LEVEL1_SELF_MAP_BASE: u64 = 0xFF80_0000_0000;

    let base = match level {
        PageTableLevel::Level4 => FOUR_LEVEL_LEVEL4_SELF_MAP_BASE,
        PageTableLevel::Level3 => {
            FOUR_LEVEL_LEVEL3_SELF_MAP_BASE + (SIZE_4KB as u64 * get_index(va, PageTableLevel::Level4))
        }
        PageTableLevel::Level2 => {
            FOUR_LEVEL_LEVEL2_SELF_MAP_BASE
                + (SIZE_2MB as u64 * get_index(va, PageTableLevel::Level4))
                + (SIZE_4KB as u64 * get_index(va, PageTableLevel::Level3))
        }
        PageTableLevel::Level1 => {
            FOUR_LEVEL_LEVEL1_SELF_MAP_BASE
                + (SIZE_1GB as u64 * get_index(va, PageTableLevel::Level4))
                + (SIZE_2MB as u64 * get_index(va, PageTableLevel::Level3))
                + (SIZE_4KB as u64 * get_index(va, PageTableLevel::Level2))
        }
    };

    // SAFETY: We are only executing this from a patina_test where we are guaranteed to be using patina_paging
    unsafe { slice::from_raw_parts(base as *const u64, 512) }
}

#[cfg(target_arch = "x86_64")]
pub(super) fn get_pte_state(pte: u64, level: PageTableLevel) -> PteInfo {
    let x64_pte = PageTableEntryX64::from_bits(pte);

    let present = x64_pte.present();
    let mut attributes = MemoryAttributes::empty();
    if !x64_pte.present() {
        attributes |= MemoryAttributes::ReadProtect;
    }

    if !x64_pte.read_write() {
        attributes |= MemoryAttributes::ReadOnly;
    }

    if x64_pte.nx() {
        attributes |= MemoryAttributes::ExecuteProtect;
    }

    let next_address = x64_pte.page_table_base_address() << UEFI_PAGE_SHIFT;
    let points_to_pa = match level {
        PageTableLevel::Level1 => true,
        PageTableLevel::Level2 | PageTableLevel::Level3 => x64_pte.page_size(),
        PageTableLevel::Level4 => false,
    };
    PteInfo { present, attributes, next_address, points_to_pa }
}

#[cfg(target_arch = "aarch64")]
pub(super) fn get_pte_state(pte: u64, level: PageTableLevel) -> PteInfo {
    let arm64_pte = PageTableEntryAArch64::from_bits(pte);

    let present = arm64_pte.valid();
    let attributes = if present {
        let mut attributes = MemoryAttributes::empty();

        // we can get the cache attributes for ARM64, but not x64. Since we aren't trying to validate
        // cache attributes here, just return access attributes. Leaving here for reference
        // match arm64_pte.attribute_index() {
        //     0 => attributes |= MemoryAttributes::Uncached,
        //     1 => attributes |= MemoryAttributes::WriteCombining,
        //     2 => attributes |= MemoryAttributes::WriteThrough,
        //     3 => attributes |= MemoryAttributes::Writeback,
        //     _ => panic!("Invalid attribute index"),
        // }

        if arm64_pte.access_permission() == 2 {
            attributes |= MemoryAttributes::ReadOnly;
        }

        if arm64_pte.uxn() {
            attributes |= MemoryAttributes::ExecuteProtect;
        }

        attributes
    } else {
        // when unmapped we use EFI_MEMORY_RP to represent that in the GCD
        MemoryAttributes::ReadProtect
    };
    let next_address = arm64_pte.page_frame_number() << UEFI_PAGE_SHIFT;
    let points_to_pa = match level {
        PageTableLevel::Level1 => true,
        PageTableLevel::Level2 | PageTableLevel::Level3 => !arm64_pte.table_desc(),
        PageTableLevel::Level4 => false,
    };
    PteInfo { present, attributes, next_address, points_to_pa }
}

pub(super) fn is_mapped(addr: u64) -> bool {
    let mut next_addr = addr;
    for &level in &[PageTableLevel::Level4, PageTableLevel::Level3, PageTableLevel::Level2, PageTableLevel::Level1] {
        let pt = get_self_mapped_page_table(next_addr, level);
        let idx = get_index(next_addr, level);
        let entry = pt.get(idx as usize).expect("Index out of bounds");
        let pte_state = get_pte_state(*entry, level);
        if !pte_state.present {
            // that's okay, not all PTs will be identity mapped
            return false;
        }
        if pte_state.points_to_pa {
            // we are identity mapped
            return true;
        } else {
            // continue down the page table levels
            next_addr = pte_state.next_address;
        }
    }
    unreachable!()
}
