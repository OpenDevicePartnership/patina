//! x64-specific architectural helpers for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{error::EfiError, pi::protocols::cpu_arch::CpuFlushType};
use core::arch::asm;
use r_efi::efi;

pub(crate) struct X64;

impl super::ArchSupport for X64 {}

/// Cache writeback granule for x86_64, using 4 bytes following precedence set by Tianocore.
const CACHE_WRITEBACK_GRANULE: u32 = 4;

/// Writes a byte to an x64 I/O port.
///
/// # Safety
///
/// The caller must ensure `port` is valid for byte writes on this platform and
/// that the side effects are safe in the current execution context.
pub unsafe fn io_out8(port: u16, value: u8) {
    // SAFETY: Guaranteed by caller.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nostack, nomem, preserves_flags)
        );
    }
}

/// Reads the time-stamp counter.
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` reads a CPU counter and does not violate memory safety.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem)) };
    ((hi as u64) << 32) | lo as u64
}

impl super::Interrupts for X64 {
    fn enable_interrupts() {
        // SAFETY: Enabling interrupts via `sti` does not violate memory safety; the caller is
        // responsible for ensuring the system is ready to service interrupts. This operation
        // preserves flags even though it sets the IF flag, because preserves_flags is only about
        // status flags, not control flags.
        unsafe {
            asm!("sti", options(nostack, nomem, preserves_flags));
        }
    }

    fn disable_interrupts() {
        // SAFETY: Disabling interrupts via `cli` does not violate memory safety. This operation
        // preserves flags even though it sets the IF flag, because preserves_flags is only about
        // status flags, not control flags.
        unsafe {
            asm!("cli", options(nostack, nomem, preserves_flags));
        }
    }

    fn interrupts_enabled() -> bool {
        let eflags: u64;
        const IF: u64 = 0x200;
        // SAFETY: Reading RFLAGS via a push and pop has no side effects.
        unsafe {
            asm!("pushfq; pop {}", out(reg) eflags);
        }
        eflags & IF != 0
    }

    fn sleep() {
        // SAFETY: This halts the CPU until the next interrupt, which has no memory safety implications.
        unsafe {
            asm!("hlt");
        }
    }
}

impl super::CacheMgmt for X64 {
    fn flush_data_cache(_start: efi::PhysicalAddress, _length: u64, flush_type: CpuFlushType) -> Result<(), EfiError> {
        match flush_type {
            CpuFlushType::EfiCpuFlushTypeWriteBackInvalidate => {
                asm_wbinvd();
                Ok(())
            }
            CpuFlushType::EfiCpuFlushTypeInvalidate => {
                asm_invd();
                Ok(())
            }
            _ => Err(EfiError::Unsupported),
        }
    }

    fn cache_writeback_granule() -> u32 {
        CACHE_WRITEBACK_GRANULE
    }
}

impl super::Timer for X64 {
    fn get_timer_value(timer_index: u32) -> Result<u64, EfiError> {
        if timer_index != 0 {
            return Err(EfiError::InvalidParameter);
        }
        Ok(read_tsc())
    }

    fn get_timer_period(timer_index: u32) -> Result<u64, EfiError> {
        if timer_index != 0 {
            return Err(EfiError::InvalidParameter);
        }
        Ok(timer_period())
    }
}

fn asm_wbinvd() {
    // SAFETY: Writing back and invalidating the cache has no memory safety implications.
    unsafe {
        asm!("wbinvd");
    }
}

fn asm_invd() {
    // SAFETY: Invalidating the cache has no memory safety implications.
    unsafe {
        asm!("invd");
    }
}

/// Reads the timestamp counter used for the CPU timer value. Currently returns 0 until a real
/// implementation is provided.
fn read_tsc() -> u64 {
    0
}

/// Computes the CPU timer period. Currently returns 0 until a real implementation is provided.
fn timer_period() -> u64 {
    0
}
