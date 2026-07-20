//! No-op `Architecture` implementation for unit tests.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{error::EfiError, pi::protocols::cpu_arch::CpuFlushType};
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicBool, Ordering};
use r_efi::efi;

/// No-op architecture used in unit tests.
pub(crate) struct NullArch;

impl super::ArchSupport for NullArch {}

static INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(true);

impl super::Interrupts for NullArch {
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

impl super::CacheMgmt for NullArch {
    fn flush_data_cache(_start: efi::PhysicalAddress, _length: u64, _flush_type: CpuFlushType) -> Result<(), EfiError> {
        Ok(())
    }

    fn cache_writeback_granule() -> u32 {
        64
    }
}

impl super::Timer for NullArch {
    fn get_timer_value() -> u64 {
        0
    }

    fn get_timer_frequency() -> Option<NonZeroU64> {
        None
    }
}
