//! UEFI Task Priority Level (TPL) Locking support
//!
//! This module provides a Mutex implementation based on UEFI TPL levels.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
use core::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use r_efi::efi;

static BOOT_SERVICES_PTR: AtomicPtr<efi::BootServices> = AtomicPtr::new(core::ptr::null_mut());

/// Called to initialize the global TplLock BootServices pointer. Prior to this call, TPL locks are collapsed to a basic
/// lock with no TPL interaction. Afterwards, all TPL locks will adjust TPL according to the TPL they were initialized
/// with.
///
// Design Note: While it would be preferable to avoid a global static BOOT_SERVICES_PTR, the alternative would require
// boot services to be available whenever a new lock is instantiated. This would have two drawbacks: 1) it would mean
// that lock instantiation could not be `const` - and therefore could not be used to easily initialize global locked
// statics (which is a primary use case for this crate), and 2) it would mean that locks could not be instantiated
// before boot services creation. Since these locks are used in many of the structures that are used to implement boot
// services, this would introduce a cyclical dependency.
pub fn init_boot_services(boot_services: *mut efi::BootServices) {
    BOOT_SERVICES_PTR.store(boot_services, Ordering::SeqCst);
}

fn boot_services() -> Option<&'static mut efi::BootServices> {
    let boot_services_ptr = BOOT_SERVICES_PTR.load(Ordering::SeqCst);
    unsafe { boot_services_ptr.as_mut() }
}

/// Used to guard data with a locked MUTEX and TPL level.
pub struct TplMutex<T: ?Sized> {
    tpl_lock_level: efi::Tpl,
    lock: AtomicBool,
    name: &'static str,
    data: UnsafeCell<T>,
}
/// Wrapper for guarded data, which can be accessed by Deref or DerefMut on this object.
pub struct TplGuard<'a, T: ?Sized + 'a> {
    release_tpl: Option<efi::Tpl>,
    lock: &'a AtomicBool,
    name: &'static str,
    data: *mut T,
}

unsafe impl<T: ?Sized + Send> Sync for TplMutex<T> {}
unsafe impl<T: ?Sized + Send> Send for TplMutex<T> {}

unsafe impl<T: ?Sized + Sync> Sync for TplGuard<'_, T> {}
unsafe impl<T: ?Sized + Send> Send for TplGuard<'_, T> {}

impl<T> TplMutex<T> {
    /// Instantiates a new TplMutex with the given TPL level, data object, and name string.
    pub const fn new(tpl_lock_level: efi::Tpl, data: T, name: &'static str) -> Self {
        Self { tpl_lock_level, lock: AtomicBool::new(false), data: UnsafeCell::new(data), name }
    }
}

impl<T: ?Sized> TplMutex<T> {
    /// Lock the TplMutex and return a TplGuard object used to access the data. This will raise the system TPL level
    /// to the level specified at TplMutex creation.
    ///
    /// Safety: Lock reentrance is not supported; attempt to re-lock something already locked will panic.
    pub fn lock(&self) -> TplGuard<'_, T> {
        self.try_lock().unwrap_or_else(|| panic!("Re-entrant locks for {:?} not permitted.", self.name))
    }

    /// Attempts to lock the TplMutex, and if successful, returns a guard object that can be used to access the data.
    pub fn try_lock(&self) -> Option<TplGuard<'_, T>> {
        let boot_services = boot_services();
        let release_tpl = boot_services.as_ref().map(|bs| (bs.raise_tpl)(self.tpl_lock_level));
        if self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Some(TplGuard { release_tpl, lock: &self.lock, name: self.name, data: unsafe { &mut *self.data.get() } })
        } else {
            if let Some(release_tpl) = release_tpl
                && let Some(bs) = boot_services
            {
                (bs.restore_tpl)(release_tpl);
            }
            None
        }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for TplMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => write!(f, "Mutex {{ data: ").and_then(|()| (*guard).fmt(f)).and_then(|()| write!(f, "}}")),
            None => write!(f, "Mutex {{ <locked> }}"),
        }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for TplGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for TplGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized> Deref for TplGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &'a T {
        //Safety: data is only accessible through the lock, which can only be obtained at the specified TPL.
        unsafe { &*self.data }
    }
}

impl<'a, T: ?Sized> DerefMut for TplGuard<'a, T> {
    fn deref_mut(&mut self) -> &'a mut T {
        //Safety: data is only accessible through the lock, which can only be obtained at the specified TPL.
        unsafe { &mut *self.data }
    }
}

impl<T: ?Sized> Drop for TplGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
        if let Some(tpl) = self.release_tpl {
            let bs = boot_services()
                .unwrap_or_else(|| panic!("Valid release TPL for {:?}, but invalid Boot Services", self.name));
            (bs.restore_tpl)(tpl);
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use std::{boxed::Box, println};

    use crate::test_support;

    use super::{TplMutex, init_boot_services};
    use core::{
        mem::MaybeUninit,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use r_efi::efi;

    static TPL: AtomicUsize = AtomicUsize::new(efi::TPL_APPLICATION);

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            f();
            //ensure that TPL mutex doesn't end up with partially initialized
            //mock boot services - otherwise tests for unrelated implementations that
            //use TplMutex might end up calling the mocks unexpectedly.
            init_boot_services(core::ptr::null_mut());
        })
        .unwrap();
    }

    extern "efiapi" fn mock_raise_tpl(new_tpl: efi::Tpl) -> efi::Tpl {
        let prev_tpl = TPL.load(Ordering::SeqCst);

        assert!(prev_tpl <= new_tpl, "cannot raise tpl to lower than current level.");

        TPL.store(new_tpl, Ordering::SeqCst);
        prev_tpl
    }

    extern "efiapi" fn mock_restore_tpl(new_tpl: efi::Tpl) {
        let prev_tpl = TPL.load(Ordering::SeqCst);
        assert!(prev_tpl >= new_tpl, "cannot restore tpl to higher than current level.");

        TPL.store(new_tpl, Ordering::SeqCst);
    }

    fn mock_boot_services() -> *mut efi::BootServices {
        let boot_services = MaybeUninit::zeroed();
        let mut boot_services: efi::BootServices = unsafe { boot_services.assume_init() };
        boot_services.raise_tpl = mock_raise_tpl;
        boot_services.restore_tpl = mock_restore_tpl;
        Box::into_raw(Box::new(boot_services))
    }

    #[test]
    fn tpl_mutex_can_be_created() {
        with_locked_state(|| {
            let tpl_mutex = TplMutex::new(efi::TPL_HIGH_LEVEL, 1_usize, "test_lock");
            *tpl_mutex.lock() = 2_usize;
            assert_eq!(2_usize, *tpl_mutex.lock());
        });
    }

    #[test]
    fn tpl_mutex_should_change_tpl_if_bs_available() {
        with_locked_state(|| {
            let boot_services = mock_boot_services();
            let tpl_mutex = TplMutex::new(efi::TPL_NOTIFY, 1_usize, "test_lock");
            init_boot_services(boot_services);

            let guard = tpl_mutex.lock();
            assert_eq!(TPL.load(Ordering::SeqCst), efi::TPL_NOTIFY);
            drop(guard);
            assert_eq!(TPL.load(Ordering::SeqCst), efi::TPL_APPLICATION);
        });
    }

    #[test]
    fn tpl_mutex_and_guard_should_support_debug_and_display() {
        with_locked_state(|| {
            let tpl_mutex = TplMutex::new(efi::TPL_HIGH_LEVEL, 1_usize, "test_lock");
            println!("{tpl_mutex:?}");
            let guard = tpl_mutex.lock();
            println!("{tpl_mutex:?}");
            println!("{guard:?}");
            println!("{guard:}");
        });
    }
}
