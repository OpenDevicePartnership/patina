//! UEFI Global Coherency Domain Support
//!
//! This library provides an implementation of the PI spec GCD.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
//! ## Examples and Usage
//!
//!```
//! # extern crate std;
//! # extern crate alloc;
//! use mu_pi::dxe_services;
//! use uefi_gcd::gcd::{AllocateType, SpinLockedGcd};
//! # const MEMORY_BLOCK_SLICE_SIZE:usize = 4096*4096;
//! # unsafe fn get_memory(size: usize) -> &'static mut [u8] {
//! #   let addr = alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, 4096).unwrap());
//! #   core::slice::from_raw_parts_mut(addr, size)
//! # }
//!
//! static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
//!
//! GCD.init(48, 16);
//!
//! # let mem = unsafe { get_memory(MEMORY_BLOCK_SLICE_SIZE) };
//! # let base_address = mem.as_ptr() as usize;
//! /* base_address is *mut u8 pointing to memory space to add */
//! /* memory_space_size is the size of the memory space to add */
//! unsafe {
//!   GCD.add_memory_space(dxe_services::GcdMemoryType::SystemMemory, base_address, MEMORY_BLOCK_SLICE_SIZE, 0).unwrap();
//! }
//!
//! let allocation_addr = GCD.allocate_memory_space(
//!   AllocateType::BottomUp(None),               //allocate_type
//!   dxe_services::GcdMemoryType::SystemMemory,  //memory_type
//!   0,                                          //alignment
//!   0x1000,                                     //size
//!   1 as _,                                     //Image Handle (fake)
//!   None                                        //Device Handle
//! ).unwrap();
//!
//! assert!(base_address <= (allocation_addr as usize));
//! assert!((allocation_addr as usize) < base_address + MEMORY_BLOCK_SLICE_SIZE);
//!
//!```
//!

#![no_std]
#![feature(get_many_mut)]
#![feature(is_sorted)]
extern crate alloc;

pub mod gcd;
pub mod io_block;
pub mod memory_block;

#[macro_export]
macro_rules! ensure {
    ($condition:expr, $err:expr) => {{
        if !($condition) {
            error!($err);
        }
    }};
}

#[macro_export]
macro_rules! error {
    ($err:expr) => {{
        return Err($err.into()).into();
    }};
}
