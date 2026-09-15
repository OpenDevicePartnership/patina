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

use super::ApWorkItem;

/// Processor-local architectural state that must remain valid for the lifetime
/// of an AP.
pub(super) struct ApCpuState {
    mtrrs: UnsafeCell<Option<MtrrSettings>>,
}

// SAFETY: The BSP writes the MTRR slot only while its AP is idle, then publishes
// work that reads it. After publication only the owning AP reads the slot.
unsafe impl Sync for ApCpuState {}

impl ApCpuState {
    pub(super) const fn new() -> Self {
        Self { mtrrs: UnsafeCell::new(None) }
    }

    /// Captures the BSP's MTRRs in this AP's persistent slot and returns work
    /// that applies them on the AP.
    ///
    /// The caller must ensure the AP is idle and hold the dispatch lock until
    /// the returned work is published.
    pub(super) fn prepare_mtrr_sync(&'static self) -> Option<ApWorkItem> {
        let mtrr = create_mtrr_lib(0);
        if !mtrr.is_supported() {
            return None;
        }
        let settings = mtrr
            .get_all_mtrrs()
            .inspect_err(|e| log::error!("Failed to read BSP MTRRs for AP synchronization: {e:?}"))
            .ok()?;

        // SAFETY: The caller guarantees this AP is idle and holds the dispatch
        // lock, so no AP can be reading the slot while it is replaced.
        let slot = unsafe { &mut *self.mtrrs.get() };
        *slot = Some(settings);
        Some(ApWorkItem::new(apply_mtrrs, self))
    }
}

/// AP procedure that programs the calling processor's MTRRs from its persistent
/// snapshot slot.
fn apply_mtrrs(state: &ApCpuState) {
    // SAFETY: `prepare_mtrr_sync` initializes this AP's snapshot while the AP is
    // idle, then dispatch publication makes it visible before this routine runs.
    let Some(settings) = (unsafe { &*state.mtrrs.get() }).as_ref() else {
        log::error!("AP MTRR synchronization ran without a prepared snapshot");
        return;
    };
    let mut mtrr = create_mtrr_lib(0);
    if mtrr.is_supported() {
        mtrr.set_all_mtrrs(settings);
    }
}
