//! Stub `Architecture` implementation for host (non-UEFI) builds.
//!
//! Provides no-op versions of the architecture-specific interfaces for test code to consume. It
//! currently only contains the set of code needed for patina to run tests.
//!
//! - Debugger
//! - Interrupts
//! - Cache management (including TLB)
//! - Timer
//! - Architecture-specific stubs (`aarch64` and `x64` modules)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{error::EfiError, pi::protocol::cpu_arch::CpuFlushType, standard::efi};
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicBool, Ordering};

//
// Debugger
//

/// Raises a breakpoint exception. No-op on host builds.
pub fn breakpoint() {}

//
// Interrupts
//

/// Interrupt state tracked by host builds.
static INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enables interrupts. Components should use TPLs.
pub fn enable_interrupts() {
    INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);
}

/// Disables interrupts. Components should use TPLs.
pub fn disable_interrupts() {
    INTERRUPTS_ENABLED.store(false, Ordering::SeqCst);
}

/// Returns whether interrupts are enabled.
pub fn interrupts_enabled() -> bool {
    INTERRUPTS_ENABLED.load(Ordering::SeqCst)
}

/// Enables interrupts and waits for the next one. Host builds only enable them.
pub fn enable_interrupts_and_sleep() {
    INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);
}

/// Runs `f` with interrupts disabled, then restores their previous state. Components should use
/// TPLs instead.
pub(crate) fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    if !interrupts_enabled() {
        return f();
    }
    disable_interrupts();
    let result = f();
    enable_interrupts();
    result
}

//
// Cache management
//

/// Performs the `flush_type` data cache operation on `[start, start + length)`. No-op on host
/// builds.
///
/// ## Errors
///
/// Never fails on host builds.
pub fn flush_data_cache(_start: efi::PhysicalAddress, _length: u64, _flush_type: CpuFlushType) -> Result<(), EfiError> {
    Ok(())
}

/// Returns the cache writeback granule in bytes, used as the CPU Arch Protocol DMA buffer
/// alignment. Host builds report 64.
pub fn cache_writeback_granule() -> u32 {
    64
}

/// Invalidates the executing CPU's TLB entries. No-op on host builds.
pub fn flush_tlb() {}

//
// Timer
//

/// Returns the timer counter value in ticks. Host builds report 0.
pub fn get_timer_value() -> u64 {
    0
}

/// Returns the timer frequency in Hz, or `None` if it is unknown. Host builds report `None`.
pub fn get_timer_frequency() -> Option<NonZeroU64> {
    None
}

//
// Architecture-specific stubs
//

/// Host stub of the aarch64 architecture surface.
#[cfg(target_arch = "aarch64")]
pub mod aarch64 {
    //
    // System registers
    //

    /// Stub of `read_sysreg!`. Evaluates to `0`.
    #[macro_export]
    macro_rules! read_sysreg {
        ($name:ident) => {{ 0u64 }};
    }

    /// Stub of `write_sysreg!`. Evaluates any value operand and writes nothing.
    #[macro_export]
    macro_rules! write_sysreg {
        (reg $dest:ident, imm $imm:expr) => {{
            let _ = $imm;
        }};
        (reg $dest:ident, imm $imm:expr, $($barrier:literal),+) => {{
            let _ = $imm;
        }};
        (reg $dest:ident, reg $src:ident) => {{}};
        (reg $dest:ident, reg $src:ident, $($barrier:literal),+) => {{}};
        (reg $name:ident, $value:expr) => {{
            let _: u64 = $value;
        }};
        (reg $name:ident, $value:expr, $($barrier:literal),+) => {{
            let _: u64 = $value;
        }};
    }

    //
    // Exception levels
    //

    /// Exception levels that Patina supports.
    #[derive(Debug, PartialEq, Eq)]
    pub enum AArch64El {
        EL1,
        EL2,
    }

    /// Returns the current exception level. Host builds report `EL2`.
    pub fn get_current_el() -> AArch64El {
        AArch64El::EL2
    }

    //
    // Secure monitor calls
    //

    /// Issues a Secure Monitor Call. Host builds return all zeros.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn smc(_function_id: u32, _args: [u64; 3]) -> [u64; 4] {
        [0; 4]
    }

    //
    // Debugger
    //

    pub use super::breakpoint;

    /// Length in bytes of the load that the `AArch64` `probe_read` executes.
    pub const PROBE_READ_INSTRUCTION_LEN: u64 = 4;

    /// Tests whether `address` is readable. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn probe_read(_address: u64) {}

    //
    // Cache management
    //

    pub use super::flush_tlb;

    /// Invalidates the executing CPU's instruction cache. No-op on host builds.
    pub fn invalidate_instruction_cache() {}
}

/// Host stub of the x64 architecture surface.
#[cfg(target_arch = "x86_64")]
pub mod x64 {
    //
    // Port I/O
    //

    /// Writes a byte to an I/O port. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn io_out8(_port: u16, _value: u8) {}

    //
    // Model-specific registers
    //

    /// Reads a model-specific register. Host builds return 0.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn read_msr(_msr: u32) -> u64 {
        0
    }

    /// Writes a model-specific register. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn write_msr(_msr: u32, _value: u64) {}

    //
    // Processor identification
    //

    /// CPUID leaf 0x1: version information.
    pub const CPUID_VERSION_INFO: u32 = 0x01;

    /// Executes `CPUID`. Host builds return all zeros.
    pub fn cpuid(_leaf: u32, _subleaf: u32) -> core::arch::x86_64::CpuidResult {
        core::arch::x86_64::CpuidResult { eax: 0, ebx: 0, ecx: 0, edx: 0 }
    }

    /// Returns CPUID leaf 0x1. Host builds return all zeros.
    pub fn cpuid_version_info() -> core::arch::x86_64::CpuidResult {
        core::arch::x86_64::CpuidResult { eax: 0, ebx: 0, ecx: 0, edx: 0 }
    }

    /// Returns the initial APIC ID. Host builds return 0.
    pub fn current_apic_id() -> u32 {
        0
    }

    /// Returns whether the executing processor is the BSP. Host builds return `true`.
    pub fn is_bsp() -> bool {
        true
    }

    //
    // Control registers
    //

    /// Reads CR0. Host builds return 0.
    pub fn read_cr0() -> u64 {
        0
    }

    /// Reads CR3. Host builds return 0.
    pub fn read_cr3() -> u64 {
        0
    }

    /// Reads CR4. Host builds return 0.
    pub fn read_cr4() -> u64 {
        0
    }

    //
    // Descriptor tables
    //

    /// Pseudo-descriptor used by `SGDT`/`LGDT` and `SIDT`/`LIDT`.
    #[repr(C, packed)]
    #[derive(Debug, Clone, Copy)]
    pub struct DescriptorTablePointer {
        /// Size of the descriptor table in bytes, minus 1.
        pub limit: u16,
        /// Linear address of the descriptor table.
        pub base: u64,
    }

    const _: () = assert!(core::mem::size_of::<DescriptorTablePointer>() == 10);

    /// Reads the GDT register. Host builds return a zeroed descriptor.
    pub fn read_gdtr() -> DescriptorTablePointer {
        DescriptorTablePointer { limit: 0, base: 0 }
    }

    /// Reads the IDT register. Host builds return a zeroed descriptor.
    pub fn read_idtr() -> DescriptorTablePointer {
        DescriptorTablePointer { limit: 0, base: 0 }
    }

    /// Loads the GDT register. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn load_gdtr(_descriptor: &DescriptorTablePointer) {}

    /// Loads the IDT register. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn load_idtr(_descriptor: &DescriptorTablePointer) {}

    //
    // Debug registers
    //

    /// Reads DR0 to DR3, DR6 or DR7, or returns `None` for any other `index`. Host builds return 0.
    pub fn read_debug_register(index: usize) -> Option<u64> {
        matches!(index, 0..=3 | 6 | 7).then_some(0)
    }

    /// Returns whether `index` names DR0 to DR3, DR6 or DR7. Host builds write nothing.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn write_debug_register(index: usize, _value: u64) -> bool {
        matches!(index, 0..=3 | 6 | 7)
    }

    //
    // SMAP
    //

    /// Disables Supervisor Mode Access Prevention. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn disable_smap() {}

    /// Re-enables Supervisor Mode Access Prevention. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn enable_smap() {}

    //
    // Debugger
    //

    pub use super::breakpoint;

    /// Length in bytes of the load that the x64 `probe_read` executes.
    pub const PROBE_READ_INSTRUCTION_LEN: u64 = 3;

    /// Tests whether `address` is readable. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe on host builds.
    pub unsafe fn probe_read(_address: u64) {}

    //
    // Halt and wait
    //

    /// Halts the executing CPU. No-op on host builds.
    pub fn hlt() {}

    /// Arms address monitoring for `mwait`. No-op on host builds.
    pub fn monitor<T>(_addr: &T) {}

    /// Waits for a store to the monitored range. No-op on host builds.
    pub fn mwait() {}

    //
    // Cache management
    //

    pub use super::flush_tlb;

    /// Writes back and invalidates all caches. No-op on host builds.
    pub fn wbinvd() {}
}
