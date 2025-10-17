//! X64 CPU initialization implementation
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#[cfg(not(test))]
use super::gdt;
use crate::{cpu::Cpu, interrupts};
#[cfg(not(test))]
use core::arch::asm;
use patina::pi::protocols::cpu_arch::{CpuFlushType, CpuInitType};
use patina::{component::service::IntoService, error::EfiError};
use r_efi::efi;

/// Struct to implement X64 Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when compiling for the x86_64 architecture.
#[derive(IntoService)]
#[service(dyn Cpu)]
pub struct EfiCpuX64 {
    timer_period: u64,
}

impl EfiCpuX64 {
    /// Creates a new instance of the x86_64 implementation of the CPU trait.
    pub fn new() -> Self {
        let mut x64_efi_init = EfiCpuX64 { timer_period: 0 };
        x64_efi_init.calculate_timer_period();
        x64_efi_init
    }

    /// This function initializes the CPU for the x86_64 architecture.
    pub fn initialize(&mut self) -> Result<(), EfiError> {
        // Initialize floating point units
        self.initialize_fpu();

        // disable interrupts
        interrupts::disable_interrupts();

        // Initialize GDT
        self.initialize_gdt();

        interrupts::enable_interrupts();

        Ok(())
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

    fn initialize_fpu(&self) {
        #[cfg(all(not(test), target_arch = "x86_64"))]
        // Safety: This assembly writes only hard coded values to CR4 register, and MMX and FPU control words. No
        // inputs are used that could violate memory safety.
        unsafe {
            // sdm vol. 1, x87 FPU Control Word configuration
            static FPU_CONTROL_WORD: u16 = 0x037F;

            // sdm vol. 1, MMX Control Status Register configuration
            static MMX_CONTROL_WORD: u32 = 0x1F80;
            asm!(
                "finit",
                "fldcw [{FPU_CONTROL_WORD}]",

                // Set OSFXSR (bit 9) in CR4 to enable SSE instructions
                "mov {temp}, cr4",
                "or {temp}, {BIT9}",
                "mov cr4, {temp}",

                "ldmxcsr [{MMX_CONTROL_WORD}]",
                temp = out(reg) _,
                FPU_CONTROL_WORD = sym FPU_CONTROL_WORD,
                MMX_CONTROL_WORD = sym MMX_CONTROL_WORD,
                BIT9 = const patina::bit!(9),
                options(nostack, preserves_flags)
            );
        }
    }
}

/// The x86_64 implementation of EFI Cpu Init.
impl Cpu for EfiCpuX64 {
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

impl Default for EfiCpuX64 {
    fn default() -> Self {
        EfiCpuX64::new()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {

    use super::*;

    #[test]
    fn test_initialize() {
        let mut x64_cpu_init = EfiCpuX64 { timer_period: 0 };
        x64_cpu_init.calculate_timer_period();

        assert_eq!(x64_cpu_init.initialize(), Ok(()));
    }

    #[test]
    fn test_flush_data_cache() {
        let mut x64_cpu_init = EfiCpuX64 { timer_period: 0 };
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
        let mut x64_cpu_init = EfiCpuX64 { timer_period: 0 };
        x64_cpu_init.calculate_timer_period();

        assert_eq!(x64_cpu_init.initialize(), Ok(()));
        assert_eq!(x64_cpu_init.get_timer_value(1), Err(EfiError::InvalidParameter));
        assert_eq!(x64_cpu_init.get_timer_value(0), Ok((0, 0)));
    }
}
