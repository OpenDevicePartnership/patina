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
