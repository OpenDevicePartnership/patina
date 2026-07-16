//! AArch64 Specific abstractions for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
//! Portions Copyright 2023 The arm-gic Authors.
//! arm-gic is dual-licensed under Apache 2.0 and MIT terms.

use crate::{error::EfiError, pi::protocols::cpu_arch::CpuFlushType};
use r_efi::efi;

pub(super) struct AArch64;

impl super::ArchSupport for AArch64 {}

const DAIF_WR_IRQ_BIT: u64 = 0x2;

/// Reads and returns the value of the given aarch64 system register.
#[macro_export]
macro_rules! read_sysreg {
    ($name:ident) => {
        {
            let mut value: u64;
            // SAFETY: The caller must provide a valid system register name
            // and ensure that the system is in a state where reading this register is safe.
            unsafe {
                core::arch::asm!(
                    concat!("mrs {value:x}, ", core::stringify!($name)),
                    value = out(reg) value,
                    options(nomem, nostack),
                );
            }
            value
        }
    }
}

/// Writes the given value to the given aarch64 system register.
/// Usage:
/// - `write_sysreg!(reg register_name, imm value)` - Write immediate value
/// - `write_sysreg!(reg register_name, imm value, "barrier1", "barrier2", ...)` - Write immediate value with barriers
/// - `write_sysreg!(reg dest_register, reg src_register)` - Copy from one register to sysreg
/// - `write_sysreg!(reg dest_register, reg src_register, "barrier1", "barrier2", ...)` - Copy from one register to sysreg with barriers
/// - `write_sysreg!(reg register_name, value)` - Write literal value
/// - `write_sysreg!(reg register_name, value, "barrier1", "barrier2", ...)` - Write literal value with barriers
#[macro_export]
macro_rules! write_sysreg {
    (reg $dest:ident, imm $imm:expr) => {
        {
            // immediate-to-register copy, no barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", {imm}"),
                    imm = const $imm,
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $dest:ident, imm $imm:expr, $($barrier:literal),+) => {
        {
            // immediate-to-register copy, barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", {imm}"),
                    $($barrier,)+
                    imm = const $imm,
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $dest:ident, reg $src:ident) => {
        {
            // register-to-register copy, no barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", ", core::stringify!($src)),
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $dest:ident, reg $src:ident, $($barrier:literal),+) => {
        {
            // register-to-register copy, barrier required case
            // SAFETY: The caller must provide valid system register names
            // and ensure that the system is in a state where reading from src and writing to dest is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($dest), ", ", core::stringify!($src)),
                    $($barrier,)+
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $name:ident, $value:expr) => {
        {
            // no barrier required case
            let v: u64 = $value;
            // SAFETY: The caller must provide a valid system register name
            // and ensure that the system is in a state where writing to this register with this value is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($name), ", {value:x}"),
                    value = in(reg) v,
                    options(nomem, nostack),
                )
            }
        }
    };
    (reg $name:ident, $value:expr, $($barrier:literal),+) => {
        {
            // barrier required case
            let v: u64 = $value;
            // SAFETY: The caller must provide a valid system register name
            // and ensure that the system is in a state where writing to this register with this value is safe.
            unsafe {
                core::arch::asm!(
                    concat!("msr ", core::stringify!($name), ", {value:x}"),
                    $($barrier,)+
                    value = in(reg) v,
                    options(nomem, nostack),
                )
            }
        }
    };
}

impl super::Interrupts for AArch64 {
    fn enable_interrupts() {
        write_sysreg!(reg daifclr, imm DAIF_WR_IRQ_BIT, "isb sy");
    }

    fn disable_interrupts() {
        write_sysreg!(reg daifset, imm DAIF_WR_IRQ_BIT, "isb sy");
    }

    fn interrupts_enabled() -> bool {
        let daif = read_sysreg!(daif);
        daif & 0x80 == 0
    }

    fn enable_interrupts_and_sleep() {
        // SAFETY: Enabling interrupts and then waiting for an interrupt has no memory safety implications.
        unsafe {
            core::arch::asm!("msr daifclr, {}; wfi", const DAIF_WR_IRQ_BIT, options(nostack, nomem, preserves_flags));
        }
    }
}

/// Supported AARCH64 Exception Levels
#[derive(Debug, PartialEq, Eq)]
pub enum AArch64El {
    EL1,
    EL2,
}

// Determine the current exception level
pub fn get_current_el() -> AArch64El {
    match read_sysreg!(CurrentEL) & 0xC {
        0xC => panic!("EL3 is not supported"),
        0x8 => AArch64El::EL2,
        0x4 => AArch64El::EL1,
        0x0 => panic!("EL0 is not supported"),
        _ => unreachable!(),
    }
}

impl super::CacheMgmt for AArch64 {
    fn flush_data_cache(start: efi::PhysicalAddress, length: u64, flush_type: CpuFlushType) -> Result<(), EfiError> {
        flush_data_cache_range(start, length, flush_type);
        Ok(())
    }

    fn cache_writeback_granule() -> u32 {
        let ctr_el0 = read_sysreg!(ctr_el0);

        // CWG (Cache Writeback Granule): CTR_EL0 bits [27:24]
        let cwg = ((ctr_el0 >> 24) & 0xF) as u32;

        // CWG is Log2 of the max size in words
        if cwg > 0 {
            4 << cwg
        } else {
            crate::base::SIZE_2KB as u32 // Default to 2K if register contains 0 per Armv8-A spec
        }
    }
}

impl super::Timer for AArch64 {
    fn get_timer_value(_timer_index: u32) -> Result<u64, EfiError> {
        Err(EfiError::Unsupported)
    }

    fn get_timer_period(_timer_index: u32) -> Result<u64, EfiError> {
        Err(EfiError::Unsupported)
    }
}

fn clean_data_entry_by_mva(mva: efi::PhysicalAddress) {
    // SAFETY: Cleaning the data cache has no impact on safety invariants.
    unsafe {
        core::arch::asm!("dc cvac, {}", in(reg) mva, options(nostack, preserves_flags));
    }
}

fn invalidate_data_cache_entry_by_mva(mva: efi::PhysicalAddress) {
    // SAFETY: Invalidating the data cache does not impact safety checks. It does have the potential
    // to corrupt memory if used incorrectly, but the caller is expected to ensure that they are
    // using this function correctly.
    unsafe {
        core::arch::asm!("dc ivac, {}", in(reg) mva, options(nostack, preserves_flags));
    }
}

fn clean_and_invalidate_data_entry_by_mva(mva: efi::PhysicalAddress) {
    // SAFETY: Cleaning and invalidating the data cache does not impact safety invariants.
    unsafe {
        core::arch::asm!("dc civac, {}", in(reg) mva, options(nostack, preserves_flags));
    }
}

fn data_cache_line_len() -> u64 {
    let ctr_el0 = read_sysreg!(ctr_el0);
    4 << ((ctr_el0 >> 16) & 0xf)
}

/// Performs a data cache maintenance operation over the virtual address range
/// `[start, start + length)` according to `op`, followed by a single `dsb sy`
/// barrier. A no-op if `length` is zero.
fn flush_data_cache_range(start: efi::PhysicalAddress, length: u64, op: CpuFlushType) {
    if length == 0 {
        return;
    }

    let cacheline_size = data_cache_line_len();
    let cacheline_mask = cacheline_size - 1;
    let mut aligned_addr = start & !cacheline_mask;
    let end_addr = match start.checked_add(length) {
        Some(end_addr) => end_addr,
        None => {
            debug_assert!(false, "Cache range overflow");
            return;
        }
    };

    while aligned_addr < end_addr {
        match op {
            CpuFlushType::EfiCpuFlushTypeWriteBack => clean_data_entry_by_mva(aligned_addr),
            CpuFlushType::EfiCpuFlushTypeInvalidate => invalidate_data_cache_entry_by_mva(aligned_addr),
            CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => clean_and_invalidate_data_entry_by_mva(aligned_addr),
        }

        match aligned_addr.checked_add(cacheline_size) {
            Some(next) => aligned_addr = next,
            None => break,
        }
    }

    // we have a data barrier after all cache lines have had the operation performed on them as an optimization
    // SAFETY: a data barrier has no impact on safety invariants.
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
}
