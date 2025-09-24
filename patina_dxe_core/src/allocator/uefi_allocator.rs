//! UEFI Allocator
//!
//! Provides memory-type tracking and UEFI pool allocation semantics on top of [`SpinLockedFixedSizeBlockAllocator`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::gcd::SpinLockedGcd;
use mu_pi::hob::EFiMemoryTypeInformation;
use patina_sdk::error::EfiError;
use r_efi::efi;

use super::{
    AllocationStrategy,
    fixed_size_block_allocator::{AllocationStatistics, SpinLockedFixedSizeBlockAllocator},
};
use core::{
    alloc::{Allocator, GlobalAlloc, Layout},
    ffi::c_void,
    fmt::{self, Display},
    ops::Range,
    ptr::NonNull,
};

const POOL_SIG: u32 = 0x04151980; //arbitrary number.
const UEFI_POOL_ALIGN: usize = 8; //per UEFI spec.

struct AllocationInfo {
    signature: u32,
    memory_type: efi::MemoryType,
    layout: Layout,
}

/// UEFI Allocator
///
/// Wraps a [`SpinLockedFixedSizeBlockAllocator`] to provide additional UEFI-specific functionality:
/// - Association of a particular [`r_efi::efi::MemoryType`] with the allocator
/// - A pool implementation that allows tracking the layout and memory_type of UEFI pool allocations.
pub struct UefiAllocator {
    allocator: SpinLockedFixedSizeBlockAllocator,
}

impl UefiAllocator {
    /// Creates a new UEFI allocator using the provided `gcd`.
    ///
    /// See [`SpinLockedFixedSizeBlockAllocator::new`]
    pub const fn new(
        gcd: &'static SpinLockedGcd,
        memory_type: NonNull<EFiMemoryTypeInformation>,
        allocator_handle: efi::Handle,
        page_allocation_granularity: usize,
    ) -> Self {
        UefiAllocator {
            allocator: SpinLockedFixedSizeBlockAllocator::new(
                gcd,
                allocator_handle,
                memory_type,
                page_allocation_granularity,
            ),
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        self.allocator.reset();
    }

    /// Indicates whether the given pointer falls within a memory region managed by this allocator.
    ///
    /// See [`SpinLockedFixedSizeBlockAllocator::contains`]
    #[allow(dead_code)]
    pub fn contains(&self, ptr: NonNull<u8>) -> bool {
        self.allocator.contains(ptr)
    }

    /// Returns the UEFI memory type associated with this allocator.
    pub fn memory_type(&self) -> efi::MemoryType {
        self.allocator.memory_type()
    }

    /// Reserves a range of memory to be used by this allocator of the given size in pages.
    ///
    /// The caller specifies a maximum number of pages this allocator is expected to require, and as long as the number
    /// of pages actually used by the allocator is less than that amount, then all the allocations for this allocator
    /// will be in a single contiguous block. This capability can be used to ensure that the memory map presented to the
    /// OS is stable from boot-to-boot despite small boot-to-boot variations in actual page usage.
    ///
    /// For best memory stability, this routine should be called only during the initialization of the memory subsystem;
    /// calling it after other allocations/frees have occurred will not cause allocation errors, but may cause the
    /// memory map to vary from boot-to-boot.
    ///
    /// This routine will return Err(efi::Status::ALREADY_STARTED) if it is called more than once.
    ///
    pub fn reserve_memory_pages(&self, pages: usize) -> Result<(), EfiError> {
        self.allocator.reserve_memory_pages(pages)
    }

    /// Returns an iterator over the memory ranges managed by this allocator.
    /// Returns an empty iterator if the allocator has no memory ranges.
    pub(crate) fn get_memory_ranges(&self) -> impl Iterator<Item = Range<efi::PhysicalAddress>> {
        self.allocator
            .get_memory_ranges()
            .map(|range| range.start as efi::PhysicalAddress..range.end as efi::PhysicalAddress)
    }

    /// Allocates a buffer to satisfy `size` and returns in `buffer`.
    ///
    /// # Safety
    /// Buffer input must be a valid memory location to write the allocation to.
    ///
    /// Memory allocated by this routine should be freed by [`Self::free_pool`]
    pub unsafe fn allocate_pool(&self, size: usize, buffer: *mut *mut c_void) -> Result<(), EfiError> {
        let mut allocation_info = AllocationInfo {
            signature: POOL_SIG,
            memory_type: self.memory_type(),
            layout: Layout::new::<AllocationInfo>(),
        };
        let offset: usize;
        (allocation_info.layout, offset) = allocation_info
            .layout
            .extend(
                Layout::from_size_align(size, UEFI_POOL_ALIGN)
                    .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}")),
            )
            .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}"));

        match self.allocator.allocate(allocation_info.layout) {
            Ok(ptr) => {
                let alloc_info_ptr = ptr.as_mut_ptr() as *mut AllocationInfo;
                unsafe {
                    alloc_info_ptr.write(allocation_info);
                    buffer.write((ptr.as_ptr() as *mut u8 as usize + offset) as *mut c_void);
                }
                Ok(())
            }
            Err(_) => Err(EfiError::OutOfResources),
        }
    }

    /// Frees a buffer allocated by [`Self::allocate_pool`]
    ///
    /// ## Safety
    ///
    /// Caller must guarantee that `buffer` was originally allocated by [`Self::allocate_pool`]
    pub unsafe fn free_pool(&self, buffer: *mut c_void) -> Result<(), EfiError> {
        let (_, offset) = Layout::new::<AllocationInfo>()
            .extend(
                Layout::from_size_align(0, UEFI_POOL_ALIGN)
                    .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}")),
            )
            .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}"));

        //TODO: trusting that "buffer" is legit is pretty naive - but performant. Presently the allocator doesn't have
        //tracking mechanisms that permit the validation of the pointer (hence the unsafe).
        let allocation_info: *mut AllocationInfo = ((buffer as usize) - offset) as *mut AllocationInfo;

        //must be true for any pool allocation
        if unsafe { (*allocation_info).signature } != POOL_SIG {
            debug_assert!(false, "Pool signature is incorrect.");
            return Err(EfiError::InvalidParameter);
        }
        // check if allocation is from this pool.
        if unsafe { (*allocation_info).memory_type } != self.memory_type() {
            return Err(EfiError::NotFound);
        }
        //zero after check so it doesn't get reused.
        unsafe {
            (*allocation_info).signature = 0;
        }
        if let Some(non_null_ptr) = NonNull::new(allocation_info as *mut u8) {
            unsafe { self.allocator.deallocate(non_null_ptr, (*allocation_info).layout) };
        } else {
            return Err(EfiError::InvalidParameter);
        }
        Ok(())
    }

    /// Attempts to allocate the given number of pages according to the given allocation strategy.
    /// Valid allocation strategies are:
    /// - BottomUp(None): Allocate the block of pages from the lowest available free memory.
    /// - BottomUp(Some(address)): Allocate the block of pages from the lowest available free memory. Fail if memory
    ///   cannot be found below `address`.
    /// - TopDown(None): Allocate the block of pages from the highest available free memory.
    /// - TopDown(Some(address)): Allocate the block of pages from the highest available free memory. Fail if memory
    ///   cannot be found above `address`.
    /// - Address(address): Allocate the block of pages at exactly the given address (or fail).
    ///
    /// If an address is specified as part of a strategy, it must be page-aligned.
    pub fn allocate_pages(
        &self,
        allocation_strategy: AllocationStrategy,
        pages: usize,
        alignment: usize,
    ) -> Result<core::ptr::NonNull<[u8]>, EfiError> {
        self.allocator.allocate_pages(allocation_strategy, pages, alignment)
    }

    /// Frees the block of pages at the given address of the given size.
    /// ## Safety
    /// Caller must ensure that the given address corresponds to a valid block of pages that was allocated with
    /// [Self::allocate_pages]
    pub unsafe fn free_pages(&self, address: usize, pages: usize) -> Result<(), EfiError> {
        unsafe { self.allocator.free_pages(address, pages) }
    }

    /// Returns the allocator handle associated with this allocator.
    pub fn handle(&self) -> efi::Handle {
        self.allocator.handle()
    }

    /// Returns the reserved memory range, if any.
    #[allow(dead_code)]
    pub fn reserved_range(&self) -> Option<Range<efi::PhysicalAddress>> {
        self.allocator.reserved_range()
    }

    /// Returns the allocator stats
    #[allow(dead_code)]
    pub fn stats(&self) -> AllocationStatistics {
        self.allocator.stats()
    }
}

unsafe impl GlobalAlloc for UefiAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        unsafe { self.allocator.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        unsafe { self.allocator.dealloc(ptr, layout) }
    }
}

unsafe impl Allocator for UefiAllocator {
    fn allocate(&self, layout: core::alloc::Layout) -> Result<core::ptr::NonNull<[u8]>, core::alloc::AllocError> {
        self.allocator.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: core::ptr::NonNull<u8>, layout: core::alloc::Layout) {
        unsafe { self.allocator.deallocate(ptr, layout) }
    }
}

// returns a string for the given memory type.
fn string_for_memory_type(memory_type: efi::MemoryType) -> &'static str {
    match memory_type {
        efi::LOADER_CODE => "Loader Code",
        efi::LOADER_DATA => "Loader Data",
        efi::BOOT_SERVICES_CODE => "BootServices Code",
        efi::BOOT_SERVICES_DATA => "BootServices Data",
        efi::RUNTIME_SERVICES_CODE => "RuntimeServices Code",
        efi::RUNTIME_SERVICES_DATA => "RuntimeServices Data",
        efi::ACPI_RECLAIM_MEMORY => "ACPI Reclaim",
        efi::ACPI_MEMORY_NVS => "ACPI NVS",
        _ => "Unknown",
    }
}

impl Display for UefiAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Memory Type: {}", string_for_memory_type(self.memory_type()))?;
        self.allocator.fmt(f)
    }
}
#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use core::cmp::max;
    use std::alloc::{GlobalAlloc, System};

    use mu_pi::dxe_services;
    use patina_sdk::base::{SIZE_4KB, SIZE_64KB, UEFI_PAGE_SIZE};

    use crate::{
        allocator::{DEFAULT_ALLOCATION_STRATEGY, DEFAULT_PAGE_ALLOCATION_GRANULARITY},
        test_support,
    };

    use super::*;

    fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
        unsafe { gcd.reset() };

        gcd.init(48, 16);
        let layout = Layout::from_size_align(size, UEFI_PAGE_SIZE).unwrap();
        let base = unsafe { System.alloc(layout) as u64 };
        unsafe {
            gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, base as usize, size, efi::MEMORY_WB)
                .unwrap();
        }
        base
    }

    // this runs each test twice, once with 4KB page allocation granularity and once with 64KB page allocation
    // granularity. This is to ensure that the allocator works correctly with both page allocation granularities.
    fn with_granularity_modulation<F: Fn(usize) + std::panic::RefUnwindSafe>(f: F) {
        f(DEFAULT_PAGE_ALLOCATION_GRANULARITY);
        f(SIZE_64KB);
    }

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_uefi_allocator_new() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
            let ua = UefiAllocator::new(
                &GCD,
                NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_DATA)),
                1 as _,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            assert_eq!(ua.memory_type(), efi::BOOT_SERVICES_DATA);
        });
    }

    #[test]
    fn test_allocate_pool() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                let base = init_gcd(&GCD, 0x400000);

                let ua = UefiAllocator::new(
                    &GCD,
                    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_DATA)),
                    1 as _,
                    granularity,
                );

                let mut buffer: *mut c_void = core::ptr::null_mut();
                assert!(unsafe { ua.allocate_pool(0x1000, core::ptr::addr_of_mut!(buffer)) }.is_ok());
                assert!(buffer as u64 > base);
                assert!((buffer as u64) < base + 0x400000);

                let (layout, offset) = Layout::new::<AllocationInfo>()
                    .extend(
                        Layout::from_size_align(0x1000, UEFI_POOL_ALIGN)
                            .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}")),
                    )
                    .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}"));

                let allocation_info: *mut AllocationInfo = ((buffer as usize) - offset) as *mut AllocationInfo;
                unsafe {
                    let allocation_info = &*allocation_info;
                    assert_eq!(allocation_info.signature, POOL_SIG);
                    assert_eq!(allocation_info.memory_type, efi::RUNTIME_SERVICES_DATA);
                    assert_eq!(allocation_info.layout, layout)
                }
            });
        });
    }

    #[test]
    fn test_free_pool() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                let base = init_gcd(&GCD, 0x400000);

                let ua = UefiAllocator::new(
                    &GCD,
                    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_DATA)),
                    1 as _,
                    granularity,
                );

                let mut buffer: *mut c_void = core::ptr::null_mut();
                assert!(unsafe { ua.allocate_pool(0x1000, core::ptr::addr_of_mut!(buffer)) }.is_ok());

                assert!(unsafe { ua.free_pool(buffer) }.is_ok());

                let (_, offset) = Layout::new::<AllocationInfo>()
                    .extend(
                        Layout::from_size_align(0x1000, UEFI_POOL_ALIGN)
                            .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}")),
                    )
                    .unwrap_or_else(|err| panic!("Allocation layout error: {err:#?}"));

                let allocation_info: *mut AllocationInfo = ((buffer as usize) - offset) as *mut AllocationInfo;
                unsafe {
                    let allocation_info = &*allocation_info;
                    assert_eq!(allocation_info.signature, 0);
                }

                let prev_buffer = buffer;
                assert!(unsafe { ua.allocate_pool(0x1000, core::ptr::addr_of_mut!(buffer)) }.is_ok());
                assert!(buffer as u64 > base);
                assert!((buffer as u64) < base + 0x400000);
                assert_eq!(buffer, prev_buffer);
            });
        });
    }

    #[test]
    fn test_allocate_and_free_pages() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                let base = init_gcd(&GCD, 0x400000);

                let ua = UefiAllocator::new(
                    &GCD,
                    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_DATA)),
                    1 as _,
                    granularity,
                );

                let buffer = ua.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 4, UEFI_PAGE_SIZE).unwrap();
                let buffer_address = buffer.as_ptr() as *mut u8 as efi::PhysicalAddress;
                assert_eq!(buffer_address & 0xFFF, 0); // must be page aligned.
                assert_eq!(buffer.len(), max(granularity, UEFI_PAGE_SIZE * 4)); //should be 4 pages or granularity pages in size.
                assert!(buffer_address >= base);
                assert!(buffer_address < base + 0x400000);

                unsafe {
                    ua.free_pages(buffer_address as usize, 4).unwrap();
                }

                let buffer =
                    ua.allocate_pages(AllocationStrategy::Address(buffer_address as usize), 4, UEFI_PAGE_SIZE).unwrap();
                let buffer_address2 = buffer.as_ptr() as *mut u8 as efi::PhysicalAddress;
                assert_eq!(buffer_address, buffer_address2);
                assert_eq!(buffer.len(), max(granularity, UEFI_PAGE_SIZE * 4)); //should be 4 pages or granularity pages in size.

                unsafe {
                    ua.free_pages(buffer_address2 as usize, 4).unwrap();
                }
            });
        });
    }

    #[test]
    fn free_pages_should_only_succeed_in_the_source_allocator() {
        with_locked_state(|| {
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            init_gcd(&GCD, 0x400000);

            let bs_allocator = UefiAllocator::new(
                &GCD,
                NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_DATA)),
                1 as _,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            let bc_allocator = UefiAllocator::new(
                &GCD,
                NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_CODE)),
                2 as _,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );

            let bs_buffer = bs_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 4, UEFI_PAGE_SIZE).unwrap();
            let bc_buffer = bc_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 4, UEFI_PAGE_SIZE).unwrap();

            let bs_buffer_address = bs_buffer.as_ptr() as *mut u8 as efi::PhysicalAddress;
            let bc_buffer_address = bc_buffer.as_ptr() as *mut u8 as efi::PhysicalAddress;

            unsafe {
                assert_eq!(bs_allocator.free_pages(bc_buffer_address as usize, 4), Err(EfiError::NotFound));
                assert_eq!(bc_allocator.free_pages(bs_buffer_address as usize, 4), Err(EfiError::NotFound));

                bs_allocator.free_pages(bs_buffer_address as usize, 4).unwrap();
                bc_allocator.free_pages(bc_buffer_address as usize, 4).unwrap();
            }
        });
    }

    #[test]
    fn test_system_alloc_dealloc() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
                let _ = init_gcd(&GCD, 0x400000);

                let ua = UefiAllocator::new(
                    &GCD,
                    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_DATA)),
                    1 as _,
                    granularity,
                );

                let layout = Layout::from_size_align(0x8, 0x8).unwrap();
                unsafe {
                    let a = ua.alloc(layout);
                    ua.dealloc(a, layout)
                }

                unsafe {
                    let a = ua.alloc(layout);
                    ua.deallocate(NonNull::new_unchecked(a), layout);
                }
            });
        });
    }

    #[test]
    fn test_contains() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let ua = UefiAllocator::new(
                &GCD,
                NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_DATA)),
                1 as _,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );

            let layout = Layout::from_size_align(0x8, 0x8).unwrap();
            let allocation = ua.allocate(layout).unwrap().as_non_null_ptr();
            assert!(ua.contains(allocation));
        });
    }

    #[test]
    fn test_uefi_allocator_fn_conformance() {
        with_locked_state(|| {
            // Create a static GCD
            static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

            // Allocate some space on the heap with the global allocator (std) to be used by expand().
            init_gcd(&GCD, 0x400000);

            let ua = UefiAllocator::new(
                &GCD,
                NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_DATA)),
                1 as _,
                DEFAULT_PAGE_ALLOCATION_GRANULARITY,
            );
            assert_eq!(ua.memory_type(), efi::BOOT_SERVICES_DATA);
            assert_eq!(ua.handle(), 1 as _);

            assert_eq!(
                std::format!("{ua}"),
                concat!(
                    "Memory Type: BootServices Data\n",
                    "Memory Type: 4\n",
                    "Allocation Ranges:\n",
                    "Bucket Range: None\n",
                    "Allocation Stats:\n",
                    "  pool_allocation_calls: 0\n",
                    "  pool_free_calls: 0\n",
                    "  page_allocation_calls: 0\n",
                    "  page_free_calls: 0\n",
                    "  reserved_size: 0\n",
                    "  reserved_used: 0\n",
                    "  claimed_pages: 0\n"
                )
            );
        });
    }

    #[test]
    fn reserve_memory_pages_reserves_the_pages() {
        with_granularity_modulation(|granularity| {
            with_locked_state(|| {
                static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

                let base = init_gcd(&GCD, 0x400000);
                let gcd_range = base..base + 0x400000;

                let reserved_allocator = UefiAllocator::new(
                    &GCD,
                    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_DATA)),
                    1 as _,
                    granularity,
                );
                reserved_allocator.reserve_memory_pages(0x100).unwrap();

                let unreserved_allocator = UefiAllocator::new(
                    &GCD,
                    NonNull::from_ref(GCD.memory_type_info(efi::LOADER_DATA)),
                    2 as _,
                    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
                );

                //check that the ranges are set up.
                let allocator = reserved_allocator.allocator.lock();
                let reserved_range = allocator.reserved_range.clone().unwrap();
                assert!(gcd_range.contains(&reserved_range.start));
                assert!(gcd_range.contains(&(reserved_range.end - 1)));
                drop(allocator);

                let allocator = unreserved_allocator.allocator.lock();
                assert!(allocator.reserved_range.is_none());
                drop(allocator);

                let mut reserved_page_addr = 0;

                //verify that the first 0x100 pages from the reserved allocator are in the reserved_range, and that allocating
                //from the unreserved allocator at the same time doesn't allocate from the reserved range or cause the reserved
                //allocator to fail in any way.
                for page in 0..0x100 {
                    // if we are using 64KB granularity, 16 pages will be allocated under the hood, so if at that
                    // granularity, only allocate 16 pages, otherwise just check these addresses are mapped.
                    if granularity == SIZE_4KB || page % 16 == 0 {
                        let reserved_page =
                            reserved_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                        reserved_page_addr = reserved_page.as_ptr() as *mut u8 as u64;
                    } else {
                        reserved_page_addr += UEFI_PAGE_SIZE as u64;
                    }
                    assert!(reserved_range.contains(&(reserved_page_addr)));
                    assert!(reserved_range.contains(&(reserved_page_addr + 0xFFF)));

                    let unreserved_page =
                        unreserved_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                    let unreserved_page_addr = unreserved_page.as_ptr() as *mut u8 as u64;
                    assert!(!reserved_range.contains(&(unreserved_page_addr)));
                    assert!(!reserved_range.contains(&(unreserved_page_addr + 0xFFF)));
                }

                //verify that further page allocations from the reserved allocator are outside the reserved range but succeed.
                let reserved_page =
                    reserved_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                let reserved_page_addr = reserved_page.as_ptr() as *mut u8 as u64;
                assert!(!reserved_range.contains(&(reserved_page_addr)));
                assert!(!reserved_range.contains(&(reserved_page_addr + 0xFFF)));

                //verify that if the reserved allocation that is not in the reserved range is freed, other allocators can
                //use it.
                unsafe {
                    reserved_allocator.free_pages(reserved_page_addr as usize, 1).unwrap();
                }
                let unreserved_page = unreserved_allocator
                    .allocate_pages(AllocationStrategy::Address(reserved_page_addr as usize), 1, UEFI_PAGE_SIZE)
                    .unwrap();
                let unreserved_page_addr = unreserved_page.as_ptr() as *mut u8 as u64;
                assert_eq!(
                    reserved_page_addr, unreserved_page_addr,
                    "reserved_page_addr: {reserved_page_addr:#x?}, unreserved_page_addr: {unreserved_page_addr:#x?}",
                );

                //verify that if pages are freed within the reserved range, that other allocators cannot use them.
                unsafe {
                    reserved_allocator.free_pages(reserved_range.start as usize, 0x10).unwrap();
                }
                let unreserved_page =
                    unreserved_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                let unreserved_page_addr = unreserved_page.as_ptr() as *mut u8 as u64;
                assert!(!reserved_range.contains(&(unreserved_page_addr)));
                assert!(!reserved_range.contains(&(unreserved_page_addr + 0xFFF)));

                let mut reserved_page_addr = 0;
                //verify that previously freed pages within the reserved range can be reused by the reserving allocator.
                for page in 0..0x10 {
                    if granularity == SIZE_4KB || page == 0 {
                        let reserved_page =
                            reserved_allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, 1, UEFI_PAGE_SIZE).unwrap();
                        reserved_page_addr = reserved_page.as_ptr() as *mut u8 as u64;
                    } else {
                        reserved_page_addr += UEFI_PAGE_SIZE as u64;
                    }

                    assert!(reserved_range.contains(&(reserved_page_addr)));
                    assert!(reserved_range.contains(&(reserved_page_addr + 0xFFF)));
                }
            });
        });
    }
}
