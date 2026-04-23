//! Arch Specific abstractions for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(all(not(test), target_arch = "aarch64"))]
pub mod aarch64;

#[cfg(all(not(test), target_arch = "x86_64"))]
pub mod x64;
