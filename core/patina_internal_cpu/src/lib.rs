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
#![feature(abi_x86_interrupt)]
#![feature(coverage_attribute)]
extern crate alloc;

pub mod cpu;
pub mod interrupts;
pub mod paging;
