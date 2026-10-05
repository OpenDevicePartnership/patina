//! DXE Core Patina Test Allocator Tests
//!
//! These tests verify the pool allocator's page retirement behavior against the live page table.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use super::test_support::is_mapped;
use crate::{
    GCD,
    allocator::{EFI_BOOT_SERVICES_DATA_ALLOCATOR, STATIC_ALLOCATORS},
    events::{raise_tpl, restore_tpl},
};
use alloc::vec::Vec;
use core::{
    alloc::{Allocator, Layout},
    ffi::c_void,
    ops::Range,
};
use patina::{UEFI_PAGE_SIZE, pi::dxe_services::GcdMemoryType};
use patina_test::{patina_test, u_assert, u_assert_eq};
// used in the macro, but not directly referenced; causes a warning if patina tests not enabled.
#[allow(unused)]
use patina::BinaryGuid;
#[allow(unused)]
use patina::standard::efi;

fn page_attributes(address: usize) -> Result<u64, &'static str> {
    GCD.get_memory_descriptor_for_address(address as efi::PhysicalAddress, |d, _| {
        d.memory_type != GcdMemoryType::NonExistent
    })
    .map(|descriptor| descriptor.attributes)
    .map_err(|_| "No GCD descriptor for pool memory")
}

/// Verifies that consecutive pool allocations return distinct addresses. The pool allocators are set up to
/// always serve new allocations in FIFO order. Interrupts are disabled in this test just to protect against the
/// rare case where event handling in the middle of the allocation could cause the free list to wrap around.
#[patina_test]
fn pool_allocations_with_interrupts_disabled_are_unique() -> patina_test::error::Result {
    const ALLOCATION_SIZE: usize = 0x20;

    let mut first: *mut c_void = core::ptr::null_mut();
    let mut second: *mut c_void = core::ptr::null_mut();

    let interrupts_enabled = patina::arch::interrupts_enabled();
    patina::arch::disable_interrupts();
    // SAFETY: Both out pointers refer to writable local variables.
    let first_result =
        unsafe { EFI_BOOT_SERVICES_DATA_ALLOCATOR.allocate_pool(ALLOCATION_SIZE, core::ptr::addr_of_mut!(first)) };
    // SAFETY: The out pointer refers to a writable local variable.
    let second_result =
        unsafe { EFI_BOOT_SERVICES_DATA_ALLOCATOR.allocate_pool(ALLOCATION_SIZE, core::ptr::addr_of_mut!(second)) };
    if interrupts_enabled {
        patina::arch::enable_interrupts();
    }

    first_result.map_err(|_| "First pool allocation failed")?;
    second_result.map_err(|_| "Second pool allocation failed")?;
    u_assert!(first != second, "Consecutive pool allocations returned the same address");

    // SAFETY: Both pointers were successfully allocated above by this allocator and have not yet been freed.
    unsafe {
        EFI_BOOT_SERVICES_DATA_ALLOCATOR.free_pool(first).map_err(|_| "Failed to free first pool allocation")?;
        EFI_BOOT_SERVICES_DATA_ALLOCATOR.free_pool(second).map_err(|_| "Failed to free second pool allocation")?;
    }

    Ok(())
}

/// Verifies that a pool page is unmapped once every block in it has been freed.
#[patina_test]
fn fully_freed_pool_page_is_unmapped() -> patina_test::error::Result {
    // A page sized block occupies an entire page on its own, so freeing it frees the whole page.
    let layout = Layout::from_size_align(UEFI_PAGE_SIZE, UEFI_PAGE_SIZE).map_err(|_| "Bad layout")?;

    for (allocator, memory_type) in &STATIC_ALLOCATORS {
        let allocation = allocator.allocate(layout).map_err(|_| "Failed to allocate a pool page")?.cast::<u8>();
        let page = allocation.addr().get();
        log::info!("Checking retirement of pool page {page:#x} for memory type {memory_type:#x}");

        u_assert_eq!(page % UEFI_PAGE_SIZE, 0);
        u_assert_eq!(page_attributes(page)? & efi::MEMORY_RP, 0, "In use pool page is marked RP");
        u_assert!(is_mapped(page as u64), "In use pool page is not mapped");

        // The page is mapped, so writing to it must not fault.
        // SAFETY: the allocation is a full page of memory owned by this test.
        unsafe { core::ptr::write_volatile(page as *mut u8, 0xA5) };

        let retired = allocator.stats().retired_pages;

        // SAFETY: the allocation was returned by this allocator for this layout.
        unsafe { allocator.deallocate(allocation, layout) };

        u_assert!(allocator.stats().retired_pages > retired, "Freed pool page was not retired");
        u_assert_eq!(page_attributes(page)? & efi::MEMORY_RP, efi::MEMORY_RP, "Freed pool page is not marked RP");
        u_assert!(!is_mapped(page as u64), "Freed pool page is still mapped");
    }

    Ok(())
}

/// Verifies that the allocators do not have memory mapped that they have not handed out, and that the retirement
/// state tracked by the GCD matches the page table.
#[patina_test]
#[on(event = BinaryGuid(efi::EVENT_GROUP_READY_TO_BOOT))]
fn unused_pool_memory_is_unmapped() -> patina_test::error::Result {
    let ranges: Vec<_> = STATIC_ALLOCATORS.iter().flat_map(|(allocator, _)| allocator.get_memory_ranges()).collect();

    // Raised so that nothing can map pool memory in while the page table is being compared against the GCD.
    let tpl = raise_tpl(efi::TPL_HIGH_LEVEL);
    let result = check_pool_pages(&ranges);
    restore_tpl(tpl);

    // Pool memory is claimed from the GCD in large regions and is only mapped as it is needed, so some of the memory
    // the allocators own is always unmapped.
    u_assert!(result? > 0, "No pool memory is unmapped");

    Ok(())
}

/// Confirms every pool page is mapped unless the GCD has it marked as retired, returning the number of retired pages.
fn check_pool_pages(ranges: &[Range<efi::PhysicalAddress>]) -> Result<usize, &'static str> {
    let mut unmapped_pages = 0;

    for range in ranges {
        for page in (range.start as usize..range.end as usize).step_by(UEFI_PAGE_SIZE) {
            let retired = page_attributes(page)? & efi::MEMORY_RP == efi::MEMORY_RP;
            if retired == is_mapped(page as u64) {
                log::error!("Pool page {page:#x} is out of sync with the GCD, GCD reports retired: {retired}");
                return Err("Pool page mapping does not match the GCD");
            }
            unmapped_pages += usize::from(retired);
        }
    }

    Ok(unmapped_pages)
}
