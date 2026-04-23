//! X64 CPU module
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
mod cpu;
pub(crate) mod gdt;

#[allow(unused)]
pub use cpu::EfiCpuX64;
