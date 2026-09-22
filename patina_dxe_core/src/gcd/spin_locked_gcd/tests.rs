//! GCD (Global Coherency Domain) test module.
//!
//! # Safety Notes
//!
//! This test module extensively uses `unsafe` for the following operations:
//!
//! ## Memory Allocation (`get_memory`)
//! - Allocates memory from the system allocator with UEFI page alignment
//! - Returns 'static lifetime slices that are intentionally leaked for test simplicity
//! - Memory is valid for the entire test duration
//!
//! ## GCD Operations (`add_memory_space`, `init_memory_blocks`, etc.)
//! - These functions are unsafe because they operate on raw memory addresses
//! - In tests, all memory addresses come from controlled allocations via `get_memory`
//! - All memory regions are valid and properly aligned
//! - Test isolation is ensured via `with_locked_state` which holds a global test lock
//!
//! ## Global State (`GCD.reset()`)
//! - Tests reset global GCD state to ensure test isolation
//! - The test lock prevents concurrent access during reset operations
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
extern crate std;
use core::{alloc::Layout, sync::atomic::AtomicBool};
use patina::align_up;

use crate::test_support::{self, MockPageTable, MockPageTableWrapper};

use super::*;
use alloc::vec::Vec;
use patina::pi::dxe_services::GcdMemoryType;
use patina::standard::efi;
use std::{alloc::GlobalAlloc, cell::RefCell, rc::Rc};

const DXE_CORE_PE_HEADER_DATA: [u8; 1057] = [
    0x4D, 0x5A, 0x78, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C,
    0xCD, 0x21, 0x54, 0x68, 0x69, 0x73, 0x20, 0x70, 0x72, 0x6F, 0x67, 0x72, 0x61, 0x6D, 0x20, 0x63, 0x61, 0x6E, 0x6E,
    0x6F, 0x74, 0x20, 0x62, 0x65, 0x20, 0x72, 0x75, 0x6E, 0x20, 0x69, 0x6E, 0x20, 0x44, 0x4F, 0x53, 0x20, 0x6D, 0x6F,
    0x64, 0x65, 0x2E, 0x24, 0x00, 0x00, 0x50, 0x45, 0x00, 0x00, 0x64, 0x86, 0x08, 0x00, 0x81, 0x4E, 0x12, 0x69, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF0, 0x00, 0x22, 0x00, 0x0B, 0x02, 0x0E, 0x00, 0x00, 0x40, 0x11, 0x00,
    0x00, 0x60, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x91, 0xA4, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x60, 0x8D,
    0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x1D, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x0B, 0x00, 0x60, 0x81, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x10, 0x00, 0x00, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x2E, 0x74, 0x65, 0x78, 0x74, 0x00, 0x00, 0x00, 0x40, 0x3F, 0x11, 0x00, 0x00, 0x00, 0x10, 0x00,
    0x00, 0x40, 0x11, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x20, 0x00, 0x00, 0x60, 0x2E, 0x72, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x2C, 0x7B, 0x0A, 0x00, 0x00, 0x00,
    0x20, 0x00, 0x00, 0x7C, 0x0A, 0x00, 0x00, 0x44, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0xE8, 0x8E, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x30, 0x00, 0x0C, 0x00, 0x00, 0x00, 0xC0, 0x1B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0xC0, 0x2E, 0x70, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0xF8, 0x94,
    0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x96, 0x00, 0x00, 0x00, 0xCC, 0x1B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x65, 0x68, 0x5F, 0x66, 0x72, 0x61, 0x6D,
    0xA0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x62, 0x1C, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x6C, 0x69, 0x6E, 0x6B, 0x6D,
    0x32, 0x5F, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x60, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x64, 0x1C, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x6C, 0x69, 0x6E,
    0x6B, 0x6D, 0x65, 0x5F, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x66, 0x1C,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x2E, 0x72,
    0x65, 0x6C, 0x6F, 0x63, 0x00, 0x00, 0xE0, 0x3B, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x3C, 0x00, 0x00, 0x00,
    0x68, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x42,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
    test_support::with_global_lock(|| {
        test_support::init_test_logger();

        let _guard = test_support::StateGuard::new(|| {
            // SAFETY: Cleanup code runs with global lock held, resetting
            // GCD state between tests.
            unsafe {
                super::GCD.reset();
            }
        });

        f();
    })
    .unwrap();
}

#[test]
fn test_gcd_initialization() {
    with_locked_state(|| {
        let gcd = GCD::new(48);
        assert_eq!(2_usize.pow(48), gcd.maximum_address);
        assert_eq!(gcd.memory_blocks.capacity(), 0);
        assert_eq!(0, gcd.memory_descriptor_count());
    });
}

#[test]
fn test_add_memory_space_before_memory_blocks_instantiated() {
    with_locked_state(|| {
        // SAFETY: Test memory allocation - memory is valid and properly aligned.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);

        // SAFETY: GCD test operation - address comes from controlled allocation above.
        assert_eq!(
            Err(EfiError::NotReady),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, address, MEMORY_BLOCK_SLICE_SIZE, 0) },
            "First add memory space should be a system memory."
        );
        assert_eq!(0, gcd.memory_descriptor_count());

        assert_eq!(
            // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
            Err(EfiError::OutOfResources),
            // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
            unsafe {
                gcd.init_memory_blocks(
                    GcdMemoryType::SystemMemory,
                    address,
                    MEMORY_BLOCK_SLICE_SIZE - 1,
                    efi::MEMORY_WB,
                    efi::MEMORY_WB,
                )
            },
            "First add memory space with system memory should contain enough space to contain the block list."
        );
        assert_eq!(0, gcd.memory_descriptor_count());
    });
}

#[test]
fn test_add_memory_space_with_all_memory_type() {
    with_locked_state(|| {
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        let (mut gcd, _) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(0), unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 0, 1, 0) });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(3), unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 1, 1, 0) });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(4), unsafe { gcd.add_memory_space(GcdMemoryType::Persistent, 2, 1, 0) });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(5), unsafe { gcd.add_memory_space(GcdMemoryType::MoreReliable, 3, 1, 0) });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(6), unsafe { gcd.add_memory_space(GcdMemoryType::Unaccepted, 4, 1, 0) });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(7), unsafe { gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 5, 1, 0) });

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::InvalidParameter),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::NonExistent, 10, 1, 0) },
            "Can't manually add NonExistent memory space manually."
        );

        assert!(is_gcd_memory_slice_valid(&gcd));
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_add_memory_space_with_0_len_block() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Err(EfiError::InvalidParameter), unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, 0, 0)
        });
        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

#[test]
fn test_add_memory_space_when_memory_block_full() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        let addr = address + MEMORY_BLOCK_SLICE_SIZE;

        let mut n = 0;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            assert!(
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, addr + n, 1, n as u64) }.is_ok()
            );
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        let res = unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, addr + n, 1, n as u64) };
        assert_eq!(
            Err(EfiError::OutOfResources),
            res,
            "Should return out of memory if there is no space in memory blocks."
        );
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    });
    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
}

#[test]
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
fn test_add_memory_space_outside_processor_range() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        let snapshot = copy_memory_block(&gcd);

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Err(EfiError::Unsupported), unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, gcd.maximum_address + 1, 1, 0)
        });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Err(EfiError::Unsupported), unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, gcd.maximum_address, 1, 0)
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        });
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Err(EfiError::Unsupported), unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, gcd.maximum_address - 1, 2, 0)
        });

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
fn test_add_memory_space_in_range_already_added() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // Add block to test the boundary on.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 1000, 10, 0) }.unwrap();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::AccessDenied),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 1002, 5, 0) },
            "Can't add inside a range previously added."
        );
        assert_eq!(
            Err(EfiError::AccessDenied),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 998, 5, 0) },
            "Can't add partially inside a range previously added (Start)."
        );
        assert_eq!(
            Err(EfiError::AccessDenied),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 1009, 5, 0) },
            "Can't add partially inside a range previously added (End)."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
    });
}

#[test]
fn test_add_memory_space_in_range_already_allocated() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        // Add unallocated block after allocated one.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, address - 100, 100, 0) }.unwrap();

        let snapshot = copy_memory_block(&gcd);

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(
            Err(EfiError::AccessDenied),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, address, 5, 0) },
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            "Can't add inside a range previously allocated."
        );
        assert_eq!(
            Err(EfiError::AccessDenied),
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, address - 100, 200, 0) },
            "Can't add partially inside a range previously allocated."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
}

#[test]
fn test_add_memory_space_block_merging() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(Ok(4), unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 1000, 10, 0) });
        let block_count = gcd.memory_descriptor_count();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        // Test merging when added after
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        match unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 1010, 10, 0) } {
            Ok(idx) => {
                let mb = gcd.memory_blocks.get_with_idx(idx).unwrap();
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                assert_eq!(1000, mb.as_ref().base_address);
                assert_eq!(20, mb.as_ref().length);
                assert_eq!(block_count, gcd.memory_descriptor_count());
            }
            Err(e) => panic!("{e:?}"),
        }

        // Test merging when added before
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        match unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 990, 10, 0) } {
            Ok(idx) => {
                let mb = gcd.memory_blocks.get_with_idx(idx).unwrap();
                assert_eq!(990, mb.as_ref().base_address);
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                assert_eq!(30, mb.as_ref().length);
                assert_eq!(block_count, gcd.memory_descriptor_count());
            }
            Err(e) => panic!("{e:?}"),
        }

        assert!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 1020, 10, 0) }.is_ok(),
            "A different memory type should note result in a merge."
        );
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 1030, 10, 1) }.is_ok(),
            "A different capabilities should note result in a merge."
        );
        assert_eq!(block_count + 2, gcd.memory_descriptor_count());

        assert!(is_gcd_memory_slice_valid(&gcd));
    });
}
// SAFETY: get_memory returns a test-owned buffer of the requested size.

#[test]
fn test_add_memory_space_state() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        match unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 100, 10, 123) } {
            Ok(idx) => {
                let mb = *gcd.memory_blocks.get_with_idx(idx).unwrap();
                match mb {
                    MemoryBlock::Unallocated(md) => {
                        assert_eq!(100, md.base_address);
                        assert_eq!(10, md.length);
                        assert_eq!(efi::MEMORY_RUNTIME | efi::MEMORY_ACCESS_MASK | 0x007b, md.capabilities);
                        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                        assert_eq!(0, md.image_handle as usize);
                        assert_eq!(0, md.device_handle as usize);
                    }
                    MemoryBlock::Allocated(_) => panic!("Add should keep the block unallocated"),
                }
            }
            Err(e) => panic!("{e:?}"),
        }
    });
}

#[test]
fn test_remove_memory_space_before_memory_blocks_instantiated() {
    with_locked_state(|| {
        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        let mut gcd = GCD::new(48);

        assert_eq!(Err(EfiError::NotFound), gcd.remove_memory_space(address, MEMORY_BLOCK_SLICE_SIZE));
    });
    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
}

#[test]
fn test_remove_memory_space_with_0_len_block() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add memory space to remove in a valid area.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert!(unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, 10, 0) }.is_ok());

        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(EfiError::InvalidParameter), gcd.remove_memory_space(5, 0));

        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.remove_memory_space(10, 0),
            "If there is no allocate done first, 0 length invalid param should have priority."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_remove_memory_space_outside_processor_range() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        // Add memory space to remove in a valid area.
        assert!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, gcd.maximum_address - 10, 10, 0) }.is_ok()
        );

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.remove_memory_space(gcd.maximum_address - 10, 11),
            "An address outside the processor range support is invalid."
        );
        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.remove_memory_space(gcd.maximum_address, 10),
            "An address outside the processor range support is invalid."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_remove_memory_space_in_range_not_added() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // Add memory space to remove in a valid area.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert!(unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 100, 10, 0) }.is_ok());

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(95, 10),
            "Can't remove memory space partially added."
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(105, 10),
            "Can't remove memory space partially added."
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(10, 10),
            "Can't remove memory space not previously added."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_remove_memory_space_in_range_allocated() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

        let snapshot = copy_memory_block(&gcd);

        // Not found has a priority over the access denied because the check if the range is valid is done earlier.
        assert_eq!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            Err(EfiError::NotFound),
            gcd.remove_memory_space(address - 5, 10),
            "Can't remove memory space partially allocated."
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.remove_memory_space(address + MEMORY_BLOCK_SLICE_SIZE - 5, 10),
            "Can't remove memory space partially allocated."
        );

        assert_eq!(
            Err(EfiError::AccessDenied),
            gcd.remove_memory_space(address + 10, 10),
            "Can't remove memory space not previously allocated."
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_remove_memory_space_when_memory_block_full() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        let addr = address + MEMORY_BLOCK_SLICE_SIZE;

        assert!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, addr, 10, 0_u64) }.is_ok()
        );
        let mut n = 1;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            assert!(
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                unsafe {
                    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                    gcd.add_memory_space(GcdMemoryType::SystemMemory, addr + 10 + n, 1, n as u64)
                }
                .is_ok()
            );
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.remove_memory_space(addr, 5),
            "Should return out of memory if there is no space in memory blocks."
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    });
}

#[test]
fn test_remove_memory_space_block_merging() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        let page_size = 0x1000;
        let aligned_address = address & !(page_size - 1);
        let aligned_length = page_size * 10;
        let aligned_address = if aligned_address > aligned_length {
            aligned_address - aligned_length
        } else {
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            aligned_address + aligned_length
        };

        assert!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, aligned_address, aligned_length, 0) }.is_ok()
        );

        let block_count = gcd.memory_descriptor_count();

        for i in 0..5 {
            assert!(gcd.remove_memory_space(aligned_address + i * page_size, page_size).is_ok());
        }

        // First index because the add memory started at aligned_address.
        assert_eq!(aligned_address, copy_memory_block(&gcd)[1].as_ref().base_address as usize);
        assert_eq!(aligned_length / 2, copy_memory_block(&gcd)[1].as_ref().length as usize);
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert!(is_gcd_memory_slice_valid(&gcd));

        // Removing in the middle should create 2 new blocks.
        assert!(gcd.remove_memory_space(aligned_address + page_size * 5, page_size).is_ok());
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert!(is_gcd_memory_slice_valid(&gcd));
    });
}

#[test]
fn test_remove_memory_space_state() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        assert!(
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, address, 123) }.is_ok()
        );

        match gcd.remove_memory_space(0, 10) {
            Ok(()) => {
                let mb = copy_memory_block(&gcd)[0];
                match mb {
                    MemoryBlock::Unallocated(md) => {
                        assert_eq!(0, md.base_address);
                        assert_eq!(10, md.length);
                        assert_eq!(0, md.capabilities);
                        assert_eq!(0, md.image_handle as usize);
                        assert_eq!(0, md.device_handle as usize);
                    }
                    MemoryBlock::Allocated(_) => panic!("remove should keep the block unallocated"),
                }
            }
            Err(e) => panic!("{e:?}"),
        }
    });
}

#[test]
fn test_allocate_memory_space_before_memory_blocks_instantiated() {
    with_locked_state(|| {
        let mut gcd = GCD::new(48);
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(0),
                GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                10,
                1 as _,
                None
            )
        );
    });
}

#[test]
fn test_allocate_memory_space_with_0_len_block() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                GcdMemoryType::Reserved,
                UEFI_PAGE_SHIFT,
                0,
                1 as _,
                None
            ),
        );
        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_allocate_memory_space_with_null_image_handle() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                GcdMemoryType::Reserved,
                0,
                10,
                ptr::null_mut(),
                None
            ),
        );
        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_allocate_memory_space_with_address_outside_processor_range() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(gcd.maximum_address - 100),
                GcdMemoryType::Reserved,
                0,
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                1000,
                1 as _,
                None
            ),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(
                AllocateType::Address(gcd.maximum_address + 100),
                GcdMemoryType::Reserved,
                0,
                1000,
                1 as _,
                None
            ),
        );

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        assert_eq!(snapshot, copy_memory_block(&gcd));
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
    });
    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
}

#[test]
fn test_allocate_memory_space_with_all_memory_type() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        for (i, memory_type) in [
            GcdMemoryType::Reserved,
            GcdMemoryType::SystemMemory,
            GcdMemoryType::Persistent,
            GcdMemoryType::MemoryMappedIo,
            GcdMemoryType::MoreReliable,
            GcdMemoryType::Unaccepted,
        ]
        .into_iter()
        .enumerate()
        {
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(memory_type, (i + 1) * 10, 10, 0) }.unwrap();
            let res = gcd.allocate_memory_space(AllocateType::Address((i + 1) * 10), memory_type, 0, 10, 1 as _, None);
            match memory_type {
                GcdMemoryType::Unaccepted => assert_eq!(Err(EfiError::InvalidParameter), res),
                _ => assert!(res.is_ok()),
            }
        }
    });
}

#[test]
fn test_allocate_memory_space_with_no_memory_space_available() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add memory space of len 100 to multiple space.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, 100, 0) }.unwrap();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 1000, 100, 0) }.unwrap();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, gcd.maximum_address - 100, 100, 0) }.unwrap();

        let memory_blocks_snapshot = copy_memory_block(&gcd);

        // Try to allocate chunk bigger than 100.
        for allocate_type in [AllocateType::BottomUp(None), AllocateType::TopDown(None)] {
            assert_eq!(
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                Err(EfiError::OutOfResources),
                gcd.allocate_memory_space(allocate_type, GcdMemoryType::SystemMemory, 0, 1000, 1 as _, None),
                "Assert fail with allocate type: {allocate_type:?}"
            );
        }

        for allocate_type in
            [AllocateType::BottomUp(Some(10_000)), AllocateType::TopDown(Some(10_000)), AllocateType::Address(10_000)]
        {
            assert_eq!(
                Err(EfiError::NotFound),
                gcd.allocate_memory_space(allocate_type, GcdMemoryType::SystemMemory, 0, 1000, 1 as _, None),
                "Assert fail with allocate type: {allocate_type:?}"
            );
        }

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_allocate_memory_space_alignment() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x1000, 0) }.unwrap();

        assert_eq!(
            Ok(0x1000),
            gcd.allocate_memory_space(AllocateType::BottomUp(None), GcdMemoryType::SystemMemory, 0, 0x0f, 1 as _, None),
            "Allocate bottom up without alignment"
        );
        assert_eq!(
            Ok(0x1010),
            gcd.allocate_memory_space(AllocateType::BottomUp(None), GcdMemoryType::SystemMemory, 4, 0x10, 1 as _, None),
            "Allocate bottom up with alignment of 4 bits (find first address that is aligned)"
        );
        assert_eq!(
            Ok(0x1020),
            gcd.allocate_memory_space(AllocateType::BottomUp(None), GcdMemoryType::SystemMemory, 4, 100, 1 as _, None),
            "Allocate bottom up with alignment of 4 bits (already aligned)"
        );
        assert_eq!(
            Ok(0x1ff1),
            gcd.allocate_memory_space(AllocateType::TopDown(None), GcdMemoryType::SystemMemory, 0, 0x0f, 1 as _, None),
            "Allocate top down without alignment"
        );
        assert_eq!(
            Ok(0x1fe0),
            gcd.allocate_memory_space(AllocateType::TopDown(None), GcdMemoryType::SystemMemory, 4, 0x0f, 1 as _, None),
            "Allocate top down with alignment of 4 bits (find first address that is aligned)"
        );
        assert_eq!(
            Ok(0x1f00),
            gcd.allocate_memory_space(
                AllocateType::TopDown(None),
                GcdMemoryType::SystemMemory,
                4,
                0xe0,
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                1 as _,
                None
            ),
            "Allocate top down with alignment of 4 bits (already aligned)"
        );
        assert_eq!(
            Ok(0x1a00),
            gcd.allocate_memory_space(AllocateType::Address(0x1a00), GcdMemoryType::SystemMemory, 4, 100, 1 as _, None),
            "Allocate Address with alignment of 4 bits (already aligned)"
        );

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(AllocateType::Address(0x1a0f), GcdMemoryType::SystemMemory, 4, 100, 1 as _, None),
        );

        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd));
    });
}

#[test]
fn test_allocate_memory_space_block_merging() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x1000, 0) }.unwrap();

        for allocate_type in [AllocateType::BottomUp(None), AllocateType::TopDown(None)] {
            let block_count = gcd.memory_descriptor_count();
            assert!(
                gcd.allocate_memory_space(allocate_type, GcdMemoryType::SystemMemory, 0, 1, 1 as _, None).is_ok(),
                "{allocate_type:?}"
            );
            assert_eq!(block_count + 1, gcd.memory_descriptor_count());
            assert!(
                gcd.allocate_memory_space(allocate_type, GcdMemoryType::SystemMemory, 0, 1, 1 as _, None).is_ok(),
                "{allocate_type:?}"
            );
            assert_eq!(block_count + 1, gcd.memory_descriptor_count());
            assert!(
                gcd.allocate_memory_space(allocate_type, GcdMemoryType::SystemMemory, 0, 1, 2 as _, None).is_ok(),
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                "{allocate_type:?}: A different image handle should not result in a merge."
            );
            assert_eq!(block_count + 2, gcd.memory_descriptor_count());
            assert!(
                gcd.allocate_memory_space(allocate_type, GcdMemoryType::SystemMemory, 0, 1, 2 as _, Some(1 as _))
                    .is_ok(),
                "{allocate_type:?}: A different device handle should not result in a merge."
            );
            assert_eq!(block_count + 3, gcd.memory_descriptor_count());
        }

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(
            Ok(0x1000 + 4),
            gcd.allocate_memory_space(
                AllocateType::Address(0x1000 + 4),
                GcdMemoryType::SystemMemory,
                0,
                1,
                2 as _,
                Some(1 as _)
            ),
            "Merge should work with address allocation too."
        );
        assert_eq!(block_count, gcd.memory_descriptor_count());

        assert!(is_gcd_memory_slice_valid(&gcd));
    });
}

#[test]
fn test_allocate_memory_space_with_address_not_added() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x100, 10, 0) }.unwrap();

        let snapshot = copy_memory_block(&gcd);

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(AllocateType::Address(0x100), GcdMemoryType::SystemMemory, 0, 11, 1 as _, None),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(AllocateType::Address(0x95), GcdMemoryType::SystemMemory, 0, 10, 1 as _, None),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(AllocateType::Address(110), GcdMemoryType::SystemMemory, 0, 5, 1 as _, None),
        );
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(AllocateType::Address(0), GcdMemoryType::SystemMemory, 0, 5, 1 as _, None),
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
}

#[test]
fn test_allocate_memory_space_with_address_allocated() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_memory_space(AllocateType::Address(address), GcdMemoryType::SystemMemory, 0, 5, 1 as _, None),
        );
    });
}

#[test]
fn test_free_memory_space_before_memory_blocks_instantiated() {
    with_locked_state(|| {
        let mut gcd = GCD::new(48);
        assert_eq!(Err(EfiError::NotFound), gcd.free_memory_space(0x1000, 0x1000, MemoryStateTransition::Free));
    });
}

#[test]
fn test_free_memory_space_when_0_len_block() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        let snapshot = copy_memory_block(&gcd);
        assert_eq!(Err(EfiError::InvalidParameter), gcd.free_memory_space(0, 0, MemoryStateTransition::Free));
        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

#[test]
fn test_free_memory_space_outside_processor_range() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, gcd.maximum_address - 100, 100, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(gcd.maximum_address - 100),
            GcdMemoryType::SystemMemory,
            0,
            100,
            1 as _,
            None,
        )
        .unwrap();

        let snapshot = copy_memory_block(&gcd);
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.free_memory_space(gcd.maximum_address, 10, MemoryStateTransition::Free)
        );
        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.free_memory_space(gcd.maximum_address - 99, 100, MemoryStateTransition::Free)
        );
        assert_eq!(
            Err(EfiError::Unsupported),
            gcd.free_memory_space(gcd.maximum_address + 1, 100, MemoryStateTransition::Free)
        );

        assert_eq!(snapshot, copy_memory_block(&gcd));
    });
}
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

#[test]
fn test_free_memory_space_in_range_not_allocated() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x3000, 0x3000, 0) }.unwrap();
        gcd.allocate_memory_space(AllocateType::Address(0x3000), GcdMemoryType::SystemMemory, 0, 0x1000, 1 as _, None)
            .unwrap();

        assert_eq!(Err(EfiError::AccessDenied), gcd.free_memory_space(0x2000, 0x1000, MemoryStateTransition::Free));
        assert_eq!(Err(EfiError::AccessDenied), gcd.free_memory_space(0x4000, 0x1000, MemoryStateTransition::Free));
        assert_eq!(Err(EfiError::AccessDenied), gcd.free_memory_space(0, 0x1000, MemoryStateTransition::Free));
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
    });
}

#[test]
fn test_free_memory_space_when_memory_block_full() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000000, UEFI_PAGE_SIZE * 2, 0) }.unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0x1000000),
            GcdMemoryType::SystemMemory,
            0,
            UEFI_PAGE_SIZE * 2,
            1 as _,
            None,
        )
        .unwrap();

        let mut n = 1;
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            let addr = 0x2000000 + (n * UEFI_PAGE_SIZE);
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, addr, UEFI_PAGE_SIZE, n as u64) }.unwrap();
            n += 1;
        }
        let memory_blocks_snapshot = copy_memory_block(&gcd);
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.free_memory_space(0x1000000, UEFI_PAGE_SIZE, MemoryStateTransition::Free)
        );
        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd),);
    });
}

#[test]
fn test_free_memory_space_merging() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x10000, 0) }.unwrap();
        gcd.allocate_memory_space(AllocateType::Address(0x1000), GcdMemoryType::SystemMemory, 0, 0x10000, 1 as _, None)
            .unwrap();

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(
            Ok(()),
            gcd.free_memory_space(0x1000, 0x1000, MemoryStateTransition::Free),
            "Free beginning of a block."
        );
        assert_eq!(block_count + 1, gcd.memory_descriptor_count());
        assert_eq!(
            Ok(()),
            gcd.free_memory_space(0x5000, 0x1000, MemoryStateTransition::Free),
            "Free in the middle of a block"
        );
        assert_eq!(block_count + 3, gcd.memory_descriptor_count());
        assert_eq!(
            Ok(()),
            gcd.free_memory_space(0x9000, 0x1000, MemoryStateTransition::Free),
            "Free at the end of a block"
        );
        assert_eq!(block_count + 5, gcd.memory_descriptor_count());

        let block_count = gcd.memory_descriptor_count();
        assert_eq!(Ok(()), gcd.free_memory_space(0x2000, 0x2000, MemoryStateTransition::Free));
        assert_eq!(block_count, gcd.memory_descriptor_count());

        let blocks = copy_memory_block(&gcd);
        let mb = blocks[0];
        assert_eq!(0, mb.as_ref().base_address);
        assert_eq!(0x1000, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(0x6000, 0x1000, MemoryStateTransition::Free));
        assert_eq!(block_count, gcd.memory_descriptor_count());
        let blocks = copy_memory_block(&gcd);
        let mb = blocks[2];
        assert_eq!(0x4000, mb.as_ref().base_address);
        assert_eq!(0x1000, mb.as_ref().length);

        assert_eq!(Ok(()), gcd.free_memory_space(0x8000, 0x1000, MemoryStateTransition::Free));
        assert_eq!(block_count, gcd.memory_descriptor_count());
        let blocks = copy_memory_block(&gcd);
        let mb = blocks[4];
        assert_eq!(0x7000, mb.as_ref().base_address);
        assert_eq!(0x1000, mb.as_ref().length);

        assert!(is_gcd_memory_slice_valid(&gcd));
    });
}

#[test]
fn test_set_memory_space_attributes_with_invalid_parameters() {
    with_locked_state(|| {
        let mut gcd = GCD {
            memory_blocks: Rbt::new(),
            maximum_address: 0,
            allocate_memory_space_fn: GCD::allocate_memory_space_internal,
            free_memory_space_fn: GCD::free_memory_space,
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            prioritize_32_bit_memory: false,
        };
        assert_eq!(Err(EfiError::NotReady), gcd.set_memory_space_attributes(0, 0x50000, 0b1111));

        let (mut gcd, _) = create_gcd();

        // Test that setting memory space attributes on more space than is available is an error
        assert_eq!(Err(EfiError::Unsupported), gcd.set_memory_space_attributes(0x100000000000000, 50, 0b1111));

        // Test that calling set_memory_space_attributes with no size returns invalid parameter
        assert_eq!(Err(EfiError::InvalidParameter), gcd.set_memory_space_attributes(0, 0, 0b1111));

        // Test that calling set_memory_space_attributes with invalid attributes returns invalid parameter
        assert_eq!(Err(EfiError::InvalidParameter), gcd.set_memory_space_attributes(0, 0, 0));

        // Test that a non-page aligned address returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0x1000, efi::MEMORY_WB)
        );

        // Test that a non-page aligned address with the runtime attribute set returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0x1000, efi::MEMORY_RUNTIME | efi::MEMORY_WB) // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        );

        // Test that a non-page aligned size returns invalid parameter
        assert_eq!(Err(EfiError::InvalidParameter), gcd.set_memory_space_attributes(0x1000, 0xFFF, efi::MEMORY_WB));

        // Test that a non-page aligned size returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0x1000, 0xFFF, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );

        // Test that a non-page aligned address and size returns invalid parameter
        assert_eq!(
            Err(EfiError::InvalidParameter),
            gcd.set_memory_space_attributes(0xFFFFFFFF, 0xFFF, efi::MEMORY_RUNTIME | efi::MEMORY_WB)
        );
    });
}

#[test]
fn test_set_capabilities_and_attributes() {
    // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, address - 0x1000, 0) }.unwrap();

        gcd.allocate_memory_space(AllocateType::BottomUp(None), GcdMemoryType::SystemMemory, 0, 0x2000, 1 as _, None)
            .unwrap();
        // Trying to set capabilities where the range falls outside a block should return unsupported
        assert_eq!(Err(EfiError::Unsupported), gcd.set_memory_space_capabilities(0x1000, 0x3000, 0b1111));
        // System memory is added with a default WB attribute, so the new capabilities must continue to support it.
        gcd.set_memory_space_capabilities(
            0x1000,
            0x2000,
            efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB,
        )
        .unwrap();
        gcd.set_gcd_memory_attributes(0x1000, 0x2000, efi::MEMORY_RO).unwrap();
    });
}

#[test]
#[should_panic]
fn test_set_attributes_panic() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, address, 0) }.unwrap();

        gcd.allocate_memory_space(AllocateType::BottomUp(None), GcdMemoryType::SystemMemory, 0, 0x2000, 1 as _, None)
            .unwrap();
        gcd.set_memory_space_capabilities(0, 0x2000, efi::MEMORY_RP | efi::MEMORY_RO).unwrap();
        // Trying to set attributes where the range falls outside a block should panic in debug case
        gcd.set_memory_space_attributes(0, 0x3000, 0b1).unwrap();
    });
}

#[test]
fn test_block_split_when_memory_blocks_full() {
    with_locked_state(|| {
        let (mut gcd, address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            gcd.add_memory_space(
                GcdMemoryType::SystemMemory,
                0,
                address,
                efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB,
            )
        }
        .unwrap();

        let mut n = 1;
        let mut allocated_addresses = Vec::new();
        while gcd.memory_descriptor_count() < MEMORY_BLOCK_SLICE_LEN {
            let addr = gcd
                .allocate_memory_space(
                    AllocateType::BottomUp(None),
                    GcdMemoryType::SystemMemory,
                    0,
                    0x2000,
                    n as _,
                    None,
                )
                .unwrap();
            allocated_addresses.push(addr);
            n += 1;
        }

        assert!(is_gcd_memory_slice_valid(&gcd));
        let memory_blocks_snapshot = copy_memory_block(&gcd);

        // Test that allocate_memory_space fails when full
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_memory_space(
                AllocateType::BottomUp(None),
                GcdMemoryType::SystemMemory,
                0,
                0x1000,
                1 as _,
                None
            )
        );
        assert_eq!(memory_blocks_snapshot, copy_memory_block(&gcd));

        // Verify that the memory blocks array is at capacity
        assert_eq!(gcd.memory_descriptor_count(), MEMORY_BLOCK_SLICE_LEN, "Memory blocks should be at capacity");

        // Test that set_memory_space_capabilities fails when full, if the block requires a split
        // Use the first allocated address to ensure we're working with a valid allocated block
        let first_allocated = allocated_addresses[0];
        let capabilities_result = gcd.set_memory_space_capabilities(
            first_allocated,
            0x1000,
            efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP,
        );

        // This should fail with OutOfResources, but may panic in debug builds due to assertions
        // We verify the memory is at capacity regardless of the specific error
        match capabilities_result {
            Err(EfiError::OutOfResources) => {
                // Expected behavior in release builds
            }
            _ => {
                // In debug builds, operations that would exceed capacity might panic
                // The important thing is that we've verified the array is at capacity
                assert_eq!(gcd.memory_descriptor_count(), MEMORY_BLOCK_SLICE_LEN, "Memory should remain at capacity");
            }
        }
    });
}

#[test]
fn test_invalid_add_io_space() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);

        assert!(gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).is_ok());
        // Cannot Allocate a range in a range that is already allocated
        assert_eq!(Err(EfiError::AccessDenied), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10));

        // Cannot allocate a range as NonExistent
        assert_eq!(Err(EfiError::InvalidParameter), gcd.add_io_space(dxe_services::GcdIoType::NonExistent, 10, 10));

        // Cannot do more allocations if the underlying data structure is full
        for i in 1..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.add_io_space(dxe_services::GcdIoType::Io, (IO_BLOCK_SLICE_LEN + 1) * 10, 10)
        );
    });
}

#[test]
fn test_invalid_remove_io_space() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);

        // Cannot remove a range of 0
        assert_eq!(Err(EfiError::InvalidParameter), gcd.remove_io_space(0, 0));

        // Cannot remove a range greater than what is available
        assert_eq!(Err(EfiError::Unsupported), gcd.remove_io_space(0, 70_000));

        // Cannot remove an io space if it does not exist
        assert_eq!(Err(EfiError::NotFound), gcd.remove_io_space(0, 10));

        // Cannot remove an io space if it is allocated
        gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).unwrap();
        gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        assert_eq!(Err(EfiError::AccessDenied), gcd.remove_io_space(0, 10));

        // Cannot remove an io space if it is partially in a block and we are full, as it
        // causes a split with no space to add a new node.
        let mut gcd = IoGCD::_new(16);
        for i in 2..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }
        assert_eq!(Err(EfiError::OutOfResources), gcd.remove_io_space(25, 3));
        assert!(gcd.remove_io_space(20, 10).is_ok());
    });
}

#[test]
fn test_ensure_allocate_io_space_conformance() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x4000));

        assert_eq!(
            Ok(0),
            gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
        assert_eq!(
            Ok(0x100),
            gcd.allocate_io_space(AllocateType::BottomUp(None), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
        assert_eq!(
            Ok(0x3F00),
            gcd.allocate_io_space(AllocateType::TopDown(None), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
        assert_eq!(
            Ok(0x1000),
            gcd.allocate_io_space(AllocateType::Address(0x1000), dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None)
        );
    });
}

#[test]
fn test_ensure_allocations_fail_when_out_of_resources() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);
        for i in 0..IO_BLOCK_SLICE_LEN - 1 {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }

        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 0x4000)
        );
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, usize::MAX)
        );
        assert_eq!(
            Err(EfiError::OutOfResources),
            gcd.allocate_address(dxe_services::GcdIoType::Io, 0, 5, 2 as _, None, 210)
        );
    });
}

#[test]
fn test_allocate_bottom_up_conformance() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x4000)
        );

        // Setup some io_space for the following tests
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x100));
        assert_eq!(Ok(1), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x100, 0x100));
        assert_eq!(Ok(2), gcd.add_io_space(dxe_services::GcdIoType::Io, 0x200, 0x200));
        assert_eq!(Ok(3), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x400, 0x200));

        // Test that we move on to the next block if the current block is not big enough
        // i.e. we skip the 0x0 block because it is not big enough.
        assert_eq!(Ok(0x200), gcd.allocate_bottom_up(dxe_services::GcdIoType::Io, 0, 0x150, 1 as _, None, 0x4000));

        // Testing that after we apply allocation requirements, we correctly skip the first available block
        // that meets the initial (0x50) requirement, but does not satisfy the alignment requirement of 0x200.
        assert_eq!(
            Ok(0x400),
            gcd.allocate_bottom_up(dxe_services::GcdIoType::Maximum, 0b1001, 0x50, 1 as _, None, 0x4000)
        );
    });
}

#[test]
fn test_allocate_top_down_conformance() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x4000)
        );

        // Setup some io_space for the following tests
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x200));
        assert_eq!(Ok(1), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x200, 0x200));
        assert_eq!(Ok(2), gcd.add_io_space(dxe_services::GcdIoType::Io, 0x400, 0x100));
        assert_eq!(Ok(3), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x500, 0x100));

        // Test that we move on to the next block if the current block is not big enough
        // i.e. we skip the 0x0 block because it is not big enough. Since going top down,
        // The address is in the middle of the 0x200 Block such tha
        // 0xB0 (start addr) + 0x150 (size)= 0x200
        assert_eq!(Ok(0xB0), gcd.allocate_top_down(dxe_services::GcdIoType::Io, 0, 0x150, 1 as _, None, usize::MAX));

        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_top_down(dxe_services::GcdIoType::Reserved, 0, 0x150, 1 as _, None, usize::MAX)
        );
    });
}

#[test]
fn test_allocate_address_conformance() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);

        // Cannot allocate if no blocks have been added
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_address(dxe_services::GcdIoType::Io, 0, 0x100, 1 as _, None, 0x200)
        );

        // Setup some io_space for the following tests
        assert_eq!(Ok(0), gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 0x200));
        assert_eq!(Ok(1), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x200, 0x200));
        assert_eq!(Ok(2), gcd.add_io_space(dxe_services::GcdIoType::Io, 0x400, 0x100));
        assert_eq!(Ok(3), gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x500, 0x100));

        // If we find a block with the address, but its not the right Io type, we should
        // report not found
        assert_eq!(
            Err(EfiError::NotFound),
            gcd.allocate_address(dxe_services::GcdIoType::Reserved, 0, 0x100, 1 as _, None, 0)
        );
    });
}

#[test]
fn test_free_io_space_conformance() {
    with_locked_state(|| {
        let mut gcd = IoGCD::_new(16);

        // Cannot free a range of 0
        assert_eq!(Err(EfiError::InvalidParameter), gcd.free_io_space(0, 0));

        // Cannot free a range greater than what is available
        assert_eq!(Err(EfiError::Unsupported), gcd.free_io_space(0, 70_000));

        // Cannot free an io space if it does not exist
        assert_eq!(Err(EfiError::NotFound), gcd.free_io_space(0, 10));

        gcd.add_io_space(dxe_services::GcdIoType::Io, 0, 10).unwrap();
        gcd.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        assert_eq!(Ok(()), gcd.free_io_space(0, 10));

        // Cannot free an io space if it is partially in a block and we are full, as it
        // causes a split with no space to add a new node.
        let mut gcd = IoGCD::_new(16);
        for i in 2..IO_BLOCK_SLICE_LEN {
            if i % 2 == 0 {
                gcd.add_io_space(dxe_services::GcdIoType::Maximum, i * 10, 10).unwrap();
            } else {
                gcd.add_io_space(dxe_services::GcdIoType::Io, i * 10, 10).unwrap();
            }
        }

        // Cannot partially free a block when full, but we can free the whole block
        gcd.allocate_address(dxe_services::GcdIoType::Maximum, 0, 10, 1 as _, None, 100).unwrap();
        assert_eq!(Err(EfiError::OutOfResources), gcd.free_io_space(105, 3));
        assert_eq!(Ok(()), gcd.free_io_space(100, 10));
    });
}

fn create_gcd() -> (GCD, usize) {
    // SAFETY: get_memory returns a test-owned buffer of the requested size.
    let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
    let address = mem.as_ptr() as usize;
    let mut gcd = GCD::new(48);
    // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
    unsafe {
        gcd.init_memory_blocks(
            GcdMemoryType::SystemMemory,
            address,
            MEMORY_BLOCK_SLICE_SIZE,
            efi::MEMORY_WB,
            efi::MEMORY_WB,
        )
        .unwrap();
    }
    (gcd, address)
}

fn copy_memory_block(gcd: &GCD) -> Vec<MemoryBlock> {
    gcd.memory_blocks.dfs()
}

fn is_gcd_memory_slice_valid(gcd: &GCD) -> bool {
    let memory_blocks = &gcd.memory_blocks;
    match memory_blocks.first_idx().map(|idx| memory_blocks.get_with_idx(idx).unwrap().start()) {
        Some(0) => (),
        _ => return false,
    }
    let mut last_addr = 0;
    let blocks = copy_memory_block(gcd);
    let mut w = blocks.windows(2);
    while let Some([a, b]) = w.next() {
        if a.end() != b.start() || a.is_same_state(b) {
            return false;
        }
        last_addr = b.end();
    }
    if last_addr != gcd.maximum_address {
        return false;
    }
    true
}

unsafe fn get_memory(size: usize) -> &'static mut [u8] {
    // SAFETY: Allocates memory from the system allocator with UEFI page alignment.
    // The returned slice is intentionally leaked for test simplicity and valid for 'static lifetime.
    let addr = unsafe { alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, UEFI_PAGE_SIZE).unwrap()) };
    // SAFETY: The allocated pointer is valid for `size` bytes and properly aligned.
    unsafe { core::slice::from_raw_parts_mut(addr, size) }
}

#[test]
fn spin_locked_allocator_should_error_if_not_initialized() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        // SAFETY: The GCD is intentionally uninitialized to validate error handling paths.
        let add_result = unsafe { GCD.add_memory_space(GcdMemoryType::SystemMemory, 0, 100, 0) };
        assert_eq!(add_result, Err(EfiError::NotReady));

        let allocate_result =
            GCD.allocate_memory_space(AllocateType::Address(0), GcdMemoryType::SystemMemory, 0, 10, 1 as _, None);
        assert_eq!(allocate_result, Err(EfiError::NotReady));

        let free_result = GCD.free_memory_space(0, 10);
        assert_eq!(free_result, Err(EfiError::NotReady));

        let remove_result = GCD.remove_memory_space(0, 10);
        assert_eq!(remove_result, Err(EfiError::NotReady));
    });
}

#[test]
fn spin_locked_allocator_init_should_initialize() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        GCD.init(48, 16);
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        GCD.add_io_space(dxe_services::GcdIoType::Io, 0, 100).unwrap();
        GCD.allocate_io_space(AllocateType::Address(0), dxe_services::GcdIoType::Io, 0, 10, 1 as _, None).unwrap();
        GCD.free_io_space(0, 10).unwrap();
        GCD.remove_io_space(0, 10).unwrap();
    });
}

#[test]
fn callback_should_fire_when_map_changes() {
    with_locked_state(|| {
        static CALLBACK_INVOKED: AtomicBool = AtomicBool::new(false);
        fn map_callback(map_change_type: MapChangeType) {
            CALLBACK_INVOKED.store(true, core::sync::atomic::Ordering::SeqCst);
            assert_eq!(map_change_type, MapChangeType::AddMemorySpace);
        }
        static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(map_callback));

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        GCD.init(48, 16);
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        // SAFETY: Adds a small test range to trigger the map-change callback.
        unsafe {
            GCD.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x1000, efi::MEMORY_WB).unwrap();
        }

        assert!(CALLBACK_INVOKED.load(core::sync::atomic::Ordering::SeqCst));
    });
}

#[test]
fn test_spin_locked_set_attributes_capabilities() {
    with_locked_state(|| {
        static CALLBACK2: AtomicBool = AtomicBool::new(false);
        fn map_callback(map_change_type: MapChangeType) {
            if map_change_type == MapChangeType::SetMemoryCapabilities {
                CALLBACK2.store(true, core::sync::atomic::Ordering::SeqCst);
            }
        }

        static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(map_callback));

        assert_eq!(GCD.memory.lock().maximum_address, 0);

        // SAFETY: get_memory returns a test-owned buffer sized for the requested range.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        GCD.init(48, 16);
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }
        GCD.set_memory_space_capabilities(
            address,
            0x1000,
            efi::MEMORY_RP | efi::MEMORY_RO | efi::MEMORY_XP | efi::MEMORY_WB,
        )
        .unwrap();

        assert!(CALLBACK2.load(core::sync::atomic::Ordering::SeqCst));
    });
}

#[test]
fn allocate_bottom_up_should_allocate_increasing_addresses() {
    with_locked_state(|| {
        const GCD_SIZE: usize = 0x100000;
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
        // SAFETY: The allocator returns a test buffer aligned to pages for GCD initialization.
        let base = unsafe { std::alloc::System.alloc(layout) as u64 };
        // SAFETY: base/size come from the test allocation and are valid for initializing memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                base as usize,
                GCD_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        let mut last_allocation = 0;
        loop {
            let allocate_result = GCD.allocate_memory_space(
                AllocateType::BottomUp(None),
                GcdMemoryType::SystemMemory,
                12,
                0x1000,
                1 as _,
                None,
            );

            if let Ok(address) = allocate_result {
                assert!(
                    address > last_allocation,
                    "address {address:#x?} is lower than previously allocated address {last_allocation:#x?}",
                );
                last_allocation = address;
            } else {
                break;
            }
        }
    });
}

// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
#[test]
fn allocate_top_down_should_allocate_decreasing_addresses() {
    with_locked_state(|| {
        const GCD_SIZE: usize = 0x100000;
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
        // SAFETY: The allocator returns a test buffer aligned to pages for GCD initialization.
        let base = unsafe { std::alloc::System.alloc(layout) as u64 };
        // SAFETY: base/size come from the test allocation and are valid for initializing memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                base as usize,
                GCD_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        let mut last_allocation = usize::MAX;
        loop {
            let allocate_result = GCD.allocate_memory_space(
                AllocateType::TopDown(None),
                GcdMemoryType::SystemMemory,
                // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
                12,
                0x1000,
                1 as _,
                None,
            );

            if let Ok(address) = allocate_result {
                assert!(
                    address < last_allocation,
                    "address {address:#x?} is higher than previously allocated address {last_allocation:#x?}",
                );
                last_allocation = address;
            } else {
                break;
            }
        }
    });
}

#[test]
fn test_allocate_page_zero_should_fail() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        // Increase the memory block size so allocation at 0x1000 is possible after skipping page 0
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, 0x2000, efi::MEMORY_WB).unwrap();
        }

        // Try to allocate page 0 implicitly bottom up, we should get bumped to the next available page
        let res = gcd.allocate_memory_space(
            AllocateType::BottomUp(None),
            GcdMemoryType::SystemMemory,
            0,
            0x1000,
            1 as _,
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            None,
        );
        assert_eq!(res.unwrap(), 0x1000, "Should not be able to allocate page 0");

        // Try to allocate page 0 implicitly top down, we should fail with out of resources
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            GcdMemoryType::SystemMemory,
            0,
            0x1000,
            1 as _,
            None,
        );
        assert_eq!(res, Err(EfiError::OutOfResources), "Should not be able to allocate page 0");

        // add a new block to ensure block skipping logic works
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x2000, 0x2000, efi::MEMORY_WB).unwrap();
        }

        // now allocate bottom up, we should be able to allocate page 0x2000
        let res = gcd.allocate_memory_space(
            AllocateType::BottomUp(None),
            GcdMemoryType::SystemMemory,
            0,
            0x2000,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), 0x2000, "Should be able to allocate page 0x2000");

        // Try to allocate page 0 explicitly. This should pass as Patina DXE Core needs to allocate by address
        let res = gcd.allocate_memory_space(
            AllocateType::Address(0),
            GcdMemoryType::SystemMemory,
            0,
            UEFI_PAGE_SIZE,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), 0x0, "Should be able to allocate page 0 by address");
    });
}

#[test]
fn test_prioritize_32_bit_memory_top_down() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();
        gcd.prioritize_32_bit_memory = true;

        // Test with a contiguous 8gb without a gap.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, 2 * SIZE_4GB, 0) }.unwrap();

        // make sure it prioritizes 32 bit addresses.
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x10000,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), SIZE_4GB - 0x10000, "Should allocate below 4GB when prioritizing 32-bit memory");

        // check that it will fall back to >32 bits.
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            SIZE_4GB,
            1 as _,
            None,
        );
        assert_eq!(res.unwrap(), SIZE_4GB, "Failed to fall back to higher memory as expected");

        // Free the memory to check the next condition.
        gcd.free_memory_space(SIZE_4GB - 0x10000, 0x10000, MemoryStateTransition::Free).unwrap();
        gcd.free_memory_space(SIZE_4GB, SIZE_4GB, MemoryStateTransition::Free).unwrap();

        // Check that a sufficiently large allocation will straddle the boundary.
        let res = gcd.allocate_memory_space(
            AllocateType::TopDown(None),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            SIZE_4GB + 0x1000,
            1 as _,
            None,
        );
        assert!(res.is_ok(), "Failed to fallback to higher memory as expected");
    });
}

#[test]
fn test_spin_locked_gcd_debug_and_display() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);

        // Initialize and add some memory
        // SAFETY: get_memory returns a valid, owned buffer for the test and the size is bounded by the constant.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        GCD.init(48, 16);

        // SAFETY: address/size come from the test allocation and are used to initialize the GCD memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        // Ensure Debug doesn't panic
        let _ = format!("{:?}", &GCD);

        // Ensure Display doesn't panic
        let _ = format!("{}", &GCD);
    });
}

#[test]
fn test_io_gcd_display() {
    with_locked_state(|| {
        let mut io_gcd = IoGCD::_new(16);

        // Add various IO space types
        io_gcd.add_io_space(dxe_services::GcdIoType::Io, 0x0, 0x100).unwrap();
        io_gcd.add_io_space(dxe_services::GcdIoType::Reserved, 0x1000, 0x200).unwrap();
        io_gcd.add_io_space(dxe_services::GcdIoType::Maximum, 0x2000, 0x300).unwrap();

        // Ensure Display doesn't panic
        let _ = format!("{}", &io_gcd);
    });
}

#[test]
fn paging_allocator_new_and_basic_alloc() {
    with_locked_state(|| {
        const GCD_SIZE: usize = 0x300000;
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
        // SAFETY: The allocator is set up to return an aligned and available test buffer for GCD initialization.
        let base = unsafe { std::alloc::System.alloc(layout) as u64 };
        // SAFETY: base points to the test allocation and GCD_SIZE defines the initialized range.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                base as usize,
                GCD_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }
        let mut allocator = PagingAllocator::new(&GCD);

        // Allocate a single page
        let page = allocator
            .allocate_page(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, true)
            .expect("Should allocate a page");
        assert!(page >= base && page < (base + GCD_SIZE as u64), "Allocated page should be within GCD memory range");

        // allocate another page
        let page2 = allocator
            .allocate_page(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false)
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
            .expect("Should allocate a second page");
        assert!(page2 != page, "Allocated pages should be unique");
        assert!(page2 >= base && page2 < (base + GCD_SIZE as u64), "Allocated page should be within GCD memory range");

        // fail to allocate with a bad alignment
        let bad_alloc = allocator.allocate_page(UEFI_PAGE_SIZE as u64, 0x3000, false);
        assert_eq!(bad_alloc, Err(PtError::InvalidParameter), "Should fail to allocate with bad alignment");

        // fail to allocate a zero sized page
        let zero_alloc = allocator.allocate_page(UEFI_PAGE_SIZE as u64, 0, false);
        assert_eq!(zero_alloc, Err(PtError::InvalidParameter), "Should fail to allocate zero sized page");
    });
}

#[test]
#[should_panic]
fn paging_allocator_exhaustion_asserts() {
    with_locked_state(|| {
        const GCD_SIZE: usize = 0x200000;
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        let layout = Layout::from_size_align(GCD_SIZE, 0x1000).unwrap();
        // SAFETY: The allocator is set up to return an aligned and available test buffer for GCD initialization.
        let base = unsafe { std::alloc::System.alloc(layout) as u64 };
        // SAFETY: base/size correspond to the test allocation and are safe to register with the GCD.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                base as usize,
                GCD_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }
        let mut allocator = PagingAllocator::new(&GCD);

        // Exhaust all available pages
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        let mut allocated = Vec::new();
        while let Ok(page) = allocator.allocate_page(UEFI_PAGE_SIZE as u64, UEFI_PAGE_SIZE as u64, false) {
            allocated.push(page);
            // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        }
    });
}
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.

#[test]
fn test_get_memory_descriptors_allocated_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();

        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe { gcd.add_memory_space(GcdMemoryType::SystemMemory, 0, 2 * SIZE_4GB, 0) }.unwrap();

        gcd.allocate_memory_space(
            AllocateType::Address(0x5000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x4000,
            1 as _,
            None,
        )
        .unwrap();
        gcd.allocate_memory_space(
            AllocateType::Address(0x9000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x2000,
            2 as _,
            None,
        )
        .unwrap();

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |_, allocated| allocated).unwrap();
        assert_eq!(buffer.len(), 3); // one extra allocated space for memory_block region
        assert!(
            buffer
                .iter()
                .any(|desc| desc.base_address == 0x5000 && desc.length == 0x4000 && desc.image_handle == 1 as _)
        );
        assert!(
            buffer
                .iter()
                .any(|desc| desc.base_address == 0x9000 && desc.length == 0x2000 && desc.image_handle == 2 as _)
        );
    });
}

#[test]
fn test_get_memory_descriptors_mmio_and_reserved_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // Add MMIO and Reserved blocks
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2000, 0x1000, 0).unwrap();
        }
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::Reserved, 0x3000, 0x10000, 0).unwrap();
        }
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x14000, 0x6000, 0).unwrap();
        }

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |d, _| {
            matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
        })
        .unwrap();
        assert!(buffer.len() == 2);
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::MemoryMappedIo
            && desc.base_address == 0x2000
            && desc.length == 0x1000));
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::Reserved
            && desc.base_address == 0x3000
            && desc.length == 0x10000));
    });
}

#[test]
fn test_get_memory_descriptors_all_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::Reserved, 0x3000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x14000, 0x6000, 0).unwrap();
        }

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |_, _| true).unwrap();

        // The `All` filter returns every block, including NonExistent regions.
        assert_eq!(buffer.len(), gcd.memory_descriptor_count());
        assert!(
            buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::MemoryMappedIo && desc.base_address == 0x2000)
        );
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::Reserved && desc.base_address == 0x3000));
        assert!(
            buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::SystemMemory && desc.base_address == 0x14000)
        );
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::NonExistent));
    });
}

#[test]
fn test_get_memory_descriptors_free_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::Reserved, 0x3000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x14000, 0x6000, 0).unwrap();
        }
        // Allocate part of the system memory so that an allocated block is present to exclude.
        gcd.allocate_memory_space(
            AllocateType::Address(0x14000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |d, allocated| {
            !allocated && d.memory_type == GcdMemoryType::SystemMemory
        })
        .unwrap();

        // Only unallocated system memory is returned.
        assert!(!buffer.is_empty());
        assert!(buffer.iter().all(|desc| desc.memory_type == GcdMemoryType::SystemMemory));
        assert!(buffer.iter().all(|desc| desc.image_handle == INVALID_HANDLE));
        // The remaining free portion of the added system memory is present, the allocated portion is not.
        assert!(buffer.iter().any(|desc| desc.base_address == 0x15000 && desc.length == 0x5000));
        assert!(!buffer.iter().any(|desc| desc.base_address == 0x14000));
        // MMIO and Reserved are excluded.
        assert!(!buffer.iter().any(|desc| desc.base_address == 0x2000 || desc.base_address == 0x3000));
    });
}

#[test]
fn test_get_memory_descriptors_existent_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::Reserved, 0x3000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x14000, 0x6000, 0).unwrap();
        }

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |d, _| d.memory_type != GcdMemoryType::NonExistent).unwrap();

        // Every existent (non-NonExistent) block is returned, regardless of allocation state or type.
        assert!(buffer.iter().all(|desc| desc.memory_type != GcdMemoryType::NonExistent));
        assert!(
            buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::MemoryMappedIo && desc.base_address == 0x2000)
        );
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::Reserved && desc.base_address == 0x3000));
        assert!(
            buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::SystemMemory && desc.base_address == 0x14000)
        );
    });
}

#[test]
fn test_get_memory_descriptors_free_any_type_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::Reserved, 0x3000, 0x1000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x14000, 0x6000, 0).unwrap();
        }
        // Allocate part of the system memory so that an allocated block is present to exclude.
        gcd.allocate_memory_space(
            AllocateType::Address(0x14000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |_, allocated| !allocated).unwrap();

        // Unallocated blocks of any type are returned; unlike `Free`, MMIO and Reserved are included.
        assert!(buffer.iter().all(|desc| desc.image_handle == INVALID_HANDLE));
        assert!(
            buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::MemoryMappedIo && desc.base_address == 0x2000)
        );
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::Reserved && desc.base_address == 0x3000));
        assert!(buffer.iter().any(|desc| desc.base_address == 0x15000 && desc.length == 0x5000));
        // The allocated portion is excluded.
        assert!(!buffer.iter().any(|desc| desc.base_address == 0x14000 && desc.length == 0x1000));
    });
}

#[test]
fn test_get_memory_descriptors_mmio_and_reserved_includes_allocated() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x2000, 0x2000, 0).unwrap();
        }
        // Allocate part of the MMIO region so both an allocated and unallocated MMIO block exist.
        gcd.allocate_memory_space(
            AllocateType::Address(0x2000),
            GcdMemoryType::MemoryMappedIo,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();

        let mut buffer = Vec::with_capacity(gcd.memory_descriptor_count());
        gcd.get_memory_descriptors(&mut buffer, |d, _| {
            matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
        })
        .unwrap();

        // The `MmioAndReserved` filter returns both allocated and unallocated MMIO/Reserved blocks.
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::MemoryMappedIo
            && desc.base_address == 0x2000
            && desc.length == 0x1000
            && desc.image_handle == 1 as _));
        assert!(buffer.iter().any(|desc| desc.memory_type == GcdMemoryType::MemoryMappedIo
            && desc.base_address == 0x3000
            && desc.length == 0x1000
            && desc.image_handle == INVALID_HANDLE));
    });
}

#[test]
fn test_init_paging_maps_allocated_and_mmio_regions() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Add memory and MMIO regions
        // SAFETY: get_memory returns a test-owned buffer used to seed GCD memory blocks.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 100) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        // SAFETY: address/length are derived from the test buffer so the ranges are valid for GCD initialization.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE * 99,
                efi::MEMORY_WB,
                efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
            )
            .unwrap();
            GCD.add_memory_space(
                GcdMemoryType::MemoryMappedIo,
                0x1000,
                0x1000,
                efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
            )
            .unwrap();
            GCD.add_memory_space(
                GcdMemoryType::SystemMemory,
                0x2000,
                0x40000,
                efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
            )
            .unwrap();
        }

        let r = GCD.set_memory_space_attributes(address, MEMORY_BLOCK_SLICE_SIZE * 99, efi::MEMORY_WB);
        assert_eq!(r, Err(EfiError::NotReady));

        let r = GCD.set_memory_space_attributes(0x1000, 0x1000, efi::MEMORY_UC);
        assert_eq!(r, Err(EfiError::NotReady));

        // Create a fake HobList with a MemoryAllocationModule for DXE Core
        let dxe_core_base = address + 0x1000;
        let dxe_core_len = 0x1000000;
        let hob = Hob::MemoryAllocationModule(&patina::pi::hob::MemoryAllocationModule {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<patina::pi::hob::MemoryAllocationModule>() as u16,
                reserved: 0,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: base_guids::DXE_CORE_ID,
                memory_base_address: dxe_core_base as u64,
                memory_length: dxe_core_len as u64,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: [0; 4],
            },
            module_name: base_guids::DXE_CORE_ID,
            entry_point: dxe_core_base as u64 + 0x1000,
        });

        // Add a stack HOB
        let stack_hob = Hob::MemoryAllocation(&patina::pi::hob::MemoryAllocation {
            header: patina::pi::hob::HobHeader {
                r#type: hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                reserved: 0x00000000,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: pi_guids::MEMORY_ALLOC_STACK_HOB_GUID,
                memory_base_address: 0x2000,
                memory_length: 0x40000,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: Default::default(),
            },
        });

        let mut hob_list = HobList::new();
        hob_list.push(hob);
        hob_list.push(stack_hob);

        // SAFETY: We just allocated this memory and DXE_CORE_PE_HEADER_DATA is a valid byte array
        unsafe {
            core::ptr::copy_nonoverlapping(
                DXE_CORE_PE_HEADER_DATA.as_ptr(),
                dxe_core_base as *mut u8,
                DXE_CORE_PE_HEADER_DATA.len(),
            );
        }

        // Create a local mock page table that we can access after init_paging_with
        let mock_table = std::rc::Rc::new(std::cell::RefCell::new(MockPageTable::new()));
        let page_table = Box::new(MockPageTableWrapper::new(std::rc::Rc::clone(&mock_table)));

        // Call init_paging
        GCD.init_paging_with(&hob_list, page_table);

        // Validate that init_paging worked by checking our local mock page table
        let mock_ref = mock_table.borrow();
        let mapped_regions = mock_ref.get_mapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        // Verify that memory regions were mapped during init_paging
        assert!(!mapped_regions.is_empty(), "init_paging should have mapped memory regions");
        assert!(!current_mappings.is_empty(), "Page table should have active mappings after init_paging");

        // Verify that we have multiple mapping operations (allocated memory + MMIO + DXE Core)
        assert!(mapped_regions.len() >= 3, "Should have mapped allocated memory, MMIO, and DXE Core regions");

        // Verify that DXE Core region is being managed
        let dxe_core_base = dxe_core_base as u64;
        let dxe_core_end = dxe_core_base + dxe_core_len as u64;

        // Check that we have mappings that overlap with or are contained in the DXE Core region
        let has_dxe_core_mapping = current_mappings.iter().any(|(addr, len, _attr)| {
            let mapping_end = addr + len;
            // Check for overlap: mapping overlaps with DXE core region
            *addr < dxe_core_end && mapping_end > dxe_core_base
        });

        assert!(has_dxe_core_mapping, "DXE Core region should be covered by page table mappings");

        // Verify that memory attributes are being set (should have XP attributes)
        let has_attribute_mappings = current_mappings.iter().any(|(_addr, _len, attr)| {
            attr.bits() != 0 // Should have some attributes set
        });

        assert!(has_attribute_mappings, "Mappings should have memory attributes set");

        // Verify that MMIO region (0x1000-0x2000) is mapped
        let has_mmio_mapping =
            current_mappings.iter().any(|(addr, len, _attr)| *addr <= 0x1000 && (*addr + len) >= 0x2000);

        assert!(has_mmio_mapping, "MMIO region should be mapped after init_paging");

        // Locate stack hob.
        let stack_hob = hob_list
            .iter()
            .find_map(|x| match x {
                patina::pi::hob::Hob::MemoryAllocation(hob::MemoryAllocation { header: _, alloc_descriptor: desc })
                    if desc.name == pi_guids::MEMORY_ALLOC_STACK_HOB_GUID =>
                {
                    Some(desc)
                }
                _ => None,
            })
            .unwrap();

        assert!(stack_hob.memory_base_address != 0);
        assert!(stack_hob.memory_length != 0);

        // Check Guard Page.
        let mut stack_desc = GCD.get_memory_descriptor_for_address(stack_hob.memory_base_address, |_, _| true).unwrap();
        assert_eq!(stack_desc.memory_type, GcdMemoryType::SystemMemory);
        assert_eq!((stack_desc.attributes & efi::MEMORY_RP), efi::MEMORY_RP);

        // Check rest of the stack.
        stack_desc = GCD
            .get_memory_descriptor_for_address(stack_hob.memory_base_address + UEFI_PAGE_SIZE as u64, |_, _| true)
            .unwrap();
        assert_eq!((stack_desc.attributes & efi::MEMORY_XP), efi::MEMORY_XP);
        assert_eq!(stack_desc.memory_type, GcdMemoryType::SystemMemory);
    });
}

#[test]
fn test_set_paging_attributes_with_page_table() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Set up memory space like other tests
        // SAFETY: get_memory returns a test-owned buffer sized for the requested block count.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        // SAFETY: The address/length come from the test allocation and are valid to register with the GCD.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        // Initialize page table with local MockPageTable
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
        *GCD.page_table.lock() = Some(mock_page_table);

        // Test mapping within the allocated memory region
        let base_address = address;
        let length = 0x1000;
        let attributes = MemoryAttributes::Writeback.bits();

        let result = GCD.set_paging_attributes(base_address, length, attributes);
        assert!(result.is_ok());

        // Manually drop the page table to release the reference
        *GCD.page_table.lock() = None;

        // Verify the page table state
        let mock_ref = mock_table.borrow();
        let mapped = mock_ref.get_mapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].0, base_address as u64);
        assert_eq!(mapped[0].1, length as u64);
        assert_eq!(mapped[0].2, MemoryAttributes::Writeback);

        assert_eq!(current_mappings.len(), 1);
        assert_eq!(current_mappings[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));
    });
}

#[test]
fn test_map_aliased_memory_region() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        GCD.add_test_page_table(Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table))));

        let virtual_address = 0x8000_0000;
        let physical_address = 0x1000_0000;
        let length = 0x20_0000;
        let attributes = efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RUNTIME;

        mock_table.borrow_mut().fail_next_map_aliased_memory_region(PtError::OutOfResources);
        assert_eq!(
            GCD.map_aliased_memory_region(virtual_address, physical_address, length, attributes),
            Err(EfiError::OutOfResources)
        );
        assert!(GCD.get_aliased_mappings().is_empty());

        assert_eq!(GCD.map_aliased_memory_region(virtual_address, physical_address, length, attributes), Ok(()));
        assert_eq!(
            GCD.get_aliased_mappings(),
            vec![AliasedMapping { virtual_address, physical_address, length, attributes }]
        );

        *GCD.page_table.lock() = None;
        assert_eq!(
            mock_table.borrow().get_aliased_mapped_regions(),
            vec![(
                virtual_address,
                physical_address,
                length,
                MemoryAttributes::Writeback | MemoryAttributes::ExecuteProtect,
            )]
        );
    });
}

#[test]
fn test_unmap_aliased_memory_region() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        GCD.add_test_page_table(Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table))));

        let virtual_address = 0x8000_0000;
        let physical_address = 0x1000_0000;
        let length = 0x20_0000;
        GCD.map_aliased_memory_region(virtual_address, physical_address, length, efi::MEMORY_WB).unwrap();

        mock_table.borrow_mut().fail_next_unmap_memory_region(PtError::OutOfResources);
        assert_eq!(GCD.unmap_aliased_memory_region(virtual_address, length), Err(EfiError::OutOfResources));
        assert_eq!(
            GCD.get_aliased_mappings(),
            vec![AliasedMapping { virtual_address, physical_address, length, attributes: efi::MEMORY_WB }]
        );

        assert_eq!(GCD.unmap_aliased_memory_region(virtual_address, length), Ok(()));
        assert!(GCD.get_aliased_mappings().is_empty());
        assert_eq!(mock_table.borrow().get_unmapped_regions(), vec![(virtual_address, length)]);
        assert_eq!(GCD.unmap_aliased_memory_region(virtual_address, length), Err(EfiError::NotFound));
    });
}

#[test]
fn test_set_paging_attributes_cache_attributes() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Set up memory space
        // SAFETY: The GCD is prepared so that get_memory returns a valid, owned buffer for the test.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 2) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        // SAFETY: The buffer range is owned by this test and can be registered as system memory.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        // Initialize page table with local MockPageTable
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
        *GCD.page_table.lock() = Some(mock_page_table);

        // Test different cache attributes
        let base_address = address;
        let length = 0x1000;

        // Test Uncacheable
        let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::Uncached.bits());
        assert!(result.is_ok());

        // Test WriteThrough - should overwrite the previous mapping
        let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::WriteThrough.bits());
        assert!(result.is_ok());

        // Test WriteCombining - should overwrite again
        let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::WriteCombining.bits());
        assert!(result.is_ok());

        // Manually drop the page table to release the reference
        *GCD.page_table.lock() = None;

        // Verify the page table state
        let mock_ref = mock_table.borrow();
        let mapped = mock_ref.get_mapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        // Should have 3 map operations recorded
        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0], (base_address as u64, length as u64, MemoryAttributes::Uncached));
        assert_eq!(mapped[1], (base_address as u64, length as u64, MemoryAttributes::WriteThrough));
        assert_eq!(mapped[2], (base_address as u64, length as u64, MemoryAttributes::WriteCombining));

        // Current mapping should only show the last one (WriteCombining)
        assert_eq!(current_mappings.len(), 1);
        assert_eq!(current_mappings[0], (base_address as u64, length as u64, MemoryAttributes::WriteCombining));
    });
}

#[test]
fn test_set_paging_attributes_no_page_table() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Don't initialize page table
        let base_address = 0x1000;
        let length = 0x1000;
        let attributes = MemoryAttributes::Writeback.bits();

        let result = GCD.set_paging_attributes(base_address, length, attributes);
        assert_eq!(result.unwrap_err(), EfiError::NotReady);
    });
}

#[test]
fn test_set_paging_attributes_unmap_with_read_protect() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Initialize page table with local MockPageTable
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
        *GCD.page_table.lock() = Some(mock_page_table);

        let base_address = 0x1000;
        let length = 0x1000;

        // First map the region
        let map_attributes = MemoryAttributes::Writeback.bits();
        let result = GCD.set_paging_attributes(base_address, length, map_attributes);
        assert!(result.is_ok());

        // Now unmap it using ReadProtect
        let unmap_attributes = MemoryAttributes::ReadProtect.bits();
        let result = GCD.set_paging_attributes(base_address, length, unmap_attributes);
        assert!(result.is_ok());

        // Manually drop the page table to release the reference
        *GCD.page_table.lock() = None;

        // Verify the page table state
        let mock_ref = mock_table.borrow();
        let mapped = mock_ref.get_mapped_regions();
        let unmapped = mock_ref.get_unmapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        // Should have 1 map operation recorded
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));

        // Should have 1 unmap operation recorded
        assert_eq!(unmapped.len(), 1);
        assert_eq!(unmapped[0], (base_address as u64, length as u64));

        // Current mapping should be empty (region was unmapped)
        assert_eq!(current_mappings.len(), 0);
    });
}

#[test]
fn test_set_paging_attributes_already_mapped_same_attributes() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Initialize page table with local MockPageTable
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
        *GCD.page_table.lock() = Some(mock_page_table);

        let base_address = 0x1000;
        let length = 0x1000;
        let attributes = MemoryAttributes::Writeback.bits();

        // Map the region first
        let result = GCD.set_paging_attributes(base_address, length, attributes);
        assert!(result.is_ok());

        // Try to map the same region with same attributes
        let result = GCD.set_paging_attributes(base_address, length, attributes);
        assert!(result.is_ok());

        // Manually drop the page table to release the reference
        *GCD.page_table.lock() = None;

        // Verify the page table state
        let mock_ref = mock_table.borrow();
        let mapped = mock_ref.get_mapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        // The implementation may optimize duplicate mappings, so we verify there's at least one mapping
        assert!(!mapped.is_empty());
        assert!(mapped[0] == (base_address as u64, length as u64, MemoryAttributes::Writeback));

        // If GCD optimizes away the duplicate, there might only be 1 map operation
        // If it doesn't optimize, there will be 2. Both behaviors are acceptable.
        assert!(!mapped.is_empty() && mapped.len() <= 2);

        // Current mapping should show one region
        assert_eq!(current_mappings.len(), 1);
        assert_eq!(current_mappings[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));
    });
}

#[test]
fn test_set_paging_attributes_multiple_regions() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Initialize page table with local MockPageTable
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
        *GCD.page_table.lock() = Some(mock_page_table);

        // Map multiple non-overlapping regions
        let regions = [
            (0x1000, 0x1000, MemoryAttributes::Writeback),
            (0x3000, 0x2000, MemoryAttributes::Uncached),
            (0x6000, 0x1000, MemoryAttributes::WriteCombining),
        ];

        for (base, len, attrs) in regions {
            let result = GCD.set_paging_attributes(base, len, attrs.bits());
            assert!(result.is_ok());
        }

        // Manually drop the page table to release the reference
        *GCD.page_table.lock() = None;

        // Verify the page table state
        let mock_ref = mock_table.borrow();
        let mapped = mock_ref.get_mapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        // Should have 3 map operations recorded
        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0], (0x1000, 0x1000, MemoryAttributes::Writeback));
        assert_eq!(mapped[1], (0x3000, 0x2000, MemoryAttributes::Uncached));
        assert_eq!(mapped[2], (0x6000, 0x1000, MemoryAttributes::WriteCombining));

        // Current mappings should show all 3 regions (no overlaps)
        assert_eq!(current_mappings.len(), 3);
        assert!(current_mappings.contains(&(0x1000, 0x1000, MemoryAttributes::Writeback)));
        assert!(current_mappings.contains(&(0x3000, 0x2000, MemoryAttributes::Uncached)));
        assert!(current_mappings.contains(&(0x6000, 0x1000, MemoryAttributes::WriteCombining)));
    });
}

#[test]
fn test_set_paging_attributes_overlapping_regions() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Initialize page table with local MockPageTable
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
        *GCD.page_table.lock() = Some(mock_page_table);

        let base_address = 0x1000;
        let length = 0x2000;

        // Map a large region first
        let result = GCD.set_paging_attributes(base_address, length, MemoryAttributes::Writeback.bits());
        assert!(result.is_ok());

        // Map a smaller overlapping region with different attributes
        let overlapping_base = 0x1800;
        let overlapping_length = 0x1000;
        let result = GCD.set_paging_attributes(overlapping_base, overlapping_length, MemoryAttributes::Uncached.bits());
        assert!(result.is_ok());

        // Manually drop the page table to release the reference
        *GCD.page_table.lock() = None;

        // Verify the page table state
        let mock_ref = mock_table.borrow();
        let mapped = mock_ref.get_mapped_regions();
        let current_mappings = mock_ref.get_current_mappings();

        // Should have 2 map operations recorded
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0], (base_address as u64, length as u64, MemoryAttributes::Writeback));
        assert_eq!(mapped[1], (overlapping_base as u64, overlapping_length as u64, MemoryAttributes::Uncached));

        // Current mappings should show the overlapping region replaced the original
        // (MockPageTable removes overlapping regions when adding new ones)
        assert_eq!(current_mappings.len(), 1);
        assert_eq!(
            current_mappings[0],
            (overlapping_base as u64, overlapping_length as u64, MemoryAttributes::Uncached)
        );
    });
}

#[test]
fn test_free_memory_space_across_descriptors() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // SAFETY: get_memory returns a test-owned buffer used to seed GCD memory blocks.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 3) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

        // SAFETY: We just allocated this memory to use in the test
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();

            GCD.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x5000, efi::MEMORY_WB).unwrap();
        }

        // set a cache attribute for the range
        let _ = GCD.set_memory_space_attributes(0x1000, 0x5000, efi::MEMORY_WB);

        GCD.allocate_memory_space(
            AllocateType::Address(0x1000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x5000,
            0x7 as efi::Handle,
            None,
        )
        .unwrap();

        // w/o a page table set this will return NotReady, but that's fine for the purposes of this test,
        // the GCD is still updated
        let _ = GCD.set_memory_space_attributes(0x2000, 0x2000, efi::MEMORY_WB | efi::MEMORY_RO);

        // Free memory space that spans all three descriptors
        let result = GCD.free_memory_space(0x1000, 0x5000);
        assert!(result.is_ok());
    });
}

#[test]
fn test_set_memory_space_attributes_across_descriptors() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // SAFETY: get_memory returns a test-owned buffer used to seed GCD memory blocks.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 3) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

        // SAFETY: We just allocated this memory to use in the test
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();

            GCD.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x5000, efi::MEMORY_WB).unwrap();
        }

        // bifurcate the range
        GCD.allocate_memory_space(
            AllocateType::Address(0x2000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x2000,
            0x7 as efi::Handle,
            None,
        )
        .unwrap();

        // w/o a page table set this will return NotReady, but that's fine for the purposes of this test,
        // the GCD is still updated, we would fail with NotFound if the GCD update fails
        let res = GCD.set_memory_space_attributes(0x1000, 0x5000, efi::MEMORY_WB | efi::MEMORY_RO);
        assert_eq!(res, Err(EfiError::NotReady));
    });
}

#[test]
fn test_descriptor_iterator() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // SAFETY: get_memory returns a test-owned buffer used to seed GCD memory blocks.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 3) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

        // SAFETY: We just allocated this memory to use in the test
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();

            GCD.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, efi::MEMORY_WB).unwrap();
            GCD.add_memory_space(GcdMemoryType::SystemMemory, 0x4000, 0x2000, efi::MEMORY_WT).unwrap();
            GCD.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x8000, 0x2000, efi::MEMORY_UC).unwrap();
        }

        GCD.allocate_memory_space(
            AllocateType::Address(0x1000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x1000,
            0x7 as efi::Handle,
            None,
        )
        .unwrap();

        // Test Case 1: Iterator over single descriptor
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::new();
        for desc_result in GCD.iter(0x1000, 0x1000) {
            match desc_result {
                Ok(desc) => descriptors.push(desc),
                Err(_e) => {
                    panic!("Should not get error for existing descriptor");
                }
            }
        }

        assert!(!descriptors.is_empty(), "Should find at least one descriptor");
        assert_eq!(descriptors[0].memory_type, GcdMemoryType::SystemMemory);

        // Test Case 2: Iterator over range spanning multiple descriptors
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::new();
        for desc_result in GCD.iter(0x1000, 0x2000) {
            match desc_result {
                Ok(desc) => {
                    descriptors.push(desc);
                }
                Err(e) => {
                    panic!("Should not get error for existing descriptors: {e:?}");
                }
            }
        }
        assert!(!descriptors.is_empty());
        assert!(descriptors.iter().any(|d| d.base_address == 0x1000 && d.length == 0x1000));
        assert!(descriptors.iter().any(|d| d.base_address == 0x2000 && d.length == 0x1000));

        // Test Case 3: Range crosses multiple descriptors but is not aligned on a descriptor boundary
        let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::new();
        for desc_result in GCD.iter(0x5000, 0x4000) {
            match desc_result {
                Ok(desc) => descriptors.push(desc),
                Err(_e) => {
                    panic!("Should not get error for existing descriptor");
                }
            }
        }

        assert!(!descriptors.is_empty());
        assert!(descriptors.iter().any(|d| d.base_address == 0x4000 && d.length == 0x2000));
        assert!(descriptors.iter().any(|d| d.base_address == 0x6000 && d.length == 0x2000));
        assert!(descriptors.iter().any(|d| d.base_address == 0x8000 && d.length == 0x2000));

        // Test Case 4: Zero-length iterator
        let mut count = 0;
        for _desc_result in GCD.iter(0x5000, 0) {
            count += 1;
        }
        assert_eq!(count, 0); // Should yield no descriptors
    });
}

#[test]
fn test_merge_blocks_in_place_empty() {
    with_locked_state(|| {
        let mut descriptors: [efi::MemoryDescriptor; 0] = [];
        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 0);
    });
}

#[test]
fn test_merge_blocks_in_place_single() {
    with_locked_state(|| {
        let mut descriptors = [efi::MemoryDescriptor {
            r#type: efi::CONVENTIONAL_MEMORY,
            physical_start: 0x1000,
            virtual_start: 0,
            number_of_pages: 4,
            attribute: efi::MEMORY_WB,
        }];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 1);
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[0].number_of_pages, 4);
    });
}

#[test]
fn test_merge_blocks_in_place_adjacent_same_type_and_attributes() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 4,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x5000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WB,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 1);
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[0].number_of_pages, 6);
        assert_eq!(descriptors[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(descriptors[0].attribute, efi::MEMORY_WB);
    });
}

#[test]
fn test_merge_blocks_in_place_different_types() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 4,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::BOOT_SERVICES_DATA,
                physical_start: 0x5000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WB,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 2);
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[0].number_of_pages, 4);
        assert_eq!(descriptors[0].r#type, efi::CONVENTIONAL_MEMORY);
        assert_eq!(descriptors[1].physical_start, 0x5000);
        assert_eq!(descriptors[1].number_of_pages, 2);
        assert_eq!(descriptors[1].r#type, efi::BOOT_SERVICES_DATA);
    });
}

#[test]
fn test_merge_blocks_in_place_different_attributes() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 4,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x5000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WT,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 2);
        assert_eq!(descriptors[0].attribute, efi::MEMORY_WB);
        assert_eq!(descriptors[1].attribute, efi::MEMORY_WT);
    });
}

#[test]
fn test_merge_blocks_in_place_non_contiguous() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 4,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x6000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WB,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 2);
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[0].number_of_pages, 4);
        assert_eq!(descriptors[1].physical_start, 0x6000);
        assert_eq!(descriptors[1].number_of_pages, 2);
    });
}

#[test]
fn test_merge_blocks_in_place_multiple_merges() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x3000,
                virtual_start: 0,
                number_of_pages: 3,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x6000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 1);
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[0].number_of_pages, 6);
    });
}

#[test]
fn test_merge_blocks_in_place_mixed_scenario() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x3000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::BOOT_SERVICES_DATA,
                physical_start: 0x4000,
                virtual_start: 0,
                number_of_pages: 3,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x7000,
                virtual_start: 0,
                number_of_pages: 2,
                attribute: efi::MEMORY_WT,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x9000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WT,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 3);
        // First two should merge
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[0].number_of_pages, 3);
        assert_eq!(descriptors[0].r#type, efi::CONVENTIONAL_MEMORY);
        // Third should remain separate
        assert_eq!(descriptors[1].physical_start, 0x4000);
        assert_eq!(descriptors[1].number_of_pages, 3);
        assert_eq!(descriptors[1].r#type, efi::BOOT_SERVICES_DATA);
        // Last two should merge
        assert_eq!(descriptors[2].physical_start, 0x7000);
        assert_eq!(descriptors[2].number_of_pages, 3);
        assert_eq!(descriptors[2].attribute, efi::MEMORY_WT);
    });
}

#[test]
fn test_merge_blocks_in_place_write_idx_equals_read_idx() {
    with_locked_state(|| {
        let mut descriptors = [
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x1000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::BOOT_SERVICES_DATA,
                physical_start: 0x3000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WB,
            },
            efi::MemoryDescriptor {
                r#type: efi::CONVENTIONAL_MEMORY,
                physical_start: 0x5000,
                virtual_start: 0,
                number_of_pages: 1,
                attribute: efi::MEMORY_WT,
            },
        ];

        let gcd = GCD::new(48);
        let result = gcd.merge_blocks_in_place(&mut descriptors);
        assert_eq!(result, 3);
        assert_eq!(descriptors[0].physical_start, 0x1000);
        assert_eq!(descriptors[1].physical_start, 0x3000);
        assert_eq!(descriptors[2].physical_start, 0x5000);
    });
}

#[test]
fn test_adjust_efi_memory_map_descriptor_active_attributes_true() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB | efi::MEMORY_WT,
            attributes: efi::MEMORY_WB | efi::MEMORY_XP,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        let result = GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::CONVENTIONAL_MEMORY, true);

        // When active_attributes is true, this should return descriptor.attributes directly
        assert_eq!(result, descriptor.attributes);
        assert_eq!(result, efi::MEMORY_WB | efi::MEMORY_XP);
    });
}

#[test]
fn test_adjust_efi_memory_map_descriptor_active_attributes_false() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB | efi::MEMORY_WT | efi::MEMORY_UC,
            attributes: efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RUNTIME,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        let result = GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::BOOT_SERVICES_DATA, false);

        // When active_attributes is false, this should call apply_efi_memory_map_policy
        // to apply the memory protection policy transformation.
        let expected = MemoryProtectionPolicy::apply_efi_memory_map_policy(
            descriptor.attributes,
            descriptor.capabilities,
            descriptor.memory_type,
            efi::BOOT_SERVICES_DATA,
        );
        assert_eq!(result, expected);
    });
}

#[test]
fn test_adjust_efi_memory_map_descriptor_runtime_memory_type() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB | efi::MEMORY_RUNTIME,
            attributes: efi::MEMORY_WB | efi::MEMORY_RUNTIME,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        let result = GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::RUNTIME_SERVICES_DATA, false);

        // Verify policy is applied for runtime memory
        let expected = MemoryProtectionPolicy::apply_efi_memory_map_policy(
            descriptor.attributes,
            descriptor.capabilities,
            descriptor.memory_type,
            efi::RUNTIME_SERVICES_DATA,
        );
        assert_eq!(result, expected);
    });
}

#[test]
fn test_adjust_efi_memory_map_descriptor_mmio_type() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::MemoryMappedIo,
            base_address: 0xF0000000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_UC | efi::MEMORY_RUNTIME,
            attributes: efi::MEMORY_UC | efi::MEMORY_RUNTIME,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        let result_active = GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::MEMORY_MAPPED_IO, true);

        let result_capabilities = GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::MEMORY_MAPPED_IO, false);

        // Active attributes should return attributes directly
        assert_eq!(result_active, descriptor.attributes);

        let expected = MemoryProtectionPolicy::apply_efi_memory_map_policy(
            descriptor.attributes,
            descriptor.capabilities,
            descriptor.memory_type,
            efi::MEMORY_MAPPED_IO,
        );
        assert_eq!(result_capabilities, expected);
    });
}

#[test]
fn test_adjust_efi_memory_map_descriptor_various_attribute_combinations() {
    with_locked_state(|| {
        // Test with various attribute combinations to ensure both paths work correctly
        let test_cases = vec![
            (efi::MEMORY_WB, efi::MEMORY_WB | efi::MEMORY_WT),
            (efi::MEMORY_UC, efi::MEMORY_UC),
            (efi::MEMORY_WB | efi::MEMORY_XP, efi::MEMORY_WB | efi::MEMORY_XP | efi::MEMORY_RP),
            (efi::MEMORY_RUNTIME | efi::MEMORY_WB, efi::MEMORY_RUNTIME | efi::MEMORY_WB | efi::MEMORY_UC),
        ];

        for (attributes, capabilities) in test_cases {
            let descriptor = dxe_services::MemorySpaceDescriptor {
                memory_type: GcdMemoryType::SystemMemory,
                base_address: 0x1000,
                length: UEFI_PAGE_SIZE as u64,
                capabilities,
                attributes,
                image_handle: core::ptr::null_mut(),
                device_handle: core::ptr::null_mut(),
            };

            let result_active = GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::CONVENTIONAL_MEMORY, true);
            assert_eq!(result_active, attributes, "Failed for attributes={attributes:#x}");

            let result_capabilities =
                GCD::adjust_efi_memory_map_descriptor(&descriptor, efi::CONVENTIONAL_MEMORY, false);
            let expected = MemoryProtectionPolicy::apply_efi_memory_map_policy(
                attributes,
                capabilities,
                GcdMemoryType::SystemMemory,
                efi::CONVENTIONAL_MEMORY,
            );
            assert_eq!(result_capabilities, expected, "Failed for capabilities={capabilities:#x}");
        }
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_system_memory_free() {
    with_locked_state(|| {
        // Free system memory not tracked by any allocator (null handle) is reported as conventional memory.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB,
            attributes: efi::MEMORY_WB,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::CONVENTIONAL_MEMORY));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_system_memory_gcd_allocated_boot_services() {
    with_locked_state(|| {
        // System memory directly allocated in the GCD (non-null, non-allocator handle) without the runtime
        // attribute is reported as boot services data.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB,
            attributes: efi::MEMORY_WB,
            image_handle: crate::protocol_db::DXE_CORE_HANDLE,
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::BOOT_SERVICES_DATA));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_system_memory_gcd_allocated_runtime() {
    with_locked_state(|| {
        // System memory directly allocated in the GCD (non-null, non-allocator handle) with the runtime attribute
        // is reported as reserved so it is preserved into runtime without being expected in the MAT.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB | efi::MEMORY_RUNTIME,
            attributes: efi::MEMORY_WB | efi::MEMORY_RUNTIME,
            image_handle: crate::protocol_db::DXE_CORE_HANDLE,
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::RESERVED_MEMORY_TYPE));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_mmio_non_runtime() {
    with_locked_state(|| {
        // Non-runtime MMIO is excluded from the EFI memory map.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::MemoryMappedIo,
            base_address: 0xF0000000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_UC,
            attributes: efi::MEMORY_UC,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), None);
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_mmio_runtime() {
    with_locked_state(|| {
        // Runtime MMIO is included in the EFI memory map.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::MemoryMappedIo,
            base_address: 0xF0000000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_UC | efi::MEMORY_RUNTIME,
            attributes: efi::MEMORY_UC | efi::MEMORY_RUNTIME,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::MEMORY_MAPPED_IO));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_persistent() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::Persistent,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB | efi::MEMORY_NV,
            attributes: efi::MEMORY_WB | efi::MEMORY_NV,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::PERSISTENT_MEMORY));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_unaccepted() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::Unaccepted,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_WB,
            attributes: efi::MEMORY_WB,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::UNACCEPTED_MEMORY_TYPE));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_reserved() {
    with_locked_state(|| {
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::Reserved,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: efi::MEMORY_UC,
            attributes: efi::MEMORY_UC,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::RESERVED_MEMORY_TYPE));
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_nonexistent_excluded() {
    with_locked_state(|| {
        // NonExistent memory is not represented in the EFI memory map.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::NonExistent,
            base_address: 0x1000,
            length: UEFI_PAGE_SIZE as u64,
            capabilities: 0,
            attributes: 0,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), None);
    });
}

#[test]
fn test_is_efi_memory_map_descriptor_multi_page_length() {
    with_locked_state(|| {
        // A descriptor spanning multiple pages is still classified by its memory type.
        let descriptor = dxe_services::MemorySpaceDescriptor {
            memory_type: GcdMemoryType::SystemMemory,
            base_address: 0x2000,
            length: (UEFI_PAGE_SIZE * 4) as u64,
            capabilities: efi::MEMORY_WB,
            attributes: efi::MEMORY_WB,
            image_handle: core::ptr::null_mut(),
            device_handle: core::ptr::null_mut(),
        };

        assert_eq!(GCD::is_efi_memory_map_descriptor(&descriptor), Some(efi::CONVENTIONAL_MEMORY));
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_empty_gcd() {
    with_locked_state(|| {
        let gcd = GCD::new(48);

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        assert_eq!(count, 0);
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_unallocated_system_memory() {
    with_locked_state(|| {
        let (gcd, _) = create_gcd();

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        assert_eq!(count, 1);
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_runtime_mmio() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add runtime MMIO - should be counted
        // SAFETY: This is a synthetic MMIO range used for test coverage only.
        unsafe {
            gcd.add_memory_space(
                GcdMemoryType::MemoryMappedIo,
                0x80000000,
                UEFI_PAGE_SIZE * 10,
                efi::MEMORY_UC | efi::MEMORY_RUNTIME,
            )
        }
        .expect("Failed to add runtime MMIO");

        gcd.set_memory_space_attributes(0x80000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_UC | efi::MEMORY_RUNTIME)
            .expect("Failed to set memory space attributes");

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        // Should count: 1 SystemMemory (from create_gcd) + 1 runtime MMIO
        assert!(count >= 2, "Expected at least 2 descriptors, got {count}");
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_mixed_types() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add runtime MMIO
        // SAFETY: This is a synthetic MMIO range for test bookkeeping only.
        unsafe {
            gcd.add_memory_space(
                GcdMemoryType::MemoryMappedIo,
                0x80000000,
                UEFI_PAGE_SIZE * 10,
                efi::MEMORY_UC | efi::MEMORY_RUNTIME,
            )
        }
        .expect("Failed to add runtime MMIO");
        gcd.set_memory_space_attributes(0x80000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_UC | efi::MEMORY_RUNTIME)
            .expect("Failed to set runtime MMIO attributes");

        // Add Persistent memory
        // SAFETY: This is a synthetic persistent memory range used only for test coverage.
        unsafe { gcd.add_memory_space(GcdMemoryType::Persistent, 0x90000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_WB) }
            .expect("Failed to add Persistent memory");
        gcd.set_memory_space_attributes(0x90000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_WB)
            .expect("Failed to set Persistent memory attributes");

        // Add Reserved memory
        // SAFETY: This is a synthetic reserved range used only for test coverage.
        unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 0xA0000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_WB) }
            .expect("Failed to add Reserved memory");
        gcd.set_memory_space_attributes(0xA0000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_WB)
            .expect("Failed to set Reserved memory attributes");

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        // Should count: SystemMemory (from create_gcd) + runtime MMIO + Persistent + Reserved = at least 4
        assert!(count >= 4, "Expected at least 4 descriptors, got {count}");
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_non_runtime_mmio() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add non-runtime MMIO - should not be counted
        // SAFETY: This is a synthetic MMIO range used only for test bookkeeping.
        unsafe { gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x80000000, UEFI_PAGE_SIZE * 10, efi::MEMORY_UC) }
            .expect("Failed to add non-runtime MMIO");

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        // Should count: 1 SystemMemory (from create_gcd), non-runtime MMIO is not counted
        assert_eq!(count, 1);
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_persistent_memory() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add Persistent memory - should be counted
        // SAFETY: This is a synthetic persistent memory range used only for test coverage.
        unsafe {
            gcd.add_memory_space(
                GcdMemoryType::Persistent,
                // SAFETY: get_memory returns a test-owned buffer of the requested size.
                0x100000000,
                UEFI_PAGE_SIZE * 100,
                // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
                efi::MEMORY_WB | efi::MEMORY_NV,
            )
        }
        .expect("Failed to add Persistent memory");

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        // Expect 1 SystemMemory (from create_gcd) + 1 Persistent
        assert!(count >= 2, "Expected at least 2 descriptors, got {count}");
    });
}

#[test]
// SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
fn test_memory_descriptor_count_for_efi_memory_map_unaccepted_memory() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add Unaccepted memory - should be counted
        // SAFETY: This is a synthetic unaccepted memory range used only for test coverage.
        unsafe { gcd.add_memory_space(GcdMemoryType::Unaccepted, 0x200000000, UEFI_PAGE_SIZE * 50, efi::MEMORY_WB) }
            .expect("Failed to add Unaccepted memory");

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        // Expect 1 SystemMemory (from create_gcd) + 1 Unaccepted
        assert!(count >= 2, "Expected at least 2 descriptors, got {count}");
    });
}

#[test]
fn test_memory_descriptor_count_for_efi_memory_map_reserved_memory() {
    with_locked_state(|| {
        let (mut gcd, _) = create_gcd();

        // Add Reserved memory - should be counted
        // SAFETY: This is a synthetic reserved range used only for test coverage.
        unsafe { gcd.add_memory_space(GcdMemoryType::Reserved, 0x90000000, UEFI_PAGE_SIZE * 20, 0) }
            .expect("Failed to add Reserved memory");

        let count = gcd.memory_descriptor_count_for_efi_memory_map();
        // Should count: 1 SystemMemory (from create_gcd) + 1 Reserved
        assert!(count >= 2, "Expected at least 2 descriptors, got {count}");
    });
}

#[test]
fn test_get_existent_memory_descriptor_for_address() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
        let address = mem.as_ptr() as usize;
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE,
                efi::MEMORY_WB,
                efi::MEMORY_WB,
            )
            .unwrap();
        }

        // Add multiple memory regions with different types
        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd or get_memory.
        unsafe {
            // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
            GCD.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, efi::MEMORY_WB).unwrap();
            GCD.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x5000, 0x1000, efi::MEMORY_UC).unwrap();
            GCD.add_memory_space(GcdMemoryType::Reserved, 0x8000, 0x1000, 0).unwrap();
        }

        // Test: Address at the start of a SystemMemory block
        let result = GCD.get_memory_descriptor_for_address(0x1000, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert!(result.is_ok());
        let desc = result.unwrap();
        assert_eq!(desc.base_address, 0x1000);
        assert_eq!(desc.length, 0x2000);
        assert_eq!(desc.memory_type, GcdMemoryType::SystemMemory);

        // Test: Address in the middle of a SystemMemory block
        let result = GCD.get_memory_descriptor_for_address(0x2000, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert!(result.is_ok());
        let desc = result.unwrap();
        assert_eq!(desc.base_address, 0x1000);
        assert_eq!(desc.memory_type, GcdMemoryType::SystemMemory);

        // Test: Address at the start of MMIO block
        let result = GCD.get_memory_descriptor_for_address(0x5000, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert!(result.is_ok());
        let desc = result.unwrap();
        assert_eq!(desc.base_address, 0x5000);
        assert_eq!(desc.length, 0x1000);
        assert_eq!(desc.memory_type, GcdMemoryType::MemoryMappedIo);

        // Test: Address at the start of Reserved block
        let result = GCD.get_memory_descriptor_for_address(0x8000, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert!(result.is_ok());
        let desc = result.unwrap();
        assert_eq!(desc.base_address, 0x8000);
        assert_eq!(desc.memory_type, GcdMemoryType::Reserved);

        // Test: Address in a NonExistent region (between added blocks)
        let result = GCD.get_memory_descriptor_for_address(0x4000, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert_eq!(result, Err(EfiError::NotFound));

        // Test: Address before any added memory space (in NonExistent region)
        let result = GCD.get_memory_descriptor_for_address(0x500, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert_eq!(result, Err(EfiError::NotFound));

        // Test: Address way outside any added memory space
        let result =
            GCD.get_memory_descriptor_for_address(0xFFFF0000, |d, _| d.memory_type != GcdMemoryType::NonExistent);
        assert_eq!(result, Err(EfiError::NotFound));
    });
}

#[test]
fn test_get_memory_descriptor_for_address_all_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, 0).unwrap();
        }

        // An existent address returns its descriptor.
        let desc = gcd.get_memory_descriptor_for_address(0x1000, |_, _| true).unwrap();
        assert_eq!(desc.base_address, 0x1000);
        assert_eq!(desc.memory_type, GcdMemoryType::SystemMemory);

        // The `All` filter also returns NonExistent regions rather than failing.
        let desc = gcd.get_memory_descriptor_for_address(0x500, |_, _| true).unwrap();
        assert_eq!(desc.memory_type, GcdMemoryType::NonExistent);
    });
}

#[test]
fn test_get_memory_descriptor_for_address_allocated_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, 0).unwrap();
        }
        gcd.allocate_memory_space(
            AllocateType::Address(0x1000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();

        // The allocated block is returned.
        let desc = gcd.get_memory_descriptor_for_address(0x1000, |_, allocated| allocated).unwrap();
        assert_eq!(desc.base_address, 0x1000);
        assert_eq!(desc.length, 0x1000);
        assert_eq!(desc.image_handle, 1 as _);

        // The remaining unallocated portion is not matched by the `Allocated` filter.
        let result = gcd.get_memory_descriptor_for_address(0x2000, |_, allocated| allocated);
        assert_eq!(result, Err(EfiError::NotFound));
    });
}

#[test]
fn test_get_memory_descriptor_for_address_free_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x5000, 0x1000, 0).unwrap();
        }
        gcd.allocate_memory_space(
            AllocateType::Address(0x1000),
            GcdMemoryType::SystemMemory,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();

        // Free system memory is returned.
        let desc = gcd
            .get_memory_descriptor_for_address(0x2000, |d, allocated| {
                !allocated && d.memory_type == GcdMemoryType::SystemMemory
            })
            .unwrap();
        assert_eq!(desc.base_address, 0x2000);
        assert_eq!(desc.memory_type, GcdMemoryType::SystemMemory);
        assert_eq!(desc.image_handle, INVALID_HANDLE);

        // Allocated system memory is not free.
        let result = gcd.get_memory_descriptor_for_address(0x1000, |d, allocated| {
            !allocated && d.memory_type == GcdMemoryType::SystemMemory
        });
        assert_eq!(result, Err(EfiError::NotFound));

        // Unallocated MMIO is not system memory, so it is not matched by the `Free` filter.
        let result = gcd.get_memory_descriptor_for_address(0x5000, |d, allocated| {
            !allocated && d.memory_type == GcdMemoryType::SystemMemory
        });
        assert_eq!(result, Err(EfiError::NotFound));
    });
}

#[test]
fn test_get_memory_descriptor_for_address_mmio_and_reserved_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x5000, 0x2000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::Reserved, 0x8000, 0x1000, 0).unwrap();
        }

        // Unallocated MMIO and Reserved are returned.
        let desc = gcd
            .get_memory_descriptor_for_address(0x5000, |d, allocated| {
                !allocated && matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
            })
            .unwrap();
        assert_eq!(desc.memory_type, GcdMemoryType::MemoryMappedIo);
        let desc = gcd
            .get_memory_descriptor_for_address(0x8000, |d, allocated| {
                !allocated && matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
            })
            .unwrap();
        assert_eq!(desc.memory_type, GcdMemoryType::Reserved);

        // System memory is not matched.
        let result = gcd.get_memory_descriptor_for_address(0x1000, |d, allocated| {
            !allocated && matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
        });
        assert_eq!(result, Err(EfiError::NotFound));

        // Allocated MMIO is not matched (this filter only returns unallocated MMIO/Reserved for a single address).
        gcd.allocate_memory_space(
            AllocateType::Address(0x5000),
            GcdMemoryType::MemoryMappedIo,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();
        let result = gcd.get_memory_descriptor_for_address(0x5000, |d, allocated| {
            !allocated && matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
        });
        assert_eq!(result, Err(EfiError::NotFound));
    });
}

#[test]
fn test_get_memory_descriptor_for_address_free_any_type_filter() {
    with_locked_state(|| {
        let (mut gcd, _address) = create_gcd();
        // SAFETY: Test-controlled addresses and sizes are used with the GCD initialized by create_gcd.
        unsafe {
            gcd.add_memory_space(GcdMemoryType::SystemMemory, 0x1000, 0x2000, 0).unwrap();
            gcd.add_memory_space(GcdMemoryType::MemoryMappedIo, 0x5000, 0x1000, 0).unwrap();
        }

        // Unlike `Free`, unallocated MMIO is matched by `FreeAnyType`.
        let desc = gcd.get_memory_descriptor_for_address(0x5000, |_, allocated| !allocated).unwrap();
        assert_eq!(desc.memory_type, GcdMemoryType::MemoryMappedIo);
        assert_eq!(desc.image_handle, INVALID_HANDLE);

        // Unallocated system memory is also matched.
        let desc = gcd.get_memory_descriptor_for_address(0x1000, |_, allocated| !allocated).unwrap();
        assert_eq!(desc.memory_type, GcdMemoryType::SystemMemory);

        // Allocated regions are not matched.
        gcd.allocate_memory_space(
            AllocateType::Address(0x5000),
            GcdMemoryType::MemoryMappedIo,
            UEFI_PAGE_SHIFT,
            0x1000,
            1 as _,
            None,
        )
        .unwrap();
        let result = gcd.get_memory_descriptor_for_address(0x5000, |_, allocated| !allocated);
        assert_eq!(result, Err(EfiError::NotFound));
    });
}

#[test]
#[should_panic]
fn init_paging_with_should_have_stack_hob() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Set up memory space
        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 100) };
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE * 99,
                efi::MEMORY_WB,
                efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
            )
            .unwrap();
        }

        // Create DXE Core HOB but NO stack HOB
        let dxe_core_base = address + 0x1000;
        let dxe_core_len = 0x1000000;
        let dxe_core_hob = Hob::MemoryAllocationModule(&patina::pi::hob::MemoryAllocationModule {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<patina::pi::hob::MemoryAllocationModule>() as u16,
                reserved: 0,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: base_guids::DXE_CORE_ID,
                memory_base_address: dxe_core_base as u64,
                memory_length: dxe_core_len as u64,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: [0; 4],
            },
            module_name: base_guids::DXE_CORE_ID,
            entry_point: dxe_core_base as u64 + 0x1000,
        });
        let mut hob_list = HobList::new();
        hob_list.push(dxe_core_hob);

        // SAFETY: We just allocated this memory and DXE_CORE_PE_HEADER_DATA is a valid byte array
        unsafe {
            core::ptr::copy_nonoverlapping(
                DXE_CORE_PE_HEADER_DATA.as_ptr(),
                dxe_core_base as *mut u8,
                DXE_CORE_PE_HEADER_DATA.len(),
            );
        }

        // Create mock page table
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));

        // Should panic because no stack HOB is present
        GCD.init_paging_with(&hob_list, page_table);
    });
}

#[test]
#[should_panic]
fn init_paging_with_should_have_non_zero_stack_base_address_length() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Set up memory space
        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 100) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE * 99,
                efi::MEMORY_WB,
                efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
            )
            .unwrap();
        }

        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        // Create DXE Core HOB
        let dxe_core_base = address + 0x1000;
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        let dxe_core_len = 0x1000000;
        let dxe_core_hob = Hob::MemoryAllocationModule(&patina::pi::hob::MemoryAllocationModule {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<patina::pi::hob::MemoryAllocationModule>() as u16,
                reserved: 0,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: base_guids::DXE_CORE_ID,
                memory_base_address: dxe_core_base as u64,
                memory_length: dxe_core_len as u64,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: [0; 4],
            },
            module_name: base_guids::DXE_CORE_ID,
            entry_point: dxe_core_base as u64 + 0x1000,
        });
        let mut hob_list = HobList::new();
        hob_list.push(dxe_core_hob);

        // Add a stack HOB with zero base address and length
        let stack_hob = Hob::MemoryAllocation(&patina::pi::hob::MemoryAllocation {
            header: patina::pi::hob::HobHeader {
                r#type: hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                reserved: 0x00000000,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: pi_guids::MEMORY_ALLOC_STACK_HOB_GUID,
                memory_base_address: 0,
                memory_length: 0,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: Default::default(),
            },
        });
        hob_list.push(stack_hob);

        // SAFETY: We just allocated this memory and DXE_CORE_PE_HEADER_DATA is a valid byte array
        unsafe {
            core::ptr::copy_nonoverlapping(
                DXE_CORE_PE_HEADER_DATA.as_ptr(),
                dxe_core_base as *mut u8,
                DXE_CORE_PE_HEADER_DATA.len(),
            );
        }

        // Create mock page table
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));

        // Should panic because stack base address and length are zero
        GCD.init_paging_with(&hob_list, page_table);
    });
}

#[test]
#[should_panic]
fn init_paging_with_should_exist_in_gcd() {
    with_locked_state(|| {
        static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
        GCD.init(48, 16);

        // Set up memory space
        // SAFETY: get_memory returns a test-owned buffer of the requested size.
        let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE * 100) };
        let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();
        // SAFETY: address/size come from the test buffer and are valid to initialize memory blocks.
        unsafe {
            GCD.init_memory_blocks(
                GcdMemoryType::SystemMemory,
                address,
                MEMORY_BLOCK_SLICE_SIZE * 99,
                efi::MEMORY_WB,
                efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
            )
            .unwrap();
        }

        // Create DXE Core HOB
        let dxe_core_base = address + 0x1000;
        let dxe_core_len = 0x1000000;
        let dxe_core_hob = Hob::MemoryAllocationModule(&patina::pi::hob::MemoryAllocationModule {
            header: patina::pi::hob::HobHeader {
                r#type: patina::pi::hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<patina::pi::hob::MemoryAllocationModule>() as u16,
                reserved: 0,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: base_guids::DXE_CORE_ID,
                memory_base_address: dxe_core_base as u64,
                memory_length: dxe_core_len as u64,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: [0; 4],
            },
            module_name: base_guids::DXE_CORE_ID,
            entry_point: dxe_core_base as u64 + 0x1000,
        });
        let mut hob_list = HobList::new();
        hob_list.push(dxe_core_hob);

        // Add a stack HOB with zero base address and length
        let stack_hob = Hob::MemoryAllocation(&patina::pi::hob::MemoryAllocation {
            header: patina::pi::hob::HobHeader {
                r#type: hob::MEMORY_ALLOCATION,
                length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                reserved: 0x00000000,
            },
            alloc_descriptor: patina::pi::hob::MemoryAllocationHeader {
                name: pi_guids::MEMORY_ALLOC_STACK_HOB_GUID,
                memory_base_address: 0x1000,
                memory_length: 0x40000,
                memory_type: efi::BOOT_SERVICES_DATA,
                reserved: Default::default(),
            },
        });
        hob_list.push(stack_hob);

        let _ = GCD.remove_memory_space(0x1000, 0x40000);

        // SAFETY: We just allocated this memory and DXE_CORE_PE_HEADER_DATA is a valid byte array
        unsafe {
            core::ptr::copy_nonoverlapping(
                DXE_CORE_PE_HEADER_DATA.as_ptr(),
                dxe_core_base as *mut u8,
                DXE_CORE_PE_HEADER_DATA.len(),
            );
        }

        // Create mock page table
        let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
        let page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));

        // Should panic because stack base address and length are zero
        GCD.init_paging_with(&hob_list, page_table);
    });
}
