//! Stub CPU initialization implementation - For doc tests
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::cpu::Cpu;
use patina::{
    component::service::IntoService,
    error::EfiError,
    pi::protocols::cpu_arch::{CpuFlushType, CpuInitType},
};
use r_efi::efi;

/// Struct to implement Null Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when not compiling for x86_64 or AArch64 UEFI architectures.
#[derive(Default, Copy, Clone, IntoService)]
#[service(dyn Cpu)]
pub struct EfiCpuStub;

impl EfiCpuStub {
    /// Creates a new instance of the null implementation of the CPU.
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
    /// Causes the CPU to enter a low power state until the next interrupt.
    // Trivial emulation of hardware access, so no coverage.
    #[coverage(off)]
    pub fn sleep() {}
}

impl Cpu for EfiCpuStub {
    fn flush_data_cache(
        &self,
        _start: efi::PhysicalAddress,
        _length: u64,
        _flush_type: CpuFlushType,
    ) -> Result<(), EfiError> {
        Ok(())
    }

    fn init(&self, _init_type: CpuInitType) -> Result<(), EfiError> {
        Ok(())
    }

    fn get_timer_value(&self, _timer_index: u32) -> Result<(u64, u64), EfiError> {
        Ok((0, 0))
    }
}
