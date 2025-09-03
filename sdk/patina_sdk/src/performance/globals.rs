//! Globals used in the Patina SDK performance code.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use crate::{
    boot_services::{StandardBootServices, tpl::Tpl},
    performance::table::FBPT,
    tpl_mutex::TplMutex,
};
use core::{
    cell::OnceCell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

static LOAD_IMAGE_COUNT: AtomicU32 = AtomicU32::new(0);
static PERF_MEASUREMENT_MASK: AtomicU32 = AtomicU32::new(0);

/// Static state for the performance component.
struct StaticState<'a> {
    /// Boot Services instance
    boot_services: OnceCell<StandardBootServices>,
    /// The FBPT protected by a TPL mutex.
    fbpt: OnceCell<TplMutex<'a, FBPT>>,
    /// Flag to indicate if the static state is in the process of being initialized.
    initializing: AtomicBool,
}

impl<'a> StaticState<'a> {
    /// Creates a new uninitialized static state.
    const fn uninit() -> Self {
        Self { boot_services: OnceCell::new(), fbpt: OnceCell::new(), initializing: AtomicBool::new(false) }
    }

    /// Initializes the static state.
    ///
    /// ## Errors
    ///
    /// Returns `Already initialized` if the static state has already been initialized.
    /// Returns `Currently initializing somewhere else` if another thread is currently initializing the static state.
    fn init(&'a self, bs: StandardBootServices) -> Result<(), &'static str> {
        if self.initializing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            self.boot_services.set(bs).map_err(|_| "Already initialized")?;
            self.fbpt
                .set(TplMutex::new(self.boot_services.get().expect("Boot Services Just Set"), Tpl::NOTIFY, FBPT::new()))
                .map_err(|_| "Failed to set FBPT")?;
            self.initializing.store(false, Ordering::Release);
            return Ok(());
        }

        Err("Currently initializing somewhere else")
    }

    /// Gets the inner static state if it has been initialized.
    ///
    /// Returns `None` if the state is not yet initialized.
    /// Returns `None` if the state is currently being initialized.
    /// Returns `Some` with references to the `StandardBootServices` and `TplMutex<FBPT>` if initialized.
    fn inner(&self) -> Option<(&StandardBootServices, &TplMutex<'a, FBPT>)> {
        if !self.initializing.load(Ordering::Acquire)
            && let Some(bs) = self.boot_services.get()
            && let Some(fbpt) = self.fbpt.get()
        {
            return Some((bs, fbpt));
        }
        None
    }
}

/// SAFETY: Initializing the `OnceCell`s via the atomic `initialize` flag satisfies the `Send` requirement for
/// synchronization on the `set` calls inside `init`. Both the `StandardBootServices` and `TplMutex` types are `Send`
/// as well, so the only other usage of the `OnceCell`s `get` method is safe.
unsafe impl Send for StaticState<'static> {}

/// SAFETY: Initializing the `OnceCell`s via the atomic `initialize` flag satisfies the `Sync` requirement for
/// synchronization on the `set` calls inside `init`. Both the `StandardBootServices` and `TplMutex` types are `Sync`
/// as well, so the only other usage of the `OnceCell`s `get` method is safe.
unsafe impl Sync for StaticState<'static> {}

static STATIC_STATE: StaticState<'static> = StaticState::uninit();

/// Set performance component static state.
pub fn set_perf_measurement_mask(mask: u32) {
    PERF_MEASUREMENT_MASK.store(mask, Ordering::Relaxed);
}

/// Get performance component static state.
pub fn get_perf_measurement_mask() -> u32 {
    PERF_MEASUREMENT_MASK.load(Ordering::Relaxed)
}

/// Get the current load image count.
pub fn get_load_image_count() -> u32 {
    LOAD_IMAGE_COUNT.load(Ordering::Relaxed)
}

/// Increment the load image count.
pub fn increment_load_image_count() {
    LOAD_IMAGE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Set load image count to a specific value.
pub fn set_load_image_count(count: u32) {
    LOAD_IMAGE_COUNT.store(count, Ordering::Relaxed);
}

/// Set performance component static state.
#[coverage(off)]
pub fn set_static_state(boot_services: StandardBootServices) -> Result<(), &'static str> {
    STATIC_STATE.init(boot_services)
}

/// Get performance component static state.
#[coverage(off)]
pub fn get_static_state() -> Option<(&'static StandardBootServices, &'static TplMutex<'static, FBPT>)> {
    STATIC_STATE.inner()
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_get_static_state() {
        static STATIC_STATE: StaticState = StaticState::uninit();
        assert!(STATIC_STATE.inner().is_none());
        assert!(STATIC_STATE.init(StandardBootServices::new_uninit()).is_ok());
        assert!(STATIC_STATE.inner().is_some());
        assert!(STATIC_STATE.init(StandardBootServices::new_uninit()).is_err());
    }
}
