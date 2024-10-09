//! UEFI Rust Allocator Lib
//!
//! Provides an allocator implementation suitable for use in tracking UEFI memory allocations.
//!
//! The foundation of the implementation is
//! [`FixedSizeBlockAllocator`](`fixed_size_block_allocator::FixedSizeBlockAllocator`), which provides a fixed-sized block
//! allocator backed by a linked list allocator, the design of which is based on
//! <https://os.phil-opp.com/allocator-designs/#fixed-size-block-allocator>.
//!
//! A spin-locked version of the implementation is available as
//! [`SpinLockedFixedSizeBlockAllocator`](`fixed_size_block_allocator::SpinLockedFixedSizeBlockAllocator`) which is
//! suitable for use as a global allocator.
//!
//! In addition, [`UefiAllocator`](`uefi_allocator::UefiAllocator`) provides an implementation on top of
//! [`SpinLockedFixedSizeBlockAllocator`](`fixed_size_block_allocator::SpinLockedFixedSizeBlockAllocator`) which
//! implements UEFI's pool semantics and adds support for assigning a UEFI memory type to a particular allocator.
//!
//! ## Examples and Usage
//!
//! Declaring a set of UEFI allocators as global static allocators and setting one of them as the system allocator:
//!
//! ```no_run
//! # use r_efi::efi;
//! use uefi_gcd::gcd::SpinLockedGcd;
//! use uefi_allocator::uefi_allocator::UefiAllocator;
//! static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
//! /* Initialize GCD */
//! //EfiBootServicesCode
//! pub static EFI_BOOT_SERVICES_CODE_ALLOCATOR: UefiAllocator = UefiAllocator::new(&GCD, efi::BOOT_SERVICES_CODE, 1 as _, None);
//! //EfiBootServicesData - (use as global allocator)
//! #[global_allocator]
//! pub static EFI_BOOT_SERVICES_DATA_ALLOCATOR: UefiAllocator = UefiAllocator::new(&GCD, efi::BOOT_SERVICES_DATA, 1 as _, None);
//! ```
//!
//! Allocating memory in a particular allocator using Box:
//! ```
//! #![feature(allocator_api)]
//! # use core::alloc::Layout;
//! # use core::ffi::c_void;
//! # use r_efi::efi;
//! # use std::alloc::{GlobalAlloc, System};
//! # use mu_pi::dxe_services;
//!
//! use uefi_allocator::uefi_allocator::UefiAllocator;
//! use uefi_gcd::gcd::SpinLockedGcd;
//! # fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
//! #   let layout = Layout::from_size_align(size, 0x1000).unwrap();
//! #   let base = unsafe { System.alloc(layout) as u64 };
//! #   unsafe {
//! #     gcd.add_memory_space(
//! #       dxe_services::GcdMemoryType::SystemMemory,
//! #       base as usize,
//! #       size,
//! #       0).unwrap();
//! #   }
//! #   base
//! # }
//!
//! static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
//! GCD.init(48,16); //hard-coded processor address size.
//! let base = init_gcd(&GCD, 0x400000);
//!
//! pub static EFI_BOOT_SERVICES_DATA_ALLOCATOR: UefiAllocator = UefiAllocator::new(&GCD, efi::BOOT_SERVICES_DATA, 1 as _, None);
//! pub static EFI_RUNTIME_SERVICES_DATA_ALLOCATOR: UefiAllocator = UefiAllocator::new(&GCD, efi::RUNTIME_SERVICES_DATA, 1 as _, None);
//!
//! //Allocate a box in Boot Services Data
//! let boot_box = Box::new_in(5, &EFI_BOOT_SERVICES_DATA_ALLOCATOR);
//!
//! //Allocate a box in Runtime Services Data
//! let runtime_box = Box::new_in(10, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR);
//!
//! ```
//!
//! Using UEFI allocator pool semantics:
//! ```
//! # use core::alloc::Layout;
//! # use core::ffi::c_void;
//! # use std::alloc::{GlobalAlloc, System};
//! # use mu_pi::dxe_services;
//!
//! use uefi_allocator::uefi_allocator::UefiAllocator;
//! use uefi_gcd::gcd::SpinLockedGcd;
//! # fn init_gcd(gcd: &SpinLockedGcd, size: usize) -> u64 {
//! #   let layout = Layout::from_size_align(size, 0x1000).unwrap();
//! #   let base = unsafe { System.alloc(layout) as u64 };
//! #   unsafe {
//! #     gcd.add_memory_space(
//! #       dxe_services::GcdMemoryType::SystemMemory,
//! #       base as usize,
//! #       size,
//! #       0).unwrap();
//! #   }
//! #   base
//! # }
//!
//! static GCD: SpinLockedGcd = SpinLockedGcd::new(None);
//! GCD.init(48,16); //hard-coded processor address size.
//! let base = init_gcd(&GCD, 0x400000);
//!
//! let ua = UefiAllocator::new(&GCD, r_efi::efi::BOOT_SERVICES_DATA, 1 as _, None);
//! unsafe {
//!   let mut buffer: *mut c_void = core::ptr::null_mut();
//!   assert!(ua.allocate_pool(0x1000, core::ptr::addr_of_mut!(buffer)) == r_efi::efi::Status::SUCCESS);
//!   assert!(buffer as u64 > base);
//!   assert!((buffer as u64) < base + 0x400000);
//!   assert!(ua.free_pool(buffer) == r_efi::efi::Status::SUCCESS);
//! }
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#![no_std]
#![feature(const_mut_refs)]
#![feature(allocator_api)]
#![feature(slice_ptr_get)]
#![feature(const_trait_impl)]

pub mod fixed_size_block_allocator;
pub mod uefi_allocator;

pub use uefi_gcd::gcd::AllocateType as AllocationStrategy;
