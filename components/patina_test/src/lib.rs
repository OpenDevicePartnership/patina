#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
)]
#![no_std]
#![feature(coverage_attribute)]
extern crate alloc;

#[doc(hidden)]
pub mod __private_api;
#[doc(hidden)]
pub use linkme;

#[cfg(any(feature = "test-runner", doc))]
pub mod component;

#[cfg(all(not(feature = "test-runner"), not(doc)))]
#[allow(unused)]
mod component;

/// Patina Test Error Definitions.
///
/// Defines Result type that must be returned by all patina test functions.
pub mod error {
    /// The result type for patina tests. All patina test functions must return this type.
    pub type Result = core::result::Result<(), &'static str>;
}

mod service;

pub use patina_macro::patina_test;

/// A macro similar to [`core::assert!`] that returns an error message instead of panicking.
#[macro_export]
macro_rules! u_assert {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            return Err($msg);
        }
    };
    ($cond:expr) => {
        u_assert!($cond, "Assertion failed");
    };
}

/// A macro similar to [`core::assert_eq!`] that returns an error message instead of panicking.
#[macro_export]
macro_rules! u_assert_eq {
    ($left:expr, $right:expr, $msg:expr) => {
        if $left != $right {
            return Err($msg);
        }
    };
    ($left:expr, $right:expr) => {
        u_assert_eq!($left, $right, concat!("assertion failed: `", stringify!($left), " == ", stringify!($right), "`"));
    };
}

/// A macro similar to [`core::assert_ne!`] that returns an error message instead of panicking.
#[macro_export]
macro_rules! u_assert_ne {
    ($left:expr, $right:expr, $msg:expr) => {
        if $left == $right {
            return Err($msg);
        }
    };
    ($left:expr, $right:expr) => {
        u_assert_ne!($left, $right, concat!("assertion failed: `", stringify!($left), " != ", stringify!($right), "`"));
    };
}
