//! No-op `Architecture` implementation for unit tests.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::{error::EfiError, pi::protocols::cpu_arch::CpuFlushType};
use r_efi::efi;

/// No-op architecture used in unit tests.
pub(crate) struct NullArch;

impl super::ArchSupport for NullArch {}

impl super::Interrupts for NullArch {
    fn enable_interrupts() {}

    fn disable_interrupts() {}

    fn interrupts_enabled() -> bool {
        false
    }

    fn sleep() {}
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
    fn get_timer_value(_timer_index: u32) -> Result<u64, EfiError> {
        Ok(0)
    }

    fn get_timer_period(_timer_index: u32) -> Result<u64, EfiError> {
        Ok(0)
    }
}
