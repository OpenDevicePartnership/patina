# A Patina testing framework for on-platform unit testing

This crate provides a macro (`patina_test`) to register dependency injectable functions as on-platform unit tests
that can be discovered and executed by the `TestRunner` component.

## Writing Tests

The patina test framework emulates the Rust provided testing framework as much as possible, so writing tests
should feel very similar to writing normal Rust unit tests with some additional configuration attributes available.

1. A developer should use `#[patina_test]` to mark a function as a test case, rather than `#[test]`. The function
   must return a [Result](crate::error::Result) type, rather than panicking on failure, which differs from the standard
   Rust testing framework.
2. To assist with (1), this crate provides `assert` equivalent macros that return an error on failure rather than
   panicking (See [crate::u_assert], [crate::u_assert_eq], [crate::u_assert_ne]).
3. Tests can be configured with the same attributes as the standard Rust provided testing framework, such as
   `#[should_fail]`, `#[should_fail = "<message>"]`, and `#[skip]`.
4. By default, tests are configured to run once during the boot process, but a macro attribute is provided to
   change when/how often a test is triggered. See the [patina_test] macro documentation for more details.
5. Test dependencies can be injected as function parameters, and the test framework will resolve them from the
   component storage system. The test will not run if the dependency cannot be resolved.

## Running Tests

Tests marked with `#[patina_test]` are not automatically executed by a platform. Instead, the platform must opt-in
to running tests by registering one or more `TestRunner` components with the Core. This is done by enabling the
`test-runner` feature of this crate, which does two things: (1) provides access to the component module, which contains
the component and (2) Globally registers any function marked with `#[patina_test]`. Once registered, it will
discover all test cases that match it's configuration and schedule them according to the component's configurations
and the test case's triggers. An overlap in test cases discovered by multiple `TestRunner` components is allowed,
but the test case will only be scheduled to run once based on it's triggers. The Test failure callbacks will be
called for each `TestRunner` that discovers the test case. `debug_mode=true` takes priority, so if any `TestRunner`
that discovers a test case has `debug_mode=true`, then debug messages will be enabled for that test case regardless
of the other `TestRunner`'s debug_mode configuration for that test case.

## Feature Flags

- `test-runner`: Will make the `component` module public, providing access to the `TestRunner` component and actually
  register patina tests globally.

## Example

```rust
use patina_test::{
    patina_test, u_assert, u_assert_eq,
    error::Result,
};

use patina::boot_services::StandardBootServices;
use patina::guids::CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP;

#[cfg_attr(target_arch = "aarch64", patina_test)]
fn test_case() -> Result {
  u_assert_eq!(1, 1);
  Ok(())
}

#[patina_test]
fn test_case2() -> Result {
  u_assert_eq!(1, 1);
  Ok(())
}

#[patina_test]
#[should_fail]
fn failing_test_case() -> Result {
   u_assert_eq!(1, 2);
   Ok(())
}

#[patina_test]
#[should_fail = "This test failed"]
fn failing_test_case_with_msg() -> Result {
  u_assert_eq!(1, 2, "This test failed");
  Ok(())
}

#[patina_test]
#[skip]
fn skipped_test_case() -> Result {
   todo!()
}

#[patina_test]
#[cfg_attr(not(target_arch = "x86_64"), skip)]
fn x86_64_only_test_case(bs: StandardBootServices) -> Result {
  todo!()
}

#[patina_test]
#[on(event = CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP)]
fn on_event_test_case() -> Result {
  Ok(())
}
```
