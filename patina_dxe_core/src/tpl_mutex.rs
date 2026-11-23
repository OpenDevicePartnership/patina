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

use crate::events::{checked_tpl_raise, restore_tpl};

pub use patina::uefi::tpl_mutex::TplMutexError;

#[derive(Clone, Copy)]
pub(crate) struct CoreTplController;

impl TplController for CoreTplController {
    fn checked_tpl_raise(&self, tpl: Tpl) -> Result<Tpl, ()> {
        checked_tpl_raise(tpl.into()).map(Into::into).ok_or(())
    }

    fn restore_tpl(&self, tpl: Tpl) {
        restore_tpl(tpl.into());
    }
}

/// Used to guard data with a locked MUTEX and TPL level.
pub struct TplMutex<T: ?Sized> {
    tpl_lock_level: efi::Tpl,
    inner: SharedTplMutex<T, CoreTplController>,
}

/// Wrapper for guarded data, which can be accessed by Deref or `DerefMut` on this object.
pub type TplGuard<'a, T> = TplMutexGuard<'a, T, CoreTplController>;

impl<T> TplMutex<T> {
    /// Instantiates a new `TplMutex` with the given TPL level, data object, and name string.
    pub const fn new(tpl_lock_level: efi::Tpl, data: T, _name: &'static str) -> Self {
        Self { tpl_lock_level, inner: SharedTplMutex::new(CoreTplController, Tpl(tpl_lock_level), data) }
    }
}

impl<T: ?Sized> TplMutex<T> {
    /// Acquires the mutex, checking that its locking invariants hold.
    ///
    /// Returns an error identifying whether the mutex is locked or the TPL transition is invalid.
    ///
    /// # Panics
    /// Panics in debug builds if acquisition fails.
    pub fn checked_lock(&self) -> Result<TplGuard<'_, T>, TplMutexError> {
        self.inner.checked_lock()
    }

    /// Attempts to lock the `TplMutex`, and if successful, returns a guard object that can be used to access the data.
    ///
    /// Returns an error identifying whether the mutex is locked or the requested TPL transition is invalid.
    ///
    /// # Panics
    /// Panics in debug builds if the requested TPL transition is invalid.
    pub fn try_lock(&self) -> Result<TplGuard<'_, T>, TplMutexError> {
        self.inner.try_lock()
    }

    /// Runs `f` while holding the mutex and its acquired TPL.
    ///
    /// Releases the lock and restores the previous TPL before returning.
    /// The closure is not called if acquisition fails.
    /// See [`patina::uefi::tpl_mutex::TplMutex::with_checked_lock`].
    ///
    /// # Errors
    ///
    /// See [`Self::checked_lock`].
    ///
    /// # Panics
    ///
    /// Panics in debug builds if acquisition fails. Propagates panics from `f`.
    pub fn with_checked_lock<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, TplMutexError> {
        self.inner.with_checked_lock(f)
    }

    /// Runs `f` while holding the mutex and its acquired TPL.
    ///
    /// Releases the lock and restores the previous TPL before returning.
    /// The closure is not called if acquisition fails.
    /// See [`patina::uefi::tpl_mutex::TplMutex::with_try_lock`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_lock`].
    ///
    /// # Panics
    ///
    /// Panics in debug builds if the requested TPL transition is invalid.
    /// Propagates panics from `f`.
    #[allow(dead_code)] // Preserve SDK API parity even when Core callers only use checked acquisition.
    pub fn with_try_lock<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, TplMutexError> {
        self.inner.with_try_lock(f)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for TplMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Ok(guard) => write!(
                f,
                "TplMutex {{ lock_tpl: {:x?}, release_tpl: {:x?}, data: ",
                self.tpl_lock_level,
                usize::from(guard.release_tpl())
            )
            .and_then(|()| (*guard).fmt(f))
            .and_then(|()| write!(f, " }}")),
            Err(_) => write!(f, "TplMutex {{ lock_tpl: {:x?}, data: <locked> }}", self.tpl_lock_level),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {

    use crate::{
        events::{checked_tpl_raise, restore_tpl},
        test_support,
    };

    use super::{TplMutex, TplMutexError};
    use patina::standard::efi;

    fn with_reset_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        let result = crate::test_support::with_global_lock(|| {
            test_support::init_test_logger();
            checked_tpl_raise(efi::TPL_HIGH_LEVEL).expect("test TPL raise should succeed");
            restore_tpl(efi::TPL_APPLICATION);

            let _guard = test_support::StateGuard::new(|| {
                checked_tpl_raise(efi::TPL_HIGH_LEVEL).expect("test TPL raise should succeed");
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
    fn test_tpl_mutex_scoped_locks() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 41, "test_lock");
            assert_eq!(
                lock.with_checked_lock(|value| {
                    assert_eq!(checked_tpl_raise(efi::TPL_HIGH_LEVEL), Some(efi::TPL_NOTIFY));
                    restore_tpl(efi::TPL_NOTIFY);
                    assert_eq!(
                        lock.with_try_lock(|_| panic!("must not run")),
                        Err::<(), _>(TplMutexError::AlreadyLocked)
                    );
                    *value += 1;
                    *value
                }),
                Ok(42)
            );
            assert_eq!(checked_tpl_raise(efi::TPL_HIGH_LEVEL), Some(efi::TPL_APPLICATION));
            restore_tpl(efi::TPL_APPLICATION);
            assert_eq!(lock.with_try_lock(|value| *value), Ok(42));
            assert_eq!(checked_tpl_raise(efi::TPL_HIGH_LEVEL), Some(efi::TPL_APPLICATION));
        });
    }

    #[test]
    fn test_tpl_mutex_basic_and_try_lock() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 42, "test_lock");
            {
                let mut guard = lock.checked_lock().expect("Failed to acquire lock");
                assert_eq!(*guard, 42);
                *guard = 43;
                assert_eq!(lock.try_lock().unwrap_err(), TplMutexError::AlreadyLocked);
            }
            let guard = lock.checked_lock().expect("Failed to acquire lock after release");
            assert_eq!(*guard, 43);
        });
    }

    #[test]
    fn test_tpl_mutex_reentrant_lock_returns_already_locked() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 42, "test_lock");
            let guard = lock.checked_lock().expect("test mutex should be available");
            #[cfg(debug_assertions)]
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock.checked_lock())).is_err());
            #[cfg(not(debug_assertions))]
            assert_eq!(lock.checked_lock().unwrap_err(), TplMutexError::AlreadyLocked);
            assert_eq!(lock.try_lock().unwrap_err(), TplMutexError::AlreadyLocked);
            drop(guard);
            assert!(lock.checked_lock().is_ok());
        });
    }

    #[test]
    fn test_tpl_mutex_tpl_inversion_rejected() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_CALLBACK, 42, "test_lock");
            checked_tpl_raise(efi::TPL_NOTIFY).expect("test TPL raise should succeed");
            #[cfg(debug_assertions)]
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock.checked_lock())).is_err());
            #[cfg(not(debug_assertions))]
            assert_eq!(lock.checked_lock().unwrap_err(), TplMutexError::InvalidTplTransition);
            #[cfg(debug_assertions)]
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock.try_lock())).is_err());
            #[cfg(not(debug_assertions))]
            assert_eq!(lock.try_lock().unwrap_err(), TplMutexError::InvalidTplTransition);
            assert_eq!(checked_tpl_raise(efi::TPL_HIGH_LEVEL), Some(efi::TPL_NOTIFY));
            restore_tpl(efi::TPL_APPLICATION);
            assert!(lock.checked_lock().is_ok());
        });
    }

    #[test]
    fn test_tpl_mutex_preserves_core_debug_format() {
        with_reset_state(|| {
            let lock = TplMutex::new(efi::TPL_NOTIFY, 42, "test_lock");
            let debug_str = format!("{lock:?}");
            assert_eq!(debug_str, "TplMutex { lock_tpl: 10, release_tpl: 4, data: 42 }");
            let _guard = lock.checked_lock().expect("test mutex should be available");
            let debug_str_locked = format!("{lock:?}");
            assert_eq!(debug_str_locked, "TplMutex { lock_tpl: 10, data: <locked> }");
        });
    }
}
