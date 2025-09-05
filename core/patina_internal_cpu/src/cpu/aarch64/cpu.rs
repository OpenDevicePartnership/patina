//! AArch64 CPU initialization implementation
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::cpu::Cpu;
#[cfg(all(not(test), target_arch = "aarch64"))]
use core::arch::asm;
use mu_pi::protocols::cpu_arch::{CpuFlushType, CpuInitType};
use patina_sdk::{component::service::IntoService, error::EfiError};
use r_efi::efi;

/// Struct to implement AArch64 Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when compiling for the AArch64 architecture.
#[derive(Default, IntoService)]
#[service(dyn Cpu)]
pub struct EfiCpuAarch64;

impl EfiCpuAarch64 {
    /// This function initializes the CPU for the AArch64 architecture.
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        Ok(())
    }
    // AArch64 related cache functions
    fn cache_range_operation(&self, _start: efi::PhysicalAddress, _length: u64, _op: CpuFlushType) {
        let cacheline_alignment = self.data_cache_line_len() - 1;
        let mut aligned_addr = _start - (_start & cacheline_alignment);
        let end_addr = _start + _length;

        loop {
            match _op {
                CpuFlushType::EfiCpuFlushTypeWriteBack => self.clean_data_entry_by_mva(aligned_addr),
                CpuFlushType::EFiCpuFlushTypeInvalidate => self.invalidate_data_cache_entry_by_mva(aligned_addr),
                CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => {
                    self.clean_and_invalidate_data_entry_by_mva(aligned_addr)
                }
            }

            aligned_addr += cacheline_alignment;
            if aligned_addr >= end_addr {
                break;
            }
        }

        #[cfg(all(not(test), target_arch = "aarch64"))]
        {
            // we have a data barrier after all cache lines have had the operation performed on them as an optimization
            unsafe {
                asm!("dsb sy", options(nostack));
            }
        }
    }

    fn clean_data_entry_by_mva(&self, _mva: efi::PhysicalAddress) {
        #[cfg(all(not(test), target_arch = "aarch64"))]
        {
            unsafe {
                asm!("dc cvac, {}", in(reg) _mva, options(nostack, preserves_flags));
            }
        }
    }

    fn invalidate_data_cache_entry_by_mva(&self, _mva: efi::PhysicalAddress) {
        #[cfg(all(not(test), target_arch = "aarch64"))]
        {
            unsafe {
                asm!("dc ivac, {}", in(reg) _mva, options(nostack, preserves_flags));
            }
        }
    }

    fn clean_and_invalidate_data_entry_by_mva(&self, _mva: efi::PhysicalAddress) {
        #[cfg(all(not(test), target_arch = "aarch64"))]
        {
            unsafe {
                asm!("dc civac, {}", in(reg) _mva, options(nostack, preserves_flags));
            }
        }
    }

    fn data_cache_line_len(&self) -> u64 {
        #[cfg(all(not(test), target_arch = "aarch64"))]
        {
            let ctr_el0 = unsafe {
                let ctr_el0: u64;
                asm!("mrs {}, ctr_el0", out(reg) ctr_el0);
                ctr_el0
            };
            return 4 << ((ctr_el0 >> 16) & 0xf);
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            // For all other cases, return 64 bytes
            64_u64
        }
    }
}

impl Cpu for EfiCpuAarch64 {
    fn flush_data_cache(
        &self,
        _start: efi::PhysicalAddress,
        _length: u64,
        flush_type: CpuFlushType,
    ) -> Result<(), EfiError> {
        self.cache_range_operation(_start, _length, flush_type);
        Ok(())
    }

    fn init(&self, _init_type: CpuInitType) -> Result<(), EfiError> {
        unimplemented!("init not implemented for AArch64")
    }

    fn get_timer_value(&self, _timer_index: u32) -> Result<(u64, u64), EfiError> {
        Err(EfiError::Unsupported)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize() {
        let mut cpu_init = EfiCpuAarch64;
        assert!(cpu_init.initialize().is_ok());
    }

    #[test]
    fn test_flush_data_cache() {
        let cpu_init = EfiCpuAarch64;

        let start: efi::PhysicalAddress = 0;
        let length: u64 = 0;
        let flush_type: CpuFlushType = CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate;
        assert_eq!(cpu_init.flush_data_cache(start, length, flush_type), Ok(()));

        let start: efi::PhysicalAddress = 0;
        let length: u64 = 0;
        let flush_type: CpuFlushType = CpuFlushType::EFiCpuFlushTypeInvalidate;
        assert_eq!(cpu_init.flush_data_cache(start, length, flush_type), Ok(()));

        let start: efi::PhysicalAddress = 0;
        let length: u64 = 0;
        let flush_type: CpuFlushType = CpuFlushType::EfiCpuFlushTypeWriteBack;
        assert_eq!(cpu_init.flush_data_cache(start, length, flush_type), Ok(()));
    }

    #[test]
    fn test_get_timer_value() {
        let cpu_init = EfiCpuAarch64;

        assert_eq!(cpu_init.get_timer_value(1), Err(EfiError::Unsupported));
        assert_eq!(cpu_init.get_timer_value(0), Err(EfiError::Unsupported));
    }
}
