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
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

static LOAD_IMAGE_COUNT: AtomicU32 = AtomicU32::new(0);
static PERF_MEASUREMENT_MASK: AtomicU32 = AtomicU32::new(0);
static STATIC_STATE_IS_INIT: AtomicBool = AtomicBool::new(false);

static mut BOOT_SERVICES: MaybeUninit<StandardBootServices> = MaybeUninit::uninit();
static mut FBPT: MaybeUninit<TplMutex<FBPT>> = MaybeUninit::uninit();

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
#[allow(static_mut_refs)]
pub fn set_static_state(boot_services: StandardBootServices) -> Option<&'static TplMutex<'static, FBPT>> {
    // Return Ok if STATIC_STATE_INIT is false and set it to true. Make this run only once.
    if STATIC_STATE_IS_INIT.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        // SAFETY: This is safe because it is the entry point and no one is reading these value yet.
        unsafe {
            let boot_services_ref = BOOT_SERVICES.write(boot_services);
            Some(FBPT.write(TplMutex::new(boot_services_ref, Tpl::NOTIFY, FBPT::new())))
        }
    } else {
        None
    }
}

/// Get performance component static state.
#[allow(static_mut_refs)]
pub fn get_static_state() -> Option<(&'static StandardBootServices, &'static TplMutex<'static, FBPT>)> {
    if STATIC_STATE_IS_INIT.load(Ordering::Relaxed) {
        // SAFETY: This is safe because the state has been init.
        unsafe { Some((BOOT_SERVICES.assume_init_ref(), FBPT.assume_init_ref())) }
    } else {
        None
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_get_static_state() {
        STATIC_STATE_IS_INIT.store(false, Ordering::Relaxed);
        unsafe {
            BOOT_SERVICES = MaybeUninit::zeroed();
            FBPT = MaybeUninit::zeroed();
        }

        assert!(get_static_state().is_none());
        assert!(set_static_state(StandardBootServices::new_uninit()).is_some());
        assert!(get_static_state().is_some());
        assert!(set_static_state(StandardBootServices::new_uninit()).is_none());
    }
}
