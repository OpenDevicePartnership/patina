//! UEFI Software Development Kit (SDK) for Rust
//!
//! This crate implements common functionality for building and executing UEFI
//! binaries in Rust.
//!
//! ## Features
//!
//! - `core`: Exposes additional items in the [component] module necessary to
//!   manage and execute components and their dependencies.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![cfg_attr(any(test, feature = "alloc"), feature(allocator_api))]
#![allow(static_mut_refs)]
#![feature(coverage_attribute)]

extern crate alloc;

#[macro_use]
pub mod macros;

pub mod base;
pub mod boot_services;
pub mod component;
pub mod driver_binding;
pub mod efi_types;
pub mod error;
pub mod guids;
pub mod log;
pub mod performance;
pub mod runtime_services;
pub mod serial;
#[coverage(off)]
pub mod test;
pub mod tpl_mutex;
pub mod uefi_protocol;
