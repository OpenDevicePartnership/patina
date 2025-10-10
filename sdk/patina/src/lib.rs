//! Software Development Kit (SDK) for Patina
//!
//! This crate implements the core SDK for Patina and is only part of the Patina
//! solution. For general knowledge on Patina, refer to the [Patina book](https://opendevicepartnership.github.io/patina/).
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

pub use base::guid::{Guid, GuidError, OwnedGuid};

/// Common GUID constants
pub mod guid_constants {
    pub use super::base::guid::OwnedGuid;
    /// Zero GUID constant (00000000-0000-0000-0000-000000000000)
    pub const ZERO_GUID: OwnedGuid = OwnedGuid::ZERO;
}

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
