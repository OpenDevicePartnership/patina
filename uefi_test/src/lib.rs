//! An UEFI testing framework for on-system unit testing
//!
//! This crate provides a UEFI component that can be registered with the pure rust DXE core that discovers and runs all
//! test cases marked with the `#[uefi_test]` attribute. The component provides multiple configuration options as
//! documented in [`TestRunnerComponent`]. The `#[uefi_test]` attribute provides multiple configuration attributes
//! as documented in [`uefi_test`]. All tests are discovered across all crates used to compile the pure-rust DXE
//! core, so it is important that test providers use the `cfg_attr` attribute to only compile tests in scenarios where
//! they are expected to run.
//!
//! Additionally, this crate provides a set of macros for writing test cases that are similar to the ones provided by
//! the `core` crate, but return an error message instead of panicking.
//!
//! ## Feature Flags
//!
//! - `off`: Will not compile any tests.
//!
//! ## Example
//!
//! ```rust
//! use uefi_test::*;
//! use uefi_component_interface::DxeComponentInterface;
//!
//! let component = TestRunnerComponent::default()
//!   .with_filter("aarch64") // Only run tests with "aarch64" in their name & path (my_crate::aarch64::test)
//!   .debug_mode(true)
//!   .fail_fast(true);
//!
//! #[cfg_attr(target_arch = "aarch64", uefi_test)]
//! fn test_case(_interface: &dyn DxeComponentInterface) -> Result {
//!   u_assert_eq!(1, 1);
//!   Ok(())
//! }
//!
//! #[uefi_test]
//! fn test_case2(_interface: &dyn DxeComponentInterface) -> Result {
//!   u_assert_eq!(1, 1);
//!   Ok(())
//! }
//!
//! #[uefi_test]
//! #[should_fail]
//! fn failing_test_case(_interface: &dyn DxeComponentInterface) -> Result {
//!    u_assert_eq!(1, 2);
//!    Ok(())
//! }
//!
//! #[uefi_test]
//! #[should_fail = "This test failed"]
//! fn failing_test_case_with_msg(_interface: &dyn DxeComponentInterface) -> Result {
//!   u_assert_eq!(1, 2, "This test failed");
//!   Ok(())
//! }
//!
//! #[uefi_test]
//! #[skip]
//! fn skipped_test_case(_interface: &dyn DxeComponentInterface) -> Result {
//!    todo!()
//! }
//!
//! #[uefi_test]
//! #[cfg_attr(not(target_arch = "x86_64"), skip)]
//! fn x86_64_only_test_case(_interface: &dyn DxeComponentInterface) -> Result {
//!   todo!()
//! }
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(test), no_std)]
use uefi_component_interface::{DxeComponent, DxeComponentInterface};
extern crate alloc;
use alloc::vec::Vec;

#[doc(hidden)]
pub use linkme;
// WARNING: this is not a part of the crate's public API and is subject to change at any time.
#[doc(hidden)]
pub mod __private_api;

/// The result type for a test case, an alias for `Result<(), &'static str>`.
pub type Result = core::result::Result<(), &'static str>;

/// A proc-macro that registers the annotated function as a test case to be run by [`TestRunnerComponent`].
///
/// There is a distinct difference between doing a #[cfg_attr(..., skip)] and a
/// #[cfg_attr(..., uefi_test)]. The first still compiles the test case, but skips it at runtime. The second does not
/// compile the test case at all.
///
/// ## Attributes
///
/// - `#[should_fail]`: Indicates that the test is expected to fail. If the test passes, the test runner will log an
///     error.
/// - `#[should_fail = "message"]`: Indicates that the test is expected to fail with the given message. If the test
///     passes or fails with a different message, the test runner will log an error.
/// - `#[skip]`: Indicates that the test should be skipped.
///
/// ## Example
///
/// ```rust
/// use uefi_test::*;
/// use uefi_component_interface::DxeComponentInterface;
///
/// #[uefi_test]
/// fn test_case(_interface: &dyn DxeComponentInterface) -> Result {
///     todo!()
/// }
///
/// #[uefi_test]
/// #[should_fail]
/// fn failing_test_case(_interface: &dyn DxeComponentInterface) -> Result {
///     u_assert_eq!(1, 2);
///     Ok(())
/// }
///
/// #[uefi_test]
/// #[should_fail = "This test failed"]
/// fn failing_test_case_with_msg(_interface: &dyn DxeComponentInterface) -> Result {
///    u_assert_eq!(1, 2, "This test failed");
///    Ok(())
/// }
///
/// #[uefi_test]
/// #[skip]
/// fn skipped_test_case(_interface: &dyn DxeComponentInterface) -> Result {
///    todo!()
/// }
///
/// #[uefi_test]
/// #[cfg_attr(not(target_arch = "x86_64"), skip)]
/// fn x86_64_only_test_case(_interface: &dyn DxeComponentInterface) -> Result {
///   todo!()
/// }
/// ```
pub use uefi_test_macro::uefi_test;

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

/// A component that runs all test cases marked with the `#[uefi_test]` attribute when loaded by the DXE core.
#[derive(Default)]
pub struct TestRunnerComponent {
    filters: Vec<&'static str>,
    debug_mode: bool,
    fail_fast: bool,
}

impl TestRunnerComponent {
    /// Adds a filter that will reduce the tests ran to only those that contain the filter value in their test name.
    ///
    /// The `name` is not just the test name, but also the module path. For example, if a test is defined in
    /// `my_crate::tests`, the name would be `my_crate::tests::test_case`.
    ///
    /// This filter is case-sensitive. It can be called multiple times to add multiple filters.
    pub fn with_filter(mut self, filter: &'static str) -> Self {
        self.filters.push(filter);
        self
    }

    /// Any log messages generated by the test case will be logged if this is set to true.
    ///
    /// Defaults to false.
    pub fn debug_mode(mut self, debug_mode: bool) -> Self {
        self.debug_mode = debug_mode;
        self
    }

    /// If set to true, the test runner will stop running tests after the first failure.
    ///
    /// Defaults to false.
    pub fn fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }
}

impl DxeComponent for TestRunnerComponent {
    fn entry_point(&self, interface: &dyn DxeComponentInterface) -> uefi_sdk::error::Result<()> {
        let test_list = __private_api::test_cases();
        let count = test_list.len();
        match count {
            0 => log::warn!("No Tests Found"),
            1 => log::info!("running 1 test"),
            _ => log::info!("running {} tests", count),
        };

        for test in test_list {
            if !test.should_run(&self.filters) {
                log::info!("{} ... skipped", test.name);
                continue;
            }

            match test.run(interface, self.debug_mode) {
                Ok(_) => log::info!("{} ... ok", test.name),
                Err(e) => {
                    log::error!("{} ... fail: {}", test.name, e);
                    if self.fail_fast {
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TestRunnerComponent;
    use core::ffi::c_void;
    use core::result::Result;
    use r_efi::efi;
    use uefi_component_interface::{DxeComponent, DxeComponentInterface};

    mockall::mock! {
        ComponentInterface {}
        impl DxeComponentInterface for ComponentInterface {
            fn install_protocol_interface(&self, handle: Option<efi::Handle>, protocol: efi::Guid, interface: *mut c_void) -> Result<efi::Handle, efi::Status>;
        }
    }

    // A test function where we mock DxeComponentInterface to return what we want for the test.
    #[allow(unused)]
    fn test_function(interface: &dyn DxeComponentInterface) -> crate::Result {
        match interface.install_protocol_interface(
            None,
            efi::Guid::from_fields(0, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 0]),
            core::ptr::null_mut(),
        ) {
            Ok(_) => Ok(()),
            Err(_) => Err("Failed to install protocol interface"),
        }
    }

    #[test]
    fn verify_default_values() {
        let component = TestRunnerComponent::default();
        assert_eq!(component.filters.len(), 0);
        assert!(!component.debug_mode);
        assert!(!component.fail_fast);
    }

    #[test]
    fn verify_config_sets_properly() {
        let component =
            TestRunnerComponent::default().with_filter("aarch64").with_filter("test").debug_mode(true).fail_fast(true);

        assert_eq!(component.filters.len(), 2);
        assert!(component.debug_mode);
        assert!(component.fail_fast);
    }

    #[cfg_attr(not(feature = "off"), linkme::distributed_slice(super::__private_api::TEST_CASES))]
    #[allow(unused)]
    static TEST_CASE1: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test",
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: test_function,
    };

    #[cfg_attr(not(feature = "off"), linkme::distributed_slice(super::__private_api::TEST_CASES))]
    #[allow(unused)]
    static TEST_CASE2: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test",
        skip: true,
        should_fail: false,
        fail_msg: None,
        func: test_function,
    };

    #[test]
    fn test_we_run_without_panicking() {
        assert_eq!(2, super::__private_api::test_cases().len());
        let component = TestRunnerComponent::default().fail_fast(true);

        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Ok(core::ptr::null_mut()));
        let _ = component.entry_point(&interface);

        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Err(efi::Status::UNSUPPORTED));
        let _ = component.entry_point(&interface);
    }
}
