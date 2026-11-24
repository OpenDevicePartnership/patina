//! Debug and diagnostics facilities for Patina.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub mod log;

#[doc(hidden)]
pub use ::log::error as __log_error;

/// Logs an error message and fires a [`debug_assert!`]`(false)` with the same message.
///
/// In debug builds, this will panic after logging. In release builds, only the [`::log::error!`] is emitted.
/// Callers do not need a direct dependency on the `log` crate.
///
/// # Parameters
///
/// - `$($arg)*`: Format string and arguments, passed directly to [`::log::error!`] and [`debug_assert!`].
///
/// # Example
///
/// ```rust ignore
/// use patina::log_debug_assert;
///
/// log_debug_assert!("unexpected state: value was {}", value);
/// ```
#[macro_export]
macro_rules! log_debug_assert {
    ($($arg:tt)*) => {{
        $crate::debug::__log_error!($($arg)*);
        debug_assert!(false, $($arg)*);
    }};
}

/// Gives a `&'static str` that is the name of the containing function.
///
/// # Example
///
/// ```rust
/// fn demo_fn() -> &'static str {
///     use patina::function;
///
///     function!()
/// }
///
/// assert!(demo_fn().ends_with("demo_fn"));
/// ```
#[macro_export]
macro_rules! function {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            core::any::type_name::<T>()
        }
        let name = type_name_of(f);
        name.strip_suffix("::f").unwrap()
    }};
}
