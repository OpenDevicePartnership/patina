//! Stub CPU initialization implementation - For doc tests
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::error::EfiError;

/// Struct to implement Null Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when not compiling for x86_64 or AArch64 UEFI architectures.
#[derive(Default, Copy, Clone)]
pub struct EfiCpuStub;

impl EfiCpuStub {
    /// Creates a new instance of the null implementation of the CPU.
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
}
