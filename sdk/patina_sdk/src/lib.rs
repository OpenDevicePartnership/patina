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
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![cfg_attr(feature = "alloc", feature(allocator_api))]
#![allow(static_mut_refs)]

extern crate alloc;

#[macro_use]
pub mod macros;

pub mod base;
pub mod component;
pub mod efi_types;
pub mod error;
pub mod guid;
pub mod log;
pub mod serial;

pub mod boot_services;
pub mod driver_binding;
pub mod runtime_services;
pub mod tpl_mutex;
pub mod uefi_protocol;

pub mod test;
