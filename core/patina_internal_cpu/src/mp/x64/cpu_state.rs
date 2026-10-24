//! CPU state adopted by APs before entering the Rust dispatch loop.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// This code can not be meaningfully tested from usermode.
#![cfg_attr(coverage, coverage(off))]

use patina_mtrr::{Mtrr, create_mtrr_lib, structs::MtrrSettings};
use spin::RwLock;

/// Global BSP MTRR snapshot shared by every AP.
static MTRR_STATE: RwLock<Option<MtrrSettings>> = RwLock::new(None);

/// Captures the BSP's MTRRs before AP startup or while every AP is idle.
pub(super) fn capture() -> Result<bool, ()> {
    let mtrr = create_mtrr_lib(0);
    if !mtrr.is_supported() {
        return Ok(false);
    }
    let settings = mtrr
        .get_all_mtrrs()
        .inspect_err(|e| log::error!("Failed to read BSP MTRRs for AP synchronization: {e:?}"))
        .map_err(|_| ())?;

    *MTRR_STATE.write() = Some(settings);
    Ok(true)
}

/// Applies the current global MTRR snapshot on the calling AP.
pub(super) fn apply() -> bool {
    let mut mtrr = create_mtrr_lib(0);
    if !mtrr.is_supported() {
        return true;
    }

    // Copy the MTRRs from the global snapshot to avoid holding the lock while applying them in case something
    // goes wrong, we don't kill the lock.
    let settings = {
        let settings = MTRR_STATE.read();
        // SAFETY: `MtrrSettings` and all of its fields are fixed-size register
        // values with no owned resources or drop behavior. TODO: replace with clone when available.
        settings.as_ref().map(|settings| unsafe { core::ptr::read(settings) })
    };

    let Some(settings) = settings else {
        log::error!("AP MTRR synchronization ran without a prepared snapshot");
        return false;
    };

    mtrr.set_all_mtrrs(&settings);
    true
}

pub(super) const IA32_EFER: u32 = 0xC000_0080;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CpuControl {
    pub(super) cr0: u64,
    pub(super) cr3: u64,
    pub(super) cr4: u64,
    pub(super) efer: u64,
}

impl CpuControl {
    pub(super) fn capture() -> Self {
        // SAFETY: IA32_EFER is available on x86_64 processors and has no read side effects.
        let efer = unsafe { patina::arch::x64::read_msr(IA32_EFER) };
        Self {
            cr0: patina::arch::x64::read_cr0(),
            cr3: patina::arch::x64::read_cr3(),
            cr4: patina::arch::x64::read_cr4(),
            efer,
        }
    }
}
