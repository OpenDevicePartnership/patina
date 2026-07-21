//! Task priority level (TPL) services for Patina components.
//!
//! [`TplServices`] exposes the UEFI `RaiseTPL`/`RestoreTPL` boot services. It offers two styles of
//! use:
//!
//! - **Ergonomic (recommended):** [`TplServicesExt::raise`] returns a [`TplGuard`] that
//!   restores the previous TPL when dropped, and [`TplServicesExt::with_raised_tpl`] runs a closure
//!   at a raised TPL.
//! - **Manual:** [`TplServices::raise_tpl`] returns an opaque [`PreviousTpl`] token that is passed
//!   back to [`TplServices::restore_tpl`], for cases where the raise and restore cannot be scoped
//!   to a single lexical block.
//!
//! Raising to a level below the current TPL, or restoring to a level above the current TPL, is a
//! programming error.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// A task priority level, used to serialize access to shared state in the UEFI event model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tpl {
    /// The lowest priority level, used for normal execution (`TPL_APPLICATION`).
    Application,
    /// The priority level for most notification callbacks (`TPL_CALLBACK`).
    Callback,
    /// The priority level for notifications that must not be interrupted by other callbacks
    /// (`TPL_NOTIFY`).
    Notify,
    /// The highest priority level. Disables interrupts for the duration (`TPL_HIGH_LEVEL`).
    HighLevel,
}

/// An opaque token representing the TPL that was active before a raise.
///
/// It is produced by [`TplServices::raise_tpl`] and consumed by [`TplServices::restore_tpl`]. It
/// captures the exact previous level (including intermediate levels), so restoring is always
/// faithful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviousTpl(usize);

impl PreviousTpl {
    /// Wraps a raw TPL value produced by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[doc(hidden)]
    pub fn from_raw(tpl: usize) -> Self {
        Self(tpl)
    }

    /// Returns the raw TPL value for use by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[doc(hidden)]
    pub fn as_raw(&self) -> usize {
        self.0
    }
}

/// Task Priority Level (TPL) Services.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn TplServices>`](crate::component::service::Service) parameter to their entry point.
///
/// Most components should prefer the ergonomic [`TplServicesExt`] methods over calling
/// [`Self::raise_tpl`]/[`Self::restore_tpl`] directly.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait TplServices {
    /// Raises the task priority level to `tpl`, returning a token for the previous level.
    ///
    /// The returned [`PreviousTpl`] must be passed to [`Self::restore_tpl`] to restore the prior
    /// level. Prefer [`TplServicesExt::raise`] or [`TplServicesExt::with_raised_tpl`], which handle
    /// the restore automatically.
    ///
    /// # Panics
    ///
    /// Panics if `tpl` is below the current TPL, matching the UEFI specification.
    fn raise_tpl(&self, tpl: Tpl) -> PreviousTpl;

    /// Restores the task priority level to a previously raised level.
    ///
    /// # Panics
    ///
    /// Panics if `previous` is above the current TPL, matching the UEFI specification.
    fn restore_tpl(&self, previous: PreviousTpl);
}

/// A guard that restores the previous task priority level when dropped.
///
/// Created by [`TplServicesExt::raise`]. While the guard is alive the TPL remains raised. When it
/// is dropped (for example at the end of a block) the previous level is restored.
#[must_use = "the TPL is restored when the guard is dropped; bind it to a variable to keep the TPL raised"]
pub struct TplGuard<'a, T: TplServices + ?Sized> {
    services: &'a T,
    previous: PreviousTpl,
}

impl<T: TplServices + ?Sized> Drop for TplGuard<'_, T> {
    fn drop(&mut self) {
        self.services.restore_tpl(self.previous);
    }
}

/// Ergonomic extension methods for [`TplServices`].
///
/// Implemented for every [`TplServices`] implementor (including
/// [`Service<dyn TplServices>`](crate::component::service::Service)).
///
/// # Examples
///
/// ```rust,no_run
/// use patina::component::service::{Service, uefi_services::tpl::{TplServices, TplServicesExt, Tpl}};
/// use patina::error::Result;
///
/// fn entry_point(tpl: Service<dyn TplServices>) -> Result<()> {
///     // Scoped raise via a guard.
///     {
///         let _guard = tpl.raise(Tpl::Notify);
///         // ... critical section runs at TPL_NOTIFY ...
///     } // previous TPL restored here
///
///     // Or run a closure at a raised TPL.
///     let value = tpl.with_raised_tpl(Tpl::Notify, || 42);
///     assert_eq!(value, 42);
///     Ok(())
/// }
/// ```
pub trait TplServicesExt: TplServices {
    /// Raises the TPL to `tpl` and returns a [`TplGuard`] that restores it when dropped.
    fn raise(&self, tpl: Tpl) -> TplGuard<'_, Self> {
        let previous = self.raise_tpl(tpl);
        TplGuard { services: self, previous }
    }

    /// Runs `f` with the TPL raised to `tpl`, restoring the previous level afterward.
    fn with_raised_tpl<R>(&self, tpl: Tpl, f: impl FnOnce() -> R) -> R {
        let _guard = self.raise(tpl);
        f()
    }
}

impl<T: TplServices + ?Sized> TplServicesExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    #[test]
    fn test_tpl_services_previous_previous_raw_is_correct() {
        let previous = PreviousTpl::from_raw(16);
        assert_eq!(previous.as_raw(), 16);
    }

    #[test]
    fn test_tpl_services_manual_raise_restore() {
        let mut mock = MockTplServices::new();
        mock.expect_raise_tpl().times(1).returning(|tpl| {
            assert_eq!(tpl, Tpl::Notify);
            PreviousTpl::from_raw(4)
        });
        mock.expect_restore_tpl().times(1).returning(|previous| {
            assert_eq!(previous.as_raw(), 4);
        });

        let previous = mock.raise_tpl(Tpl::Notify);
        mock.restore_tpl(previous);
    }

    #[test]
    fn test_tpl_services_guard_restores_on_drop() {
        let mut mock = MockTplServices::new();
        mock.expect_raise_tpl().times(1).returning(|_| PreviousTpl::from_raw(8));
        mock.expect_restore_tpl().times(1).returning(|previous| {
            assert_eq!(previous.as_raw(), 8);
        });

        {
            let _guard = mock.raise(Tpl::HighLevel);
            // Guard is alive here. Restore has not been called yet.
        }
        // Guard dropped. Restore has now been called (verified by mock expectations).
    }

    #[test]
    fn test_tpl_services_with_raised_tpl_runs_closure() {
        let mut mock = MockTplServices::new();
        mock.expect_raise_tpl().times(1).returning(|_| PreviousTpl::from_raw(8));
        mock.expect_restore_tpl().times(1).returning(|_| {});

        let ran = Cell::new(false);
        let result = mock.with_raised_tpl(Tpl::Callback, || {
            ran.set(true);
            123
        });
        assert!(ran.get());
        assert_eq!(result, 123);
    }
}
