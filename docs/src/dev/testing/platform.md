# Platform Testing

Platform testing is supported through the `uefi_test` crate, which provides a testing framework similar to the typical
rust testing framework. The key difference is that instead of tests being collected and executed on the host based
system, they are instead collected and executed via a component (`uefi_test::TestRunner`) provided by the same crate.
The platform must register this component with the dxe_core. The dxe_core will then dispatch this component, which will
run all registered tests.

``` admonish note
The most up to date documentation on the `uefi_test` crate can be found on crates.io. It is suggested that you review
the documentation in that crate. However, for ease of access, some high level concepts can be read about below.
```

## Writing On-Platform Tests

Writing a test to be run on-platform is as simple as setting the `uefi_test` attribute on a function with the following
interface where `...` can be any number of parameters that implement the `Param` trait from `uefi_sdk::component::*`:

``` rust
use uefi_test::{Result, uefi_test};

#[uefi_test]
fn my_test(...) -> Result { todo!() }
```

Writing on-platform tests is not just for driver testing, it can also be used for testing general purpose code on a
platform. Any function tagged with `#[uefi_test]` will be collected and executed on a platform. The test runner has the
ability to filter out tests, but you should also be conscious of when tests should run. using `cfg_attr` paired with
`skip` attribute is a great way to have tests ignored for reasons like host architecture, or through feature flags!

``` admonish note
uefi_test::Result is simply `core::result::Result<(), &'static str>`, and you could use that instead.
```

Similar to `test` attribute, there are a few additional attribute customizations to help with writing tests platform
based tests. The first is the `skip` attribute, which paired with `cfg_attr` can be used to skip certain tests.

``` rust
use uefi_sdk::boot_services::StandardBootServices;

#[uefi_test]
#[cfg_attr(target_arch = "aarch64", skip)]
fn my_test(bs: StandardBootServices) -> Result { todo!() }
```

Next is the `should_fail` attribute which allows you to specify that this test should fail. It has an additional
customization that allows you to specify the expected failure message.

``` rust
#[uefi_test]
#[should_fail]
fn my_test1() -> Result { todo!() }

#[uefi_test]
#[should_fail = "Failed for this reason"]
fn my_test2() -> Result { todo!() }
```

## Running On-Platform Tests

Running all these tests on a platform is as easy as instantiating the test runner component and registering it with the
dxe core:

``` rust

let test_runner = TestRunner::default();

Core::default()
    .init_memory()
    .with_component(test_runner)
    .start()
    .unwrap();
```

This will execute all tests marked with the `uefi_test` attribute across all crates used to compile this binary. Due to
this fact, we have some configuration options with the test component. The most important customization is the
`with_filter` function, which allows you to filter down the tests to run. The logic behind this is similar to the
filtering provided by `cargo test`. That is to say, if you pass it a filter of `X64`, it will only run tests with `X64`
in their name. The function name is `<module_path>::<name>`. You can call `with_filter` multiple times.

The next customization is `debug_mode` which enables logging during test execution (false by default). The final
customization is `fail_fast` which will immediately exit the test harness as soon as a single test fails (false by
default). These two customizations can only be called once. Subsequent calls will overwrite the previous value.

``` rust
let test_runner = TestRunnerComponent::default()
    .with_filter("X64")
    .debug_mode(true)
    .fail_fast(true);

Core::default()
    .init_memory()
    .with_component(test_runner)
    .start()
    .unwrap();
```
