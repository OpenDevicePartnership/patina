//! X64 CPU initialization implementation
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#[cfg(not(test))]
use super::gdt;
use crate::{cpu::EfiCpuInit, interrupts};
#[cfg(not(test))]
use core::arch::asm;
use mu_pi::protocols::cpu_arch::{CpuFlushType, CpuInitType};
use r_efi::efi;
use uefi_sdk::error::EfiError;

/// Struct to implement X64 Cpu Init.
pub struct EfiCpuInitX64 {
    timer_period: u64,
}

impl EfiCpuInitX64 {
    pub fn new() -> Self {
        let mut x64_efi_init = EfiCpuInitX64 { timer_period: 0 };
        x64_efi_init.calculate_timer_period();
        x64_efi_init
    }

    fn calculate_timer_period(&mut self) {
        // Read time stamp counter before and after delay of 100 microseconds
        let begin_value = self.asm_read_tsc(); // Assuming asm_read_tsc is defined
        self.microsecond_delay(100); // Assuming microsecond_delay is defined
        let end_value = self.asm_read_tsc();

        // Calculate the actual frequency
        if end_value != begin_value {
            self.timer_period = (1000 * 1000 * 1000 * 100) / (end_value - begin_value);
        }
    }

    fn initialize_gdt(&self) {
        #[cfg(all(not(test), target_arch = "x86_64"))]
        gdt::init();
    }

    // X64 related asm functions
    fn asm_wbinvd(&self) {
        #[cfg(all(not(test), target_arch = "x86_64"))]
        {
            unsafe {
                asm!("wbinvd");
            }
        }
    }

    fn asm_invd(&self) {
        #[cfg(all(not(test), target_arch = "x86_64"))]
        {
            unsafe {
                asm!("invd");
            }
        }
    }

    fn asm_read_tsc(&self) -> u64 {
        // unimplemented!();
        0
    }

    fn microsecond_delay(&self, _microseconds: u64) {
        // unimplemented!();
    }
}

/// The x86_64 implementation of EFI Cpu Init.
impl EfiCpuInit for EfiCpuInitX64 {
    /// This function initializes the CPU for the x86_64 architecture.
    fn initialize(&mut self) -> Result<(), EfiError> {
        // Initialize floating point units

        // disable interrupts
        interrupts::disable_interrupts();

        // Initialize GDT
        self.initialize_gdt();

        interrupts::enable_interrupts();

        Ok(())
    }

    fn flush_data_cache(
        &self,
        _start: efi::PhysicalAddress,
        _length: u64,
        flush_type: CpuFlushType,
    ) -> Result<(), EfiError> {
        match flush_type {
            CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => {
                self.asm_wbinvd();
                Ok(())
            }
            CpuFlushType::EFiCpuFlushTypeInvalidate => {
                self.asm_invd();
                Ok(())
            }
            _ => Err(EfiError::Unsupported),
        }
    }

    fn init(&self, _init_type: CpuInitType) -> Result<(), EfiError> {
        unimplemented!()
    }

    fn get_timer_value(&self, timer_index: u32) -> Result<(u64, u64), EfiError> {
        if timer_index != 0 {
            return Err(EfiError::InvalidParameter);
        }

        let timer_value = self.asm_read_tsc(); // Assuming asm_read_tsc is defined elsewhere

        Ok((timer_value, self.timer_period))
    }
}

impl Default for EfiCpuInitX64 {
    fn default() -> Self {
        EfiCpuInitX64::new()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_initialize() {
        let mut x64_cpu_init = EfiCpuInitX64 { timer_period: 0 };
        x64_cpu_init.calculate_timer_period();

        assert_eq!(x64_cpu_init.initialize(), Ok(()));
    }

    #[test]
    fn test_flush_data_cache() {
        let mut x64_cpu_init = EfiCpuInitX64 { timer_period: 0 };
        x64_cpu_init.calculate_timer_period();

        assert_eq!(x64_cpu_init.initialize(), Ok(()));
        let start: efi::PhysicalAddress = 0;
        let length: u64 = 0;
        let flush_type: CpuFlushType = CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate;
        assert_eq!(x64_cpu_init.flush_data_cache(start, length, flush_type), Ok(()));

        let start: efi::PhysicalAddress = 0;
        let length: u64 = 0;
        let flush_type: CpuFlushType = CpuFlushType::EFiCpuFlushTypeInvalidate;
        assert_eq!(x64_cpu_init.flush_data_cache(start, length, flush_type), Ok(()));

        let start: efi::PhysicalAddress = 0;
        let length: u64 = 0;
        let flush_type: CpuFlushType = CpuFlushType::EfiCpuFlushTypeWriteBack;
        assert_eq!(x64_cpu_init.flush_data_cache(start, length, flush_type), Err(EfiError::Unsupported));
    }

    #[test]
    fn test_get_timer_value() {
        let mut x64_cpu_init = EfiCpuInitX64 { timer_period: 0 };
        x64_cpu_init.calculate_timer_period();

        assert_eq!(x64_cpu_init.initialize(), Ok(()));
        assert_eq!(x64_cpu_init.get_timer_value(1), Err(EfiError::InvalidParameter));
        assert_eq!(x64_cpu_init.get_timer_value(0), Ok((0, 0)));
    }
}
