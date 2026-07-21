//! Software Development Kit (SDK) for Patina
//!
//! This crate implements the core SDK for Patina and is only part of the Patina
//! solution. For general knowledge on Patina, refer to the [Patina book](https://opendevicepartnership.github.io/patina/).
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![cfg_attr(any(test, feature = "alloc"), feature(allocator_api))]
#![cfg_attr(coverage, feature(coverage_attribute))]

#[cfg(any(test, feature = "alloc"))]
extern crate alloc;

// The base module gets republished from the root to flatten dependencies for common structures.
// Additionally, certain types will also be directly exposed from the root for convenience.
mod base;
pub use base::*;
pub use guid::{BinaryGuid, Guid, GuidError, OwnedGuid};
#[doc(inline)]
pub use patina_macro::devpath;
pub use string::{Char8Array, Char8Str, Char16Array, Char16Str, StringError};

#[cfg(any(test, feature = "alloc"))]
pub use base::string::{Char8String, Char16String};

pub mod arch;
#[cfg(feature = "alloc")]
pub mod component;
pub mod debug;
pub mod management_mode;
#[cfg(feature = "alloc")]
pub mod performance;
pub mod peripheral;
pub mod pi;
pub mod standard;
pub mod uefi;

/// Re-export of the [`safe-mmio`](https://crates.io/crates/safe-mmio) crate.
///
/// Consumers should use `patina::mmio` instead of depending on `safe-mmio` directly.
pub mod mmio {
    pub use safe_mmio::*;
}
