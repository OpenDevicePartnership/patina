//! Internal API for the uefi_test crate.
//!
//! This module must be public so that the macros can access it, but it is not intended for use by consumers of the
//! crate.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use uefi_component_interface::DxeComponentInterface;

/// Where all the test cases marked with `#[uefi_test]` are collated to.
#[cfg(not(feature = "off"))]
#[linkme::distributed_slice]
pub static TEST_CASES: [TestCase];

/// returns the test cases to run.
///
/// [`static@TEST_CASES`] does not exist when the `off` feature is enabled because there must be at least one registered test
/// case for `linkme` to work, or we get a compile time error. In this scenario, we just return an empty slice.
pub fn test_cases() -> &'static [TestCase] {
    #[cfg(not(feature = "off"))]
    {
        &TEST_CASES
    }
    #[cfg(feature = "off")]
    {
        &[]
    }
}

/// Internal struct to hold the test case information.
#[derive(Debug, Clone, Copy)]
pub struct TestCase {
    pub name: &'static str,
    pub skip: bool,
    pub should_fail: bool,
    pub fail_msg: Option<&'static str>,
    pub func: fn(&dyn DxeComponentInterface) -> super::Result,
}

impl TestCase {
    pub fn should_run(&self, filters: &[&str]) -> bool {
        if filters.is_empty() {
            return !self.skip;
        }
        filters.iter().any(|pattern| self.name.contains(pattern)) && !self.skip
    }

    pub fn run(&self, interface: &dyn DxeComponentInterface, debug_mode: bool) -> super::Result {
        let ret = if debug_mode {
            log::debug!("#### {} Output Start ####", self.name);
            let ret = (self.func)(interface);
            log::debug!("####  {} Output End  ####", self.name);
            ret
        } else {
            let level = log::max_level();
            log::set_max_level(log::LevelFilter::Off);
            let ret = (self.func)(interface);
            log::set_max_level(level);
            ret
        };

        match (self.should_fail, ret) {
            (true, Ok(_)) => Err("Test passed when it should have failed"),
            (true, Err(msg)) if self.fail_msg.is_some() && Some(msg) != self.fail_msg => Err(msg),
            (true, Err(msg)) if self.fail_msg.is_some() && Some(msg) == self.fail_msg => Ok(()),
            (true, Err(_)) if self.fail_msg.is_none() => Ok(()),
            _ => ret,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{ffi::c_void, result::Result};
    use r_efi::efi;

    mockall::mock! {
        ComponentInterface {}
        impl DxeComponentInterface for ComponentInterface {
            fn install_protocol_interface(&self, handle: Option<efi::Handle>, protocol: efi::Guid, interface: *mut c_void) -> Result<efi::Handle, efi::Status>;
        }
    }

    // A test function where we mock DxeComponentInterface to return what we want for the test.
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
    fn test_should_run() {
        let test_case = TestCase {
            name: "test",
            skip: false,
            should_fail: false,
            fail_msg: None,
            func: |_: &dyn DxeComponentInterface| Ok(()),
        };

        std::assert!(test_case.should_run(&["test"]));
        std::assert!(test_case.should_run(&["t"]));
        std::assert!(test_case.should_run(&[]));
        std::assert!(!test_case.should_run(&["not"]));
    }

    #[test]
    fn test_run_with_default_settings() {
        let test_case = TestCase { name: "test", skip: false, should_fail: false, fail_msg: None, func: test_function };

        // Test that a passing test passes
        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Ok(core::ptr::null_mut()));
        let result = test_case.run(&interface, true);
        std::assert_eq!(result, Ok(()));

        // Test that a failing test fails
        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Err(efi::Status::UNSUPPORTED));
        let result = test_case.run(&interface, true);
        std::assert_eq!(result, Err("Failed to install protocol interface"));
    }

    #[test]
    fn test_run_with_should_fail() {
        let test_case = TestCase { name: "test", skip: false, should_fail: true, fail_msg: None, func: test_function };

        // Test that a test that passes, should fail because its expected to fail
        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Ok(core::ptr::null_mut()));
        let result = test_case.run(&interface, true);
        std::assert_eq!(result, Err("Test passed when it should have failed"));

        // Test that a test that fails, should pass because its expected to fail
        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Err(efi::Status::UNSUPPORTED));
        let result = test_case.run(&interface, true);
        std::assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_run_with_should_fail_and_fail_msg_matches() {
        // Test that a test that fails with the expected message, should pass
        let test_case = TestCase {
            name: "test",
            skip: false,
            should_fail: true,
            fail_msg: Some("Failed to install protocol interface"),
            func: test_function,
        };

        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Err(efi::Status::UNSUPPORTED));
        let result = test_case.run(&interface, false);
        std::assert_eq!(result, Ok(()));

        // Test that a test that fails with an unexpected message, should fail
        let test_case = TestCase {
            name: "test",
            skip: false,
            should_fail: true,
            fail_msg: Some("Other failure"),
            func: test_function,
        };

        let mut interface = MockComponentInterface::new();
        interface.expect_install_protocol_interface().return_once(move |_, _, _| Err(efi::Status::UNSUPPORTED));
        let result = test_case.run(&interface, false);
        std::assert_eq!(result, Err("Failed to install protocol interface"));
    }
}
