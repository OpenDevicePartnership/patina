//! Stub `Architecture` implementation for host (non-UEFI) builds.
//!
//! Provides no-op versions of the architecture-specific interfaces for test code to consume. It
//! currently only contains the set of code needed for patina to run tests.
//!
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

/// Stub architecture used for host/unit-test builds.
pub(crate) struct StubArch;

impl super::ArchSupport for StubArch {}

static INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(true);

impl super::Interrupts for StubArch {
    fn enable_interrupts() {
        INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);
    }

    fn disable_interrupts() {
        INTERRUPTS_ENABLED.store(false, Ordering::SeqCst);
    }

    fn interrupts_enabled() -> bool {
        INTERRUPTS_ENABLED.load(Ordering::SeqCst)
    }

    fn enable_interrupts_and_sleep() {
        INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);
    }
}

impl super::CacheMgmt for StubArch {
    fn flush_data_cache(_start: efi::PhysicalAddress, _length: u64, _flush_type: CpuFlushType) -> Result<(), EfiError> {
        Ok(())
    }

    fn cache_writeback_granule() -> u32 {
        64
    }
}

impl super::Timer for StubArch {
    fn get_timer_value() -> u64 {
        0
    }

    fn get_timer_frequency() -> Option<NonZeroU64> {
        None
    }
}

//
// Architecture-specific stubs
//

/// Host stub of the aarch64 architecture surface.
#[cfg(target_arch = "aarch64")]
pub mod aarch64 {
    /// Stub of the real `aarch64` exception level enum for host builds.
    #[derive(Debug, PartialEq, Eq)]
    pub enum AArch64El {
        EL1,
        EL2,
    }

    /// Stub of the real `get_current_el`. Reports `EL2` always.
    pub fn get_current_el() -> AArch64El {
        AArch64El::EL2
    }

    /// Stub of the `read_sysreg!` macro. Evaluates to `0` without touching any system register.
    #[macro_export]
    macro_rules! read_sysreg {
        ($name:ident) => {{ 0u64 }};
    }

    /// Stub of the `write_sysreg!` macro. Consumes any value operand and expands to a no-op.
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
}

/// Host stub of the x64 architecture surface.
#[cfg(target_arch = "x86_64")]
pub mod x64 {
    /// Stub of the real `x64` `io_out8`. No-op on host builds.
    ///
    /// # Safety
    ///
    /// Always safe; this stub performs no I/O.
    pub unsafe fn io_out8(_port: u16, _value: u8) {}
}
