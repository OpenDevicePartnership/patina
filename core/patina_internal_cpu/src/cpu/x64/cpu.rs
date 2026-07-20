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
#[cfg(not(test))]
use core::arch::asm;
use patina::arch as interrupts;
use patina::error::EfiError;

/// Struct to implement X64 Cpu Init.
///
/// This struct cannot be used directly. It replaces the `EfiCpu` struct when compiling for the x86_64 architecture.
#[derive(Default)]
pub struct EfiCpuX64;

#[allow(dead_code)]
impl EfiCpuX64 {
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

    fn initialize_gdt(&self) {
        #[cfg(not(test))]
        gdt::init();
    }

    #[cfg_attr(coverage, coverage(off))]
    fn initialize_fpu(&self) {
        #[cfg(not(test))]
        // SAFETY: This assembly writes only hard coded values to CR4 register, and MMX and FPU control words. No
        // inputs are used that could violate memory safety.
        unsafe {
            // sdm vol. 1, x87 FPU Control Word configuration
            static FPU_CONTROL_WORD: u16 = 0x037F;

            // sdm vol. 1, MMX Control Status Register configuration
            static MMX_CONTROL_WORD: u32 = 0x1F80;
            let fpu_cw = &raw const FPU_CONTROL_WORD;
            let mmx_cw = &raw const MMX_CONTROL_WORD;
            asm!(
                "finit",
                "fldcw [{fpu_cw}]",

                // Set OSFXSR (bit 9) in CR4 to enable SSE instructions
                "mov {temp}, cr4",
                "or {temp}, {BIT9}",
                "mov cr4, {temp}",

                "ldmxcsr [{mmx_cw}]",
                temp = out(reg) _,
                fpu_cw = in(reg) fpu_cw,
                mmx_cw = in(reg) mmx_cw,
                BIT9 = const patina::bit!(9),
                options(nostack)
            );
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {

    use super::*;

    #[test]
    fn test_initialize() {
        let mut x64_cpu_init = EfiCpuX64;

        assert_eq!(x64_cpu_init.initialize(), Ok(()));
    }
}
