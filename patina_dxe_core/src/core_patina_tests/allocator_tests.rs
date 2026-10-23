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

#[cfg(target_os = "uefi")]
mod isolated {
    use super::{
        Allocator, GCD, Layout, UEFI_PAGE_SIZE, Vec, efi, is_mapped, page_attributes, patina_test, raise_tpl,
        restore_tpl, u_assert, u_assert_eq,
    };
    use crate::allocator::{DEFAULT_PAGE_ALLOCATION_GRANULARITY, SpinLockedFixedSizeBlockAllocator};
    use core::ptr::NonNull;

    struct RaisedTpl(efi::Tpl);

    impl RaisedTpl {
        fn new() -> Self {
            Self(raise_tpl(efi::TPL_HIGH_LEVEL))
        }
    }

    impl Drop for RaisedTpl {
        fn drop(&mut self) {
            restore_tpl(self.0);
        }
    }

    struct LiveBlock {
        pointer: NonNull<u8>,
        layout: Layout,
        live: bool,
    }

    struct Fixture<'owner> {
        allocator: Option<SpinLockedFixedSizeBlockAllocator>,
        blocks: Vec<LiveBlock>,
        _owner: &'owner u8,
    }

    impl<'owner> Fixture<'owner> {
        fn new(owner: &'owner u8) -> Self {
            Self {
                allocator: Some(SpinLockedFixedSizeBlockAllocator::new(
                    &GCD,
                    core::ptr::from_ref(owner).cast_mut().cast(),
                    efi::BOOT_SERVICES_DATA,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                    16 * UEFI_PAGE_SIZE,
                )),
                blocks: Vec::new(),
                _owner: owner,
            }
        }

        fn allocator(&self) -> &SpinLockedFixedSizeBlockAllocator {
            self.allocator.as_ref().expect("Fixture allocator exists until cleanup")
        }

        fn allocate(&mut self, size: usize) -> Result<usize, &'static str> {
            let layout = Layout::from_size_align(size, size).map_err(|_| "Bad isolated layout")?;
            self.blocks.try_reserve(1).map_err(|_| "Could not reserve test bookkeeping")?;
            let pointer = self.allocator().allocate(layout).map_err(|_| "Isolated allocation failed")?.cast::<u8>();
            let index = self.blocks.len();
            self.blocks.push(LiveBlock { pointer, layout, live: true });
            Ok(index)
        }

        fn page(&self, index: usize) -> usize {
            self.blocks.get(index).expect("Index belongs to fixture ledger").pointer.addr().get()
                & !(UEFI_PAGE_SIZE - 1)
        }

        fn free(&mut self, index: usize) {
            let block = self.blocks.get(index).expect("Index belongs to fixture ledger");
            if block.live {
                // SAFETY: bookkeeping retains the original pointer and layout, and prevents double frees.
                unsafe { self.allocator().deallocate(block.pointer, block.layout) };
                self.blocks.get_mut(index).expect("Index belongs to fixture ledger").live = false;
            }
        }

        fn check_pages(&self) -> patina_test::error::Result {
            for region in self.allocator().get_memory_ranges() {
                let metadata = self
                    .allocator()
                    .lock()
                    .firmware_test_metadata_range(region.start)
                    .ok_or("Missing isolated region metadata")?;
                for page in (region.start..region.end).step_by(UEFI_PAGE_SIZE) {
                    let occupied = metadata.contains(&page)
                        || self
                            .blocks
                            .iter()
                            .any(|block| block.live && (block.pointer.addr().get() & !(UEFI_PAGE_SIZE - 1)) == page);
                    u_assert_eq!(is_mapped(page as u64), occupied, "Page table does not match live block occupancy");
                    u_assert_eq!(
                        (page_attributes(page)? & efi::MEMORY_RP == 0),
                        occupied,
                        "GCD does not match live block occupancy"
                    );
                }
            }
            Ok(())
        }

        fn check_live_bytes(&self) -> patina_test::error::Result {
            for block in self.blocks.iter().filter(|block| block.live) {
                u_assert!(is_mapped(block.pointer.addr().get() as u64), "Live neighbor is unmapped");
                // SAFETY: the block remains live and writable; its page was checked before access.
                unsafe {
                    core::ptr::write_volatile(block.pointer.as_ptr(), 0xA5);
                    core::ptr::write_volatile(block.pointer.as_ptr().add(block.layout.size() - 1), 0x5A);
                    u_assert_eq!(core::ptr::read_volatile(block.pointer.as_ptr()), 0xA5);
                    u_assert_eq!(core::ptr::read_volatile(block.pointer.as_ptr().add(block.layout.size() - 1)), 0x5A);
                }
            }
            Ok(())
        }

        fn cleanup(&mut self) -> Result<(), &'static str> {
            if self.allocator.is_none() {
                return Ok(());
            }
            for index in 0..self.blocks.len() {
                self.free(index);
            }
            let regions = self.allocator().get_memory_ranges();
            self.allocator = None;
            let mut result = Ok(());
            for region in regions {
                if GCD.free_memory_space(region.start, region.end - region.start).is_err() {
                    result = Err("Failed to release isolated allocator backing region");
                }
            }
            result
        }
    }

    impl Drop for Fixture<'_> {
        fn drop(&mut self) {
            if let Err(error) = self.cleanup() {
                log::error!("{error}");
            }
        }
    }

    fn with_fixture(test: impl FnOnce(&mut Fixture<'_>) -> patina_test::error::Result) -> patina_test::error::Result {
        let _tpl = RaisedTpl::new();
        let owner = 0u8;
        let mut fixture = Fixture::new(&owner);
        let result = test(&mut fixture);
        fixture.cleanup()?;
        result
    }

    /// A partially freed small-block page stays accessible until its last live block is freed.
    #[patina_test]
    fn small_block_page_retires_only_after_last_free() -> patina_test::error::Result {
        with_fixture(|fixture| {
            let first = loop {
                let index = fixture.allocate(32)?;
                let page = fixture.page(index);
                let metadata_page = fixture.allocator().get_memory_ranges().any(|region| {
                    region.contains(&page)
                        && fixture
                            .allocator()
                            .lock()
                            .firmware_test_metadata_range(region.start)
                            .is_some_and(|metadata| metadata.contains(&page))
                });
                if !metadata_page {
                    break index;
                }
            };
            let page = fixture.page(first);
            for _ in 1..UEFI_PAGE_SIZE / 32 {
                let index = fixture.allocate(32)?;
                u_assert_eq!(fixture.page(index), page, "Small blocks did not fill one page");
            }
            let neighbor = fixture.allocate(32)?;
            u_assert_eq!(fixture.page(neighbor).abs_diff(page), UEFI_PAGE_SIZE, "Expected an adjacent live page");
            for index in first..neighbor - 1 {
                fixture.free(index);
            }
            fixture.check_pages()?;
            fixture.check_live_bytes()?;
            fixture.free(neighbor - 1);
            u_assert!(!is_mapped(page as u64), "Last free did not unmap the small-block page");
            fixture.check_pages()?;
            fixture.check_live_bytes()?;
            fixture.free(neighbor);
            fixture.check_pages()
        })
    }

    /// Every nonmetadata page is mapped exactly when independently tracked live blocks occupy it.
    #[patina_test]
    fn whole_regions_match_live_block_occupancy() -> patina_test::error::Result {
        with_fixture(|fixture| {
            for _ in 0..20 {
                fixture.allocate(UEFI_PAGE_SIZE)?;
            }
            u_assert!(fixture.allocator().get_memory_ranges().count() > 1, "Expected multiple backing regions");
            fixture.check_pages()?;
            for index in (0..fixture.blocks.len()).step_by(2) {
                fixture.free(index);
            }
            fixture.check_pages()?;
            fixture.check_live_bytes()?;
            for index in 0..fixture.blocks.len() {
                fixture.free(index);
            }
            fixture.check_pages()
        })
    }

    /// Retired pages can change block class without keeping empty pages mapped or harming live neighbors.
    #[patina_test]
    fn mixed_classes_reuse_and_retire_pages() -> patina_test::error::Result {
        with_fixture(|fixture| {
            let mut original_pages = Vec::new();
            for _ in 0..15 {
                let index = fixture.allocate(UEFI_PAGE_SIZE)?;
                original_pages.push(fixture.page(index));
            }
            let neighbor = fixture.allocate(UEFI_PAGE_SIZE)?;
            for index in 0..neighbor {
                fixture.free(index);
            }
            fixture.check_pages()?;
            let mut groups = Vec::new();
            for size in [16, 32, 64, 128, 256, 512, 1024, 2048] {
                let first = fixture.blocks.len();
                let mut reused = false;
                for _ in 0..UEFI_PAGE_SIZE / size {
                    let index = fixture.allocate(size)?;
                    reused |= original_pages.contains(&fixture.page(index));
                }
                u_assert!(reused, "Block class did not reuse a retired page from another class");
                groups.push(first..fixture.blocks.len());
                fixture.check_pages()?;
                fixture.check_live_bytes()?;
            }
            for group in &groups {
                for index in group.clone().step_by(2) {
                    fixture.free(index);
                }
            }
            fixture.check_pages()?;
            fixture.check_live_bytes()?;
            for group in groups {
                for index in group {
                    fixture.free(index);
                }
                fixture.check_pages()?;
                fixture.check_live_bytes()?;
            }
            fixture.free(neighbor);
            fixture.check_pages()
        })
    }
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
