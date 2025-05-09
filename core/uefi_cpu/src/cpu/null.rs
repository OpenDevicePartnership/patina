//! Null CPU initialization implementation - For doc tests
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use crate::cpu::Cpu;
use mu_pi::protocols::cpu_arch::{CpuFlushType, CpuInitType};
use r_efi::efi;
use uefi_sdk::{component::service::IntoService, error::EfiError};

/// Struct to implement Null Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when not compiling for x86_64 or AArch64 UEFI architectures.
#[derive(Default, Copy, Clone, IntoService)]
#[service(dyn Cpu)]
pub struct EfiCpuNull;

impl EfiCpuNull {
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
}

impl Cpu for EfiCpuNull {
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
