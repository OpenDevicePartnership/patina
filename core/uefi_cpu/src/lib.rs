//! UEFI CPU Crate
//!
//! This crate provides implementation for the Cpu.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![feature(abi_x86_interrupt)]
extern crate alloc;

pub mod cpu;
pub mod interrupts;
pub mod paging;
