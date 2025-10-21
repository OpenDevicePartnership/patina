//! An UEFI testing framework for on-system unit testing
//!
//! This module provides a UEFI component that can be registered with the pure rust DXE core that discovers and runs all
//! test cases marked with the `#[patina_test]` attribute. The component provides multiple configuration options as
//! documented in [TestRunner] object. The `#[patina_test]` attribute provides multiple configuration attributes
//! as documented in [`patina_test`]. All tests are discovered across all crates used to compile the pure-rust DXE
//! core, so it is important that test providers use the `cfg_attr` attribute to only compile tests in scenarios where
//! they are expected to run.
//!
//! Additionally, this module provides a set of macros for writing test cases that are similar to the ones provided by
//! the `core` crate, but return an error message instead of panicking.
//!
//! ## Feature Flags
//!
//! - `patina-tests`: Will opt-in to compile any tests.
//!
//! ## Example
//!
//! ```rust
//! use patina::test::*;
//! use patina::boot_services::StandardBootServices;
//! use patina::test::patina_test;
//! use patina::{u_assert, u_assert_eq};
//!
//! let component = patina::test::TestRunner::default()
//!   .with_filter("aarch64") // Only run tests with "aarch64" in their name & path (my_crate::aarch64::test)
//!   .debug_mode(true)
//!   .fail_fast(true);
//!
//! #[cfg_attr(target_arch = "aarch64", patina_test)]
//! fn test_case() -> Result {
//!   u_assert_eq!(1, 1);
//!   Ok(())
//! }
//!
//! #[patina_test]
//! fn test_case2() -> Result {
//!   u_assert_eq!(1, 1);
//!   Ok(())
//! }
//!
//! #[patina_test]
//! #[should_fail]
//! fn failing_test_case() -> Result {
//!    u_assert_eq!(1, 2);
//!    Ok(())
//! }
//!
//! #[patina_test]
//! #[should_fail = "This test failed"]
//! fn failing_test_case_with_msg() -> Result {
//!   u_assert_eq!(1, 2, "This test failed");
//!   Ok(())
//! }
//!
//! #[patina_test]
//! #[skip]
//! fn skipped_test_case() -> Result {
//!    todo!()
//! }
//!
//! #[patina_test]
//! #[cfg_attr(not(target_arch = "x86_64"), skip)]
//! fn x86_64_only_test_case(bs: StandardBootServices) -> Result {
//!   todo!()
//! }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;
use alloc::vec::Vec;

use crate as patina;
use crate::component::{IntoComponent, Storage};

#[doc(hidden)]
pub use linkme;
// WARNING: this is not a part of the crate's public API and is subject to change at any time.
#[doc(hidden)]
pub mod __private_api;

/// The result type for a test case, an alias for `Result<(), &'static str>`.
pub type Result = core::result::Result<(), &'static str>;

/// A proc-macro that registers the annotated function as a test case to be run by patina_test component.
///
/// There is a distinct difference between doing a #[cfg_attr(..., skip)] and a
/// #[cfg_attr(..., patina_test)]. The first still compiles the test case, but skips it at runtime. The second does not
/// compile the test case at all.
///
/// ## Attributes
///
/// - `#[should_fail]`: Indicates that the test is expected to fail. If the test passes, the test runner will log an
///   error.
/// - `#[should_fail = "message"]`: Indicates that the test is expected to fail with the given message. If the test
///   passes or fails with a different message, the test runner will log an error.
/// - `#[skip]`: Indicates that the test should be skipped.
///
/// ## Example
///
/// ```rust
/// use patina::test::*;
/// use patina::boot_services::StandardBootServices;
/// use patina::test::patina_test;
/// use patina::{u_assert, u_assert_eq};
///
/// #[patina_test]
/// fn test_case() -> Result {
///     todo!()
/// }
///
/// #[patina_test]
/// #[should_fail]
/// fn failing_test_case() -> Result {
///     u_assert_eq!(1, 2);
///     Ok(())
/// }
///
/// #[patina_test]
/// #[should_fail = "This test failed"]
/// fn failing_test_case_with_msg() -> Result {
///    u_assert_eq!(1, 2, "This test failed");
///    Ok(())
/// }
///
/// #[patina_test]
/// #[skip]
/// fn skipped_test_case() -> Result {
///    todo!()
/// }
///
/// #[patina_test]
/// #[cfg_attr(not(target_arch = "x86_64"), skip)]
/// fn x86_64_only_test_case(bs: StandardBootServices) -> Result {
///   todo!()
/// }
/// ```
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

/// A component that runs all test cases marked with the `#[patina_test]` attribute when loaded by the DXE core.
#[derive(IntoComponent, Default, Clone)]
pub struct TestRunner {
    filters: Vec<&'static str>,
    debug_mode: bool,
    fail_fast: bool,
}

impl TestRunner {
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

    /// The entry point for the test runner component.
    #[coverage(off)]
    fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
        let test_list: &[__private_api::TestCase] = __private_api::test_cases();
        self.run_tests(test_list, storage)
    }

    /// Runs the provided list of test cases, applying the configuration options set on the TestRunner.
    fn run_tests(&self, test_list: &[__private_api::TestCase], storage: &mut Storage) -> patina::error::Result<()> {
        let count = test_list.len();
        match count {
            0 => log::warn!("No Tests Found"),
            1 => log::info!("running 1 test"),
            _ => log::info!("running {count} tests"),
        }

        let mut did_error = false;
        for test in test_list {
            if !test.should_run(&self.filters) {
                log::info!("{} ... skipped", test.name);
                continue;
            }

            match test.run(storage, self.debug_mode) {
                Ok(_) => log::info!("{} ... ok", test.name),
                Err(e) => {
                    log::error!("{} ... fail: {}", test.name, e);
                    did_error = true;
                    if self.fail_fast {
                        return Err(patina::error::EfiError::Aborted);
                    }
                }
            }
        }

        match did_error {
            true => Err(patina::error::EfiError::Aborted),
            false => Ok(()),
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use crate::component::{IntoComponent, Storage, params::Config};

    // A test function where we mock DxeComponentInterface to return what we want for the test.
    fn test_function(config: Config<i32>) -> Result<(), &'static str> {
        assert!(*config == 1);
        Ok(())
    }

    fn test_function_fail() -> Result<(), &'static str> {
        Err("Intentional Failure")
    }

    #[test]
    fn test_func_implements_into_component() {
        let _ = super::TestRunner::default().into_component();
    }

    #[test]
    fn verify_default_values() {
        let config = super::TestRunner::default();
        assert_eq!(config.filters.len(), 0);
        assert!(!config.debug_mode);
        assert!(!config.fail_fast);
    }

    #[test]
    fn verify_config_sets_properly() {
        let config =
            super::TestRunner::default().with_filter("aarch64").with_filter("test").debug_mode(true).fail_fast(true);
        assert_eq!(config.filters.len(), 2);
        assert!(config.debug_mode);
        assert!(config.fail_fast);
    }

    // This is mirroring the logic in __private_api.rs to ensure we do properly register test cases.
    #[linkme::distributed_slice]
    static TEST_TESTS: [super::__private_api::TestCase];

    #[linkme::distributed_slice(TEST_TESTS)]
    static TEST_CASE1: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test",
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function).run(storage.into()),
    };

    #[linkme::distributed_slice(TEST_TESTS)]
    static TEST_CASE2: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test",
        skip: true,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function).run(storage.into()),
    };

    static TEST_CASE3: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test_that_fails",
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function_fail).run(storage.into()),
    };

    #[test]
    fn test_we_can_initialize_the_component() {
        let mut storage = Storage::new();

        let mut component = super::TestRunner::default().fail_fast(true).into_component();
        component.initialize(&mut storage);
    }

    #[test]
    fn test_we_can_collect_and_execute_tests() {
        assert_eq!(TEST_TESTS.len(), 2);
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let component = super::TestRunner::default().fail_fast(true);
        let result = component.run_tests(&TEST_TESTS, &mut storage);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_different_test_counts() {
        let mut test_cases = vec![];
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let component = super::TestRunner::default().fail_fast(true);
        let result = component.run_tests(&test_cases, &mut storage);
        assert!(result.is_ok());

        test_cases.push(TEST_CASE1);
        let result = component.run_tests(&test_cases, &mut storage);
        assert!(result.is_ok());

        test_cases.push(TEST_CASE2);
        let result = component.run_tests(&test_cases, &mut storage);
        assert!(result.is_ok());

        test_cases.push(TEST_CASE3);
        let result = component.run_tests(&test_cases, &mut storage);
        assert!(result.is_err());

        let result = component.fail_fast(false).run_tests(&test_cases, &mut storage);
        assert!(result.is_err());
    }
}
