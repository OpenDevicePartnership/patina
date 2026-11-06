//! Internal API for the test module.
//!
//! This module must be public so that the macros can access it, but it is not intended for use by consumers of the
//! crate.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{any::TypeId, marker::PhantomData};

use patina::{
    BinaryGuid,
    component::{
        MetaData, Storage, UnsafeStorageCell,
        params::{Param, ParamFunction},
    },
};

use crate::component::Filter;

/// Where all the test cases marked with `#[patina_test]` are collated to.
#[cfg(feature = "test-runner")]
#[linkme::distributed_slice]
pub static TEST_CASES: [TestCase];

/// Returns the test cases to run.
///
/// Tests are only collected when the `test-runner` feature is
/// explicitly enabled. This feature is opt-in and explicit because external
/// consumers of `patina` who do not register at least one test case with
/// the `#[patina_test]` attribute may encounter a surprising linker crash (not
/// just a linker failure), due to the testing infrastructure relying on the
/// `linkme` crate.
///
/// If the `test-runner` feature is not enabled, this function will return an empty slice.
pub fn test_cases() -> &'static [TestCase] {
    #[cfg(feature = "test-runner")]
    {
        &TEST_CASES
    }
    #[cfg(not(feature = "test-runner"))]
    {
        &[]
    }
}

/// An enum to describe how / when a unit test should be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestTrigger {
    /// The test case should be executed manually.
    Manual,
    /// The test case should be executed when the specified event triggers.
    Event(BinaryGuid),
    /// The test case should be executed after the specified units of 100ns have elapsed.
    Timer(u64),
}

/// Represents the dispatch status of a component.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum DispatchStatus {
    /// Waiting for its dependencies before dispatch.
    #[default]
    Pending,
    /// Completed successfully.
    Succeeded,
    /// Returned an error during dispatch.
    Failed,
    /// Failed parameter initialization and cannot be dispatched.
    Rejected,
}

/// Patina Test private `QueryData` that the Patina DXE Core uses to track the dispatch status of
/// registered components. Defining this struct here allows the test infrastructure to query the
/// dispatch status (and presence) of registered components to determine test execution readiness.
///
/// **Warning:** This struct is intended for internal use by the Patina DXE Core and the test
/// infrastructure and should not be used by any other component, which is why this is defined
/// within `__private_api.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchInfo {
    component_type_id: TypeId,
    status: DispatchStatus,
}

impl DispatchInfo {
    /// Creates pending dispatch information for a component registration.
    pub fn new(component_type_id: TypeId) -> Self {
        Self { component_type_id, status: DispatchStatus::default() }
    }

    /// Returns the concrete type identifier of the component registered with the DXE Core.
    pub fn component_type_id(&self) -> TypeId {
        self.component_type_id
    }

    /// Returns whether the component still needs a dispatch attempt.
    pub fn is_pending(&self) -> bool {
        self.status == DispatchStatus::Pending
    }

    /// Returns whether the component dispatched successfully.
    pub fn is_succeeded(&self) -> bool {
        self.status == DispatchStatus::Succeeded
    }

    /// Returns whether the component entry point returned an error.
    pub fn is_failed(&self) -> bool {
        self.status == DispatchStatus::Failed
    }

    /// Returns whether the component failed parameter initialization.
    pub fn is_rejected(&self) -> bool {
        self.status == DispatchStatus::Rejected
    }

    /// Marks the component as pending.
    pub fn set_pending(&mut self) {
        self.status = DispatchStatus::Pending;
    }

    /// Marks the component as successfully dispatched.
    pub fn set_succeeded(&mut self) {
        self.status = DispatchStatus::Succeeded;
    }

    /// Marks the component as failed during dispatch.
    pub fn set_failed(&mut self) {
        self.status = DispatchStatus::Failed;
    }

    /// Marks the component as rejected during parameter initialization.
    pub fn set_rejected(&mut self) {
        self.status = DispatchStatus::Rejected;
    }
}

/// Internal struct to hold the test case information.
#[derive(Debug, Clone, Copy)]
pub struct TestCase {
    pub name: &'static str,
    pub component_id: Option<fn() -> TypeId>,
    pub triggers: &'static [TestTrigger],
    pub skip: bool,
    pub should_fail: bool,
    pub fail_msg: Option<&'static str>,
    pub func: fn(&mut Storage) -> Result<bool, &'static str>,
}

impl TestCase {
    pub fn should_run(&self, filters: &[Filter], component_ids: &[TypeId]) -> bool {
        if self.skip {
            return false;
        }

        let mut has_includes = false;
        let mut included = false;

        for filter in filters {
            match filter {
                Filter::Exclude(p) if self.name.contains(p) => return false,
                Filter::Exclude(_) => {}
                Filter::Include(p) => {
                    has_includes = true;
                    included |= self.name.contains(p);
                }
            }
        }

        let component_registered = self.component_id.is_none_or(|required_id| component_ids.contains(&required_id()));
        component_registered && (included || !has_includes)
    }

    pub fn run(&self, storage: &mut Storage, debug_mode: bool) -> crate::error::Result {
        let ret = if debug_mode {
            log::debug!("#### {} Test Output Start ####", self.name);
            let ret = (self.func)(storage);
            log::debug!("####  {} Test Output End  ####", self.name);
            ret
        } else {
            let level = log::max_level();
            log::set_max_level(log::LevelFilter::Off);
            let ret = (self.func)(storage);
            log::set_max_level(level);
            ret
        };

        match (self.should_fail, ret) {
            (_, Ok(false)) => Err("Test failed to run due to un-retrievable parameters."),
            (true, Ok(true)) => Err("Test passed when it should have failed"),
            (true, Err(msg)) if self.fail_msg.is_some() && Some(msg) != self.fail_msg => Err(msg),
            (true, Err(msg)) if self.fail_msg.is_some() && Some(msg) == self.fail_msg => Ok(()),
            (true, Err(_)) if self.fail_msg.is_none() => Ok(()),
            _ => ret.map(|_| ()),
        }
    }
}

/// A [`ParamFunction`] implementation for an on-system unit test.
///
/// note: Once we can unwind a panic, we can remove the `Result` return type in favor of () and wrap the function in a
/// `catch_unwind` that maps the panic message to a Err(&'static str).
pub struct FunctionTest<Marker, Func>
where
    Func: ParamFunction<Marker, In = (), Out = Result<(), &'static str>>,
{
    func: Func,
    _marker: PhantomData<fn() -> Marker>,
}

impl<Marker, Func> FunctionTest<Marker, Func>
where
    Marker: 'static,
    Func: ParamFunction<Marker, In = (), Out = Result<(), &'static str>>,
{
    pub const fn new(func: Func) -> Self {
        Self { func, _marker: PhantomData }
    }

    pub fn run(&mut self, storage: UnsafeStorageCell) -> Result<bool, &'static str> {
        let mut metadata = MetaData::default();

        // SAFETY: init_state requires mutable access to storage. UnsafeStorageCell provides controlled access.
        // This is the initialization phase before parameter validation.
        let param_state = match Func::Param::init_state(unsafe { storage.storage_mut() }, &mut metadata) {
            Ok(param_state) => param_state,
            Err(error) => {
                log::warn!("Failed to initialize test state: {error:?}");
                return Ok(false);
            }
        };

        if let Err(bad_param) = Func::Param::try_validate(&param_state, storage) {
            log::warn!("Failed to retreive parameter: {bad_param:?}");
            return Ok(false);
        }

        // SAFETY: Parameter was successfully validated by try_validate. get_param extracts the validated parameter
        // from storage using the param_state that was initialized above.
        let param_value = unsafe { Func::Param::get_param(&param_state, storage) };

        self.func.run(&mut Some(()), param_value).map(|()| true)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    extern crate std;

    struct OwningComponent;

    fn owning_component_type_id() -> TypeId {
        TypeId::of::<OwningComponent>()
    }

    #[test]
    fn test_dispatch_info_transitions() {
        struct TestComponent;

        let mut dispatch_info = DispatchInfo::new(TypeId::of::<TestComponent>());
        assert_eq!(dispatch_info.component_type_id(), TypeId::of::<TestComponent>());
        assert!(dispatch_info.is_pending());

        dispatch_info.set_succeeded();
        assert!(dispatch_info.is_succeeded());
        dispatch_info.set_failed();
        assert!(dispatch_info.is_failed());
        dispatch_info.set_rejected();
        assert!(dispatch_info.is_rejected());
        dispatch_info.set_pending();
        assert!(dispatch_info.is_pending());
    }

    #[test]
    fn test_should_run() {
        let test_case = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        std::assert!(test_case.should_run(&[Filter::include("test")], &[]));
        std::assert!(test_case.should_run(&[Filter::include("t")], &[]));
        std::assert!(test_case.should_run(&[], &[]));
        std::assert!(!test_case.should_run(&[Filter::include("not")], &[]));
    }

    #[test]
    fn test_should_run_with_no_filters() {
        let test_case = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        std::assert!(test_case.should_run(&[], &[]));
    }

    #[test]
    fn test_should_run_with_exclude_filters() {
        let test_case = TestCase {
            name: "my_crate::tests::test_case",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        // Exclude filter matches - should not run
        std::assert!(!test_case.should_run(&[Filter::exclude("test_case")], &[]));
        // Exclude filter does not match - should run
        std::assert!(test_case.should_run(&[Filter::exclude("other")], &[]));
        // Include filter matches but exclude filter also matches - should not run
        std::assert!(!test_case.should_run(&[Filter::include("my_crate"), Filter::exclude("test_case")], &[]));
        // Include filter matches and exclude filter does not match - should run
        std::assert!(test_case.should_run(&[Filter::include("my_crate"), Filter::exclude("other")], &[]));
    }

    #[test]
    fn test_should_run_requires_registered_component() {
        let test_case = TestCase {
            name: "component_test",
            component_id: Some(owning_component_type_id),
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        std::assert!(!test_case.should_run(&[], &[]));
        std::assert!(!test_case.should_run(&[], &[TypeId::of::<u32>()]));
        std::assert!(test_case.should_run(&[], &[owning_component_type_id()]));
    }

    #[test]
    fn test_run_with_default_settings() {
        let mut storage = Storage::new();

        let test_case_pass = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        let test_case_fail = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Err("Failed to install protocol interface"),
        };

        // Test that a passing test passes
        let result = test_case_pass.run(&mut storage, true);
        std::assert_eq!(result, Ok(()));

        // Test that a failing test fails
        let result = test_case_fail.run(&mut storage, true);
        std::assert_eq!(result, Err("Failed to install protocol interface"));
    }

    #[test]
    fn test_run_with_should_fail() {
        let mut storage = Storage::new();

        let test_case_pass = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: true,
            fail_msg: None,
            func: |_| Ok(true),
        };
        let test_case_fail = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: true,
            fail_msg: None,
            func: |_| Err("Failed to install protocol interface"),
        };

        // Test that a test that passes, should fail because its expected to fail
        let result = test_case_pass.run(&mut storage, true);
        std::assert_eq!(result, Err("Test passed when it should have failed"));

        // Test that a test that fails, should pass because its expected to fail
        let result = test_case_fail.run(&mut storage, true);
        std::assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_run_with_should_fail_and_fail_msg_matches() {
        let mut storage = Storage::new();

        // Test that a test that fails with the expected message, should pass
        let test_case = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: true,
            fail_msg: Some("Failed to install protocol interface"),
            func: |_| Err("Failed to install protocol interface"),
        };

        let result = test_case.run(&mut storage, false);
        std::assert_eq!(result, Ok(()));

        // Test that a test that fails with an unexpected message, should fail
        let test_case = TestCase {
            name: "test",
            component_id: None,
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: true,
            fail_msg: Some("Other failure"),
            func: |_| Err("Failed to install protocol interface"),
        };

        let result = test_case.run(&mut storage, false);
        std::assert_eq!(result, Err("Failed to install protocol interface"));
    }

    #[test]
    fn test_test_with_invalid_param_combination_is_caught() {
        assert_eq!(
            crate::component::tests::TEST_CASE_INVALID.run(&mut Storage::new(), false),
            Err("Test failed to run due to un-retrievable parameters.")
        );
    }
}
