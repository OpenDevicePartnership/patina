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
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod cpu;
pub mod interrupts;
pub mod paging;
pub mod save_state;
