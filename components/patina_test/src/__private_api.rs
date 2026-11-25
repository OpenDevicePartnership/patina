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

use core::marker::PhantomData;

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

/// Internal struct to hold the test case information.
#[derive(Debug, Clone, Copy)]
pub struct TestCase {
    pub name: &'static str,
    pub category: Option<&'static str>,
    pub modifiers: &'static [&'static str],
    pub triggers: &'static [TestTrigger],
    pub skip: bool,
    pub should_fail: bool,
    pub fail_msg: Option<&'static str>,
    pub func: fn(&mut Storage) -> Result<bool, &'static str>,
}

impl TestCase {
    pub fn should_run(&self, filters: &[Filter]) -> bool {
        if self.skip {
            return false;
        }

        let mut has_includes = false;
        let mut included = false;

        for filter in filters {
            match filter {
                Filter::Exclude(p) if self.name.contains(p) => return false,
                Filter::Modifier(modifier) if !self.modifiers.contains(modifier) => return false,
                Filter::ExcludeModifier(modifier) if self.modifiers.contains(modifier) => return false,
                Filter::Exclude(_) | Filter::Modifier(_) | Filter::ExcludeModifier(_) => {}
                Filter::Include(p) => {
                    has_includes = true;
                    included |= self.name.contains(p);
                }
                Filter::Category(category) => {
                    has_includes = true;
                    included |= self.category == Some(*category);
                }
            }
        }

        included || !has_includes
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

    #[test]
    fn test_standard_filters_match_test_case() {
        let test = TestCase {
            name: "test_crate::stress::exercises_mapping_slow_heavy",
            category: Some("stress"),
            modifiers: &["slow", "heavy"],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };
        assert!(test.should_run(&[Filter::CATEGORY_STRESS, Filter::MODIFIER_SLOW, Filter::MODIFIER_HEAVY]));
        assert!(!test.should_run(&[Filter::CATEGORY_INTEGRATION]));
        assert!(!test.should_run(&[Filter::CATEGORY_STRESS, Filter::EXCLUDE_MODIFIER_SLOW]));
        assert!(!test.should_run(&[Filter::CATEGORY_STRESS, Filter::EXCLUDE_MODIFIER_HEAVY]));
    }

    #[test]
    fn test_should_run() {
        let test_case = TestCase {
            name: "test",
            category: None,
            modifiers: &[],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        std::assert!(test_case.should_run(&[Filter::include("test")]));
        std::assert!(test_case.should_run(&[Filter::include("t")]));
        std::assert!(test_case.should_run(&[]));
        std::assert!(!test_case.should_run(&[Filter::include("not")]));
    }

    #[test]
    fn test_should_run_with_no_filters() {
        let test_case = TestCase {
            name: "test",
            category: None,
            modifiers: &[],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        std::assert!(test_case.should_run(&[]));
    }

    #[test]
    fn test_should_run_with_exclude_filters() {
        let test_case = TestCase {
            name: "my_crate::tests::test_case",
            category: None,
            modifiers: &[],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        // Exclude filter matches - should not run
        std::assert!(!test_case.should_run(&[Filter::exclude("test_case")]));
        // Exclude filter does not match - should run
        std::assert!(test_case.should_run(&[Filter::exclude("other")]));
        // Include filter matches but exclude filter also matches - should not run
        std::assert!(!test_case.should_run(&[Filter::include("my_crate"), Filter::exclude("test_case")]));
        // Include filter matches and exclude filter does not match - should run
        std::assert!(test_case.should_run(&[Filter::include("my_crate"), Filter::exclude("other")]));
    }

    #[test]
    fn test_should_run_with_standard_filters() {
        for (category, modifiers, expected) in [
            (Some("smoke"), &[][..], [true, false, false, false, false]),
            (Some("integration"), &["slow"][..], [false, true, false, true, false]),
            (Some("stress"), &["heavy", "slow"][..], [false, false, true, true, true]),
            (Some("stress"), &["heavy"][..], [false, false, true, false, true]),
            (Some("Stress"), &["Slow"][..], [false, false, false, false, false]),
            (None, &[][..], [false, false, false, false, false]),
        ] {
            let test_case = TestCase {
                name: "crate::integration::slow_heavy",
                category,
                modifiers,
                triggers: &[TestTrigger::Manual],
                skip: false,
                should_fail: false,
                fail_msg: None,
                func: |_| Ok(true),
            };
            for (filter, expected) in [
                Filter::CATEGORY_SMOKE,
                Filter::CATEGORY_INTEGRATION,
                Filter::CATEGORY_STRESS,
                Filter::MODIFIER_SLOW,
                Filter::MODIFIER_HEAVY,
            ]
            .iter()
            .zip(expected)
            {
                assert_eq!(test_case.should_run(core::slice::from_ref(filter)), expected);
            }
            assert_eq!(test_case.should_run(&[Filter::EXCLUDE_MODIFIER_SLOW]), !expected[3]);
            assert_eq!(test_case.should_run(&[Filter::EXCLUDE_MODIFIER_HEAVY]), !expected[4]);
        }

        let test_case = TestCase {
            name: "crate::stress::mapping_heavy",
            category: Some("stress"),
            modifiers: &["heavy"],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };
        assert!(test_case.should_run(&[
            Filter::CATEGORY_INTEGRATION,
            Filter::CATEGORY_STRESS,
            Filter::EXCLUDE_MODIFIER_SLOW,
        ]));
        assert!(!test_case.should_run(&[Filter::CATEGORY_STRESS, Filter::EXCLUDE_MODIFIER_HEAVY]));
        assert!(!test_case.should_run(&[Filter::MODIFIER_SLOW, Filter::MODIFIER_HEAVY]));
    }

    #[test]
    fn test_should_run_with_custom_categories_and_modifiers() {
        const SECURITY: Filter = Filter::Category("security");
        const DESTRUCTIVE: Filter = Filter::Modifier("destructive");
        const NOT_SLOW: Filter = Filter::ExcludeModifier("slow");
        let filters = [SECURITY.clone(), DESTRUCTIVE.clone(), NOT_SLOW.clone()];
        for (category, modifiers, expected) in [
            (Some("security"), &["destructive"][..], true),
            (Some("security"), &["destructive", "heavy"][..], true),
            (Some("security"), &["slow", "destructive"][..], false),
            (Some("security"), &[][..], false),
            (Some("security_extra"), &["destructive"][..], false),
            (None, &["destructive"][..], false),
            (Some("security"), &["nondestructive"][..], false),
        ] {
            let test_case = TestCase {
                name: "crate::security::key_rotation_destructive",
                category,
                modifiers,
                triggers: &[TestTrigger::Manual],
                skip: false,
                should_fail: false,
                fail_msg: None,
                func: |_| Ok(true),
            };
            assert_eq!(test_case.should_run(&filters), expected);
            assert!(!TestCase { skip: true, ..test_case }.should_run(&filters));
        }

        let test_case = TestCase {
            name: "crate::security::key_rotation_destructive",
            category: Some("security"),
            modifiers: &["destructive", "needs_reboot"],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };
        assert!(test_case.should_run(&[Filter::CATEGORY_SMOKE, Filter::include("::security::")]));
        assert!(test_case.should_run(&[Filter::include("missing"), Filter::include("::security::")]));
        assert!(test_case.should_run(&[Filter::include("::missing::"), Filter::include("key_rotation")]));
        assert!(!test_case.should_run(&[Filter::include("::security::"), Filter::exclude("key_rotation")]));
        assert!(!test_case.should_run(&[SECURITY, Filter::ExcludeModifier("destructive")]));
        assert!(!test_case.should_run(&[DESTRUCTIVE.clone(), Filter::MODIFIER_SLOW]));
        assert!(!test_case.should_run(&[DESTRUCTIVE, Filter::exclude("key_rotation")]));
        assert!(test_case.should_run(&[Filter::EXCLUDE_MODIFIER_SLOW]));
        assert!(test_case.should_run(&[Filter::Modifier("needs_reboot")]));
        assert!(!test_case.should_run(&[Filter::Modifier("needs")]));
        assert!(test_case.should_run(&[Filter::Category("security"), Filter::include("missing")]));
        assert!(!test_case.should_run(&[Filter::Category("missing"), Filter::include("missing")]));
    }

    #[test]
    fn test_run_with_default_settings() {
        let mut storage = Storage::new();

        let test_case_pass = TestCase {
            name: "test",
            category: None,
            modifiers: &[],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_| Ok(true),
        };

        let test_case_fail = TestCase {
            name: "test",
            category: None,
            modifiers: &[],
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
            category: None,
            modifiers: &[],
            triggers: &[TestTrigger::Manual],
            skip: false,
            should_fail: true,
            fail_msg: None,
            func: |_| Ok(true),
        };
        let test_case_fail = TestCase {
            name: "test",
            category: None,
            modifiers: &[],
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
            category: None,
            modifiers: &[],
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
            category: None,
            modifiers: &[],
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
