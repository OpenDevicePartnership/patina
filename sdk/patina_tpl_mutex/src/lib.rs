#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::{
    cell::UnsafeCell,
    fmt::{self, Debug, Display},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

use patina_boot_services::{tpl::Tpl, BootServices, StandardBootServices};

/// Type use for mutual exclusion of data across Tpl (task priority level)
pub struct TplMutex<'a, T: ?Sized, B: BootServices = StandardBootServices> {
    boot_services: &'a B,
    tpl_lock_level: Tpl,
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// RAII implementation of a [TplMutex] lock. When this structure is dropped, the lock will be unlocked.
#[must_use = "if unused the TplMutex will immediately unlock"]
pub struct TplMutexGuard<'a, T: ?Sized, B: BootServices> {
    tpl_mutex: &'a TplMutex<'a, T, B>,
    release_tpl: Tpl,
}

impl<'a, T, B: BootServices> TplMutex<'a, T, B> {
    /// Create an new TplMutex in an unlock state.
    pub const fn new(boot_services: &'a B, tpl_lock_level: Tpl, data: T) -> Self {
        Self { boot_services, tpl_lock_level, lock: AtomicBool::new(false), data: UnsafeCell::new(data) }
    }
}

impl<'a, T: ?Sized, B: BootServices> TplMutex<'a, T, B> {
    /// Attempt to lock the mutex and return a [TplMutexGuard] if the mutex was not locked.
    ///
    /// # Panics
    /// This call will panic if the mutex is already locked.
    pub fn lock(&'a self) -> TplMutexGuard<'a, T, B> {
        self.try_lock().map_err(|_| "Re-entrant lock").unwrap()
    }

    /// Attempt to lock the mutex and return [TplMutexGuard] if the mutex was not locked.
    ///
    /// # Errors
    /// If the mutex is already lock, then this call will return [Err].
    #[allow(clippy::result_unit_err)]
    pub fn try_lock(&'a self) -> Result<TplMutexGuard<'a, T, B>, ()> {
        self.lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| TplMutexGuard { release_tpl: self.boot_services.raise_tpl(self.tpl_lock_level), tpl_mutex: self })
            .map_err(|_| ())
    }
}

impl<T: ?Sized, B: BootServices> Drop for TplMutexGuard<'_, T, B> {
    fn drop(&mut self) {
        self.tpl_mutex.boot_services.restore_tpl(self.release_tpl);
        self.tpl_mutex.lock.store(false, Ordering::Release);
    }
}

impl<'a, T: ?Sized, B: BootServices> Deref for TplMutexGuard<'a, T, B> {
    type Target = T;
    fn deref(&self) -> &'a T {
        // SAFETY:
        // `as_ref` is guarantee to have a valid pointer because it come from a UnsafeCell.
        // This also comply to the aliasing rule because it is the only way to get a reference to the data, thus no other mutable reference to this data exist.
        unsafe { self.tpl_mutex.data.get().as_ref::<'a>().unwrap() }
    }
}

impl<'a, T: ?Sized, B: BootServices> DerefMut for TplMutexGuard<'a, T, B> {
    fn deref_mut(&mut self) -> &'a mut T {
        // SAFETY:
        // `as_ref` is guarantee to have a valid pointer because it come from a UnsafeCell.
        // This also comply to the mutability rule because it is the only way to get a reference to the data, thus no other mutable reference to this data exist.
        unsafe { self.tpl_mutex.data.get().as_mut().unwrap() }
    }
}

impl<T: ?Sized + fmt::Debug, B: BootServices> fmt::Debug for TplMutex<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut dbg = f.debug_struct("TplMutex");
        match self.try_lock() {
            Ok(guard) => dbg.field("data", &guard),
            Err(()) => dbg.field("data", &format_args!("<locked>")),
        };
        dbg.finish_non_exhaustive()
    }
}

impl<T: ?Sized + fmt::Debug, B: BootServices> fmt::Debug for TplMutexGuard<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(self.deref(), f)
    }
}

impl<T: ?Sized + fmt::Display, B: BootServices> fmt::Display for TplMutexGuard<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(self.deref(), f)
    }
}

unsafe impl<T: ?Sized + Send, B: BootServices> Sync for TplMutex<'_, T, B> {}
unsafe impl<T: ?Sized + Send, B: BootServices> Send for TplMutex<'_, T, B> {}

unsafe impl<T: ?Sized + Sync, B: BootServices> Sync for TplMutexGuard<'_, T, B> {}

#[cfg(test)]
mod test {
    use super::*;
    use mockall::predicate::*;
    use patina_boot_services::MockBootServices;

    #[derive(Debug, Default)]
    struct TestStruct {
        field: u32,
    }
    impl Display for TestStruct {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", &self.field)
        }
    }

    fn boot_services() -> MockBootServices {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().with(eq(Tpl::NOTIFY)).return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().with(eq(Tpl::APPLICATION)).return_const(());
        boot_services
    }

    #[test]
    fn test_try_lock() {
        let boot_services = boot_services();
        let mutex = TplMutex::new(&boot_services, Tpl::NOTIFY, 0);

        let guard_result = mutex.try_lock();
        assert!(guard_result.is_ok(), "First lock should work.");

        for _ in 0..2 {
            assert!(
                matches!(mutex.try_lock(), Err(())),
                "Try lock should not work when there is already a lock guard."
            );
        }

        drop(guard_result);
        let guard_result = mutex.try_lock();
        assert!(guard_result.is_ok(), "Lock should work after the guard has been dropped.");
    }

    #[test]
    #[should_panic(expected = "Re-entrant lock")]
    fn test_that_locking_a_locked_mutex_with_lock_fn_should_panic() {
        let boot_services = boot_services();
        let mutex = TplMutex::new(&boot_services, Tpl::NOTIFY, TestStruct::default());
        let guard_result = mutex.try_lock();
        assert!(guard_result.is_ok());
        let _ = mutex.lock();
    }

    #[test]
    fn test_debug_output_for_tpl_mutex() {
        let boot_services = boot_services();
        let mutex = TplMutex::new(&boot_services, Tpl::NOTIFY, TestStruct::default());
        assert_eq!("TplMutex { data: TestStruct { field: 0 }, .. }", format!("{mutex:?}"));
        let _guard = mutex.lock();
        assert_eq!("TplMutex { data: <locked>, .. }", format!("{mutex:?}"));
    }

    #[test]
    fn test_display_and_debug_output_for_tpl_mutex_guard() {
        let boot_services = boot_services();
        let mutex = TplMutex::new(&boot_services, Tpl::NOTIFY, TestStruct::default());
        let guard = mutex.lock();
        assert_eq!("0", format!("{guard}"));
        assert_eq!("TestStruct { field: 0 }", format!("{guard:?}"));
    }
}
