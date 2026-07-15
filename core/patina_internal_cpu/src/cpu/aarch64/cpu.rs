//! AArch64 CPU initialization implementation
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::error::EfiError;

/// Struct to implement AArch64 Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when compiling for the AArch64 architecture.
#[derive(Default)]
pub struct EfiCpuAarch64;

#[allow(dead_code)]
impl EfiCpuAarch64 {
    /// This function initializes the CPU for the AArch64 architecture.
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_initialize() {
        let mut cpu_init = EfiCpuAarch64;
        assert!(cpu_init.initialize().is_ok());
    }
}
