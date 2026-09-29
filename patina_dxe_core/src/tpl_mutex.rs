//! DXE Core adapter for the SDK TPL-aware mutex.
//!
//! The mutex implementation is shared with the SDK. This module supplies the
//! DXE Core's direct TPL routines and preserves the Core-facing API.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
use core::fmt;

use patina::{
    standard::efi,
    uefi::{
        boot_services::tpl::Tpl,
        tpl_mutex::{TplController, TplMutex as SharedTplMutex, TplMutexGuard},
    },
};

use crate::events::{raise_tpl, restore_tpl};

#[derive(Clone, Copy)]
pub(crate) struct CoreTplController;

impl TplController for CoreTplController {
    fn raise_tpl(&self, tpl: Tpl) -> Tpl {
        raise_tpl(tpl.into()).into()
    }

    fn restore_tpl(&self, tpl: Tpl) {
        restore_tpl(tpl.into());
    }
}

/// Used to guard data with a locked MUTEX and TPL level.
pub struct TplMutex<T: ?Sized> {
    tpl_lock_level: efi::Tpl,
    name: &'static str,
    inner: SharedTplMutex<T, CoreTplController>,
}

/// Wrapper for guarded data, which can be accessed by Deref or `DerefMut` on this object.
pub type TplGuard<'a, T> = TplMutexGuard<'a, T, CoreTplController>;

impl<T> TplMutex<T> {
    /// Instantiates a new `TplMutex` with the given TPL level, data object, and name string.
    pub const fn new(tpl_lock_level: efi::Tpl, data: T, name: &'static str) -> Self {
        Self { tpl_lock_level, name, inner: SharedTplMutex::new(CoreTplController, Tpl(tpl_lock_level), data) }
    }
}

impl<T: ?Sized> TplMutex<T> {
    /// Lock the `TplMutex` and return a `TplGuard` object used to access the data. This will raise the system TPL level
    /// to the level specified at `TplMutex` creation.
    ///
    /// # Panics
    ///
    /// Lock re-entrance is not supported; attempt to re-lock something already locked will panic.
    ///
    /// Attempting to acquire the lock while running at a TPL level higher than the lock's TPL level will panic due to
    /// TPL inversion.
    pub fn lock(&self) -> TplGuard<'_, T> {
        self.try_lock().unwrap_or_else(|| panic!("Re-entrant locks for {:?} not permitted.", self.name))
    }

    /// Attempts to lock the `TplMutex`, and if successful, returns a guard object that can be used to access the data.
    ///
    /// # Panics
    ///
    /// Attempting to acquire the lock while running at a TPL level higher than the lock's TPL level will panic due to
    /// TPL inversion.
    pub fn try_lock(&self) -> Option<TplGuard<'_, T>> {
        self.inner.try_lock().ok()
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for TplMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => write!(
                f,
                "TplMutex {{ lock_tpl: {:x?}, release_tpl: {:x?}, data: ",
                self.tpl_lock_level,
                usize::from(guard.release_tpl())
            )
            .and_then(|()| (*guard).fmt(f))
            .and_then(|()| write!(f, " }}")),
            None => write!(f, "TplMutex {{ lock_tpl: {:x?}, data: <locked> }}", self.tpl_lock_level),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {

    use crate::{
        events::{raise_tpl, restore_tpl},
        test_support,
    };

    use super::TplMutex;
    use patina::standard::efi;

    fn with_reset_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        let result = crate::test_support::with_global_lock(|| {
            test_support::init_test_logger();
            raise_tpl(efi::TPL_HIGH_LEVEL);
            restore_tpl(efi::TPL_APPLICATION);

            let _guard = test_support::StateGuard::new(|| {
                raise_tpl(efi::TPL_HIGH_LEVEL);
                restore_tpl(efi::TPL_APPLICATION);
            });

            f();
        });
        match result {
            Ok(()) => {}
            Err(e) => {
                std::panic::resume_unwind(e);
            }
        }
    }

    #[test]
    fn test_tpl_mutex_basic_and_try_lock() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 42, "test_lock");
            {
                let mut guard = lock.try_lock().expect("Failed to acquire lock");
                assert_eq!(*guard, 42);
                *guard = 43;
                assert!(lock.try_lock().is_none(), "Should not acquire a lock while it is already held");
            }
            let guard = lock.try_lock().expect("Failed to acquire lock after release");
            assert_eq!(*guard, 43);
        });
    }

    #[test]
    #[should_panic(expected = "Re-entrant locks for \"test_lock\" not permitted.")]
    fn test_tpl_mutex_reentrant_lock_displays_name() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 42, "test_lock");
            let _guard1 = lock.lock();
            let _guard2 = lock.lock(); // This should panic
        });
    }

    #[test]
    fn test_tpl_mutex_preserves_core_debug_format() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 42, "test_lock");
            let debug_str = format!("{lock:?}");
            assert_eq!(debug_str, "TplMutex { lock_tpl: 10, release_tpl: 4, data: 42 }");
            let _guard = lock.lock();
            let debug_str_locked = format!("{lock:?}");
            assert_eq!(debug_str_locked, "TplMutex { lock_tpl: 10, data: <locked> }");
        });
    }
}
