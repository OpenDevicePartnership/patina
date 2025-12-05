//! DXE Core Patina Test Stability Tests
//!
//! These tests are intended to verify the stability and reliability of the Patina DXE Core.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::test_support::*;
use crate::{GCD, gcd::AllocateType};
use alloc::vec::Vec;
use patina::{
    base::{SIZE_1GB, SIZE_2MB, SIZE_4KB},
    pi::dxe_services::GcdMemoryType,
    test::patina_test,
    u_assert, u_assert_eq,
};
use patina_paging::MemoryAttributes;
use r_efi::efi;

/// Stability Test: Split a 2MB page into 4KB pages and verify correctness
#[patina_test]
#[on(timer = 3_000_000)] // 300ms interval
fn page_table_tests_2mb_split() -> patina::test::Result {
    let mut addr_vec = Vec::new();
    for _ in 0..19 {
        addr_vec.push(
            GCD.allocate_memory_space(
                AllocateType::TopDown(None),
                GcdMemoryType::SystemMemory,
                21, // alignment shift for 2MB
                SIZE_2MB,
                0x05D as efi::Handle,
                None,
            )
            .unwrap(),
        );
    }

    for addr in addr_vec {
        u_assert_eq!(addr % SIZE_2MB, 0);

        // set the whole region as RWX first so we can adjust the attributes easily after
        GCD.set_memory_space_attributes(addr, SIZE_2MB, efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 2MB region")?;

        // Make sure the page is actually a 2MB page by reading the entry in the self map
        let mut pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level2);
        let mut idx = get_index(addr as u64, PageTableLevel::Level2);
        let mut entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        let mut self_map_entry = entry;
        let mut pte_state = get_pte_state(*entry, PageTableLevel::Level2);

        u_assert!(pte_state.points_to_pa);
        u_assert_eq!(pte_state.attributes, MemoryAttributes::empty());
        u_assert_eq!(pte_state.next_address, addr as u64);
        u_assert!(pte_state.present);

        // if the identity mapped page table address is mapped, let's check that that matches
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level3);
        idx = get_index(addr as u64, PageTableLevel::Level3);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        let mut id_mapped_pt = pte_state.next_address;

        if is_mapped(id_mapped_pt) {
            // we are identity mapped, so check this matches the self map
            let pt_ptr = id_mapped_pt as *const u64;
            // No matter what level the page table itself is mapped at, we want the level 2 page table entry
            // for the actual address, because we confirmed above that it is a 2MB page
            idx = get_index(addr as u64, PageTableLevel::Level2);
            // SAFETY: We have just confirmed this PT is mapped, so we can read it
            let entry_value = unsafe { pt_ptr.add(idx as usize).read() };
            u_assert_eq!(*self_map_entry, entry_value);
        }

        // force a split
        GCD.set_memory_space_attributes(addr, SIZE_4KB, efi::MEMORY_XP | efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 4KB region")?;

        // SAFETY: We just allocated this memory and marked it writeable so it is safe to write/read
        unsafe {
            // ensure we can now write to the memory and read it back
            for i in 0..SIZE_2MB / core::mem::size_of::<u64>() {
                core::ptr::write_volatile((addr + i * core::mem::size_of::<u64>()) as *mut u64, 0x05D05D05D05D05D0);
            }

            for i in 0..SIZE_2MB / core::mem::size_of::<u64>() {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0
                );
            }
        }

        // Now let's check that we split the page correctly
        // The PTE at level 2 should now point to a page table, not a 2MB page
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level2);
        idx = get_index(addr as u64, PageTableLevel::Level2);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        self_map_entry = entry;
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(!pte_state.points_to_pa);
        u_assert!(pte_state.present);
        u_assert!(pte_state.next_address != addr as u64);

        // confirm identity map still matches after the split
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level3);
        idx = get_index(addr as u64, PageTableLevel::Level3);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        id_mapped_pt = pte_state.next_address;

        if is_mapped(id_mapped_pt) {
            // we are identity mapped, so check this matches the self map
            let pt_ptr = id_mapped_pt as *const u64;
            // No matter what level the page table itself is mapped at, we want the level 2 page table entry
            idx = get_index(addr as u64, PageTableLevel::Level2);
            // SAFETY: We have just confirmed this PT is mapped, so we can read it
            let entry_value = unsafe { pt_ptr.add(idx as usize).read() };
            u_assert_eq!(*self_map_entry, entry_value);
        }

        // At level 1 we should see we are pointing to all to 4KB pages since we mapped the whole range
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level1);
        for (i, pte) in pt.iter().enumerate() {
            pte_state = get_pte_state(*pte, PageTableLevel::Level1);
            u_assert!(pte_state.points_to_pa);
            u_assert!(pte_state.next_address == (addr + (i * SIZE_4KB)) as u64);
            u_assert!(pte_state.present);
            if i == 0 {
                u_assert_eq!(pte_state.attributes, MemoryAttributes::ExecuteProtect);
            } else {
                u_assert_eq!(pte_state.attributes, MemoryAttributes::empty());
            }
        }

        // now let's make sure that the paging crate did all the TLB invalidation it needed to
        flush_tlb();

        for i in 0..SIZE_2MB / core::mem::size_of::<u64>() {
            u_assert_eq!(
                // SAFETY: We have just written this memory and confirmed it is accessible
                unsafe { core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64) },
                0x05D05D05D05D05D0
            );
        }

        GCD.free_memory_space(addr, SIZE_2MB).map_err(|_| "Failed to free 2MB memory")?;
    }
    Ok(())
}

/// Stability Test: Split a 1GB page into 4KB pages and verify correctness
#[patina_test]
#[on(timer = 3_000_000)] // 300ms interval
fn page_table_tests_1gb_split() -> patina::test::Result {
    let addr = GCD.allocate_memory_space(
        AllocateType::TopDown(None),
        GcdMemoryType::SystemMemory,
        30, // alignment shift for 1GB
        SIZE_1GB,
        0x05D as efi::Handle,
        None,
    );

    // let's give 'em a break if they don't have a free gig laying around
    if let Ok(addr) = addr {
        u_assert_eq!(addr % SIZE_1GB, 0);
        GCD.set_memory_space_attributes(addr, SIZE_1GB, efi::MEMORY_XP | efi::MEMORY_RO)
            .map_err(|_| "Failed to set attributes on 1GB region")?;

        // SAFETY: We just allocated this memory and it is marked read only, so it is safe to read from it.
        unsafe {
            // ensure we can still read the memory after marking it read only
            let _ = core::ptr::read_volatile(addr as *const u8);
        }

        // Make sure the page is actually a 1GB page by reading the entry in the self map
        let mut pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level3);
        let mut idx = get_index(addr as u64, PageTableLevel::Level3);
        let mut entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        let mut pte_state = get_pte_state(*entry, PageTableLevel::Level3);

        u_assert!(pte_state.points_to_pa);
        u_assert_eq!(pte_state.attributes, MemoryAttributes::ExecuteProtect | MemoryAttributes::ReadOnly);
        u_assert_eq!(pte_state.next_address, addr as u64);
        u_assert!(pte_state.present);

        GCD.set_memory_space_attributes(addr, SIZE_4KB, efi::MEMORY_XP)
            .map_err(|_| "Failed to set attributes on 4KB region")?;

        // SAFETY: We just allocated this memory and marked it writeable, so it is safe to write/read
        unsafe {
            // ensure we can now write to the memory and read it back
            core::ptr::write_volatile(addr as *mut u32, 0x05D);
            u_assert_eq!(core::ptr::read_volatile(addr as *const u8), 0x05D);
        }

        // Now let's check that we split the page correctly
        // The PTE at level 3 should now point to a page table, not a 1GB page
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level3);
        idx = get_index(addr as u64, PageTableLevel::Level3);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        u_assert!(!pte_state.points_to_pa);
        u_assert!(pte_state.present);
        u_assert!(pte_state.next_address != addr as u64);

        // At level 1 we should see we are pointing to all to 4KB pages since we mapped the whole range
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level1);
        for (i, pte) in pt.iter().enumerate() {
            pte_state = get_pte_state(*pte, PageTableLevel::Level1);
            u_assert!(pte_state.points_to_pa);
            u_assert!(pte_state.next_address == (addr + (i * SIZE_4KB)) as u64);
            u_assert!(pte_state.present);
            if i == 0 {
                u_assert_eq!(pte_state.attributes, MemoryAttributes::ExecuteProtect);
            } else {
                u_assert_eq!(pte_state.attributes, MemoryAttributes::ReadOnly | MemoryAttributes::ExecuteProtect);
            }
        }

        // now let's confirm that the next 2MB is still a large page
        pt = get_self_mapped_page_table((addr + SIZE_2MB) as u64, PageTableLevel::Level2);
        idx = get_index((addr + SIZE_2MB) as u64, PageTableLevel::Level2);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(pte_state.points_to_pa);
        u_assert!(pte_state.present);
        u_assert_eq!(pte_state.next_address, (addr + SIZE_2MB) as u64);

        GCD.free_memory_space(addr, SIZE_1GB).map_err(|_| "Failed to free 1GB memory")?;
    }

    Ok(())
}

/// Stability Test: Map a 2MB page, unmap it, map a 4KB region in it, pattern it, flush tlbs, and verify pattern
#[patina_test]
#[on(timer = 3_000_000)] // 300ms interval
fn page_table_tests_2mb_unmap() -> patina::test::Result {
    let mut addr_vec = Vec::new();
    for _ in 0..19 {
        addr_vec.push(
            GCD.allocate_memory_space(
                AllocateType::TopDown(None),
                GcdMemoryType::SystemMemory,
                21, // alignment shift for 2MB
                SIZE_2MB,
                0x05D as efi::Handle,
                None,
            )
            .unwrap(),
        );
    }

    for addr in addr_vec {
        u_assert_eq!(addr % SIZE_2MB, 0, "Allocated address is not 2MB aligned");

        // Make sure the page is actually a 2MB page by reading the entry in the self map
        let mut pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level2);
        let mut idx = get_index(addr as u64, PageTableLevel::Level2);
        let mut entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        let mut pte_state = get_pte_state(*entry, PageTableLevel::Level2);

        u_assert!(pte_state.points_to_pa, "2MB page does not point to PA");
        u_assert_eq!(pte_state.attributes, MemoryAttributes::ExecuteProtect, "2MB page attributes incorrect");
        u_assert_eq!(pte_state.next_address, addr as u64, "2MB page next address incorrect");
        u_assert!(pte_state.present, "2MB page not present");

        // unmap the region
        GCD.set_memory_space_attributes(addr, SIZE_2MB, efi::MEMORY_RP | efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 2MB region")?;

        // confirm it got unmapped
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(!pte_state.present, "2MB page still present after unmap");

        // now remap a 4KB region in the 2MB range
        GCD.set_memory_space_attributes(addr, SIZE_4KB, efi::MEMORY_WB | efi::MEMORY_XP)
            .map_err(|_| "Failed to set attributes on 4KB region")?;

        // make sure it got remapped as a 4KB page
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(!pte_state.points_to_pa, "Split did not occur");
        u_assert!(pte_state.next_address != addr as u64, "Next address incorrect, split did not occur");
        u_assert!(pte_state.present, "Split failed to mark new page table entry present");

        // make sure the next 4KB entry is still unmapped
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level1);
        idx = get_index(addr as u64, PageTableLevel::Level1);
        entry = pt.get((idx + 1) as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level1);
        u_assert!(!pte_state.present, "Next 4KB page present when it should be unmapped");

        // pattern the page
        // SAFETY: We just allocated this memory and marked it writeable so it is safe to write/read
        unsafe {
            // ensure we can now write to the memory and read it back
            for i in 0..SIZE_4KB / core::mem::size_of::<u64>() {
                core::ptr::write_volatile((addr + i * core::mem::size_of::<u64>()) as *mut u64, 0x05D05D05D05D05D0);
            }

            for i in 0..SIZE_4KB / core::mem::size_of::<u64>() {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0,
                    "Pattern verification failed before TLB flush"
                );
            }
        }

        flush_tlb();

        // Make sure that we can still read back the pattern after TLB flush
        for i in 0..SIZE_4KB / core::mem::size_of::<u64>() {
            // SAFETY: We have just mapped this memory and confirmed it is accessible
            unsafe {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0,
                    "Pattern verification failed after TLB flush"
                );
            }
        }

        GCD.free_memory_space(addr, SIZE_2MB).map_err(|_| "Failed to free 2MB memory")?;
    }
    Ok(())
}

/// Stability Test: Map a 1GB page, unmap it, map a 2MB region in it, pattern it, flush tlbs, and verify pattern
#[patina_test]
#[on(timer = 3_000_000)] // 300ms interval
fn page_table_tests_1gb_unmap_2mb_remap() -> patina::test::Result {
    let addr = GCD.allocate_memory_space(
        AllocateType::TopDown(None),
        GcdMemoryType::SystemMemory,
        30, // alignment shift for 1GB
        SIZE_1GB,
        0x05D as efi::Handle,
        None,
    );

    // let's give 'em a break if they don't have a free gig laying around
    if let Ok(addr) = addr {
        u_assert_eq!(addr % SIZE_1GB, 0, "Allocated address is not 1GB aligned");

        // Make sure the page is actually a 1GB page by reading the entry in the self map
        let mut pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level3);
        let mut idx = get_index(addr as u64, PageTableLevel::Level3);
        let mut entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        let mut pte_state = get_pte_state(*entry, PageTableLevel::Level3);

        u_assert!(pte_state.points_to_pa, "1GB page does not point to PA");
        u_assert_eq!(pte_state.attributes, MemoryAttributes::ExecuteProtect, "1GB page attributes incorrect");
        u_assert_eq!(pte_state.next_address, addr as u64, "1GB page next address incorrect");
        u_assert!(pte_state.present, "1GB page not present");

        // unmap the region
        GCD.set_memory_space_attributes(addr, SIZE_1GB, efi::MEMORY_RP | efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 1GB region")?;

        // confirm it got unmapped
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        u_assert!(!pte_state.present, "1GB page still present after unmap");

        // now remap a 2MB region in the 1GB range
        GCD.set_memory_space_attributes(addr, SIZE_2MB, efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 2MB region")?;

        // make sure it got remapped as a 2MB page
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        u_assert!(!pte_state.points_to_pa, "Split did not occur");
        u_assert!(pte_state.next_address != addr as u64, "Next address incorrect, split did not occur");
        u_assert!(pte_state.present, "Split failed to mark new page table entry present");

        // Should be a 2MB page now, so check level 2 entry
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level2);
        idx = get_index(addr as u64, PageTableLevel::Level2);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(pte_state.points_to_pa, "2MB page does not point to PA after remap");
        u_assert_eq!(pte_state.next_address, addr as u64, "2MB page next address incorrect after remap");
        u_assert!(pte_state.present, "2MB page not present after remap");

        // make sure the next 2MB entry is still unmapped
        entry = pt.get((idx + 1) as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(!pte_state.present, "Next 2MB page present when it should be unmapped");

        // pattern the 2MB region
        // SAFETY: We just allocated this memory and marked it writeable so it is safe to write/read
        unsafe {
            // ensure we can now write to the memory and read it back
            for i in 0..SIZE_2MB / core::mem::size_of::<u64>() {
                core::ptr::write_volatile((addr + i * core::mem::size_of::<u64>()) as *mut u64, 0x05D05D05D05D05D0);
            }

            for i in 0..SIZE_2MB / core::mem::size_of::<u64>() {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0,
                    "Pattern verification failed before TLB flush"
                );
            }
        }

        flush_tlb();

        // Make sure that we can still read back the pattern after TLB flush
        for i in 0..SIZE_2MB / core::mem::size_of::<u64>() {
            // SAFETY: We have just mapped this memory and confirmed it is accessible
            unsafe {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0,
                    "Pattern verification failed after TLB flush"
                );
            }
        }

        GCD.free_memory_space(addr, SIZE_1GB).map_err(|_| "Failed to free 1GB memory")?;
    }
    Ok(())
}

/// Stability Test: Map a 1GB page, unmap it, map a 4KB region in it, pattern it, flush tlbs, and verify pattern
#[patina_test]
#[on(timer = 3_000_000)] // 300ms interval
fn page_table_tests_1gb_unmap_4kb_remap() -> patina::test::Result {
    let addr = GCD.allocate_memory_space(
        AllocateType::TopDown(None),
        GcdMemoryType::SystemMemory,
        30, // alignment shift for 1GB
        SIZE_1GB,
        0x05D as efi::Handle,
        None,
    );

    // let's give 'em a break if they don't have a free gig laying around
    if let Ok(addr) = addr {
        u_assert_eq!(addr % SIZE_1GB, 0, "Allocated address is not 1GB aligned");

        // Make sure the page is actually a 1GB page by reading the entry in the self map
        let mut pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level3);
        let mut idx = get_index(addr as u64, PageTableLevel::Level3);
        let mut entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        let mut pte_state = get_pte_state(*entry, PageTableLevel::Level3);

        u_assert!(pte_state.points_to_pa, "1GB page does not point to PA");
        u_assert_eq!(pte_state.attributes, MemoryAttributes::ExecuteProtect, "1GB page attributes incorrect");
        u_assert_eq!(pte_state.next_address, addr as u64, "1GB page next address incorrect");
        u_assert!(pte_state.present, "1GB page not present");

        // unmap the region
        GCD.set_memory_space_attributes(addr, SIZE_1GB, efi::MEMORY_RP | efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 1GB region")?;

        // confirm it got unmapped
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        u_assert!(!pte_state.present, "1GB page still present after unmap");

        // now remap a 4KB region in the 1GB range
        GCD.set_memory_space_attributes(addr, SIZE_4KB, efi::MEMORY_WB)
            .map_err(|_| "Failed to set attributes on 4KB region")?;

        // make sure it got remapped as a 4KB page
        pte_state = get_pte_state(*entry, PageTableLevel::Level3);
        u_assert!(!pte_state.points_to_pa, "Split did not occur");
        u_assert!(pte_state.next_address != addr as u64, "Next address incorrect, split did not occur");
        u_assert!(pte_state.present, "Split failed to mark new page table entry present");

        // 2MB pte should exist now
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level2);
        idx = get_index(addr as u64, PageTableLevel::Level2);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level2);
        u_assert!(!pte_state.points_to_pa, "2MB page points to PA after remap");
        u_assert!(pte_state.next_address != addr as u64, "2MB page next address incorrect after remap");
        u_assert!(pte_state.present, "2MB page not present after remap");

        // 4KB pte should exist now
        pt = get_self_mapped_page_table(addr as u64, PageTableLevel::Level1);
        idx = get_index(addr as u64, PageTableLevel::Level1);
        entry = pt.get(idx as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level1);
        u_assert!(pte_state.points_to_pa, "4KB page does not point to PA after remap");
        u_assert_eq!(pte_state.next_address, addr as u64, "4KB page next address incorrect after remap");
        u_assert!(pte_state.present, "4KB page not present after remap");

        // make sure the next 4KB entry is still unmapped
        entry = pt.get((idx + 1) as usize).ok_or("Index out of bounds")?;
        pte_state = get_pte_state(*entry, PageTableLevel::Level1);
        u_assert!(!pte_state.present, "Next 4KB page present when it should be unmapped");

        // pattern the 4KB region
        // SAFETY: We just allocated this memory and marked it writeable so it is safe to write/read
        unsafe {
            // ensure we can now write to the memory and read it back
            for i in 0..SIZE_4KB / core::mem::size_of::<u64>() {
                core::ptr::write_volatile((addr + i * core::mem::size_of::<u64>()) as *mut u64, 0x05D05D05D05D05D0);
            }

            for i in 0..SIZE_4KB / core::mem::size_of::<u64>() {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0,
                    "Pattern verification failed before TLB flush"
                );
            }
        }

        flush_tlb();

        // Make sure that we can still read back the pattern after TLB flush
        for i in 0..SIZE_4KB / core::mem::size_of::<u64>() {
            // SAFETY: We have just mapped this memory and confirmed it is accessible
            unsafe {
                u_assert_eq!(
                    core::ptr::read_volatile((addr + i * core::mem::size_of::<u64>()) as *const u64),
                    0x05D05D05D05D05D0,
                    "Pattern verification failed after TLB flush"
                );
            }
        }

        GCD.free_memory_space(addr, SIZE_1GB).map_err(|_| "Failed to free 1GB memory")?;
    }
    Ok(())
}
