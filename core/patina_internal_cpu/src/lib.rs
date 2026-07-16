//! UEFI CPU Crate
//!
//! This crate provides implementation for the Cpu.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![cfg_attr(coverage, feature(coverage_attribute))]

#[cfg(target_arch = "x86_64")]
pub mod gdt;
pub mod interrupts;
pub mod paging;
