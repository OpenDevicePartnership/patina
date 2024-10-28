//! CPU Initialization Trait Implementations
//!
//! This crate provides default implementations for the [uefi_core::interface::CpuInitializer] trait.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![feature(abi_x86_interrupt)]

mod null;
pub use null::NullCpuInitializer;

uefi_core::if_x64! {
    mod x64;
    pub use x64::cpu::X64CpuInitializer as X64CpuInitializer;
}
