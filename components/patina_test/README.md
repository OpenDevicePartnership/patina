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
   panicking (See [crate::u_assert], [`crate::u_assert_eq`], [`crate::u_assert_ne`]).
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
of the other `TestRunner`'s `debug_mode` configuration for that test case.

For example, configure a runner:

```rust,no_run
# #[cfg(feature = "test-runner")]
# {
use patina_test::component::TestRunner;

let runner = TestRunner::default()
    .debug_mode(true)
    .with_callback(|name, message| log::error!("Test {name} failed: {message}"));

# let _ = runner;
# }
```

In a platform's `ComponentInfo::components` implementation, register the runner with
`add.component(runner)` before component dispatch starts. The Core performs the conversion automatically.

> [!IMPORTANT]
> `TestRunner` depends on `Service<dyn TimerEventServices>`. The DXE Core does not produce that service until the
> `EFI_TIMER_ARCH_PROTOCOL` is available. As a result, `TestRunner` and all registered `patina_test` tests wait to run
> until the platform installs the Timer Architectural Protocol. Tests will not run on a platform that never installs
> it.

## Test Organization and Filtering

Patina provides standard test categories and modifiers with predefined filters. Platforms and component crates
should use the standard names where applicable, but they are not an exhaustive list: either may define custom
categories and modifiers for their own needs. Add metadata after `#[patina_test]`: an optional `#[category(name)]`
and any number of modifiers using `#[modifier(name, ...)]`. Names must be unquoted identifiers, stored as
case-sensitive strings; hyphens and spaces are not supported. Raw identifiers are stored without the `r#` prefix.
The modifier attribute may be repeated; specifying more than one category is an error.

The standard categories are:

- `integration`: verifies interactions across interfaces or components on a platform (`Filter::CATEGORY_INTEGRATION`).
- `stress`: exercises a capability repeatedly or intensively (`Filter::CATEGORY_STRESS`).
- `smoke`: performs a small, basic check of a critical capability (`Filter::CATEGORY_SMOKE`).

Use these standardized modifiers when applicable:

- `slow`: expected to take substantially longer than typical tests or run repeatedly during boot
  (`Filter::MODIFIER_SLOW` and `Filter::EXCLUDE_MODIFIER_SLOW`).
- `heavy`: requires substantially more resources than typical tests, such as memory or processor capacity
  (`Filter::MODIFIER_HEAVY` and `Filter::EXCLUDE_MODIFIER_HEAVY`).

These modifiers are relative guidelines, not fixed time or resource limits. A test can have multiple standard
or custom modifiers:

```rust,no_run
mod integration {
    #[patina_test::patina_test]
    #[category(integration)]
    fn protocol_installation() -> patina_test::error::Result {
        Ok(())
    }

    #[patina_test::patina_test]
    #[category(integration)]
    #[modifier(slow)]
    fn protocol_timeout_slow() -> patina_test::error::Result {
        Ok(())
    }
}

mod stress {
    #[patina_test::patina_test]
    #[category(stress)]
    #[modifier(heavy, slow)]
    fn page_mapping_heavy_slow() -> patina_test::error::Result {
        Ok(())
    }
}

mod smoke {
    #[patina_test::patina_test]
    #[category(smoke)]
    fn processor_startup() -> patina_test::error::Result {
        Ok(())
    }
}

mod security {
    #[patina_test::patina_test]
    #[category(security)]
    #[modifier(slow, destructive)]
    fn key_rotation_slow_destructive() -> patina_test::error::Result {
        Ok(())
    }
}
```

Here, `security` is a custom category and `destructive` is a custom modifier. The
[platform testing guide](../../docs/src/dev/testing/platform.md) describes the convention in more detail.

Name filters match fully qualified test names, including their module paths. Category filters and substring include
filters combine with **OR**; exclusions take priority; and **all** modifier requirements must
match. With no category or substring include filters, tests from all categories, including unclassified tests,
remain eligible. Categories and modifiers match the explicit metadata exactly; names and module paths do not
implicitly assign metadata. Existing tests must be annotated to match metadata filters.

For a quick run, select smoke tests and exclude slow or heavy tests:

```rust,no_run
# #[cfg(feature = "test-runner")]
# {
use patina_test::component::{Filter, TestRunner};

let runner = TestRunner::default()
    .with_filter(Filter::CATEGORY_SMOKE)
    .with_filter(Filter::EXCLUDE_MODIFIER_SLOW)
    .with_filter(Filter::EXCLUDE_MODIFIER_HEAVY);
# let _ = runner;
# }
```

To select integration **or** stress tests, use multiple category includes:

```rust,no_run
# #[cfg(feature = "test-runner")]
# {
use patina_test::component::{Filter, TestRunner};

let runner = TestRunner::default()
    .with_filter(Filter::CATEGORY_INTEGRATION)
    .with_filter(Filter::CATEGORY_STRESS);
# let _ = runner;
# }
```

For a specialized run, select only heavy stress tests. Modifiers narrow the selected category, so combining
`Filter::MODIFIER_HEAVY` and `Filter::MODIFIER_SLOW` would select tests with both modifiers:

```rust,no_run
# #[cfg(feature = "test-runner")]
# {
use patina_test::component::{Filter, TestRunner};

let runner = TestRunner::default()
    .with_filter(Filter::CATEGORY_STRESS)
    .with_filter(Filter::MODIFIER_HEAVY);
# let _ = runner;
# }
```

Use `Filter::Category`, `Filter::Modifier`, and `Filter::ExcludeModifier` to filter custom categories and
modifiers. For example, select slow tests in the `security` category, except those with the `destructive` modifier:

```rust,no_run
# #[cfg(feature = "test-runner")]
# {
use patina_test::component::{Filter, TestRunner};

let runner = TestRunner::default()
    .with_filter(Filter::Category("security"))
    .with_filter(Filter::MODIFIER_SLOW)
    .with_filter(Filter::ExcludeModifier("destructive"));
# let _ = runner;
# }
```

Register whichever runner configuration is appropriate for the platform. Multiple runners can be registered,
but a test selected by more than one runner is scheduled only once, as described above.

## Feature Flags

- `test-runner`: Will make the `component` module public, providing access to the `TestRunner` component and actually
  register patina tests globally.

## Example

```rust
use patina_test::{
    patina_test, u_assert, u_assert_eq,
    error::Result,
};

use patina::component::service::{
    Service,
    uefi_services::timing::TimingServices,
};
use patina::uefi::event::CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID;

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
fn x86_64_only_test_case(_timing: Service<dyn TimingServices>) -> Result {
  Ok(())
}

#[patina_test]
#[on(event = CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID)]
fn on_event_test_case() -> Result {
  Ok(())
}
```
