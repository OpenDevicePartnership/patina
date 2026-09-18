//! CPU state adopted by APs before entering the Rust dispatch loop.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::cell::UnsafeCell;

use patina_mtrr::{Mtrr, create_mtrr_lib, structs::MtrrSettings};

/// Global BSP MTRR snapshot shared by every AP.
struct MtrrState {
    mtrrs: UnsafeCell<Option<MtrrSettings>>,
}

// SAFETY: The BSP writes the snapshot before initial AP startup or while every
// dispatchable AP is idle under the dispatch lock. APs only read it after work
// publication or during startup.
unsafe impl Sync for MtrrState {}

static MTRR_STATE: MtrrState = MtrrState { mtrrs: UnsafeCell::new(None) };

impl MtrrState {
    fn capture(&self) -> Result<bool, ()> {
        let mtrr = create_mtrr_lib(0);
        if !mtrr.is_supported() {
            return Ok(false);
        }
        let settings = mtrr
            .get_all_mtrrs()
            .inspect_err(|e| log::error!("Failed to read BSP MTRRs for AP synchronization: {e:?}"))
            .map_err(|_| ())?;

        // SAFETY: The caller guarantees no AP can read the slot while it is replaced.
        let slot = unsafe { &mut *self.mtrrs.get() };
        *slot = Some(settings);
        Ok(true)
    }

    fn apply(&self) -> bool {
        let mut mtrr = create_mtrr_lib(0);
        if !mtrr.is_supported() {
            return true;
        }

        // SAFETY: The BSP publishes the snapshot before dispatching this work
        // or starting an AP, and does not replace it until every AP is idle.
        let Some(settings) = (unsafe { &*self.mtrrs.get() }).as_ref() else {
            log::error!("AP MTRR synchronization ran without a prepared snapshot");
            return false;
        };
        mtrr.set_all_mtrrs(settings);
        true
    }
}

/// Captures the BSP's MTRRs before AP startup or while every AP is idle.
pub(super) fn capture() -> Result<bool, ()> {
    MTRR_STATE.capture()
}

/// Applies the current global MTRR snapshot on the calling AP.
pub(super) fn apply() -> bool {
    MTRR_STATE.apply()
}
