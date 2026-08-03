//! A module containing a TPL aware Mutex implementation.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
//! ## Examples
//!
//! SDK consumers normally construct a [`TplMutex`] with an owned or cloned Boot
//! Services implementation and access the protected data through the guard
//! returned by [`TplMutex::lock`].
//!
//! ```
//! use patina::uefi::{
//!     boot_services::{StandardBootServices, tpl::Tpl},
//!     tpl_mutex::TplMutex,
//! };
//!
//! struct SharedState {
//!     value: usize,
//! }
//!
//! struct Consumer {
//!     state: TplMutex<SharedState>,
//!     boot_services: StandardBootServices,
//! }
//!
//! impl Consumer {
//!     fn new(boot_services: StandardBootServices) -> Self {
//!         Self {
//!             state: TplMutex::new(
//!                 boot_services.clone(),
//!                 Tpl::NOTIFY,
//!                 SharedState { value: 0 },
//!             ),
//!             boot_services,
//!         }
//!     }
//!
//!     fn increment(&self) {
//!         self.state.lock().value += 1;
//!     }
//! }
//! ```
//!
//! ### Delayed initialization
//!
//! A static mutex can be constructed before Boot Services are available and
//! initialized later. [`TplMutex::init`] must be called exactly once before the
//! mutex is locked.
//!
//! ```
//! use patina::uefi::{
//!     boot_services::{StandardBootServices, tpl::Tpl},
//!     tpl_mutex::TplMutex,
//! };
//!
//! struct SharedState {
//!     value: usize,
//! }
//!
//! static STATE: TplMutex<SharedState> =
//!     TplMutex::new_uninit(Tpl::NOTIFY, SharedState { value: 0 });
//!
//! fn initialize(boot_services: StandardBootServices) {
//!     STATE.init(boot_services);
//! }
//!
//! fn increment() {
//!     STATE.lock().value += 1;
//! }
//! ```
//!
use core::{
    cell::{OnceCell, UnsafeCell},
    fmt::{self, Debug, Display},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::uefi::boot_services::{BootServices, StandardBootServices, tpl::Tpl};

/// Provides the TPL operations required by [`TplMutex`].
///
/// [`BootServices`] implementations automatically implement this trait. Other
/// execution environments, such as the DXE Core, may implement this trait to
/// provide TPL operations without depending on Boot Services. This abstraction
/// is intended to support execution environments with different requirements
/// for accessing TPL primitives; SDK consumers should normally use the blanket
/// implementation provided for [`BootServices`].
pub trait TplController {
    /// Raises the current TPL and returns the previous TPL.
    fn raise_tpl(&self, tpl: Tpl) -> Tpl;

    /// Restores the current TPL to a previously returned value.
    fn restore_tpl(&self, tpl: Tpl);
}

impl<B: BootServices> TplController for B {
    fn raise_tpl(&self, tpl: Tpl) -> Tpl {
        BootServices::raise_tpl(self, tpl)
    }

    fn restore_tpl(&self, tpl: Tpl) {
        BootServices::restore_tpl(self, tpl);
    }
}

// `OnceCell::from` and `OnceCell::set` are not `const`. This enumeration
// allows `const` construction of TplMutex by bypassing the need for non-const
// `OnceCell` APIs using the `Ready` variant in the case where TplController is
// available at construction.
enum TplControllerStorage<C> {
    Ready(C),
    Deferred(OnceCell<C>),
}

/// Type use for mutual exclusion of data across Tpl (task priority level)
///
/// This mutex will raise the TPL to the specified level when locked, and restore it when the lock is released.
///
/// The mutex owns its TPL controller. SDK callers normally provide an owned
/// BootServices instance or a clone if they need to retain a copy.
pub struct TplMutex<T: ?Sized, B: TplController = StandardBootServices> {
    tpl_controller: TplControllerStorage<B>,
    tpl_lock_level: Tpl,
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// RAII implementation of a [TplMutex] lock. When this structure is dropped, the lock will be unlocked.
#[must_use = "if unused the TplMutex will immediately unlock"]
pub struct TplMutexGuard<'a, T: ?Sized, B: TplController> {
    tpl_mutex: &'a TplMutex<T, B>,
    release_tpl: Tpl,
}

impl<T, B: TplController> TplMutex<T, B> {
    /// Create a new TplMutex in an unlocked state.
    /// Takes ownership of the TPL controller.
    pub const fn new(tpl_controller: B, tpl_lock_level: Tpl, data: T) -> Self {
        Self {
            tpl_controller: TplControllerStorage::Ready(tpl_controller),
            tpl_lock_level,
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Create a new TplMutex in an unlocked, uninitialized state.
    /// The resulting TplMutex will not be usable until its TPL controller is initialized.
    pub const fn new_uninit(tpl_lock_level: Tpl, data: T) -> Self {
        Self {
            tpl_controller: TplControllerStorage::Deferred(OnceCell::new()),
            tpl_lock_level,
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Initialize the TPL controller for this TplMutex. This must be called before the mutex can be used.
    ///
    /// # Panics
    /// This call will panic if the mutex is already initialized.
    pub fn init(&self, tpl_controller: B) {
        match &self.tpl_controller {
            TplControllerStorage::Ready(_) => Err(tpl_controller),
            TplControllerStorage::Deferred(controller) => controller.set(tpl_controller),
        }
        .map_err(|_| "TPL controller already initialized!")
        .unwrap();
    }
}

impl<T: ?Sized, B: TplController> TplMutex<T, B> {
    fn tpl_controller(&self) -> &B {
        match &self.tpl_controller {
            TplControllerStorage::Ready(controller) => controller,
            TplControllerStorage::Deferred(controller) => controller.get().expect("TPL controller not initialized!"),
        }
    }

    /// Attempt to lock the mutex and return a [TplMutexGuard] if the mutex was not locked.
    ///
    /// # Panics
    /// This call will panic if the mutex is already locked.
    pub fn lock(&self) -> TplMutexGuard<'_, T, B> {
        self.try_lock().map_err(|_| "Re-entrant lock").unwrap()
    }

    /// Attempt to lock the mutex and return [TplMutexGuard] if the mutex was not locked.
    ///
    /// # Errors
    /// If the mutex is already lock, then this call will return [Err].
    ///
    /// # Panics
    /// This call will panic if the mutex is not initialized.
    #[allow(clippy::result_unit_err)]
    pub fn try_lock(&self) -> Result<TplMutexGuard<'_, T, B>, ()> {
        if self.lock.load(Ordering::Relaxed) {
            return Err(());
        }

        let tpl_controller = self.tpl_controller();
        let release_tpl = tpl_controller.raise_tpl(self.tpl_lock_level);

        if self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Ok(TplMutexGuard { release_tpl, tpl_mutex: self })
        } else {
            tpl_controller.restore_tpl(release_tpl);
            Err(())
        }
    }
}

impl<T: ?Sized, B: TplController> TplMutexGuard<'_, T, B> {
    /// Returns the TPL that will be restored when this guard is dropped.
    pub fn release_tpl(&self) -> Tpl {
        self.release_tpl
    }
}

impl<T: ?Sized, B: TplController> Drop for TplMutexGuard<'_, T, B> {
    fn drop(&mut self) {
        self.tpl_mutex.lock.store(false, Ordering::Release);
        self.tpl_mutex.tpl_controller().restore_tpl(self.release_tpl);
    }
}

impl<T: ?Sized, B: TplController> Deref for TplMutexGuard<'_, T, B> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY:
        // `as_ref` is guarantee to have a valid pointer because it come from a UnsafeCell.
        // This also comply to the aliasing rule because it is the only way to get a reference to the data, thus no other mutable reference to this data exist.
        unsafe { self.tpl_mutex.data.get().as_ref().unwrap() }
    }
}

impl<T: ?Sized, B: TplController> DerefMut for TplMutexGuard<'_, T, B> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY:
        // `as_ref` is guarantee to have a valid pointer because it come from a UnsafeCell.
        // This also comply to the mutability rule because it is the only way to get a reference to the data, thus no other mutable reference to this data exist.
        unsafe { self.tpl_mutex.data.get().as_mut().unwrap() }
    }
}

impl<T: ?Sized + fmt::Debug, B: TplController> fmt::Debug for TplMutex<T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut dbg = f.debug_struct("TplMutex");
        match self.try_lock() {
            Ok(guard) => dbg.field("data", &guard),
            Err(()) => dbg.field("data", &format_args!("<locked>")),
        };
        dbg.finish_non_exhaustive()
    }
}

impl<T: ?Sized + fmt::Debug, B: TplController> fmt::Debug for TplMutexGuard<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(self.deref(), f)
    }
}

impl<T: ?Sized + fmt::Display, B: TplController> fmt::Display for TplMutexGuard<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(self.deref(), f)
    }
}

// SAFETY: TplMutex is Sync because it ensures exclusive access to T through TPL-based locking.
// The lock/unlock operations at TPL_HIGH_LEVEL prevent concurrent access. T must be Send to
// allow transfer between threads, and the mutex ensures only one thread accesses T at a time.
unsafe impl<T: ?Sized + Send, B: TplController + Send> Sync for TplMutex<T, B> {}
// SAFETY: TplMutex is Send because it owns T (which is Send) and uses TPL locking to ensure
// thread-safe access. The mutex can be safely transferred between threads.
unsafe impl<T: ?Sized + Send, B: TplController + Send> Send for TplMutex<T, B> {}

// SAFETY: TplMutexGuard is Sync when T is Sync because the guard represents exclusive access
// to T through the TPL mutex. The guard can be shared across threads safely.
unsafe impl<T: ?Sized + Sync, B: TplController> Sync for TplMutexGuard<'_, T, B> {}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::uefi::boot_services::MockBootServices;
    use mockall::predicate::*;

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
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().with(eq(Tpl::NOTIFY)).times(2).return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().with(eq(Tpl::APPLICATION)).times(2).return_const(());
        let mutex = TplMutex::new(boot_services, Tpl::NOTIFY, 0);

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
    #[should_panic(expected = "TPL controller already initialized!")]
    fn test_init_panics_when_tpl_controller_is_already_initialized() {
        let mutex = TplMutex::new(MockBootServices::new(), Tpl::NOTIFY, 0);
        mutex.init(MockBootServices::new());
    }

    #[test]
    #[should_panic(expected = "TPL controller not initialized!")]
    fn test_lock_panics_when_tpl_controller_is_not_initialized() {
        let mutex = TplMutex::<_, MockBootServices>::new_uninit(Tpl::NOTIFY, 0);
        let _guard = mutex.lock();
    }

    #[test]
    #[should_panic(expected = "Re-entrant lock")]
    fn test_that_locking_a_locked_mutex_with_lock_fn_should_panic() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, TestStruct::default());
        let guard_result = mutex.try_lock();
        assert!(guard_result.is_ok());
        let _ = mutex.lock();
    }

    #[test]
    fn test_debug_output_for_tpl_mutex() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, TestStruct::default());
        assert_eq!("TplMutex { data: TestStruct { field: 0 }, .. }", format!("{mutex:?}"));
        let _guard = mutex.lock();
        assert_eq!("TplMutex { data: <locked>, .. }", format!("{mutex:?}"));
    }

    #[test]
    fn test_display_and_debug_output_for_tpl_mutex_guard() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, TestStruct::default());
        let guard = mutex.lock();
        assert_eq!("0", format!("{guard}"));
        assert_eq!("TestStruct { field: 0 }", format!("{guard:?}"));
    }
}
