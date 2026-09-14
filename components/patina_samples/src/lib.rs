//! Sample Patina Components
//!
//! This crate provides example component implementations demonstrating various
//! Patina component patterns and usage models.
//!
//! ## Examples
//!
//! - [`component::hello_world::HelloStruct`]: Demonstrates a struct-based component with default entry point
//! - [`component::hello_world::GreetingsEnum`]: Demonstrates an enum-based component with custom entry point
//! - [`smbios_platform`]: Demonstrates SMBIOS platform configuration and record creation
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![deny(missing_docs)]
#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, coverage(off))] // Disable all coverage instrumentation for sample code
extern crate alloc;

pub mod component;
pub mod smbios_platform;
