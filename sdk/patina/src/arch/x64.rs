//! x64-specific architectural helpers for Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Writes a byte to an x64 I/O port.
///
/// # Safety
///
/// The caller must ensure `port` is valid for byte writes on this platform and
/// that the side effects are safe in the current execution context.
#[coverage(off)]
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
#[coverage(off)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` reads a CPU counter and does not violate memory safety.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem)) };
    ((hi as u64) << 32) | lo as u64
}
