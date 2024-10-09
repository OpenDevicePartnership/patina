#![feature(allocator_api)]
#![feature(slice_ptr_get)]
use mu_pi::dxe_services;
use uefi_allocator::fixed_size_block_allocator::SpinLockedFixedSizeBlockAllocator;
use uefi_gcd::gcd::SpinLockedGcd;

use std::alloc::{Allocator, GlobalAlloc, Layout, System};

fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
    let layout = Layout::from_size_align(size, 0x1000).unwrap();
    let base = unsafe { System.alloc(layout) as u64 };
    unsafe {
        gcd.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, base as usize, size, 0).unwrap();
    }
    base
}

#[test]
fn allocate_deallocate_test() {
    // Create a static GCD for test.
    static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
    GCD.init(48, 16);

    // Allocate some space on the heap with the global allocator (std) to be used by expand().
    init_gcd(&GCD, 0x400000);

    let fsb = SpinLockedFixedSizeBlockAllocator::new(&GCD, 1 as _, None);

    let layout = Layout::from_size_align(0x8, 0x8).unwrap();
    let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();

    unsafe { fsb.deallocate(allocation, layout) };

    let layout = Layout::from_size_align(0x20, 0x20).unwrap();
    let allocation = fsb.allocate(layout).unwrap().as_non_null_ptr();

    unsafe { fsb.deallocate(allocation, layout) };
}
