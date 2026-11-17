//! SBOM ACPI table publication.
//!
//! Copyright (C) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0
#![no_std]
#![deny(missing_docs)]

extern crate alloc;

#[cfg(feature = "unstable-sbom")]
pub mod component;

#[cfg(feature = "unstable-sbom")]
pub mod hob;
