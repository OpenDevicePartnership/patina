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
//! Services implementation. [`TplMutex::with_checked_lock`] and [`TplMutex::with_try_lock`]
//! scope access to a closure, releasing the lock and restoring TPL before returning.
//! Guard-based access is also available through [`TplMutex::checked_lock`].
//!
//! ```
//! use patina::uefi::{
//!     boot_services::{StandardBootServices, tpl::Tpl},
//!     tpl_mutex::{TplMutex, TplMutexError},
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
//!     fn increment(&self) -> Result<(), TplMutexError> {
//!         self.state.with_checked_lock(|state| state.value += 1)
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
//!     tpl_mutex::{TplMutex, TplMutexError},
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
//! fn increment() -> Result<(), TplMutexError> {
//!     STATE.with_checked_lock(|state| state.value += 1)
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
    /// Raises the current TPL, checking that the transition is valid.
    ///
    /// # Errors
    /// Returns an error if the controller rejects the transition.
    ///
    /// # Panics
    /// Panics in debug builds if the controller rejects the transition.
    #[allow(clippy::result_unit_err)]
    fn checked_tpl_raise(&self, tpl: Tpl) -> Result<Tpl, ()>;

    /// Restores the current TPL to a previously returned value.
    fn restore_tpl(&self, tpl: Tpl);
}

impl<B: BootServices> TplController for B {
    fn checked_tpl_raise(&self, tpl: Tpl) -> Result<Tpl, ()> {
        Ok(BootServices::raise_tpl(self, tpl))
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

/// A failure to acquire a [`TplMutex`].
///
/// Converts to [`crate::error::EfiError::AccessDenied`] or
/// [`crate::standard::efi::Status::ACCESS_DENIED`]. Services may provide their own
/// conversions when lock failures have a more specific domain meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TplMutexError {
    /// The mutex is already held, including by a reentrant caller.
    AlreadyLocked,
    /// The deferred TPL controller has not been initialized.
    UninitializedController,
    /// The controller rejected the TPL transition, for example due to TPL inversion.
    InvalidTplTransition,
}

impl Display for TplMutexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AlreadyLocked => "TPL mutex is already locked",
            Self::UninitializedController => "TPL mutex controller is not initialized",
            Self::InvalidTplTransition => "TPL mutex requested an invalid TPL transition",
        })
    }
}

impl core::error::Error for TplMutexError {}

#[cfg_attr(coverage, coverage(off))]
impl From<TplMutexError> for crate::error::EfiError {
    fn from(_: TplMutexError) -> Self {
        Self::AccessDenied
    }
}

#[cfg_attr(coverage, coverage(off))]
impl From<TplMutexError> for crate::standard::efi::Status {
    fn from(error: TplMutexError) -> Self {
        crate::error::EfiError::from(error).into()
    }
}

/// Type use for mutual exclusion of data across Tpl (task priority level)
///
/// This mutex will raise the TPL to the specified level when locked, and restore it when the lock is released.
///
/// The mutex owns its TPL controller. SDK callers normally provide an owned
/// `BootServices` instance or a clone if they need to retain a copy.
pub struct TplMutex<T: ?Sized, B: TplController = StandardBootServices> {
    tpl_controller: TplControllerStorage<B>,
    tpl_lock_level: Tpl,
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// RAII implementation of a [`TplMutex`] lock. When this structure is dropped, the lock will be unlocked.
#[must_use = "if unused the TplMutex will immediately unlock"]
pub struct TplMutexGuard<'a, T: ?Sized, B: TplController> {
    tpl_mutex: &'a TplMutex<T, B>,
    tpl_controller: &'a B,
    release_tpl: Tpl,
}

impl<T, B: TplController> TplMutex<T, B> {
    /// Create a new `TplMutex` in an unlocked state.
    /// Takes ownership of the TPL controller.
    pub const fn new(tpl_controller: B, tpl_lock_level: Tpl, data: T) -> Self {
        Self {
            tpl_controller: TplControllerStorage::Ready(tpl_controller),
            tpl_lock_level,
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Create a new `TplMutex` in an unlocked, uninitialized state.
    /// The resulting `TplMutex` will not be usable until its TPL controller is initialized.
    pub const fn new_uninit(tpl_lock_level: Tpl, data: T) -> Self {
        Self {
            tpl_controller: TplControllerStorage::Deferred(OnceCell::new()),
            tpl_lock_level,
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Initialize the TPL controller for this `TplMutex`. This must be called before the mutex can be used.
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
    fn tpl_controller(&self) -> Option<&B> {
        match &self.tpl_controller {
            TplControllerStorage::Ready(controller) => Some(controller),
            TplControllerStorage::Deferred(controller) => controller.get(),
        }
    }

    /// Acquires the mutex, checking that its locking invariants hold.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::try_lock`] in non-debug builds.
    ///
    /// # Panics
    /// Panics in debug builds if acquisition fails.
    pub fn checked_lock(&self) -> Result<TplMutexGuard<'_, T, B>, TplMutexError> {
        let result = self.try_lock();
        debug_assert!(result.is_ok(), "Failed to acquire TPL mutex: {:?}", result.as_ref().err());
        result
    }

    /// Runs `f` while holding the mutex and its acquired TPL.
    ///
    /// The lock is held and the acquired TPL is maintained throughout `f`. Before returning,
    /// the lock is released and the previous TPL is restored.
    /// The closure runs exactly once on success and is not called if acquisition fails.
    /// Its return value is preserved, including any nested `Result`.
    ///
    /// # Errors
    ///
    /// See [`Self::checked_lock`].
    ///
    /// # Panics
    ///
    /// Panics in debug builds if acquisition fails. Propagates panics from `f`.
    ///
    /// # Examples
    ///
    /// ```
    /// use patina::uefi::{
    ///     boot_services::{StandardBootServices, tpl::Tpl},
    ///     tpl_mutex::{TplMutex, TplMutexError},
    /// };
    ///
    /// fn increment(services: StandardBootServices) -> Result<usize, TplMutexError> {
    ///     let mutex = TplMutex::new(services, Tpl::NOTIFY, 0);
    ///     mutex.with_checked_lock(|value| {
    ///         *value += 1;
    ///         *value
    ///     })
    /// }
    /// ```
    ///
    /// References borrowed from the protected data cannot escape the closure:
    ///
    /// ```compile_fail
    /// use patina::uefi::tpl_mutex::TplMutex;
    ///
    /// fn escape(mutex: &TplMutex<usize>) -> &usize {
    ///     mutex.with_checked_lock(|value| &*value).unwrap()
    /// }
    /// ```
    pub fn with_checked_lock<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, TplMutexError> {
        let mut guard = self.checked_lock()?;
        Ok(f(&mut guard))
    }

    /// Runs `f` while holding the mutex and its acquired TPL.
    ///
    /// The lock and TPL lifetime, closure execution, and return-value behavior match
    /// [`Self::with_checked_lock`], but acquisition uses [`Self::try_lock`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_lock`].
    ///
    /// # Panics
    ///
    /// The controller may panic in debug builds if the TPL transition is invalid.
    /// Propagates panics from `f`.
    ///
    /// # Examples
    ///
    /// References borrowed from the protected data cannot escape the closure:
    ///
    /// ```compile_fail
    /// use patina::uefi::tpl_mutex::TplMutex;
    ///
    /// fn escape(mutex: &TplMutex<usize>) -> &mut usize {
    ///     mutex.with_try_lock(|value| value).unwrap()
    /// }
    /// ```
    pub fn with_try_lock<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, TplMutexError> {
        let mut guard = self.try_lock()?;
        Ok(f(&mut guard))
    }

    /// Attempt to lock the mutex and return [`TplMutexGuard`] if the mutex was not locked.
    ///
    /// # Errors
    /// Returns [`TplMutexError::AlreadyLocked`] if the mutex is already locked,
    /// [`TplMutexError::UninitializedController`] if its controller is not initialized,
    /// or [`TplMutexError::InvalidTplTransition`] if the controller rejects the transition.
    /// The lock flag is checked first, so an already-held mutex reports `AlreadyLocked`
    /// even if the TPL transition would also be invalid.
    ///
    /// Contention and an uninitialized controller do not assert. Use [`Self::checked_lock`]
    /// unless work can be deferred or retried after the current owner releases the mutex.
    ///
    /// # Panics
    /// Panics in debug builds if the controller rejects the TPL transition.
    pub fn try_lock(&self) -> Result<TplMutexGuard<'_, T, B>, TplMutexError> {
        if self.lock.load(Ordering::Relaxed) {
            return Err(TplMutexError::AlreadyLocked);
        }

        let tpl_controller = self.tpl_controller().ok_or(TplMutexError::UninitializedController)?;
        let release_tpl =
            tpl_controller.checked_tpl_raise(self.tpl_lock_level).map_err(|()| TplMutexError::InvalidTplTransition)?;

        if self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Ok(TplMutexGuard { release_tpl, tpl_mutex: self, tpl_controller })
        } else {
            tpl_controller.restore_tpl(release_tpl);
            Err(TplMutexError::AlreadyLocked)
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
        self.tpl_controller.restore_tpl(self.release_tpl);
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
            Err(_) => dbg.field("data", &format_args!("<locked>")),
        };
        dbg.finish_non_exhaustive()
    }
}

impl<T: ?Sized + fmt::Debug, B: TplController> fmt::Debug for TplMutexGuard<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display, B: TplController> fmt::Display for TplMutexGuard<'_, T, B> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&**self, f)
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
            write!(f, "{}", self.field)
        }
    }

    fn boot_services() -> MockBootServices {
        let mut boot_services = MockBootServices::new();
        boot_services.expect_raise_tpl().with(eq(Tpl::NOTIFY)).return_const(Tpl::APPLICATION);
        boot_services.expect_restore_tpl().with(eq(Tpl::APPLICATION)).return_const(());
        boot_services
    }

    #[test]
    fn test_tpl_mutex_scoped_locks_hold_lock_and_restore_tpl() {
        for checked in [true, false] {
            let current_tpl = core::cell::Cell::new(Tpl::APPLICATION);
            let mut controller = MockBootServices::new();
            controller.expect_raise_tpl().with(eq(Tpl::NOTIFY)).times(1).return_const(Tpl::APPLICATION);
            controller.expect_restore_tpl().with(eq(Tpl::APPLICATION)).times(1).return_const(());
            // Track transitions as well as their exact number to verify closure boundaries.
            struct TrackingController<'a> {
                inner: MockBootServices,
                current: &'a core::cell::Cell<Tpl>,
            }
            impl TplController for TrackingController<'_> {
                fn checked_tpl_raise(&self, tpl: Tpl) -> Result<Tpl, ()> {
                    let previous = TplController::checked_tpl_raise(&self.inner, tpl)?;
                    self.current.set(tpl);
                    Ok(previous)
                }

                fn restore_tpl(&self, tpl: Tpl) {
                    self.current.set(tpl);
                    TplController::restore_tpl(&self.inner, tpl);
                }
            }
            let mutex = TplMutex::new(TrackingController { inner: controller, current: &current_tpl }, Tpl::NOTIFY, 41);
            let owned = alloc::boxed::Box::new(1);
            let operation = |value: &mut usize| {
                assert_eq!(current_tpl.get(), Tpl::NOTIFY);
                assert_eq!(mutex.with_try_lock(|_| panic!("must not run")), Err(TplMutexError::AlreadyLocked));
                *value += *owned;
                drop(owned);
                Err::<(), _>(*value)
            };
            let result = if checked { mutex.with_checked_lock(operation) } else { mutex.with_try_lock(operation) };
            assert_eq!(result, Ok(Err(42)));
            assert_eq!(current_tpl.get(), Tpl::APPLICATION);
            assert!(!mutex.lock.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn test_tpl_mutex_scoped_locks_skip_closure_on_acquisition_failure() {
        for checked in [true, false] {
            let mutex = TplMutex::<_, MockBootServices>::new_uninit(Tpl::NOTIFY, 42);
            let called = core::cell::Cell::new(false);
            let attempt = || {
                let operation = |_: &mut i32| called.set(true);
                if checked { mutex.with_checked_lock(operation) } else { mutex.with_try_lock(operation) }
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(attempt));
            assert!(!called.get());
            if checked && cfg!(debug_assertions) {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), Err::<(), _>(TplMutexError::UninitializedController));
            }

            mutex.init(boot_services());
            let guard = mutex.checked_lock().unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(attempt));
            assert!(!called.get());
            if checked && cfg!(debug_assertions) {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), Err::<(), _>(TplMutexError::AlreadyLocked));
            }
            drop(guard);
            assert_eq!(mutex.with_try_lock(|value| *value), Ok(42));
        }
    }

    #[test]
    fn test_tpl_mutex_scoped_locks_reject_invalid_tpl_transition() {
        struct RejectController;

        impl TplController for RejectController {
            fn checked_tpl_raise(&self, _: Tpl) -> Result<Tpl, ()> {
                Err(())
            }

            fn restore_tpl(&self, _: Tpl) {
                panic!("rejected acquisition must not restore TPL");
            }
        }

        let mutex = TplMutex::new(RejectController, Tpl::NOTIFY, 42);
        let called = core::cell::Cell::new(false);
        assert_eq!(mutex.with_try_lock(|_| called.set(true)), Err(TplMutexError::InvalidTplTransition));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutex.with_checked_lock(|_| called.set(true))));
        #[cfg(debug_assertions)]
        assert!(result.is_err());
        #[cfg(not(debug_assertions))]
        assert_eq!(result.unwrap(), Err(TplMutexError::InvalidTplTransition));
        assert!(!called.get());
        assert!(!mutex.lock.load(Ordering::Relaxed));
    }

    #[test]
    fn test_tpl_mutex_scoped_locks_release_on_unwind() {
        for checked in [true, false] {
            let mut controller = MockBootServices::new();
            controller.expect_raise_tpl().with(eq(Tpl::NOTIFY)).times(2).return_const(Tpl::APPLICATION);
            controller.expect_restore_tpl().with(eq(Tpl::APPLICATION)).times(2).return_const(());
            let mutex = TplMutex::new(controller, Tpl::NOTIFY, 42);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let operation = |value: &mut usize| {
                    *value = 43;
                    panic!("test closure panic");
                };
                if checked { mutex.with_checked_lock(operation) } else { mutex.with_try_lock(operation) }
            }));
            assert!(result.is_err());
            assert_eq!(mutex.with_try_lock(|value| *value), Ok(43));
        }
    }

    #[test]
    fn test_checked_lock_acquires_and_releases_mutex() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, 42);
        {
            let mut guard = mutex.checked_lock().expect("test mutex should be available");
            *guard = 43;
            assert!(mutex.try_lock().is_err());
        }
        assert_eq!(*mutex.checked_lock().expect("dropping the guard must release the mutex"), 43);
    }

    #[test]
    fn test_checked_lock_rejects_reentrant_acquisition() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, 42);
        let guard = mutex.checked_lock().expect("test mutex should be available");
        #[cfg(debug_assertions)]
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutex.checked_lock())).is_err());
        #[cfg(not(debug_assertions))]
        assert_eq!(mutex.checked_lock().unwrap_err(), TplMutexError::AlreadyLocked);
        assert_eq!(mutex.try_lock().unwrap_err(), TplMutexError::AlreadyLocked);
        drop(guard);
        assert!(mutex.checked_lock().is_ok());
    }

    #[test]
    fn test_checked_lock_rejects_uninitialized_controller() {
        let mutex = TplMutex::<_, MockBootServices>::new_uninit(Tpl::NOTIFY, 42);
        #[cfg(debug_assertions)]
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutex.checked_lock())).is_err());
        #[cfg(not(debug_assertions))]
        assert_eq!(mutex.checked_lock().unwrap_err(), TplMutexError::UninitializedController);
        assert_eq!(mutex.try_lock().unwrap_err(), TplMutexError::UninitializedController);
        mutex.init(boot_services());
        assert!(mutex.checked_lock().is_ok());
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
                matches!(mutex.try_lock(), Err(TplMutexError::AlreadyLocked)),
                "Try lock should not work when there is already a lock guard."
            );
        }

        drop(guard_result);
        let guard_result = mutex.try_lock();
        assert!(guard_result.is_ok(), "Lock should work after the guard has been dropped.");
    }

    #[test]
    fn test_checked_and_try_lock_rejected_tpl_transition_does_not_acquire_mutex() {
        struct RejectOnceController(core::cell::Cell<bool>);

        impl TplController for RejectOnceController {
            fn checked_tpl_raise(&self, tpl: Tpl) -> Result<Tpl, ()> {
                assert_eq!(tpl, Tpl::NOTIFY);
                let result = if self.0.replace(false) { Err(()) } else { Ok(Tpl::APPLICATION) };
                debug_assert!(result.is_ok(), "test controller rejected TPL transition");
                result
            }

            fn restore_tpl(&self, tpl: Tpl) {
                assert_eq!(tpl, Tpl::APPLICATION);
            }
        }

        let mutex = TplMutex::new(RejectOnceController(core::cell::Cell::new(true)), Tpl::NOTIFY, 42);
        #[cfg(debug_assertions)]
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutex.try_lock())).is_err());
        #[cfg(not(debug_assertions))]
        assert_eq!(mutex.try_lock().unwrap_err(), TplMutexError::InvalidTplTransition);
        let guard = mutex.try_lock().expect("rejected transition must leave the mutex unlocked");
        assert_eq!(*guard, 42);
        drop(guard);
        assert!(mutex.try_lock().is_ok());

        mutex.tpl_controller().expect("test controller should be initialized").0.set(true);
        #[cfg(debug_assertions)]
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutex.checked_lock())).is_err());
        #[cfg(not(debug_assertions))]
        assert_eq!(mutex.checked_lock().unwrap_err(), TplMutexError::InvalidTplTransition);
        assert!(mutex.checked_lock().is_ok());
    }

    #[test]
    #[should_panic(expected = "TPL controller already initialized!")]
    fn test_init_panics_when_tpl_controller_is_already_initialized() {
        let mutex = TplMutex::new(MockBootServices::new(), Tpl::NOTIFY, 0);
        mutex.init(MockBootServices::new());
    }

    #[test]
    fn test_try_lock_fails_until_tpl_controller_is_initialized() {
        let mutex = TplMutex::<_, MockBootServices>::new_uninit(Tpl::NOTIFY, 0);
        assert_eq!(mutex.try_lock().unwrap_err(), TplMutexError::UninitializedController);
        assert_eq!(mutex.try_lock().unwrap_err(), TplMutexError::UninitializedController);
        mutex.init(boot_services());
        assert!(mutex.try_lock().is_ok());
    }

    #[test]
    fn test_tpl_mutex_error_display_and_source() {
        for (error, message) in [
            (TplMutexError::AlreadyLocked, "TPL mutex is already locked"),
            (TplMutexError::UninitializedController, "TPL mutex controller is not initialized"),
            (TplMutexError::InvalidTplTransition, "TPL mutex requested an invalid TPL transition"),
        ] {
            assert_eq!(error.to_string(), message);
            assert!(core::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn test_debug_output_for_tpl_mutex() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, TestStruct::default());
        assert_eq!("TplMutex { data: TestStruct { field: 0 }, .. }", format!("{mutex:?}"));
        let _guard = mutex.checked_lock().unwrap();
        assert_eq!("TplMutex { data: <locked>, .. }", format!("{mutex:?}"));
    }

    #[test]
    fn test_display_and_debug_output_for_tpl_mutex_guard() {
        let mutex = TplMutex::new(boot_services(), Tpl::NOTIFY, TestStruct::default());
        let guard = mutex.checked_lock().unwrap();
        assert_eq!("0", format!("{guard}"));
        assert_eq!("TestStruct { field: 0 }", format!("{guard:?}"));
    }
}
