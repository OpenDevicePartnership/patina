//! CPU Initialization Trait Implementations
//!
//! This crate provides default implementations for the [uefi_core::CpuInitializer] trait.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![feature(abi_x86_interrupt)]

#[macro_use]
extern crate alloc;

pub trait CpuInitializer {
    fn initialize(&mut self);
    // return a list of protocol instance - GUID pairs that the CPU supports
    fn post_init(&mut self, boot_services: *mut r_efi::efi::BootServices);
}

uefi_core::if_x64! {
    mod x64;
    pub use x64::cpu::X64CpuInitializer as X64CpuInitializer;
}

uefi_core::if_aarch64! {
    mod aarch64;
    pub use aarch64::cpu::AArch64CpuInitializer as AArch64CpuInitializer;
}