//! A Patina testing framework for on-platform unit testing
//!
//! This module provides a macro ([patina_test]) to register dependency injectable functions as on-platform unit tests
//! that can be discovered and executed by the [TestRunner] component.
//!
//! ## Writing Tests
//!
//! The patina test framework emulates the Rust provided testing framework as much as possible, so writing tests
//! should feel very similar to writing normal Rust unit tests with some additional configuration attributes available.
//!
//! 1. A developer should use `#[patina_test]` to mark a function as a test case, rather than `#[test]`. The function
//!    must return a [Result] type, rather than panicking on failure, which differs from the standard Rust testing
//!    framework.
//! 2. To assist with (1), this crate provides `assert` equivalent macros that return an error on failure rather than
//!    panicking (See [crate::u_assert], [crate::u_assert_eq], [crate::u_assert_ne]).
//! 3. Tests can be configured with the same attributes as the standard Rust provided testing framework, such as
//!    `#[should_fail]`, `#[should_fail = "<message>"]`, and `#[skip]`.
//! 4. By default, tests are configured to run once during the boot process, but a macro attribute is provided to
//!    change when/how often a test is triggered. See the [patina_test] macro documentation for more details.
//! 5. Test dependencies can be injected as function parameters, and the test framework will resolve them from the
//!    component storage system. The test will not run if the dependency cannot be resolved.
//!
//! ## Running Tests
//!
//! Tests marked with `#[patina_test]` are not automatically executed by a platform. Instead, the platform must opt-in
//! to running tests by registering one or more [TestRunner] components with the Core. Each [TestRunner] component will
//! discover all test cases that match it's configuration and schedule them according to the component's configurations
//! and the test case's triggers. An overlap in test cases discovered by multiple [TestRunner] components is allowed,
//! but the test case will only be scheduled to run once based on it's triggers. The Test failure callbacks will be
//! called for each [TestRunner] that discovers the test case. `debug_mode=true` takes priority, so if any [TestRunner]
//! that discovers a test case has `debug_mode=true`, then debug messages will be enabled for that test case regardless
//! of the other [TestRunner]'s debug_mode configuration for that test case.
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
//! use patina::guids::CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP;
//!
//! // Registered with the Core.
//! let test_config = patina::test::TestRunner::default()
//!   .with_filter("aarch64") // Only run tests with "aarch64" in their name & path (my_crate::aarch64::test)
//!   .debug_mode(true); // Allow any log messages from the test to be printed
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
//!
//! #[patina_test]
//! #[on(event = CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP)]
//! fn on_event_test_case() -> Result {
//!   Ok(())
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

use core::{fmt::Display, ops::DerefMut, ptr::NonNull};

use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use patina_macro::{IntoService, component};
use r_efi::efi::EVENT_GROUP_READY_TO_BOOT;

use crate as patina;
use crate::{
    boot_services::{
        BootServices, StandardBootServices,
        event::{EventTimerType, EventType},
        tpl::Tpl,
    },
    component::Storage,
    test::__private_api::{TestCase, TestTrigger},
};

#[doc(hidden)]
pub use linkme;
// WARNING: this is not a part of the crate's public API and is subject to change at any time.
#[doc(hidden)]
pub mod __private_api;

/// The result type for a test case, an alias for `Result<(), &'static str>`.
pub type Result = core::result::Result<(), &'static str>;

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

/// A private service to record test results.
#[derive(IntoService, Default)]
#[service(Recorder)]
struct Recorder {
    records: spin::Mutex<BTreeMap<&'static str, TestRecord>>,
}

impl Recorder {
    /// Allows updates to the test records via a closure to ensure interior mutability safety.
    fn with_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut BTreeMap<&'static str, TestRecord>) -> R,
    {
        let mut records = self.records.lock();
        f(records.deref_mut())
    }

    /// Registers UEFI event callbacks to log the test results at specific points in the boot process.
    fn initialize(&self, storage: &mut Storage) -> patina::error::Result<()> {
        // Log results at ready to boot
        storage.boot_services().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(Self::run_tests_and_report),
            NonNull::from_ref(storage),
            &EVENT_GROUP_READY_TO_BOOT,
        )?;

        // log results at exit boot services
        storage.boot_services().create_event(
            EventType::SIGNAL_EXIT_BOOT_SERVICES,
            Tpl::CALLBACK,
            Some(Self::run_tests_and_report),
            NonNull::from_ref(storage),
        )?;

        Ok(())
    }

    /// Returns true if a test with the given name is already registered, false otherwise.
    fn test_registered(&self, test_name: &str) -> bool {
        self.with_mut(|data| data.contains_key(test_name))
    }

    // Updates an existing record or inserts a new record if it does not exist.
    fn update_record(&self, record: TestRecord) {
        let name = record.test_case.name;

        self.with_mut(|data| {
            if let Some(existing_record) = data.get_mut(name) {
                existing_record.merge(&record);
            } else {
                data.insert(name, record);
            }
        });
    }

    /// Runs all tests that are triggered by the [TestTrigger::Manual] trigger if they have not been run before.
    fn run_manual_tests(&self, storage: &mut Storage) {
        self.with_mut(|data| {
            data.values_mut()
                .filter(|record| {
                    record.test_case.triggers.contains(&TestTrigger::Manual) && record.pass == 0 && record.fail == 0
                })
                .for_each(|record| record.run(storage));
        });
    }

    /// An EFIAPI compatible event callback to run the manually triggered tests and log the current results of patina-test
    extern "efiapi" fn run_tests_and_report(event: r_efi::efi::Event, mut storage: NonNull<Storage>) {
        // SAFETY: event callbacks are executed in series, so there exists no other mutable access to storage.
        let storage = unsafe { storage.as_mut() };

        if let Some(recorder) = storage.get_service::<Recorder>() {
            recorder.run_manual_tests(storage);

            log::info!("{}", *recorder);
        }

        let _ = storage.boot_services().close_event(event);
    }
}

impl Display for Recorder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.with_mut(|records| {
            let mut total_passes = 0;
            let mut total_fails = 0;
            writeln!(f, "Patina on-system unit-test results:")?;
            for (name, record) in records.iter() {
                total_passes += record.pass;
                total_fails += record.fail;
                if record.fail == 0 && record.pass == 0 {
                    writeln!(f, "  {name} ... not triggered")?;
                    continue;
                }
                if record.fail == 0 {
                    writeln!(f, "  {name} ... ok ({} passes)", record.pass)?;
                } else {
                    writeln!(
                        f,
                        "  {name} ... fail ({} fails, {} passes): {}",
                        record.fail,
                        record.pass,
                        record.err_msg.unwrap_or("<no error message>")
                    )?;
                }
            }
            writeln!(f, "Patina on-system unit-test result totals: {total_passes} passes, {total_fails} fails")?;

            Ok(())
        })
    }
}

/// A structure containing all necessary data to execute a test at any time.
#[derive(Clone)]
struct TestRecord {
    /// Whether or not to log debug messages in the test or not
    debug_mode: bool,
    /// The test case to execute.
    test_case: &'static TestCase,
    /// Callback functions to be called on test failure.
    callback: Vec<fn(&'static str, &'static str)>,
    /// The number of times this test has executed and passed.
    pass: u32,
    /// The number of times this test has executed and failed.
    fail: u32,
    /// The error message from the most recent failure, if any.
    err_msg: Option<&'static str>,
}

impl TestRecord {
    /// Creates a new instance of TestRecord.
    fn new(debug_mode: bool, test_case: &'static TestCase, callback: Option<fn(&'static str, &'static str)>) -> Self {
        let callback = callback.into_iter().collect();
        Self { debug_mode, test_case, callback, pass: 0, fail: 0, err_msg: None }
    }

    /// Merges another test record into this one, combining their results and callbacks.
    fn merge(&mut self, other: &Self) {
        assert_eq!(self.test_case.name, other.test_case.name, "Can only merge records for the same test case.");
        self.debug_mode |= other.debug_mode;
        self.pass += other.pass;
        self.fail += other.fail;
        self.callback.extend(other.callback.clone());
        if self.err_msg.is_none() && other.err_msg.is_some() {
            self.err_msg = other.err_msg;
        }
    }

    /// Runs the test case case.
    ///
    /// Calls the test failure callbacks if the test fails.
    fn run(&mut self, storage: &mut Storage) {
        let result = self.test_case.run(storage, self.debug_mode);

        match result {
            Ok(()) => self.pass += 1,
            Err(msg) => {
                self.fail += 1;
                self.err_msg = Some(msg);
                self.callback.iter().for_each(|cb| cb(self.test_case.name, msg));
            }
        }
    }

    /// Schedules the test to be run according to its triggers.
    fn schedule_run(&self, storage: &mut Storage) -> patina::error::Result<()> {
        let name = self.test_case.name;

        for trigger in self.test_case.triggers {
            match trigger {
                TestTrigger::Manual => {
                    // Do nothing. Test must be manually triggered.
                }
                TestTrigger::Event(guid) => {
                    storage.boot_services().create_event_ex(
                        EventType::NOTIFY_SIGNAL,
                        Tpl::CALLBACK,
                        Some(Self::run_test),
                        Box::leak(Box::new((name, NonNull::from_ref(storage)))),
                        guid,
                    )?;
                }
                TestTrigger::Timer(interval) => {
                    let event = storage.boot_services().create_event(
                        EventType::NOTIFY_SIGNAL | EventType::TIMER,
                        Tpl::CALLBACK,
                        Some(Self::run_test),
                        // We are setting up this timer to be periodic, so we need to leak it so it is available for
                        // multiple test runs
                        Box::leak(Box::new((name, NonNull::from_ref(storage)))),
                    )?;

                    // We need to disable the timer at ReadyToBoot so it does not continue firing while a
                    // bootloader is running.
                    let _ = storage.boot_services().create_event_ex(
                        EventType::NOTIFY_SIGNAL,
                        Tpl::CALLBACK,
                        Some(Self::disable_timer),
                        NonNull::from_ref(Box::leak(Box::new((event, storage.boot_services().clone())))).as_ptr()
                            as *mut core::ffi::c_void,
                        &EVENT_GROUP_READY_TO_BOOT,
                    )?;

                    storage.boot_services().set_timer(event, EventTimerType::Periodic, *interval)?;
                }
            }
        }

        Ok(())
    }

    /// EFIAPI event callback to locate a specific test and run it.
    extern "efiapi" fn run_test(_: r_efi::efi::Event, &(test, mut storage): &'static (&'static str, NonNull<Storage>)) {
        // SAFETY: Storage is a valid pointer as the pointer is generated from a static reference.
        let storage = unsafe { storage.as_mut() };

        if let Some(recorder) = storage.get_service::<Recorder>() {
            let _ = recorder.with_mut(|records| records.get_mut(test).map(|record| record.run(storage)));
        }
    }

    #[coverage(off)]
    /// An EFIAPI compatible event callback to disable a timer event at ReadyToBoot
    extern "efiapi" fn disable_timer(rtb_event: r_efi::efi::Event, context: *mut core::ffi::c_void) {
        // SAFETY: We set up the context pointer in `run_tests` to point to a valid tuple of (Event, &mut Storage).
        let (timer_event, boot_services) = unsafe { &mut *(context as *mut (r_efi::efi::Event, StandardBootServices)) };
        let _ = boot_services.set_timer(*timer_event, EventTimerType::Cancel, 0);
        let _ = boot_services.close_event(rtb_event);
    }
}

/// A component that runs all test cases marked with the `#[patina_test]` attribute when loaded by the DXE core.
#[derive(Default, Clone)]
pub struct TestRunner {
    filters: Vec<&'static str>,
    debug_mode: bool,
    fail_callback: Option<fn(&'static str, &'static str)>,
}

#[component]
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

    /// Attach a callback function that will be called on test failure.
    ///
    /// fn(test_name: &'static str, fail_msg: &'static str)
    pub fn with_callback(mut self, callback: fn(&'static str, &'static str)) -> Self {
        self.fail_callback = Some(callback);
        self
    }

    /// The entry point for the test runner component.
    #[coverage(off)]
    fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
        let test_list: &'static [__private_api::TestCase] = __private_api::test_cases();
        self.register_tests(test_list, storage)
    }

    /// Registers the tests to be executed by the test runner.
    fn register_tests(
        &self,
        test_list: &'static [__private_api::TestCase],
        storage: &mut Storage,
    ) -> patina::error::Result<()> {
        let recorder = match storage.get_service::<Recorder>() {
            Some(recorder) => recorder,
            None => {
                let recorder = Recorder::default();
                recorder.initialize(storage)?;
                storage.add_service(recorder);
                storage.get_service::<Recorder>().expect("Recorder service should be registered.")
            }
        };

        let records = test_list
            .iter()
            .filter(|&test_case| test_case.should_run(&self.filters))
            .map(|test_case| TestRecord::new(self.debug_mode, test_case, self.fail_callback));

        for record in records {
            // Only schedule a run if we have not already scheduled for this test.
            if !recorder.test_registered(record.test_case.name) {
                record.schedule_run(storage)?;
            }

            recorder.update_record(record);
        }

        Ok(())
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::mem::MaybeUninit;

    use super::*;
    use crate::{
        boot_services::StandardBootServices,
        component::{IntoComponent, Storage, params::Config},
    };

    // A test function where we mock DxeComponentInterface to return what we want for the test.
    fn test_function(config: Config<i32>) -> Result {
        assert!(*config == 1);
        Ok(())
    }

    fn test_function_fail() -> Result {
        Err("Intentional Failure")
    }

    fn test_function_invalid(_: &mut Storage, _: &mut Storage) -> Result {
        Ok(())
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
    }

    #[test]
    fn verify_config_sets_properly() {
        let config = super::TestRunner::default().with_filter("aarch64").with_filter("test").debug_mode(true);
        assert_eq!(config.filters.len(), 2);
        assert!(config.debug_mode);
    }

    // This is mirroring the logic in __private_api.rs to ensure we do properly register test cases.
    #[linkme::distributed_slice]
    static TEST_TESTS: [super::__private_api::TestCase];

    #[linkme::distributed_slice(TEST_TESTS)]
    static TEST_CASE1: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test",
        triggers: &[super::__private_api::TestTrigger::Manual],
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function).run(storage.into()),
    };

    #[linkme::distributed_slice(TEST_TESTS)]
    static TEST_CASE2: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test",
        triggers: &[super::__private_api::TestTrigger::Manual],
        skip: true,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function).run(storage.into()),
    };

    static TEST_CASE3: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "test_that_fails",
        triggers: &[super::__private_api::TestTrigger::Manual],
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function_fail).run(storage.into()),
    };

    static TEST_CASE4: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "event_triggered_test",
        triggers: &[super::__private_api::TestTrigger::Event(crate::BinaryGuid::from_bytes(&[0; 16]))],
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function_fail).run(storage.into()),
    };

    static TEST_CASE5: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "timer_triggered_test",
        triggers: &[super::__private_api::TestTrigger::Timer(1_000_000)],
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function_fail).run(storage.into()),
    };

    static TEST_CASE_INVALID: super::__private_api::TestCase = super::__private_api::TestCase {
        name: "invalid_test",
        triggers: &[super::__private_api::TestTrigger::Event(crate::BinaryGuid::from_bytes(&[0; 16]))],
        skip: false,
        should_fail: false,
        fail_msg: None,
        func: |storage| crate::test::__private_api::FunctionTest::new(test_function_invalid).run(storage.into()),
    };

    #[test]
    #[ignore = "Skipping test until the service for UEFI services is out, so we can mock it."]
    fn test_we_can_initialize_the_component() {
        let mut storage = Storage::new();

        let mut component = super::TestRunner::default().into_component();
        component.initialize(&mut storage);
    }

    #[test]
    #[ignore = "Skipping test until the service for UEFI services is out, so we can mock it."]
    fn test_we_can_collect_and_execute_tests() {
        assert_eq!(TEST_TESTS.len(), 2);
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let component = super::TestRunner::default();
        let result = component.register_tests(&TEST_TESTS, &mut storage);
        assert!(result.is_ok());
    }

    #[test]
    #[ignore = "Skipping test until the service for UEFI services is out, so we can mock it."]
    fn test_handle_different_test_counts() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let test_cases: &'static [TestCase] = Box::leak(Box::new([]));
        let component = super::TestRunner::default();
        let result = component.register_tests(test_cases, &mut storage);
        assert!(result.is_ok());

        let test_cases: &'static [TestCase] = Box::leak(Box::new([TEST_CASE1]));
        let result = component.register_tests(test_cases, &mut storage);
        assert!(result.is_ok());

        let test_cases: &'static [TestCase] = Box::leak(Box::new([TEST_CASE1, TEST_CASE2]));
        let result = component.register_tests(test_cases, &mut storage);
        assert!(result.is_ok());

        let test_cases: &'static [TestCase] = Box::leak(Box::new([TEST_CASE1, TEST_CASE2, TEST_CASE3]));
        let result = component.register_tests(test_cases, &mut storage);
        assert!(result.is_ok());
    }

    #[test]
    fn test_recorder_records_results() {
        let recorder = Recorder::default();

        let mut tr1 = TestRecord::new(false, &TEST_CASE2, None);
        tr1.pass = 2;
        tr1.fail = 1;
        tr1.err_msg = Some("Failure 1");
        recorder.update_record(tr1);

        let mut tr2 = TestRecord::new(false, &TEST_CASE3, None);
        tr2.pass = 0;
        tr2.fail = 2;
        tr2.err_msg = Some("Failure 2");
        recorder.update_record(tr2);

        let mut tr3 = TestRecord::new(false, &TEST_CASE4, None);
        tr3.pass = 1;
        recorder.update_record(tr3);

        let output = format!("{}", recorder);
        assert!(output.contains("test ... fail (1 fails, 2 passes): Failure 1"));
        assert!(output.contains("test_that_fails ... fail (2 fails, 0 passes): Failure 2"));
        assert!(output.contains("event_triggered_test ... ok (1 passes)"));
    }

    #[test]
    fn test_test_data_test_running() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);
        storage.add_service(Recorder::default());

        let test_case = &TEST_CASE1;
        let mut test_data = TestRecord::new(false, test_case, None);

        test_data.run(&mut storage);

        let recorder = storage.get_service::<Recorder>().expect("Recorder service should be registered.");
        recorder.update_record(test_data);

        let output = format!("{}", *recorder);
        println!("{}", output);
        assert!(output.contains("test ... ok (1 passes)"));
    }

    #[test]
    #[should_panic(expected = "Callback called")]
    fn test_test_failure_callback_handler() {
        let test_runner = TestRunner::default().with_callback(|_, _| {
            panic!("Callback called");
        });

        let mut storage = Storage::new();
        storage.add_service(Recorder::default());
        let bs: MaybeUninit<r_efi::efi::BootServices> = MaybeUninit::uninit();

        // SAFETY: This is very unsafe, because it is not initialized, however this code path only calls create_event
        // and create_event_ex, which we will fill in with no-op functions.
        let mut bs = unsafe { bs.assume_init() };
        extern "efiapi" fn noop_create_event(
            _type: u32,
            _tpl: r_efi::efi::Tpl,
            _notify_function: Option<extern "efiapi" fn(r_efi::efi::Event, *mut core::ffi::c_void)>,
            _notify_context: *mut core::ffi::c_void,
            _event: *mut r_efi::efi::Event,
        ) -> r_efi::efi::Status {
            r_efi::efi::Status::SUCCESS
        }

        extern "efiapi" fn noop_create_event_ex(
            _type: u32,
            _tpl: r_efi::efi::Tpl,
            _notify_function: Option<extern "efiapi" fn(r_efi::efi::Event, *mut core::ffi::c_void)>,
            _notify_context: *const core::ffi::c_void,
            _guid: *const r_efi::efi::Guid,
            _event: *mut r_efi::efi::Event,
        ) -> r_efi::efi::Status {
            r_efi::efi::Status::SUCCESS
        }

        bs.create_event = noop_create_event;
        bs.create_event_ex = noop_create_event_ex;

        storage.set_boot_services(StandardBootServices::new(Box::leak(Box::new(bs))));

        // TEST_CASE3 is designed to fail.
        let _ = test_runner.register_tests(Box::leak(Box::new([TEST_CASE3])), &mut storage);
        storage.get_service::<Recorder>().unwrap().run_manual_tests(&mut storage);
    }

    #[test]
    fn test_filter_should_work() {
        let test_runner = TestRunner::default().with_filter("triggered_test");

        let mut storage = Storage::new();
        let bs: MaybeUninit<r_efi::efi::BootServices> = MaybeUninit::uninit();

        // SAFETY: This is very unsafe, because it is not initialized, however this code path only calls create_event
        // create_event_ex, and set_timer which we will fill in with no-op functions.
        let mut bs = unsafe { bs.assume_init() };
        extern "efiapi" fn noop_create_event(
            _type: u32,
            _tpl: r_efi::efi::Tpl,
            _notify_function: Option<extern "efiapi" fn(r_efi::efi::Event, *mut core::ffi::c_void)>,
            _notify_context: *mut core::ffi::c_void,
            _event: *mut r_efi::efi::Event,
        ) -> r_efi::efi::Status {
            r_efi::efi::Status::SUCCESS
        }

        extern "efiapi" fn noop_create_event_ex(
            _type: u32,
            _tpl: r_efi::efi::Tpl,
            _notify_function: Option<extern "efiapi" fn(r_efi::efi::Event, *mut core::ffi::c_void)>,
            _notify_context: *const core::ffi::c_void,
            _guid: *const r_efi::efi::Guid,
            _event: *mut r_efi::efi::Event,
        ) -> r_efi::efi::Status {
            r_efi::efi::Status::SUCCESS
        }

        extern "efiapi" fn noop_set_timer(
            _event: r_efi::efi::Event,
            _type: r_efi::efi::TimerDelay,
            _trigger_time: u64,
        ) -> r_efi::efi::Status {
            r_efi::efi::Status::SUCCESS
        }

        bs.create_event = noop_create_event;
        bs.create_event_ex = noop_create_event_ex;
        bs.set_timer = noop_set_timer;

        storage.set_boot_services(StandardBootServices::new(Box::leak(Box::new(bs))));

        // Failure tests
        assert!(
            test_runner.register_tests(Box::leak(Box::new([TEST_CASE3, TEST_CASE4, TEST_CASE5])), &mut storage).is_ok()
        );
        let recorder = storage.get_service::<Recorder>().expect("Recorder service should be registered.");
        recorder.run_manual_tests(&mut storage);

        let output = format!("{}", *recorder);

        // This test is filtered out, so it should not even show up in the results.
        assert!(!output.contains("test_that_fails"));
        // This test is not filtered out, but never run, so should log as such.
        println!("{}", output);
        assert!(output.contains("event_triggered_test ... not triggered"));
    }

    #[test]
    fn test_test_with_invalid_param_combination_is_caught() {
        assert_eq!(
            TEST_CASE_INVALID.run(&mut Storage::new(), false),
            Err("Test failed to run due to un-retrievable parameters.")
        );
    }

    #[test]
    fn test_update_record_with_existing_record() {
        let mut record1 = TestRecord::new(false, &TEST_CASE1, Some(|_, _| ()));
        record1.pass = 1;
        record1.fail = 0;

        let mut record2 = TestRecord::new(true, &TEST_CASE1, Some(|_, _| ()));
        record2.pass = 0;
        record2.fail = 2;
        record2.err_msg = Some("Failure");

        let recorder = Recorder::default();
        recorder.update_record(record1);
        recorder.update_record(record2);

        let record = recorder.with_mut(|data| data.get(&TEST_CASE1.name).cloned().expect("Record should exist."));

        assert!(record.debug_mode);
        assert_eq!(record.pass, 1);
        assert_eq!(record.fail, 2);
        assert_eq!(record.err_msg, Some("Failure"));
        assert!(record.debug_mode);
        assert_eq!(record.callback.len(), 2);
    }

    #[test]
    fn test_efiapi_run_test() {
        let mut storage = Storage::new();
        storage.add_config(1_i32);

        let recorder = Recorder::default();
        recorder.update_record(TestRecord::new(false, &TEST_CASE1, None));
        storage.add_service(recorder);

        let context = Box::leak(Box::new(("test", NonNull::from_ref(&storage))));
        TestRecord::run_test(core::ptr::null_mut(), context);
    }

    #[test]
    fn test_efiapi_run_tests_and_report() {
        let bs: MaybeUninit<r_efi::efi::BootServices> = MaybeUninit::uninit();
        // SAFETY: This is very unsafe, because it is not initialized, however this code path only calls create_event
        // create_event_ex, and set_timer which we will fill in with no-op functions.
        let mut bs = unsafe { bs.assume_init() };

        extern "efiapi" fn noop_close_event(_: r_efi::efi::Event) -> r_efi::efi::Status {
            r_efi::efi::Status::SUCCESS
        }

        bs.close_event = noop_close_event;

        let mut storage = Storage::new();
        storage.set_boot_services(StandardBootServices::new(Box::leak(Box::new(bs))));
        storage.add_config(1_i32);

        let recorder = Recorder::default();
        recorder.update_record(TestRecord::new(false, &TEST_CASE1, None));
        storage.add_service(recorder);

        Recorder::run_tests_and_report(core::ptr::null_mut(), NonNull::from_ref(&storage));

        // Check that the test run
        let recorder = storage.get_service::<Recorder>().expect("Recorder service should be registered.");
        let output = format!("{}", *recorder);
        assert!(output.contains("test ... ok (1 passes)"));
    }
}
