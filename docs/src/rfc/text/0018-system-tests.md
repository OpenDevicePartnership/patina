# RFC: Patina Test Execution Configuration

Add the ability to control on-system unit test execution to a limited degree, to include (1) Tests executing multiple
times via event triggers and (2) Allow execution at different points of DXE, rather than only during monolithic
component execution prior to normal EDK II style component dispatch

## Change Log

This text can be modified over time. Add a change log entry for every change made to the RFC.

- 2025-07-21: Initial RFC created.
- 2025-07-21: Updated timer trigger to be based on ms.
- 2025-07-21: Updated display results event based off of `OnReadyToBoot` with a secondary diff at `ExitBootServices`.
- 2025-07-22: Resolve test result reporting question.

## Motivation

This change was motivated via two separate scenarios to include (1) The ability to test / validate the memory map
throughout boot in a way that can easily be removed when not validating and (2) The ability to run tests / validation
on code that may not be bootstrapped during component execution.

## Technology Background

This feature will rely almost entirely on the EDK II style eventing system, however it will not be implemented until
[Uefi Services](https://github.com/OpenDevicePartnership/patina/pull/592) is implemented, which will provide an
abstraction for eventing.

## Goals

- #[patina_test] macro configuration for specifying how the unit test is triggered; via a timer event, protocol
  install, event group, etc.
- Maintain the ability to report all unit test results together
- Further standardize testing
- Current default configuration should not change.

## Requirements

- Support Timer based validation tests (Trigger every X ms)
- Support Protocol installation based validation tests
- Support Event based validation tests

## Unresolved Questions

- Should the report service have a log function that tests log to, so we can report all log results at once instead
  of throughout boot?

- Should we remove the "fail_fast" configuration, now that we don't run all tests immediately?

## Prior Art (Existing PI C Implementation)

The current C implementation way of running on-system unit tests have each individually compiled as a UEFI_APPLICATION
that is executed manually from the UEFI shell.

Currently, on-system unit tests via patina-test has no support for running tests at any other time then monolithic
component execution.

## Alternatives

Each entity that wants these type(s) of test would directly write and register their own component that in-turn
register an event callback that runs their validation testing

## Rust Code Design

### patina_test macro interface

```rust
// Normal execution, no change
#[patina_test]
fn my_test() -> Result { ... }

// Run every "X"
#[patina_test]
#[on(timer = 1)]
fn my_test() -> Result { ... }

// Run on protocol installation
#[patina_test]
#[on(protocol = "GUID")]
fn my_test() -> Result { ... }

// Run on an event triggered, uninstall after first trigger
#[patina_test]
#[on(event = "GUID", once)]
fn my_test() -> Result { ... }
```

### `TestCase` Changes

`TestCase` will be changed to include a trigger type to inform the component on how to register / run a component

``` rust
pub enum Trigger {
    /// A Test that runs immediately during Test component execution.
    Component,
    /// A test that runs multiple times, every 'X' milliseconds.
    Timer(u32)
    /// A test that runs whenever the specified protocol is installed. Can run multiple times if the protocol is
    /// installed multiple times.
    Protocol(efi::Guid),
    /// A test that runs whenever the specified event is triggered. Can run multiple times if the event is triggered
    /// multiple times. Can be set to run only once.
    Event(efi::Guid, bool),
}

pub struct TestCase {
    pub name: &'static str,
    pub skip: bool,
    pub should_fail: bool,
    pub fail_msg: Option<&'static str>,
    pub trigger: Trigger,
    pub func: fn(&mut Storage) -> Result<bool, &'static str>,
}
```

### `TestRunner` Changes

This change will be more complex, as we now need to support different ways to register and execute tests. Due to this,
we also can no longer immediately report test results, because if we did, the results would be spread out across the
entire boot. Instead, we must provide a way to test runners to report their status back to a service. Then at a later
event, a event callback will use this service to report all results to the user.

```cmd
┌─────────────────────────────────┐                    
│ TestRunner Component Execution  │                    
└─────────────────────────────────┤                    
│┌───────────────────┐    ┌────────────────────────────┐
││ Produce TestReport│ ┌──► Component                  │
││ Service           │ │  └────────────────────────────┘
│└────────┬──────────┘ │  │ Run component immediately  │
│         │            │  │ and report results to      │
│┌────────▼─────────┐  │  │ service                    │
││ Foreach Test:    │  │  └───────┬────────────────────┘
││ │                │  │  ┌───────┴────────────────────┐
││ │ match Trigger: │  │┌─► Timer                      │
││ │ │              │  ││ └────────────────────────────┘
││ │ │- Component ──┼──┘│ │ Create event callback      │
││ │ │              │   │ │ that runs and reports      │
││ │ │- Timer ──────┼───┘ │ Results.                   │
││ │ │              │     │ Use SetTimer to setup a    │
││ │ │- Protocol────┼────┐│ Timer for the test.        │
││ │ │              │    │└────────┬───────────────────┘
││ │ │- Event ──────┼──┐ │┌────────┴───────────────────┐
│└────────┬─────────┘  │ └► Protocol                   │
│         │            │  └────────────────────────────┘
│┌────────▼─────────┐  │  │ Create event callback      │
││Register          │  │  │ that runs and reports      │
││ReportResults     │  │  │ Results.                   │
││Callback          │  │  │ Use RegisterProtocolNotify │
│└────────┬─────────┘  │  │ to setup a callback for    │
│         │            │  └───────┬────────────────────┘
│         │            │  ┌───────┴────────────────────┐
│         │            └──► Event                      │
│         │               └────────────────────────────┘
└─────────▼───────────────│ Use CreateEventEx to       │
                          │ register an event callback │
                          └────────────────────────────┘
```

### `ReportResults` Callback

The `ReportResults` Callback will be registered against two separate event groups - `OnReadyToBoot` and
`ExitBootServices`. On the first callback, A table will be logged showing all tests that have been executed to this
point in boot. If / when the `ExitBootServices` callback is executed, a table will be logged showing only the
additional tests that have been executed between `OnReadyToBoot` and `ExitBootServices`. Each table will be properly
marked with the event group being executed under. This logic is added due to the fact that most boot / test scenarios
involve booting to UEFI shell, which does not trigger `ExitBootServices`; however in scenarios where we do reach
`ExitBootServices`, we wish to print any additional tests that have executed.

The format of the table will be as such:

``` cmd
| Test Name | Number of Executions | Status             |
| MyTest    | 3                    | Success            |
| OtherTest | 5                    | Failed [1 time(s)] |
```

## Guide-Level Explanation

This feature allows a user to select how a particular `patina_test` is executed. In addition to the default execution
time of a `patina_test` (which is during `TestRunner` component execution), developers can now chose to delay execution
of a particular unit test to execute periodically based off a timer using `#[on(timer = X)]`, when a protocol is
registered using `#[on(protocol = "GUID")]`, or whenever a particular event is triggered using
`#[on(event = "GUID")]`.

This new functionality will not provide any breaking changes to the consumer, only allowing additional configuration.
This change will delay the reporting of all test results until `OnReadyToBoot`. We may also support a secondary display
of test results of tests executed between `OnReadyToBoot` and `ExitBootServices`. This is because in most testing
scenarios, we boot to the UEFI shell, which does not trigger `ExitBootServices`.
